use std::path::Path;

use eyre::Context;
use reth_db::{
    database::Database,
    mdbx::{tx::Tx, DatabaseArguments, RO},
    models::client_version::ClientVersion,
    tables,
    transaction::DbTx,
    DatabaseEnv, DatabaseEnvKind,
};
use reth_network_api::noop::NoopNetwork;
use std::sync::Arc;

use reth_beacon_consensus::BeaconConsensus;
use reth_blockchain_tree::{
    BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
};

use reth_node_ethereum::EthEvmConfig;
use reth_primitives::{constants::ETHEREUM_BLOCK_GAS_LIMIT, MAINNET};
use reth_provider::{providers::BlockchainProvider, DatabaseProvider, ProviderFactory};
use reth_revm::EvmProcessorFactory;
use reth_rpc::{
    eth::{
        cache::{EthStateCache, EthStateCacheConfig},
        gas_oracle::{GasPriceOracle, GasPriceOracleConfig},
        EthFilterConfig, FeeHistoryCache, FeeHistoryCacheConfig,
    },
    DebugApi, EthApi, EthFilter, TraceApi,
};

use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskManager,
};
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, TransactionValidationTaskExecutor,
};
use tokio::runtime::Handle;

type RethClient = BlockchainProvider<
    Arc<DatabaseEnv>,
    ShareableBlockchainTree<Arc<DatabaseEnv>, EvmProcessorFactory<EthEvmConfig>>,
>;
type RethTxPool = Pool<
    TransactionValidationTaskExecutor<EthTransactionValidator<RethClient, EthPooledTransaction>>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    InMemoryBlobStore,
>;
pub type RethEthApi = EthApi<RethClient, RethTxPool, NoopNetwork, EthEvmConfig>;
pub type RethFilter = EthFilter<RethClient, RethTxPool>;
pub type RethTrace = TraceApi<RethClient, RethEthApi>;
pub type RethDebug = DebugApi<RethClient, RethEthApi>;
pub type RethDbProvider = DatabaseProvider<Tx<RO>>;

/// initializes the reth api endpoints via libmdbx
pub(crate) fn init_eth(
    db_path: &Path,
    handle: Handle,
) -> eyre::Result<(RethEthApi, RethFilter, RethTrace, RethDebug, RethDbProvider)> {
    let task_manager = TaskManager::new(handle.clone());
    let task_executor = task_manager.executor();

    handle.spawn(task_manager);

    let db = Arc::new(open_libmdbx(db_path).unwrap());
    let mut static_files_path = db_path.to_path_buf();
    static_files_path.pop();
    static_files_path.push("static_files");

    let chain_spec = MAINNET.clone();
    let provider_factory =
        ProviderFactory::new(Arc::clone(&db), chain_spec.clone(), static_files_path)?;

    let tree_externals = TreeExternals::new(
        provider_factory.clone(),
        Arc::new(BeaconConsensus::new(chain_spec.clone())),
        EvmProcessorFactory::new(chain_spec.clone(), EthEvmConfig::default()),
    );

    let tree_config = BlockchainTreeConfig::default();

    let blockchain_tree =
        ShareableBlockchainTree::new(BlockchainTree::new(tree_externals, tree_config, None)?);

    let provider = BlockchainProvider::new(provider_factory.clone(), blockchain_tree)?;

    let db_provider = provider_factory.clone().provider()?;

    let state_cache = EthStateCache::spawn(
        provider.clone(),
        EthStateCacheConfig::default(),
        EthEvmConfig::default(),
    );

    let blob_store = InMemoryBlobStore::default();
    let tx_pool = reth_transaction_pool::Pool::eth_pool(
        TransactionValidationTaskExecutor::eth(
            provider.clone(),
            chain_spec.clone(),
            blob_store.clone(),
            task_executor.clone(),
        ),
        blob_store,
        Default::default(),
    );

    let reth_api = EthApi::new(
        provider.clone(),
        tx_pool.clone(),
        Default::default(),
        state_cache.clone(),
        GasPriceOracle::new(
            provider.clone(),
            GasPriceOracleConfig::default(),
            state_cache.clone(),
        ),
        ETHEREUM_BLOCK_GAS_LIMIT,
        BlockingTaskPool::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(5)
                .build()
                .unwrap(),
        ),
        FeeHistoryCache::new(state_cache.clone(), FeeHistoryCacheConfig::default()),
        EthEvmConfig::default(),
        None,
    );

    let blocking_task_guard = BlockingTaskGuard::new(10);

    let reth_trace = TraceApi::new(
        provider.clone(),
        reth_api.clone(),
        blocking_task_guard.clone(),
    );

    let reth_debug = DebugApi::new(provider.clone(), reth_api.clone(), blocking_task_guard);

    let reth_filter = EthFilter::new(
        provider,
        tx_pool,
        state_cache,
        EthFilterConfig::default(),
        Box::new(task_executor),
    );

    Ok((reth_api, reth_filter, reth_trace, reth_debug, db_provider))
}

/// Opens up an existing libmdbx database at the specified path.
fn open_libmdbx<P: AsRef<Path>>(path: P) -> eyre::Result<DatabaseEnv> {
    let _ = std::fs::create_dir_all(path.as_ref());
    let db = DatabaseEnv::open(
        path.as_ref(),
        DatabaseEnvKind::RO,
        DatabaseArguments::new(ClientVersion::default()),
    )?;

    view(&db, |tx| {
        for table in tables::Tables::ALL.iter().map(|table| table.name()) {
            tx.inner
                .open_db(Some(table))
                .wrap_err("Could not open db.")
                .unwrap();
        }
    })?;

    Ok(db)
}

/// allows for a function to be passed in through a RO libmdbx transaction
fn view<F, T>(db: &DatabaseEnv, f: F) -> eyre::Result<T>
where
    F: FnOnce(&<DatabaseEnv as Database>::TX) -> T,
{
    let tx = db.tx()?;
    let res = f(&tx);
    tx.commit()?;

    Ok(res)
}
