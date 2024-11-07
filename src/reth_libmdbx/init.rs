use std::{path::PathBuf, str::FromStr, sync::Arc};

use alloy_eips::eip1559::ETHEREUM_BLOCK_GAS_LIMIT;
use alloy_rpc_types::simulate::MAX_SIMULATE_BLOCKS;
use clap::Parser;
use futures::future::Either;
use reth::{
    args::{DatadirArgs, DebugArgs},
    chainspec::EthereumChainSpecParser,
    cli::Commands,
    core::exit::NodeExitFuture,
    dirs::{ChainPath, MaybePlatformPath, PlatformPath},
    network::{NetworkHandle, NetworkManager},
    payload::ExecutionPayloadValidator,
    rpc::types::engine::PayloadAttributes
};
use reth_beacon_consensus::{BeaconConsensusEngineHandle, EthBeaconConsensus};
use reth_blockchain_tree::{externals::TreeExternals, BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree};
use reth_chain_state::CanonicalInMemoryState;
use reth_chainspec::{ChainSpec, MAINNET};
use reth_cli_commands::{node::NoArgs, NodeCommand};
use reth_config::PruneConfig;
use reth_db::{mdbx::tx::Tx, DatabaseEnv};
use reth_engine_local::{miner::MiningMode, service::LocalEngineService, LocalPayloadAttributesBuilder};
use reth_engine_tree::tree::{InvalidBlockHooks, TreeConfig};
use reth_engine_util::EngineMessageStreamExt;
use reth_libmdbx::RO;
use reth_network_api::noop::NoopNetwork;
use reth_node_builder::{
    components::{ComponentsBuilder, ConsensusBuilder, NetworkBuilder},
    BuilderContext, FullNodeTypes, InvalidBlockHook, LaunchContext, Node, NodeBuilder, NodeConfig, NodeHandle,
    PayloadAttributesBuilder
};
use reth_node_ethereum::{
    node::{
        EthereumAddOns, EthereumConsensusBuilder, EthereumEngineValidatorBuilder, EthereumExecutorBuilder,
        EthereumNetworkBuilder, EthereumPayloadBuilder, EthereumPoolBuilder
    },
    BasicBlockExecutorProvider, EthEvmConfig, EthExecutionStrategyFactory, EthExecutorProvider, EthereumNode
};
use reth_node_types::{NodeTypes, NodeTypesWithDBAdapter};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives_traits::constants::*;
use reth_provider::{
    providers::{BlockchainProvider, BlockchainProvider2, StaticFileProvider},
    DatabaseProvider, DatabaseProviderFactory, ProviderFactory
};
use reth_prune::{PruneModes, Pruner, PrunerBuilder};
use reth_rpc::{DebugApi, EthApi, EthFilter, TraceApi};
use reth_rpc_eth_types::{
    EthFilterConfig, EthStateCache, EthStateCacheConfig, FeeHistoryCache, FeeHistoryCacheConfig, GasPriceOracle,
    GasPriceOracleConfig
};
use reth_rpc_server_types::constants::{DEFAULT_ETH_PROOF_WINDOW, DEFAULT_PROOF_PERMITS};
use reth_static_file::StaticFileProducer;
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskExecutor, TaskManager, TaskSpawner
};
use reth_tokio_util::EventSender;
use reth_transaction_pool::{
    blobstore::{DiskFileBlobStore, NoopBlobStore},
    validate::EthTransactionValidatorBuilder,
    CoinbaseTipOrdering, EthPooledTransaction, EthTransactionValidator, Pool, TransactionPool,
    TransactionValidationTaskExecutor
};
use tokio::sync::mpsc::unbounded_channel;
use tokio_stream::wrappers::UnboundedReceiverStream;

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

    let consensus = Arc::new(EthBeaconConsensus::new(Arc::clone(&chain)));
    let tree_externals =
        TreeExternals::new(provider_factory.clone(), consensus.clone(), EthExecutorProvider::ethereum(chain.clone()));

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
        Box::new(task_executor.clone()),
        api.tx_resp_builder.clone()
    );

    let (metrics_tx, _) = unbounded_channel();
    let (payload_tx, _) = unbounded_channel();
    let (consensus_engine_tx, consensus_engine_rx) = unbounded_channel();
    let (to_engine, from_engine) = unbounded_channel();

    let debug_consensus_args = DebugArgs::default();
    let consensus_engine_stream = UnboundedReceiverStream::from(consensus_engine_rx)
        .maybe_skip_fcu(debug_consensus_args.skip_fcu)
        .maybe_skip_new_payload(debug_consensus_args.skip_new_payload)
        .maybe_reorg(
            blockchain_tree.clone(),
            evm_config.clone(),
            ExecutionPayloadValidator::new(chain.clone()),
            debug_consensus_args.reorg_frequency,
            debug_consensus_args.reorg_depth
        ); // .maybe_store_messages(debug_consensus_args.engine_api_store.clone());

    let engine_service = LocalEngineService::new(
        consensus.clone(),
        provider_executor.clone(),
        provider_factory.clone(),
        BlockchainProvider2::new(provider_factory.clone())?,
        PrunerBuilder::new(PruneConfig::default())
            .delete_limit(chain.prune_delete_limit)
            .timeout(PrunerBuilder::DEFAULT_TIMEOUT)
            .build_with_provider_factory(provider_factory.clone()),
        PayloadBuilderHandle::new(payload_tx),
        TreeConfig::default(),
        Box::new(InvalidBlockHooks(vec![])),
        metrics_tx,
        to_engine,
        Box::pin(UnboundedReceiverStream::new(from_engine)),
        MiningMode::instant(tx_pool.clone()),
        LocalPayloadAttributesBuilder::new(chain.clone())
    );

    let event_sender = EventSender::default();
    let beacon_engine_handle = BeaconConsensusEngineHandle::new(consensus_engine_tx, event_sender.clone());

    let exectur = TaskManager::current().executor();
    let ctx = LaunchContext::new(
        exectur,
        ChainPath::new(PlatformPath::from_str("").unwrap(), chain.chain.clone(), DatadirArgs::default())
    );

    let static_file_producer = StaticFileProducer::new(provider_factory.clone(), PruneModes::none());
    let static_file_producer_events = static_file_producer.lock().events();
    let events = futures::stream_select!(
        ctx.components().network().event_listener().map(Into::into),
        beacon_engine_handle.event_listener().map(Into::into),
        pipeline_events.map(Into::into),
        // Either::Left(ConsensusLayerHealthEvents::new(Box::new(ctx.blockchain_db().
        // clone())).map(Into::into))
        // } else {
        //     Either::Right(stream::empty())
        // },
        Either::Right(futures::stream::empty()),
        pruner_events.map(Into::into),
        static_file_producer_events.map(Into::into),
    );

    task_executor.spawn_critical(
        "events task",
        reth_node_events::node::handle_events(
            Some(Box::new(ctx.components().network().clone())),
            Some(ctx.head().number),
            events
        )
    );

    // let engine_service = EngineService::new(
    //     EthBeaconConsensus::new(chain.clone()),
    //     provider_executor.clone(),
    //     chain,
    //     EthLocalPayloadAttributesBuilder,
    //     provider_factory.clone(),
    //     PrunerBuilder::new(PruneConfig::default())
    //         .delete_limit(chain.prune_delete_limit)
    //         .timeout(PrunerBuilder::DEFAULT_TIMEOUT)
    //         .build_with_provider_factory(provider_factory.clone()),
    //     CanonicalInMemoryState::empty(),
    //     metrics_tx,
    //     mining_mode
    // );

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

