use std::{path::PathBuf, sync::Arc};

use alloy_network::Ethereum;
use reth_chainspec::ChainSpec;
use reth_db::DatabaseEnv;

use exe_runners::TaskSpawner;
use reth_network_api::noop::NoopNetwork;
use reth_node_ethereum::{EthEvmConfig, EthereumNode};
use reth_node_types::NodeTypesWithDBAdapter;
use reth_provider::providers::{BlockchainProvider, StaticFileProvider};
use reth_rpc::{DebugApi, EthApi, EthFilter, TraceApi};
use reth_rpc_eth_api::node::RpcNodeCoreAdapter;
use reth_rpc_eth_api::RpcConverter;
use reth_rpc_eth_types::{receipt::EthReceiptConverter, EthConfig, EthFilterConfig};
use reth_tasks::pool::BlockingTaskGuard;
use reth_transaction_pool::{
    blobstore::NoopBlobStore, validate::EthTransactionValidatorBuilder, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, PoolConfig, TransactionValidationTaskExecutor,
};

use super::RethLibmdbxClient;

pub(super) type RethApi = EthApi<
    RpcNodeCoreAdapter<RethDbProvider, RethTxPool, NoopNetwork, EthEvmConfig>,
    RpcConverter<Ethereum, EthEvmConfig, EthReceiptConverter<ChainSpec>>,
>;
pub(super) type RethFilter = EthFilter<RethApi>;
pub(super) type RethTrace = TraceApi<RethApi>;
pub(super) type RethDebug = DebugApi<RethApi>;
pub(super) type RethTxPool = Pool<
    TransactionValidationTaskExecutor<EthTransactionValidator<RethDbProvider, EthPooledTransaction>>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    NoopBlobStore,
>;
pub(super) type RethDbProvider = BlockchainProvider<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>;

/// spawns the reth libmdbx client
pub(super) fn new_with_db<T: TaskSpawner + Clone + 'static>(
    db: Arc<DatabaseEnv>,
    max_tasks: usize,
    task_executor: T,
    static_files_path: PathBuf,
    chain: Arc<ChainSpec>,
) -> eyre::Result<RethLibmdbxClient> {
    let static_file_provider = StaticFileProvider::read_only(static_files_path.clone(), true)?;
    let provider_factory = EthereumNode::provider_factory_builder()
        .db(db)
        .chainspec(chain.clone())
        .static_file(static_file_provider)
        .build_provider_factory();

    let blockchain_provider = BlockchainProvider::new(provider_factory.clone())?;

    let transaction_validator = EthTransactionValidatorBuilder::new(blockchain_provider.clone())
        .build_with_tasks(task_executor.clone(), NoopBlobStore::default());

    let tx_pool = Pool::eth_pool(transaction_validator, NoopBlobStore::default(), PoolConfig::default());

    let api = EthApi::builder(blockchain_provider.clone(), tx_pool.clone(), NoopNetwork::default(), EthEvmConfig::mainnet())
        .build();

    let tracing_call_guard = BlockingTaskGuard::new(max_tasks);
    let trace = TraceApi::new(api.clone(), tracing_call_guard.clone(), EthConfig::default());

    let debug = DebugApi::new(api.clone(), tracing_call_guard);
    let filter = EthFilter::new(api.clone(), EthFilterConfig::default(), Box::new(task_executor.clone()));

    Ok(RethLibmdbxClient { api, trace, filter, debug, tx_pool, db_provider: blockchain_provider })
}

#[cfg(test)]
mod tests {
    use alloy_network::Ethereum;
    use alloy_provider::{IpcConnect, Provider, RootProvider};
    use alloy_rpc_client::ClientBuilder;
    use futures::StreamExt;
    use reth_chainspec::MAINNET;
    use reth_provider::StaticFileProviderFactory;
    use reth_rpc_eth_api::EthApiServer;

    use crate::reth_libmdbx::RethLibmdbxClientBuilder;

    #[tokio::test]
    async fn test_read_live_blocks() {
        let reth_client = RethLibmdbxClientBuilder::new("/home/data/reth", 100000, MAINNET.clone())
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
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
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
