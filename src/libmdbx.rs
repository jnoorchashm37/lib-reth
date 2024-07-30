use std::{
    path::{Path, PathBuf},
    sync::Arc
};

use alloy_primitives::{TxHash, U256};
use alloy_rpc_types::{
    eth::{Block, Filter, Log, Transaction},
    BlockTransactions, FilteredParams, Header
};
use futures::{Stream, StreamExt};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_blockchain_tree::{externals::TreeExternals, BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree};
use reth_chainspec::MAINNET;
use reth_db::{
    init_db,
    mdbx::{tx::Tx, DatabaseArguments},
    DatabaseEnv
};
use reth_libmdbx::RO;
use reth_network_api::noop::NoopNetwork;
use reth_node_ethereum::{EthEvmConfig, EthExecutorProvider};
use reth_primitives_traits::constants::*;
use reth_provider::{
    providers::{BlockchainProvider, StaticFileProvider},
    CanonStateSubscriptions, DatabaseProvider, ProviderFactory
};
use reth_prune_types::PruneModes;
use reth_rpc::{DebugApi, EthApi, EthFilter, TraceApi};
use reth_rpc_eth_types::{
    logs_utils, EthFilterConfig, EthStateCache, EthStateCacheConfig, FeeHistoryCache, FeeHistoryCacheConfig, GasPriceOracle,
    GasPriceOracleConfig
};
use reth_rpc_server_types::constants::{DEFAULT_ETH_PROOF_WINDOW, DEFAULT_PROOF_PERMITS};
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner, TokioTaskExecutor
};
use reth_transaction_pool::{
    blobstore::NoopBlobStore, validate::EthTransactionValidatorBuilder, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, TransactionPool, TransactionValidationTaskExecutor
};
use tokio_stream::wrappers::BroadcastStream;

use crate::streams::EthStream;

type RethProvider = BlockchainProvider<Arc<DatabaseEnv>>;
type RethApi = EthApi<RethProvider, RethTxPool, NoopNetwork, EthEvmConfig>;
type RethFilter = EthFilter<RethProvider, RethTxPool>;
type RethTrace = TraceApi<RethProvider, RethApi>;
type RethDebug = DebugApi<RethProvider, RethApi>;
type RethDbProvider = DatabaseProvider<Tx<RO>>;
type RethTxPool = Pool<
    TransactionValidationTaskExecutor<EthTransactionValidator<RethProvider, EthPooledTransaction>>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    NoopBlobStore
>;

/// direct libmdbx database connection to a reth node
pub struct RethLibmdbxClient {
    api:         RethApi,
    filter:      RethFilter,
    trace:       RethTrace,
    debug:       RethDebug,
    tx_pool:     RethTxPool,
    db_provider: RethDbProvider
}

impl RethLibmdbxClient {
    /// initializes the reth api with the default [TokioTaskExecutor] task
    /// executor
    pub fn new(db_path: &str, max_tasks: u64) -> eyre::Result<Self> {
        let db_path = Path::new(&db_path);
        let db = Arc::new(init_db(db_path, DatabaseArguments::new(Default::default()))?);
        let mut static_files = db_path.to_path_buf();
        static_files.pop();
        static_files.push("static_files");

        let executor = TokioTaskExecutor::default();
        new_with_db(db, max_tasks, executor, static_files)
    }

    /// initializes the reth api with a specific task executor
    pub fn new_with_executor<T: TaskSpawner + Clone + 'static>(
        db_path: &str,
        max_tasks: u64,
        task_executor: T
    ) -> eyre::Result<Self> {
        let db_path = Path::new(&db_path);
        let db = Arc::new(init_db(db_path, DatabaseArguments::new(Default::default()))?);
        let mut static_files = db_path.to_path_buf();
        static_files.pop();
        static_files.push("static_files");
        new_with_db(db, max_tasks, task_executor, static_files)
    }

    pub fn eth_api(&self) -> RethApi {
        self.api.clone()
    }

    pub fn eth_filter(&self) -> RethFilter {
        self.filter.clone()
    }

    pub fn eth_trace(&self) -> RethTrace {
        self.trace.clone()
    }

    pub fn eth_debug(&self) -> RethDebug {
        self.debug.clone()
    }

    pub fn eth_tx_pool(&self) -> RethTxPool {
        self.tx_pool.clone()
    }

    pub fn eth_db_provider(&self) -> &RethDbProvider {
        &self.db_provider
    }
}