pub(super) async fn new_with_db2(
    db: Arc<DatabaseEnv>,
    max_tasks: usize,
    task_executor: TaskExecutor,
    static_files_path: PathBuf
) -> eyre::Result<()> {
    let cli = reth::cli::Cli::<EthereumChainSpecParser, NoArgs>::parse();
    let mut eth_cmd = match cli.command {
        Commands::Node(cmd) => cmd,
        _ => unreachable!()
    };

    // let NodeCommand::<EthereumChainSpecParser> {
    //     datadir,
    //     config,
    //     chain,
    //     metrics,
    //     instance,
    //     with_unused_ports,
    //     network,
    //     rpc,
    //     txpool,
    //     builder,
    //     debug,
    //     db,
    //     dev,
    //     pruning,
    //     ext,
    // } = eth_cmd;

    eth_cmd.network.discovery.disable_discovery = true;
    eth_cmd.network.bootnodes = None;
    eth_cmd.network.no_persist_peers = true;
    eth_cmd.network.no_persist_peers = true;

    eth_cmd.metrics = None;

    // set up node config
    let mut node_config = NodeConfig {
        datadir:  eth_cmd.datadir,
        config:   eth_cmd.config,
        chain:    eth_cmd.chain,
        metrics:  eth_cmd.metrics,
        instance: eth_cmd.instance,
        network:  eth_cmd.network,
        rpc:      eth_cmd.rpc,
        txpool:   eth_cmd.txpool,
        builder:  eth_cmd.builder,
        debug:    eth_cmd.debug,
        db:       eth_cmd.db,
        dev:      eth_cmd.dev,
        pruning:  eth_cmd.pruning
    };

    let builder = NodeBuilder::new(node_config)
        .with_database(db)
        .with_launch_context(task_executor.clone())
        .launch_node(EthereumNode::default())
        .await
        .unwrap();

    let api = builder.node.eth_api().clone();
    let debug = builder.node.debug_api();
    let trace = builder.node.trace_api();
    let tx_pool = builder.node.pool().clone();
    let db_provider = builder.node.provider().database_provider_ro().unwrap();

    let filter = EthFilter::new(
        builder.node.provider().clone(),
        tx_pool.clone(),
        api.cache().clone(),
        EthFilterConfig::default(),
        Box::new(task_executor.clone()),
        api.tx_resp_builder.clone()
    );

    let client = RethLibmdbxClient2 { api, filter, trace, debug, tx_pool, db_provider, _private: builder.node_exit_future };

    Ok(())
}

