use std::{path::PathBuf, sync::Arc};

use exe_runners::TaskSpawner;
use op_alloy_network::Optimism;
use reth_db::DatabaseEnv;
use reth_network_api::noop::NoopNetwork;
use reth_node_types::NodeTypesWithDBAdapter;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_evm::OpEvmConfig;
use reth_optimism_node::{txpool::OpTransactionValidator, OpNode};
use reth_optimism_rpc::eth::receipt::OpReceiptConverter;
use reth_provider::providers::{BlockchainProvider, StaticFileProvider};
use reth_rpc::{DebugApi, EthApi, EthFilter, TraceApi};
use reth_rpc_eth_api::node::RpcNodeCoreAdapter;
use reth_rpc_eth_api::RpcConverter;
use reth_rpc_eth_types::{EthConfig, EthFilterConfig};
use reth_tasks::pool::BlockingTaskGuard;
use reth_transaction_pool::{
    blobstore::NoopBlobStore, validate::EthTransactionValidatorBuilder, CoinbaseTipOrdering, EthPooledTransaction, Pool,
    PoolConfig, TransactionValidationTaskExecutor,
};

pub struct OpRethLibmdbxClient {
    api: OpRethApi,
    filter: OpRethFilter,
    trace: OpRethTrace,
    debug: OpRethDebug,
    tx_pool: OpRethTxPool,
    db_provider: OpRethDbProvider,
}

pub(super) type OpRethApi = EthApi<
    RpcNodeCoreAdapter<OpRethDbProvider, OpRethTxPool, NoopNetwork, OpEvmConfig>,
    RpcConverter<Optimism, OpEvmConfig, OpReceiptConverter<OpRethDbProvider>>,
>;
pub(super) type OpRethFilter = EthFilter<OpRethApi>;
pub(super) type OpRethTrace = TraceApi<OpRethApi>;
pub(super) type OpRethDebug = DebugApi<OpRethApi>;
pub(super) type OpRethTxPool = Pool<
    TransactionValidationTaskExecutor<OpTransactionValidator<OpRethDbProvider, EthPooledTransaction>>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    NoopBlobStore,
>;
pub(super) type OpRethDbProvider = BlockchainProvider<NodeTypesWithDBAdapter<OpNode, Arc<DatabaseEnv>>>;

/// spawns the reth libmdbx client
pub(super) fn new_with_db<T: TaskSpawner + Clone + 'static>(
    db: Arc<DatabaseEnv>,
    max_tasks: usize,
    task_executor: T,
    static_files_path: PathBuf,
    chain: Arc<OpChainSpec>,
) -> eyre::Result<OpRethLibmdbxClient> {
    let static_file_provider = StaticFileProvider::read_only(static_files_path.clone(), true)?;
    let provider_factory = OpNode::provider_factory_builder()
        .db(db)
        .chainspec(chain.clone())
        .static_file(static_file_provider)
        .build_provider_factory();

    let blockchain_provider = BlockchainProvider::new(provider_factory.clone())?;

    let transaction_validator = EthTransactionValidatorBuilder::new(blockchain_provider.clone())
        .build_with_tasks(task_executor.clone(), NoopBlobStore::default())
        .map(OpTransactionValidator::new);

    let tx_pool =
        Pool::new(transaction_validator, CoinbaseTipOrdering::default(), NoopBlobStore::default(), PoolConfig::default());

    let evm_config = OpEvmConfig::optimism(chain);
    let rpc_converter = RpcConverter::<Optimism, OpEvmConfig, OpReceiptConverter<OpRethDbProvider>>::new(
        OpReceiptConverter::new(blockchain_provider.clone()),
    );

    let api = EthApi::builder(blockchain_provider.clone(), tx_pool.clone(), NoopNetwork::default(), evm_config)
        .with_rpc_converter(rpc_converter)
        .build();

    let tracing_call_guard = BlockingTaskGuard::new(max_tasks);
    let trace = TraceApi::new(api.clone(), tracing_call_guard.clone(), EthConfig::default());

    let debug = DebugApi::new(api.clone(), tracing_call_guard);
    let filter = EthFilter::new(api.clone(), EthFilterConfig::default(), Box::new(task_executor.clone()));

    Ok(OpRethLibmdbxClient { api, trace, filter, debug, tx_pool, db_provider: blockchain_provider })
}

impl OpRethLibmdbxClient {
    pub fn eth_api(&self) -> OpRethApi {
        self.api.clone()
    }

    pub fn eth_filter(&self) -> OpRethFilter {
        self.filter.clone()
    }

    pub fn eth_trace(&self) -> OpRethTrace {
        self.trace.clone()
    }

    pub fn eth_debug(&self) -> OpRethDebug {
        self.debug.clone()
    }

    pub fn eth_tx_pool(&self) -> OpRethTxPool {
        self.tx_pool.clone()
    }

    pub fn eth_db_provider(&self) -> &OpRethDbProvider {
        &self.db_provider
    }
}
