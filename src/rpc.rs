#[cfg(any(feature = "ipc", feature = "ws"))]
use alloy_primitives::TxHash;
use alloy_provider::{Provider, RootProvider};
#[cfg(any(feature = "ipc", feature = "ws"))]
use alloy_pubsub::PubSubFrontend;
use alloy_rpc_client::ClientBuilder;
#[cfg(feature = "ipc")]
use alloy_rpc_client::IpcConnect;
#[cfg(feature = "ws")]
use alloy_rpc_client::WsConnect;
#[cfg(any(feature = "ipc", feature = "ws"))]
use alloy_rpc_types::{Block, Filter, Log, Transaction};
use alloy_transport::Transport;
use alloy_transport_http::{Client, Http};
#[cfg(any(feature = "ipc", feature = "ws"))]
use futures::Stream;
#[cfg(feature = "revm")]
use revm::database_interface::WrapDatabaseAsync;
#[cfg(feature = "revm")]
use revm_database::AlloyDB;

#[cfg(any(feature = "ipc", feature = "ws"))]
use crate::traits::EthStream;

pub struct EthRpcClient<P> {
    provider: RootProvider<P>
}

impl<P> EthRpcClient<P>
where
    P: Transport + Clone
{
    pub fn provider(&self) -> &RootProvider<P> {
        &self.provider
    }
}

#[cfg(any(feature = "ipc", feature = "ws"))]
impl EthRpcClient<PubSubFrontend> {
    #[cfg(feature = "ws")]
    pub async fn new_ws(ws_url: &str) -> eyre::Result<Self> {
        let builder = ClientBuilder::default().ws(WsConnect::new(ws_url)).await?;
        let provider = RootProvider::new(builder);
        Ok(Self { provider })
    }

    #[cfg(feature = "ipc")]
    pub async fn new_ipc(ipc_path: &str) -> eyre::Result<Self> {
        let builder = ClientBuilder::default()
            .ipc(IpcConnect::new(ipc_path.to_string()))
            .await?;
        let provider = RootProvider::new(builder);
        Ok(Self { provider })
    }
}

impl EthRpcClient<Http<Client>> {
    pub fn new_http(http_url: &str) -> eyre::Result<Self> {
        let builder = ClientBuilder::default().http(http_url.parse()?);

        let provider = RootProvider::new(builder);
        Ok(Self { provider })
    }
}

#[cfg(any(feature = "ipc", feature = "ws"))]
impl EthStream for EthRpcClient<PubSubFrontend> {
    async fn block_stream(&self) -> eyre::Result<impl Stream<Item = Block> + Send> {
        Ok(self.provider.subscribe_blocks().await?.into_stream())
    }

    async fn full_pending_transaction_stream(&self) -> eyre::Result<impl Stream<Item = Transaction> + Send> {
        Ok(self
            .provider
            .subscribe_full_pending_transactions()
            .await?
            .into_stream())
    }

    async fn pending_transaction_hashes_stream(&self) -> eyre::Result<impl Stream<Item = TxHash> + Send> {
        Ok(self
            .provider
            .subscribe_pending_transactions()
            .await?
            .into_stream())
    }

    async fn log_stream(&self, filter: Filter) -> eyre::Result<impl Stream<Item = Log> + Send> {
        Ok(self.provider.subscribe_logs(&filter).await?.into_stream())
    }
}

impl<T: Transport + Clone> alloy_provider::Provider<T> for EthRpcClient<T> {
    fn root(&self) -> &RootProvider<T> {
        &self.provider.root()
    }
}

#[cfg(feature = "revm")]
impl<T> crate::traits::AsyncEthRevm for EthRpcClient<T>
where
    T: Transport + Clone
{
    type InnerDb = AlloyDB<T, alloy_network::Ethereum, RootProvider<T>>;

    fn make_inner_db(
        &self,
        block_number: u64,
        handle: tokio::runtime::Handle
    ) -> eyre::Result<WrapDatabaseAsync<Self::InnerDb>> {
        Ok(WrapDatabaseAsync::with_handle(AlloyDB::new(self.provider.clone(), block_number.into()), handle))
    }
}