type RethProvider2 = BlockchainProvider<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>;
type RethApi2 = EthApi<RethProvider, RethTxPool2, NetworkHandle, EthEvmConfig>;
type RethFilter2 = EthFilter<RethProvider, RethTxPool2, RethApi2>;
type RethTrace2 = TraceApi<RethProvider, RethApi2>;
type RethDebug2 = DebugApi<RethProvider, RethApi2, BasicBlockExecutorProvider<EthExecutionStrategyFactory>>;
type RethDbProvider2 = DatabaseProvider<Tx<RO>, NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>;
type RethTxPool2 = Pool<
    TransactionValidationTaskExecutor<EthTransactionValidator<RethProvider, EthPooledTransaction>>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    DiskFileBlobStore
>;

/// direct libmdbx database connection to a reth node
pub struct RethLibmdbxClient2 {
    api:         RethApi2,
    filter:      RethFilter2,
    trace:       RethTrace2,
    debug:       RethDebug2,
    tx_pool:     RethTxPool2,
    db_provider: RethDbProvider2,
    _private:    NodeExitFuture
}

// struct NoopNetworkBuidler;

// impl<Node, Pool> NetworkBuilder<Node, Pool> for NoopNetworkBuidler
// where
//     Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec>>,
//     Pool: TransactionPool + Unpin + 'static
// {
//     async fn build_network(self, ctx: &BuilderContext<Node>, pool: Pool) ->
// eyre::Result<NetworkHandle> {         let network =
// ctx.network_builder().await?;         let handle = ctx.start_network(network,
// pool);

//         let mut network_config = ctx.network_config()?;
//         // network_config.discovery_v4_config

//         let network = NetworkManager::builder(network_config).await?;
//         let handle = ctx.start_network(network, pool);

//         /*

//                 let mut network_config = ctx.network_config()?;
//         network_config.extra_protocols.push(self.custom_protocol);

//         let network = NetworkManager::builder(network_config).await?;
//         let handle = ctx.start_network(network, pool);

//         Ok(handle)
//          */
//         Ok(handle)
//     }
// }