impl<'a> EthStream<'a> for RethLibmdbxClient {
    async fn block_stream(&self) -> eyre::Result<impl Stream<Item = Block> + Send + 'a> {
        let stream = self
            .api
            .provider()
            .canonical_state_stream()
            .flat_map(|new_chain| {
                let sealed_blocks = new_chain
                    .committed()
                    .blocks_iter()
                    .map(|sealed_block| Block {
                        header:       Header {
                            hash:                     Some(sealed_block.hash()),
                            parent_hash:              sealed_block.parent_hash,
                            uncles_hash:              sealed_block.ommers_hash,
                            miner:                    sealed_block.beneficiary,
                            state_root:               sealed_block.state_root,
                            transactions_root:        sealed_block.transactions_root,
                            receipts_root:            sealed_block.receipts_root,
                            logs_bloom:               sealed_block.logs_bloom,
                            difficulty:               sealed_block.difficulty,
                            number:                   Some(sealed_block.number),
                            gas_limit:                sealed_block.gas_limit as u128,
                            gas_used:                 sealed_block.gas_used as u128,
                            timestamp:                sealed_block.timestamp,
                            total_difficulty:         Some(sealed_block.difficulty),
                            extra_data:               sealed_block.extra_data.clone(),
                            mix_hash:                 Some(sealed_block.mix_hash),
                            nonce:                    Some(sealed_block.nonce.into()),
                            base_fee_per_gas:         sealed_block.base_fee_per_gas.map(|v| v as u128),
                            withdrawals_root:         sealed_block.withdrawals_root,
                            blob_gas_used:            sealed_block.blob_gas_used.map(|v| v as u128),
                            excess_blob_gas:          sealed_block.excess_blob_gas.map(|v| v as u128),
                            parent_beacon_block_root: sealed_block.parent_beacon_block_root,
                            requests_root:            sealed_block.requests_root
                        },
                        uncles:       sealed_block
                            .ommers
                            .clone()
                            .into_iter()
                            .map(|uncle| uncle.ommers_hash)
                            .collect(),
                        transactions: BlockTransactions::Full(
                            sealed_block
                                .body
                                .clone()
                                .into_iter()
                                .filter_map(|tx| {
                                    tx.recover_signer().map(|signer| {
                                        reth_rpc_types_compat::transaction::from_recovered(tx.with_signer(signer))
                                    })
                                })
                                .collect()
                        ),
                        size:         Some(U256::from(sealed_block.size())),
                        withdrawals:  sealed_block
                            .withdrawals
                            .clone()
                            .map(|wit| wit.into_iter().collect()),
                        other:        Default::default()
                    })
                    .collect::<Vec<_>>();
                futures::stream::iter(sealed_blocks)
            });

        Ok(stream)
    }

    async fn full_pending_transaction_stream(&self) -> eyre::Result<impl Stream<Item = Transaction> + Send + 'a> {
        let stream = self
            .api
            .pool()
            .new_pending_pool_transactions_listener()
            .map(|tx| reth_rpc_types_compat::transaction::from_recovered(tx.transaction.transaction.transaction().clone()));

        Ok(stream)
    }

    async fn pending_transaction_hashes_stream(&self) -> eyre::Result<impl Stream<Item = TxHash> + Send + 'a> {
        let stream = self
            .api
            .pool()
            .new_pending_pool_transactions_listener()
            .map(|tx| *tx.transaction.hash());

        Ok(stream)
    }

    async fn log_stream(&self, filter: &Filter) -> eyre::Result<impl Stream<Item = Log> + Send + 'a> {
        let stream = BroadcastStream::new(self.api.provider().subscribe_to_canonical_state())
            .map(move |canon_state| {
                canon_state
                    .expect("new block subscription never ends")
                    .block_receipts()
            })
            .flat_map(futures::stream::iter)
            .flat_map(move |(block_receipts, removed)| {
                let all_logs = logs_utils::matching_block_logs_with_tx_hashes(
                    &FilteredParams { filter: Some(filter.clone()) },
                    block_receipts.block,
                    block_receipts
                        .tx_receipts
                        .iter()
                        .map(|(tx, receipt)| (*tx, receipt)),
                    removed
                );
                futures::stream::iter(all_logs)
            });

        Ok(stream)
    }
}

