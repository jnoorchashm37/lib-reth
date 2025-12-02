use std::marker::PhantomData;

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
use alloy_rpc_types::Filter;
#[cfg(any(feature = "ipc", feature = "ws"))]
use futures::Stream;

#[cfg(feature = "revm")]
use revm_database::{AlloyDB, WrapDatabaseAsync};

#[cfg(any(feature = "ipc", feature = "ws"))]
use crate::traits::EthStream;

pub struct EthRpcClient<P, N> {
    provider: P,
    _phantom: PhantomData<N>,
}

impl<P, N> EthRpcClient<P, N>
where
    P: Provider<N> + Clone,
    N: Network,
{
    pub fn provider(&self) -> &P {
        &self.provider
    }
}

#[cfg(any(feature = "ipc", feature = "ws"))]
impl<N: Network> EthRpcClient<RootProvider<N>, N> {
    #[cfg(feature = "ws")]
    pub async fn new_ws(ws_url: &str) -> eyre::Result<Self> {
        let builder = ClientBuilder::default().ws(WsConnect::new(ws_url)).await?;
        let provider = RootProvider::new(builder);
        Ok(Self { provider, _phantom: PhantomData::default() })
    }

    #[cfg(feature = "ipc")]
    pub async fn new_ipc(ipc_path: &str) -> eyre::Result<Self> {
        let builder = ClientBuilder::default()
            .ipc(IpcConnect::new(ipc_path.to_string()))
            .await?;
        let provider = RootProvider::new(builder);
        Ok(Self { provider, _phantom: PhantomData::default() })
    }

    pub fn new_http(http_url: &str) -> eyre::Result<Self> {
        let builder = ClientBuilder::default().http(http_url.parse()?);
        let provider = RootProvider::new(builder);
        Ok(Self { provider, _phantom: PhantomData::default() })
    }
}

#[cfg(any(feature = "ipc", feature = "ws"))]
#[async_trait::async_trait]
impl<N> EthStream<N> for RootProvider<N>
where
    N: Network,
{
    async fn root_provider(&self) -> eyre::Result<RootProvider<N>> {
        Ok(self.root().clone())
    }

    async fn block_stream(&self) -> eyre::Result<impl Stream<Item = <N as Network>::HeaderResponse>> {
        Ok(self.subscribe_blocks().await?.into_stream())
    }

    async fn full_pending_transaction_stream(
        &self,
    ) -> eyre::Result<impl Stream<Item = <N as Network>::TransactionResponse>> {
        Ok(self
            .subscribe_full_pending_transactions()
            .await?
            .into_stream())
    }

    async fn pending_transaction_hashes_stream(&self) -> eyre::Result<impl Stream<Item = TxHash>> {
        Ok(self.subscribe_pending_transactions().await?.into_stream())
    }

    async fn log_stream(&self, filter: Filter) -> eyre::Result<impl Stream<Item = alloy_rpc_types::Log>> {
        Ok(self.subscribe_logs(&filter).await?.into_stream())
    }
}

impl<P, N> Provider<N> for EthRpcClient<P, N>
where
    P: Provider<N> + Clone,
    N: Network,
{
    fn root(&self) -> &RootProvider<N> {
        self.provider.root()
    }
}

#[cfg(feature = "revm")]
impl<P, N> crate::traits::AsyncEthRevm for EthRpcClient<P, N>
where
    P: Provider<N> + Clone,
    N: Network,
{
    type InnerDb = AlloyDB<N, P>;

    fn make_inner_db(
        &self,
        block_number: u64,
        handle: tokio::runtime::Handle,
    ) -> eyre::Result<WrapDatabaseAsync<Self::InnerDb>> {
        Ok(WrapDatabaseAsync::with_handle(AlloyDB::new(self.provider.clone(), block_number.into()), handle))
    }
}
