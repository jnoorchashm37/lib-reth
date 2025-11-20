use std::{path::PathBuf, sync::Arc};

use alloy_primitives::U256;
use alloy_rpc_types::TransactionInfo;
use op_alloy_consensus::transaction::{OpDepositInfo, OpTransactionInfo};
use reth_rpc_eth_types::{EthConfig, EthFilterConfig};
use reth_tasks::pool::BlockingTaskGuard;
use reth_transaction_pool::{validate::EthTransactionValidatorBuilder, PoolConfig};

use op_alloy_network::Optimism;
use reth_db::DatabaseEnv;
use reth_network_api::noop::NoopNetwork;
use reth_node_types::NodeTypesWithDBAdapter;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_evm::OpEvmConfig;
use reth_optimism_node::{
    txpool::{OpPooledTransaction, OpTransactionValidator},
    OpNode,
};
use reth_optimism_primitives::OpTransactionSigned;
use reth_optimism_rpc::eth::{receipt::OpReceiptConverter, transaction::OpTxInfoMapper};
use reth_optimism_rpc::OpEthApi;
use reth_provider::providers::{BlockchainProvider, StaticFileProvider};
use reth_rpc::{DebugApi, EthApi, EthFilter, TraceApi};
use reth_rpc_eth_api::{node::RpcNodeCoreAdapter, RpcConverter, TxInfoMapper};
use reth_transaction_pool::{blobstore::NoopBlobStore, CoinbaseTipOrdering, Pool, TransactionValidationTaskExecutor};

use crate::reth_libmdbx::{NodeClientSpec, RethNodeClient};

type OpRethApi = OpEthApi<
    RpcNodeCoreAdapter<OpRethDbProvider, OpRethTxPool, NoopNetwork, OpEvmConfig>,
    RpcConverter<Optimism, OpEvmConfig, OpReceiptConverter<OpRethDbProvider>, (), OpTxInfoMapper<OpRethDbProvider>>,
>;
type OpRethFilter = EthFilter<OpRethApi>;
type OpRethTrace = TraceApi<OpRethApi>;
type OpRethDebug = DebugApi<OpRethApi>;
type OpRethTxPool = Pool<
    TransactionValidationTaskExecutor<OpTransactionValidator<OpRethDbProvider, OpPooledTransaction>>,
    CoinbaseTipOrdering<OpPooledTransaction>,
    NoopBlobStore,
>;

type OpRethDbProvider = BlockchainProvider<NodeTypesWithDBAdapter<OpNode, Arc<DatabaseEnv>>>;

impl NodeClientSpec for OpNode {
    type NodeChainSpec = OpChainSpec;
    type Api = OpRethApi;
    type Filter = OpRethFilter;
    type Trace = OpRethTrace;
    type Debug = OpRethDebug;
    type TxPool = OpRethTxPool;
    type DbProvider = OpRethDbProvider;

