use alloy_consensus::TxEnvelope;
use alloy_network::Network;
#[cfg(any(feature = "ipc", feature = "ws"))]
use alloy_primitives::TxHash;
use alloy_provider::{Provider, RootProvider};
#[cfg(any(feature = "ipc", feature = "ws"))]
use alloy_rpc_client::ClientBuilder;
#[cfg(feature = "ipc")]
use alloy_rpc_client::IpcConnect;
#[cfg(feature = "ws")]
use alloy_rpc_client::WsConnect;
#[cfg(any(feature = "ipc", feature = "ws"))]
use alloy_rpc_types::{Filter, Log};
#[cfg(any(feature = "ipc", feature = "ws"))]
use futures::Stream;
use futures::StreamExt;
// #[cfg(feature = "revm")]
// use revm::database_interface::WrapDatabaseAsync;
#[cfg(feature = "revm")]
use revm_database::{AlloyDB, WrapDatabaseAsync};

#[cfg(any(feature = "ipc", feature = "ws"))]
use crate::traits::EthStream;

pub struct EthRpcClient<P> {
    provider: P,
}

impl<P> EthRpcClient<P>
where
    P: Provider + Clone,
{
    pub fn provider(&self) -> &P {
        &self.provider
    }
}

#[cfg(any(feature = "ipc", feature = "ws"))]
impl<N: Network> EthRpcClient<RootProvider<N>> {
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

    pub fn new_http(http_url: &str) -> eyre::Result<Self> {
        let builder = ClientBuilder::default().http(http_url.parse()?);
        let provider = RootProvider::new(builder);
        Ok(Self { provider })
    }
}

#[cfg(any(feature = "ipc", feature = "ws"))]
impl<P: Provider + Clone> EthStream for EthRpcClient<P> {
    async fn block_stream(&self) -> eyre::Result<impl Stream<Item = alloy_rpc_types_eth::Header> + Send> {
        Ok(self.provider.subscribe_blocks().await?.into_stream())
    }

    async fn full_pending_transaction_stream(&self) -> eyre::Result<impl Stream<Item = TxEnvelope> + Send> {
        Ok(self
            .provider
            .subscribe_full_pending_transactions()
            .await?
            .into_stream()
            .map(|val| val.inner))
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

impl<P: Provider + Clone> Provider for EthRpcClient<P> {
    fn root(&self) -> &RootProvider {
        &self.provider.root()
    }
}

#[cfg(feature = "revm")]
impl<P: Provider + Clone> crate::traits::AsyncEthRevm for EthRpcClient<P> {
    type InnerDb = AlloyDB<alloy_network::Ethereum, P>;

    fn make_inner_db(
        &self,
        block_number: u64,
        handle: tokio::runtime::Handle,
    ) -> eyre::Result<WrapDatabaseAsync<Self::InnerDb>> {
        Ok(WrapDatabaseAsync::with_handle(AlloyDB::new(self.provider.clone(), block_number.into()), handle))
    }
}