/// spawns the reth libmdbx client
fn new_with_db<T: TaskSpawner + Clone + 'static>(
    db: Arc<DatabaseEnv>,
    max_tasks: u64,
    task_executor: T,
    static_files_path: PathBuf
) -> eyre::Result<RethLibmdbxClient> {
    let chain = MAINNET.clone();
    let msg = format!("could not make 'StaticFileProvider' at '{}'", static_files_path.display());
    let provider_factory = ProviderFactory::new(
        Arc::clone(&db),
        Arc::clone(&chain),
        StaticFileProvider::read_only(static_files_path).expect(&msg)
    );

    let db_provider = provider_factory.clone().provider()?;

    let tree_externals = TreeExternals::new(
        provider_factory.clone(),
        Arc::new(EthBeaconConsensus::new(Arc::clone(&chain))),
        EthExecutorProvider::ethereum(chain.clone())
    );

    let tree_config = BlockchainTreeConfig::default();

    let blockchain_tree =
        ShareableBlockchainTree::new(BlockchainTree::new(tree_externals, tree_config, PruneModes::none())?);

    let provider = BlockchainProvider::new(provider_factory.clone(), Arc::new(blockchain_tree))?;

    let state_cache = EthStateCache::spawn_with(
        provider.clone(),
        EthStateCacheConfig::default(),
        task_executor.clone(),
        EthEvmConfig::default()
    );

    let transaction_validator = EthTransactionValidatorBuilder::new(chain.clone()).build_with_tasks(
        provider.clone(),
        task_executor.clone(),
        NoopBlobStore::default()
    );

    let tx_pool = reth_transaction_pool::Pool::eth_pool(transaction_validator, NoopBlobStore::default(), Default::default());

    let blocking = BlockingTaskPool::build()?;
    let eth_state_config = EthStateCacheConfig::default();
    let fee_history = FeeHistoryCache::new(
        EthStateCache::spawn_with(provider.clone(), eth_state_config, task_executor.clone(), EthEvmConfig::default()),
        FeeHistoryCacheConfig::default()
    );

    let api = EthApi::new(
        provider.clone(),
        tx_pool.clone(),
        NoopNetwork::default(),
        state_cache.clone(),
        GasPriceOracle::new(provider.clone(), GasPriceOracleConfig::default(), state_cache.clone()),
        ETHEREUM_BLOCK_GAS_LIMIT,
        DEFAULT_ETH_PROOF_WINDOW,
        blocking,
        fee_history,
        EthEvmConfig::default(),
        None,
        DEFAULT_PROOF_PERMITS
    );

    let blocking_task_guard = BlockingTaskGuard::new(10);

    let tracing_call_guard = BlockingTaskGuard::new(max_tasks as usize);
    let trace = TraceApi::new(provider.clone(), api.clone(), tracing_call_guard);
    let debug = DebugApi::new(provider.clone(), api.clone(), blocking_task_guard);
    let filter = EthFilter::new(
        provider.clone(),
        tx_pool.clone(),
        state_cache,
        EthFilterConfig::default(),
        Box::new(task_executor.clone())
    );

    Ok(RethLibmdbxClient { api, trace, filter, debug, db_provider, tx_pool })
}