    fn new_with_db<T: reth_tasks::TaskSpawner + Clone + 'static>(
        db: Arc<DatabaseEnv>,
        max_tasks: usize,
        task_executor: T,
        static_files_path: PathBuf,
        chain_spec: Arc<Self::NodeChainSpec>,
    ) -> eyre::Result<RethNodeClient<Self>> {
        let static_file_provider = StaticFileProvider::read_only(static_files_path.clone(), true)?;
        let provider_factory = OpNode::provider_factory_builder()
            .db(db)
            .chainspec(chain_spec.clone())
            .static_file(static_file_provider)
            .build_provider_factory();

        let blockchain_provider = BlockchainProvider::new(provider_factory.clone())?;

        let transaction_validator = EthTransactionValidatorBuilder::new(blockchain_provider.clone())
            .build_with_tasks(task_executor.clone(), NoopBlobStore::default())
            .map(OpTransactionValidator::new);

        let tx_pool = Pool::new(
            transaction_validator,
            CoinbaseTipOrdering::default(),
            NoopBlobStore::default(),
            PoolConfig::default(),
        );

        let evm_config = OpEvmConfig::optimism(chain_spec.clone());
        let rpc_converter = RpcConverter::new(OpReceiptConverter::new(blockchain_provider.clone()))
            .with_mapper(OpTxInfoMapper::new(blockchain_provider.clone()));

        let eth_api_inner =
            EthApi::builder(blockchain_provider.clone(), tx_pool.clone(), NoopNetwork::default(), evm_config)
                .with_rpc_converter(rpc_converter)
                .build_inner();
        let api = OpEthApi::new(eth_api_inner, None, U256::from(1_000_000u64), None);

        let tracing_call_guard = BlockingTaskGuard::new(max_tasks);
        let trace = TraceApi::new(api.clone(), tracing_call_guard.clone(), EthConfig::default());

        let debug = DebugApi::new(api.clone(), tracing_call_guard);
        let filter = EthFilter::new(api.clone(), EthFilterConfig::default(), Box::new(task_executor.clone()));

        Ok(RethNodeClient { api, trace, filter, debug, tx_pool, db_provider: blockchain_provider, chain_spec })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SimpleOpTxInfoMapper;

impl TxInfoMapper<OpTransactionSigned> for SimpleOpTxInfoMapper {
    type Out = OpTransactionInfo;
    type Err = std::convert::Infallible;

    fn try_map(&self, _tx: &OpTransactionSigned, tx_info: TransactionInfo) -> Result<Self::Out, Self::Err> {
        Ok(OpTransactionInfo::new(tx_info, OpDepositInfo::default()))
    }
}

pub fn get_op_superchain_spec(str: &str) -> Arc<OpChainSpec> {
    reth_optimism_chainspec::generated_chain_value_parser(str).unwrap()
}

#[cfg(test)]
mod tests {
    use crate::test_utils::stream_timeout;
    use crate::traits::EthStream;
    use alloy_network::Ethereum;
    use alloy_provider::{IpcConnect, Provider, RootProvider};
    use alloy_rpc_client::ClientBuilder;
    use alloy_rpc_types::Filter;
    use futures::StreamExt;
    use reth_optimism_chainspec::BASE_MAINNET;
    use reth_optimism_node::OpNode;
    use reth_provider::StaticFileProviderFactory;
    use reth_rpc_eth_api::EthApiServer;

    use crate::reth_libmdbx::RethNodeClientBuilder;

    #[tokio::test]
    async fn test_read_live_blocks() {
        let reth_client = RethNodeClientBuilder::<OpNode>::new("/var/lib/eth/mainnet/reth", 100000, BASE_MAINNET.clone())
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

        let api = reth_client.eth_api();
        while let Some(block_header) = block_stream.next().await {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            reth_client
                .eth_db_provider()
                .static_file_provider()
                .initialize_index()
                .unwrap();

            let full_block = api
                .block_by_number(block_header.number.into(), true)
                .await
                .unwrap();
            assert!(full_block.is_some())
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn can_build() {
        let builder = RethNodeClientBuilder::<OpNode>::new("/var/lib/eth/base/reth/", 1000, BASE_MAINNET.clone());
        assert!(builder.build().is_ok())
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn test_block_stream() {
        let builder = RethNodeClientBuilder::<OpNode>::new("/var/lib/eth/base/reth/", 1000, BASE_MAINNET.clone());
        let client = builder.build().unwrap();

        let block_stream = client.block_stream().await.unwrap();
        assert!(stream_timeout(block_stream, 2, 30).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn test_log_stream() {
        let builder = RethNodeClientBuilder::<OpNode>::new("/var/lib/eth/base/reth/", 1000, BASE_MAINNET.clone());
        let client = builder.build().unwrap();

        let log_stream = client.log_stream(Filter::new()).await.unwrap();
        assert!(stream_timeout(log_stream, 2, 30).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn test_full_pending_transaction_stream() {
        let builder = RethNodeClientBuilder::<OpNode>::new("/var/lib/eth/base/reth/", 1000, BASE_MAINNET.clone());
        let client = builder.build().unwrap();

        let mempool_full_stream = client.full_pending_transaction_stream().await.unwrap();
        assert!(stream_timeout(mempool_full_stream, 2, 30).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn test_pending_transaction_hashes_stream() {
        let builder = RethNodeClientBuilder::<OpNode>::new("/var/lib/eth/base/reth/", 1000, BASE_MAINNET.clone());
        let client = builder.build().unwrap();

        let mempool_hash_stream = client.pending_transaction_hashes_stream().await.unwrap();
        assert!(stream_timeout(mempool_hash_stream, 2, 30).await.is_ok());
    }
}
