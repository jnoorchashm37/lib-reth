use std::{path::PathBuf, sync::Arc};

use alloy_rpc_types_eth::TransactionInfo;

use op_alloy_consensus::{
    transaction::{OpDepositInfo, OpTransactionInfo},
    OpTxEnvelope,
};
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
use reth_optimism_rpc::eth::receipt::OpReceiptConverter;
use reth_provider::providers::{BlockchainProvider, StaticFileProvider};
use reth_rpc::{DebugApi, EthApi, EthFilter, TraceApi};
use reth_rpc_eth_api::{node::RpcNodeCoreAdapter, RpcConverter, TxInfoMapper};
use reth_transaction_pool::{blobstore::NoopBlobStore, CoinbaseTipOrdering, Pool, TransactionValidationTaskExecutor};

use crate::reth_libmdbx::{NodeClientSpec, RethNodeClient};

type OpRethApi = EthApi<
    RpcNodeCoreAdapter<OpRethDbProvider, OpRethTxPool, NoopNetwork, OpEvmConfig>,
    RpcConverter<Optimism, OpEvmConfig, OpReceiptConverter<OpRethDbProvider>, (), SimpleOpTxInfoMapper>,
>;
type OpRethFilter = EthFilter<OpRethApi>;
type OpRethTrace = TraceApi<OpRethApi>;
type OpRethDebug = DebugApi<OpRethApi>;
type OpRethTxPool = Pool<
    TransactionValidationTaskExecutor<
        OpTransactionValidator<BlockchainProvider<NodeTypesWithDBAdapter<OpNode, Arc<DatabaseEnv>>>, OpPooledTransaction>,
    >,
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
        let rpc_converter =
            RpcConverter::new(OpReceiptConverter::new(blockchain_provider.clone())).with_mapper(SimpleOpTxInfoMapper);

        let api = EthApi::builder(blockchain_provider.clone(), tx_pool.clone(), NoopNetwork::default(), evm_config)
            .with_rpc_converter(rpc_converter)
            .build();

        let tracing_call_guard = BlockingTaskGuard::new(max_tasks);
        let trace = TraceApi::new(api.clone(), tracing_call_guard.clone(), EthConfig::default());

        let debug = DebugApi::new(api.clone(), tracing_call_guard);
        let filter = EthFilter::new(api.clone(), EthFilterConfig::default(), Box::new(task_executor.clone()));

        Ok(RethNodeClient { api, trace, filter, debug, tx_pool, db_provider: blockchain_provider, chain_spec })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SimpleOpTxInfoMapper;

impl TxInfoMapper<OpTxEnvelope> for SimpleOpTxInfoMapper {
    type Out = OpTransactionInfo;
    type Err = std::convert::Infallible;

    fn try_map(&self, _tx: &OpTxEnvelope, tx_info: TransactionInfo) -> Result<Self::Out, Self::Err> {
        Ok(OpTransactionInfo::new(tx_info, OpDepositInfo::default()))
    }
}

// #[cfg(test)]
// mod tests {

//     use std::fmt::Debug;

//     use reth_chainspec::MAINNET;

//     use super::*;

//     async fn stream_timeout<O: Debug>(
//         stream: impl Stream<Item = O> + Unpin,
//         values: usize,
//         timeout: u64,
//     ) -> eyre::Result<()> {
//         let mut sub_stream = stream.take(values);
//         let f = async move {
//             let mut vals = values;
//             while let Some(v) = sub_stream.next().await {
//                 println!("{v:?}");
//                 vals -= 1;
//                 if vals == 0 {
//                     break;
//                 }
//             }
//         };

//         tokio::select! {
//             _ = f => (),
//             _ = tokio::time::sleep(std::time::Duration::from_secs(timeout)) => eyre::bail!("timed out")
//         };

//         Ok(())
//     }

//     #[tokio::test]
//     #[serial_test::serial]
//     async fn can_build() {
//         let builder = RethNodeClientBuilder::new("/var/lib/eth/mainnet/reth/", 1000, MAINNET.clone());
//         assert!(builder.build().is_ok())
//     }

//     #[tokio::test(flavor = "multi_thread")]
//     #[serial_test::serial]
//     async fn test_block_stream() {
//         let builder = RethNodeClientBuilder::new("/var/lib/eth/mainnet/reth/", 1000, MAINNET.clone());
//         let client = builder.build().unwrap();

//         let block_stream = client.block_stream().await.unwrap();
//         assert!(stream_timeout(block_stream, 2, 30).await.is_ok());
//     }

//     #[tokio::test(flavor = "multi_thread")]
//     #[serial_test::serial]
//     async fn test_log_stream() {
//         let builder = RethNodeClientBuilder::new("/var/lib/eth/mainnet/reth/", 1000, MAINNET.clone());
//         let client = builder.build().unwrap();

//         let log_stream = client.log_stream(Filter::new()).await.unwrap();
//         assert!(stream_timeout(log_stream, 2, 30).await.is_ok());
//     }

//     #[tokio::test(flavor = "multi_thread")]
//     #[serial_test::serial]
//     async fn test_full_pending_transaction_stream() {
//         let builder = RethNodeClientBuilder::new("/var/lib/eth/mainnet/reth/", 1000, MAINNET.clone());
//         let client = builder.build().unwrap();

//         let mempool_full_stream = client.full_pending_transaction_stream().await.unwrap();
//         assert!(stream_timeout(mempool_full_stream, 2, 30).await.is_ok());
//     }

//     #[tokio::test(flavor = "multi_thread")]
//     #[serial_test::serial]
//     async fn test_pending_transaction_hashes_stream() {
//         let builder = RethNodeClientBuilder::new("/var/lib/eth/mainnet/reth/", 1000, MAINNET.clone());
//         let client = builder.build().unwrap();

//         let mempool_hash_stream = client.pending_transaction_hashes_stream().await.unwrap();
//         assert!(stream_timeout(mempool_hash_stream, 2, 30).await.is_ok());
//     }
// }
