use std::{path::PathBuf, sync::Arc};

use reth_chainspec::MAINNET;
use reth_db::DatabaseEnv;

use reth_network_api::noop::NoopNetwork;
use reth_node_ethereum::{
    BasicBlockExecutorProvider, EthEvmConfig, EthExecutionStrategyFactory, EthExecutorProvider, EthereumNode,
};
use reth_node_types::NodeTypesWithDBAdapter;

use reth_provider::{
    providers::{BlockchainProvider, StaticFileProvider},
    ProviderFactory,
};

use reth_rpc::{DebugApi, EthApi, EthFilter, TraceApi};
use reth_rpc_eth_types::{EthApiBuilderCtx, EthConfig, EthFilterConfig, EthStateCache, EthStateCacheConfig};
use reth_tasks::{pool::BlockingTaskGuard, TaskSpawner};
use reth_transaction_pool::{
    blobstore::NoopBlobStore, validate::EthTransactionValidatorBuilder, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, PoolConfig, TransactionValidationTaskExecutor,
};

use super::RethLibmdbxClient;

pub(super) type RethProvider = BlockchainProvider<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>;
pub(super) type RethDbProvider = BlockchainProvider<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>;
pub(super) type RethApi = EthApi<RethProvider, RethTxPool, NoopNetwork, EthEvmConfig>;
pub(super) type RethFilter = EthFilter<RethApi>;
pub(super) type RethTrace = TraceApi<RethApi>;
pub(super) type RethDebug = DebugApi<RethApi, BasicBlockExecutorProvider<EthExecutionStrategyFactory>>;
pub(super) type RethTxPool = Pool<
    TransactionValidationTaskExecutor<EthTransactionValidator<RethProvider, EthPooledTransaction>>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    NoopBlobStore,
>;

/// spawns the reth libmdbx client
pub(super) fn new_with_db<T: TaskSpawner + Clone + 'static>(
    db: Arc<DatabaseEnv>,
    max_tasks: usize,
    task_executor: T,
    static_files_path: PathBuf,
) -> eyre::Result<RethLibmdbxClient> {
    let chain = MAINNET.clone();
    let static_file_provider = StaticFileProvider::read_only(static_files_path.clone(), true)?;

    let provider_factory: ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>> =
        ProviderFactory::new(db.clone(), chain.clone(), static_file_provider);

    let provider = BlockchainProvider::new(provider_factory.clone()).unwrap();

    let state_cache = EthStateCache::spawn_with(provider.clone(), EthStateCacheConfig::default(), task_executor.clone());

    let transaction_validator = EthTransactionValidatorBuilder::new(provider.clone())
        .build_with_tasks(task_executor.clone(), NoopBlobStore::default());

    let tx_pool = Pool::eth_pool(transaction_validator.clone(), NoopBlobStore::default(), PoolConfig::default());

    let ctx = EthApiBuilderCtx {
        provider: provider.clone(),
        pool: tx_pool.clone(),
        network: NoopNetwork::default(),
        evm_config: EthEvmConfig::new(chain.clone()),
        config: EthConfig::default(),
        executor: task_executor.clone(),
        cache: state_cache.clone(),
    };

    let api = EthApi::with_spawner(&ctx);

    let tracing_call_guard = BlockingTaskGuard::new(max_tasks);
    let trace = TraceApi::new(api.clone(), tracing_call_guard.clone());

    let provider_executor = EthExecutorProvider::ethereum(chain.clone());
    let debug = DebugApi::new(api.clone(), tracing_call_guard, provider_executor);
    let filter = EthFilter::new(api.clone(), EthFilterConfig::default(), Box::new(task_executor.clone()));

    Ok(RethLibmdbxClient { api, trace, filter, debug, tx_pool, db_provider: provider })
}

#[cfg(test)]
mod tests {
    use alloy_network::Ethereum;
    use alloy_provider::{IpcConnect, Provider, RootProvider};
    use alloy_rpc_client::ClientBuilder;
    use futures::StreamExt;
    use reth_provider::StaticFileProviderFactory;
    use reth_rpc_eth_api::EthApiServer;

    use crate::reth_libmdbx::RethLibmdbxClientBuilder;

    #[tokio::test]
    async fn test_read_live_blocks() {
        let reth_client = RethLibmdbxClientBuilder::new("/home/data/reth", 100000)
            .build()
            .unwrap();

        let ipc_builder = ClientBuilder::default()
            .ipc(IpcConnect::new("/tmp/reth.ipc".to_string()))
            .await
            .unwrap();
        let ipc_provider = RootProvider::<Ethereum>::new(ipc_builder);

        let mut block_stream = ipc_provider
            .subscribe_blocks()
            .await
            .unwrap()
            .into_stream()
            .take(5);

        while let Some(block_header) = block_stream.next().await {
            reth_client
                .db_provider
                .static_file_provider()
                .initialize_index()
                .unwrap();
            let full_block = reth_client
                .eth_api()
                .block_by_number(block_header.number.into(), true)
                .await
                .unwrap();
            assert!(full_block.is_some())
        }
    }
}
