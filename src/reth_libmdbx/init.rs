use std::{path::PathBuf, sync::Arc};

use alloy_rpc_types::simulate::MAX_SIMULATE_BLOCKS;
use clap::Parser;
use reth::chainspec::EthereumChainSpecParser;
use reth_beacon_consensus::EthBeaconConsensus;
use reth_blockchain_tree::{externals::TreeExternals, BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree};
use reth_chainspec::{ChainSpec, MAINNET};
use reth_config::PruneConfig;
use reth_db::{mdbx::tx::Tx, DatabaseEnv};
use reth_engine_local::{miner::MiningMode, payload::EthLocalPayloadAttributesBuilder, service::LocalEngineService};
use reth_libmdbx::RO;
use reth_network_api::noop::NoopNetwork;
use reth_node_builder::{
    components::{ComponentsBuilder, NetworkBuilder},
    Node, NodeHandle, PayloadAttributesBuilder
};
use reth_node_ethereum::{
    node::{
        EthereumAddOns, EthereumConsensusBuilder, EthereumEngineValidatorBuilder, EthereumExecutorBuilder,
        EthereumPayloadBuilder, EthereumPoolBuilder
    },
    EthEvmConfig, EthExecutorProvider, EthereumNode
};
use reth_node_types::NodeTypesWithDBAdapter;
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives_traits::constants::*;
use reth_provider::{
    providers::{BlockchainProvider, StaticFileProvider},
    DatabaseProvider, ProviderFactory
};
use reth_prune::{Pruner, PrunerBuilder};
use reth_rpc::{DebugApi, EthApi, EthFilter, TraceApi};
use reth_rpc_eth_types::{
    EthFilterConfig, EthStateCache, EthStateCacheConfig, FeeHistoryCache, FeeHistoryCacheConfig, GasPriceOracle,
    GasPriceOracleConfig
};
use reth_rpc_server_types::constants::{DEFAULT_ETH_PROOF_WINDOW, DEFAULT_PROOF_PERMITS};
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner
};
use reth_transaction_pool::{
    blobstore::NoopBlobStore, validate::EthTransactionValidatorBuilder, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, TransactionValidationTaskExecutor
};
use tokio::sync::mpsc::unbounded_channel;

use super::RethLibmdbxClient;

type RethProvider = BlockchainProvider<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>;
type RethApi = EthApi<RethProvider, RethTxPool, NoopNetwork, EthEvmConfig>;
type RethFilter = EthFilter<RethProvider, RethTxPool, RethApi>;
type RethTrace = TraceApi<RethProvider, RethApi>;
type RethDebug = DebugApi<RethProvider, RethApi, EthExecutorProvider>;
type RethDbProvider = DatabaseProvider<Tx<RO>, ChainSpec>;
type RethTxPool = Pool<
    TransactionValidationTaskExecutor<EthTransactionValidator<RethProvider, EthPooledTransaction>>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    NoopBlobStore
>;

/// spawns the reth libmdbx client
pub(super) fn new_with_db<T: TaskSpawner + Clone + 'static>(
    db: Arc<DatabaseEnv>,
    max_tasks: usize,
    task_executor: T,
    static_files_path: PathBuf
) -> eyre::Result<RethLibmdbxClient> {
    let chain = MAINNET.clone();
    let evm_config = EthEvmConfig::new(chain.clone());
    let msg = format!("could not make 'StaticFileProvider' at '{}'", static_files_path.display());
    let provider_factory = ProviderFactory::<NodeTypesWithDBAdapter<_, Arc<DatabaseEnv>>>::new(
        Arc::clone(&db),
        Arc::clone(&chain),
        StaticFileProvider::read_only(static_files_path, true).expect(&msg)
    );

    let db_provider = provider_factory.clone().provider()?;

    let tree_externals = TreeExternals::new(
        provider_factory.clone(),
        Arc::new(EthBeaconConsensus::new(Arc::clone(&chain))),
        EthExecutorProvider::ethereum(chain.clone())
    );

    let tree_config = BlockchainTreeConfig::default();

    let blockchain_tree = ShareableBlockchainTree::new(BlockchainTree::new(tree_externals, tree_config)?);

    let provider = BlockchainProvider::new(provider_factory.clone(), Arc::new(blockchain_tree))?;

    let state_cache = EthStateCache::spawn_with(
        provider.clone(),
        EthStateCacheConfig::default(),
        task_executor.clone(),
        evm_config.clone()
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
        EthStateCache::spawn_with(provider.clone(), eth_state_config, task_executor.clone(), evm_config.clone()),
        FeeHistoryCacheConfig::default()
    );

    let api = EthApi::new(
        provider.clone(),
        tx_pool.clone(),
        NoopNetwork::default(),
        state_cache.clone(),
        GasPriceOracle::new(provider.clone(), GasPriceOracleConfig::default(), state_cache.clone()),
        ETHEREUM_BLOCK_GAS_LIMIT,
        MAX_SIMULATE_BLOCKS,
        DEFAULT_ETH_PROOF_WINDOW,
        blocking,
        fee_history,
        evm_config.clone(),
        DEFAULT_PROOF_PERMITS
    );

    let blocking_task_guard = BlockingTaskGuard::new(max_tasks);
    let provider_executor = EthExecutorProvider::ethereum(chain.clone());

    let trace = TraceApi::new(provider.clone(), api.clone(), blocking_task_guard.clone());
    let debug = DebugApi::new(provider.clone(), api.clone(), blocking_task_guard, provider_executor);
    let filter = EthFilter::new(
        provider.clone(),
        tx_pool.clone(),
        state_cache,
        EthFilterConfig::default(),
        Box::new(task_executor.clone())
    );

    let (metrics_tx, _) = unbounded_channel();

    let mut engine_service = {
        let mining_mode = MiningMode::instant(tx_pool.clone());
        let eth_service = LocalEngineService::new(
            PayloadBuilderHandle::new(),
            EthLocalPayloadAttributesBuilder,
            // payload attributes builder
            provider_factory.clone(),
            PrunerBuilder::new(PruneConfig::default())
                .delete_limit(chain.prune_delete_limit)
                .timeout(PrunerBuilder::DEFAULT_TIMEOUT)
                .build_with_provider_factory(provider_factory.clone()),
            CanonicalInMemoryState::new(),
            metrics_tx,
            mining_mode
        );
    };

    Ok(RethLibmdbxClient { api, trace, filter, debug, db_provider, tx_pool })
}

// async fn launch_node() -> eyre::Result<()> {
//     reth::cli::Cli::<EthereumChainSpecParser, _>::parse().run(|builder, args|
// async move {         let executor = builder.task_executor().clone();

//         let NodeHandle { node, node_exit_future } = builder
//             .with_types::<EthereumNode>()
//             .with_components(
//                 EthereumNode::default()
//                     .components_builder()
//                     .network(AngstromNetworkBuilder::new(protocol_handle))
//             )
//             .with_add_ons::<EthereumAddOns>(Default::default())
//             .launch()
//             .await?;

//         Ok(())
//     });

//     Ok(())
// }
