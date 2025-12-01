use std::{path::PathBuf, sync::Arc};

use alloy_network::Ethereum;
use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_db::DatabaseEnv;

use reth_network_api::noop::NoopNetwork;
use reth_node_ethereum::{EthEvmConfig, EthereumNode};
use reth_node_types::NodeTypesWithDBAdapter;
use reth_provider::providers::{BlockchainProvider, StaticFileProvider};
use reth_rpc::{DebugApi, EthApi, EthFilter, TraceApi};
use reth_rpc_eth_api::node::RpcNodeCoreAdapter;
use reth_rpc_eth_api::RpcConverter;
use reth_rpc_eth_types::{receipt::EthReceiptConverter, EthConfig, EthFilterConfig};
use reth_tasks::{pool::BlockingTaskGuard, TaskSpawner};
use reth_transaction_pool::{
    blobstore::NoopBlobStore, validate::EthTransactionValidatorBuilder, CoinbaseTipOrdering, EthPooledTransaction,
    EthTransactionValidator, Pool, PoolConfig, TransactionValidationTaskExecutor,
};

use crate::reth_libmdbx::{state_stream::LiveStateStream, NodeClientSpec, RethNodeClient, SupportedChains};

type RethApi = EthApi<
    RpcNodeCoreAdapter<RethDbProvider, RethTxPool, NoopNetwork, EthEvmConfig>,
    RpcConverter<Ethereum, EthEvmConfig, EthReceiptConverter<ChainSpec>>,
>;
type RethFilter = EthFilter<RethApi>;
type RethTrace = TraceApi<RethApi>;
type RethDebug = DebugApi<RethApi>;
type RethTxPool = Pool<
    TransactionValidationTaskExecutor<EthTransactionValidator<RethDbProvider, EthPooledTransaction>>,
    CoinbaseTipOrdering<EthPooledTransaction>,
    NoopBlobStore,
>;
type RethDbProvider = BlockchainProvider<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>;

impl NodeClientSpec for EthereumNode {
    type NodeChainSpec = ChainSpec;
    type Api = RethApi;
    type Filter = RethFilter;
    type Trace = RethTrace;
    type Debug = RethDebug;
    type TxPool = RethTxPool;
    type DbProvider = RethDbProvider;

    fn new_with_db<T: TaskSpawner + Clone + 'static>(
        db: Arc<DatabaseEnv>,
        max_tasks: usize,
        task_executor: T,
        static_files_path: PathBuf,
        chain_spec: Arc<Self::NodeChainSpec>,
    ) -> eyre::Result<RethNodeClient<Self>> {
        let static_file_provider = StaticFileProvider::read_only(static_files_path.clone(), true)?;
        let provider_factory = EthereumNode::provider_factory_builder()
            .db(db)
            .chainspec(chain_spec.clone())
            .static_file(static_file_provider)
            .build_provider_factory();

        let blockchain_provider = BlockchainProvider::new(provider_factory.clone())?;

        let transaction_validator = EthTransactionValidatorBuilder::new(blockchain_provider.clone())
            .build_with_tasks(task_executor.clone(), NoopBlobStore::default());

        let tx_pool = Pool::eth_pool(transaction_validator, NoopBlobStore::default(), PoolConfig::default());

        let api =
            EthApi::builder(blockchain_provider.clone(), tx_pool.clone(), NoopNetwork::default(), EthEvmConfig::mainnet())
                .build();

        let tracing_call_guard = BlockingTaskGuard::new(max_tasks);
        let trace = TraceApi::new(api.clone(), tracing_call_guard.clone(), EthConfig::default());

        let debug = DebugApi::new(api.clone(), tracing_call_guard);
        let filter = EthFilter::new(api.clone(), EthFilterConfig::default(), Box::new(task_executor.clone()));

        let chain =
            SupportedChains::from_chain_id(chain_spec.chain_id()).ok_or_else(|| eyre::eyre!("chain no supported"))?;
        let live_state_stream = LiveStateStream::new(api.clone(), chain);

        Ok(RethNodeClient {
            api,
            trace,
            filter,
            debug,
            tx_pool,
            db_provider: blockchain_provider,
            chain_spec,
            live_state_stream,
            chain,
        })
    }
}

#[cfg(test)]
mod tests {
    use alloy_network::Ethereum;
    use alloy_provider::{IpcConnect, Provider, RootProvider};
    use alloy_rpc_client::ClientBuilder;
    use alloy_rpc_types::Filter;
    use futures::StreamExt;
    use reth_chainspec::MAINNET;
    use reth_node_ethereum::EthereumNode;
    use reth_provider::StaticFileProviderFactory;
    use reth_rpc_eth_api::EthApiServer;

    use crate::test_utils::stream_timeout;
    use crate::traits::EthStream;

    use crate::reth_libmdbx::RethNodeClientBuilder;

    #[tokio::test]
    async fn test_read_live_blocks() {
        let reth_client = RethNodeClientBuilder::<EthereumNode>::new("/var/lib/eth/mainnet/reth", 100000, MAINNET.clone())
            .build()
            .unwrap();

        let ipc_builder = ClientBuilder::default()
            .ipc(IpcConnect::new("/tmp/mainnet/reth.ipc".to_string()))
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
                .eth_db_provider()
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

    #[tokio::test]
    #[serial_test::serial]
    async fn can_build() {
        let builder = RethNodeClientBuilder::<EthereumNode>::new("/var/lib/eth/mainnet/reth/", 1000, MAINNET.clone());
        assert!(builder.build().is_ok())
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn test_block_stream() {
        let builder = RethNodeClientBuilder::<EthereumNode>::new("/var/lib/eth/mainnet/reth/", 1000, MAINNET.clone());
        let client = builder.build().unwrap();

        let block_stream = client.block_stream().await.unwrap();
        assert!(stream_timeout(block_stream, 2, 30).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn test_log_stream() {
        let builder = RethNodeClientBuilder::<EthereumNode>::new("/var/lib/eth/mainnet/reth/", 1000, MAINNET.clone());
        let client = builder.build().unwrap();

        let log_stream = client.log_stream(Filter::new()).await.unwrap();
        assert!(stream_timeout(log_stream, 2, 30).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn test_full_pending_transaction_stream() {
        let builder = RethNodeClientBuilder::<EthereumNode>::new("/var/lib/eth/mainnet/reth/", 1000, MAINNET.clone());
        let client = builder.build().unwrap();

        let mempool_full_stream = client.full_pending_transaction_stream().await.unwrap();
        assert!(stream_timeout(mempool_full_stream, 2, 30).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn test_pending_transaction_hashes_stream() {
        let builder = RethNodeClientBuilder::<EthereumNode>::new("/var/lib/eth/mainnet/reth/", 1000, MAINNET.clone());
        let client = builder.build().unwrap();

        let mempool_hash_stream = client.pending_transaction_hashes_stream().await.unwrap();
        assert!(stream_timeout(mempool_hash_stream, 2, 30).await.is_ok());
    }
}
