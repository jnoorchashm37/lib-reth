use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "revm")]
use crate::traits::BlockNumberOrHash;
use crate::traits::EthStream;

use alloy_network::Network;
use alloy_provider::{builder, IpcConnect, RootProvider, WsConnect};
use reth_db::DatabaseEnv;
use reth_node_types::NodeTypes;
use reth_provider::{BlockNumReader, DatabaseProviderFactory, StateProviderFactory, TryIntoHistoricalStateProvider};
use reth_tasks::TaskSpawner;

use reth_provider::CanonStateSubscriptions;
use reth_rpc_eth_api::{helpers::FullEthApi, EthApiTypes, FullEthApiServer, RpcNodeCore};

pub mod node;
#[cfg(feature = "op-reth-libmdbx")]
pub mod op_node;

pub trait NodeClientSpec: NodeTypes + Send + Sync {
    type AlloyNetwork: Network;
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
        ipc_path_or_rpc_url: Option<String>,
    ) -> eyre::Result<RethNodeClient<Self>>;
}

pub struct RethNodeClient<N: NodeClientSpec> {
    api: N::Api,
    filter: N::Filter,
    trace: N::Trace,
    debug: N::Debug,
    tx_pool: N::TxPool,
    db_provider: N::DbProvider,
    chain_spec: Arc<<N as NodeClientSpec>::NodeChainSpec>,
    ipc_path_or_rpc_url: Option<String>,
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

#[async_trait::async_trait]
impl<N: NodeClientSpec> EthStream<<N as NodeClientSpec>::AlloyNetwork> for RethNodeClient<N> {
    async fn root_provider(&self) -> eyre::Result<RootProvider<<N as NodeClientSpec>::AlloyNetwork>> {
        let conn_url = self
            .ipc_path_or_rpc_url
            .as_ref()
            .ok_or_else(|| eyre::eyre!("no ipc path or rpc url has been set"))?;

        let builder = builder::<<N as NodeClientSpec>::AlloyNetwork>();

        let client = if conn_url.ends_with(".ipc") {
            builder
                .connect_ipc(IpcConnect::new(conn_url.to_string()))
                .await?
        } else if conn_url.starts_with("ws:") || conn_url.contains("wss:") {
            builder.connect_ws(WsConnect::new(conn_url)).await?
        } else if conn_url.starts_with("http:") || conn_url.contains("https:") {
            builder.connect_http(conn_url.parse()?)
        } else {
            builder.connect(conn_url).await?
        };

        Ok(client)
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
