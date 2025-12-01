use std::sync::Arc;
use std::{path::PathBuf, pin::Pin};

use crate::reth_libmdbx::state_stream::LiveStateStreamError;
#[cfg(feature = "revm")]
use crate::traits::BlockNumberOrHash;
use crate::{
    reth_libmdbx::{
        state_stream::{LiveStateStream, NodeBlock},
        SupportedChains,
    },
    traits::EthStream,
};

use alloy_primitives::TxHash;
use alloy_rpc_types::{eth::Filter, Log};
use futures::{Stream, StreamExt};
use reth_db::DatabaseEnv;
use reth_node_types::NodeTypes;
use reth_provider::{BlockNumReader, DatabaseProviderFactory, StateProviderFactory, TryIntoHistoricalStateProvider};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;

use reth_provider::CanonStateSubscriptions;
use reth_rpc_eth_api::{helpers::FullEthApi, EthApiTypes, FullEthApiServer, RpcNodeCore};
use tokio_stream::wrappers::BroadcastStream;

pub mod node;
#[cfg(feature = "op-reth-libmdbx")]
pub mod op_node;

pub trait NodeClientSpec: NodeTypes + Send + Sync {
    type NodeChainSpec: Clone + Send + Sync;
    type Api: FullEthApi + FullEthApiServer + EthApiTypes + RpcNodeCore + Clone + Send + Sync;
    type Filter: Clone + Send + Sync;
    type Trace: Clone + Send + Sync;
    type Debug: Clone + Send + Sync;
    type TxPool: Clone + Send + Sync;
    type DbProvider: DatabaseProviderFactory<Provider: TryIntoHistoricalStateProvider + BlockNumReader>
        + StateProviderFactory
        + CanonStateSubscriptions
        + Send
        + Sync
        + Clone
        + 'static;

    fn new_with_db<T: TaskSpawner + Clone + 'static>(
        db: Arc<DatabaseEnv>,
        max_tasks: usize,
        task_executor: T,
        static_files_path: PathBuf,
        chain: Arc<Self::NodeChainSpec>,
    ) -> eyre::Result<RethNodeClient<Self>>;
}

pub struct RethNodeClient<N: NodeClientSpec> {
    api: N::Api,
    filter: N::Filter,
    trace: N::Trace,
    debug: N::Debug,
    tx_pool: N::TxPool,
    db_provider: N::DbProvider,
    live_state_stream: LiveStateStream<N>,
    chain: SupportedChains,
    chain_spec: Arc<<N as NodeClientSpec>::NodeChainSpec>,
}

impl<N: NodeClientSpec> RethNodeClient<N> {
    pub fn chain_spec(&self) -> Arc<<N as NodeClientSpec>::NodeChainSpec> {
        self.chain_spec.clone()
    }

    pub fn eth_api(&self) -> N::Api {
        self.api.clone()
    }

    pub fn eth_filter(&self) -> N::Filter {
        self.filter.clone()
    }

    pub fn eth_trace(&self) -> N::Trace {
        self.trace.clone()
    }

    pub fn eth_debug(&self) -> N::Debug {
        self.debug.clone()
    }

    pub fn eth_tx_pool(&self) -> N::TxPool {
        self.tx_pool.clone()
    }

    pub fn eth_db_provider(&self) -> &N::DbProvider {
        &self.db_provider
    }
}

impl<N: NodeClientSpec> EthStream for RethNodeClient<N> {
    type TxEnvelope = <<<N as NodeClientSpec>::Api as RpcNodeCore>::Pool as TransactionPool>::Transaction;
    type FullBlock = NodeBlock<N>;

    async fn block_stream(&self) -> eyre::Result<impl Stream<Item = Result<Self::FullBlock, LiveStateStreamError>> + Send> {
        let rx = BroadcastStream::new(self.live_state_stream.subscribe_blocks()).map(|v| match v {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(LiveStateStreamError::StreamError(e.to_string())),
        });
        Ok(Box::pin(rx) as Pin<Box<dyn Stream<Item = Result<Self::FullBlock, LiveStateStreamError>> + Send + Sync>>)
    }

    // async fn full_pending_transaction_stream(&self) -> eyre::Result<impl Stream<Item = Self::TxEnvelope> + Send> {
    //     let stream = self
    //         .eth_api()
    //         .pool()
    //         .new_pending_pool_transactions_listener()
    //         .map(|pooled_tx| pooled_tx.transaction.transaction.clone());

    //     Ok(stream)
    // }

    // async fn pending_transaction_hashes_stream(&self) -> eyre::Result<impl Stream<Item = TxHash> + Send> {
    //     let stream = self
    //         .eth_api()
    //         .pool()
    //         .new_pending_pool_transactions_listener()
    //         .map(|tx| *tx.transaction.hash());

    //     Ok(stream)
    // }

    async fn log_stream(
        &self,
        filter: Filter,
    ) -> eyre::Result<impl Stream<Item = Result<Log, LiveStateStreamError>> + Send> {
        let rx = BroadcastStream::new(self.live_state_stream.subscribe_logs()).flat_map(move |v| {
            futures::stream::iter(match v {
                Ok(Ok(logs)) => logs
                    .into_iter()
                    .filter(|log| filter.matches(&log.inner))
                    .map(Ok)
                    .collect::<Vec<_>>(),
                Ok(Err(e)) => vec![Err(e)],
                Err(e) => vec![Err(LiveStateStreamError::StreamError(e.to_string()))],
            })
        });

        Ok(Box::pin(rx) as Pin<Box<dyn Stream<Item = Result<Log, LiveStateStreamError>> + Send + Sync>>)
    }
}

#[cfg(feature = "revm")]
impl<N: NodeClientSpec> crate::traits::EthRevm for RethNodeClient<N> {
    type InnerDb = crate::traits::reth_revm_utils::RethLibmdbxDatabaseRef;

    fn make_inner_db<T: Into<BlockNumberOrHash>>(&self, block: T) -> eyre::Result<Self::InnerDb> {
        use reth_provider::StateProviderFactory;

        let block: BlockNumberOrHash = block.into();

        let state_provider = match block {
            BlockNumberOrHash::Number(num) => self
                .eth_db_provider()
                .state_by_block_number_or_tag(num.into())?,
            BlockNumberOrHash::Hash(hash) => self.eth_db_provider().state_by_block_hash(hash)?,
        };

        let this = reth_revm::database::StateProviderDatabase::new(state_provider);
        Ok(crate::traits::reth_revm_utils::RethLibmdbxDatabaseRef::new(this))
    }
}
