use alloy_network::Network;
use alloy_primitives::TxHash;
use alloy_provider::Provider;
use alloy_provider::RootProvider;
use alloy_rpc_types::eth::Filter;
use futures::Stream;

/// `eth_subscribe`
#[async_trait::async_trait]
pub trait EthStream<N: Network> {
    async fn root_provider(&self) -> eyre::Result<RootProvider<N>>;

    /// `newHeads`
    async fn block_stream(&self) -> eyre::Result<impl Stream<Item = <N as Network>::HeaderResponse>> {
        let root = EthStream::root_provider(self).await?;
        eyre::Ok(root.subscribe_blocks().await?.into_stream())
    }

    /// `newPendingTransactions` (true)
    async fn full_pending_transaction_stream(
        &self,
    ) -> eyre::Result<impl Stream<Item = <N as Network>::TransactionResponse>> {
        let root = EthStream::root_provider(self).await?;
        eyre::Ok(
            root.subscribe_full_pending_transactions()
                .await?
                .into_stream(),
        )
    }

    /// `newPendingTransactions` (false)
    async fn pending_transaction_hashes_stream(&self) -> eyre::Result<impl Stream<Item = TxHash>> {
        let root = EthStream::root_provider(self).await?;
        eyre::Ok(root.subscribe_pending_transactions().await?.into_stream())
    }

    /// `logs`
    async fn log_stream(&self, filter: Filter) -> eyre::Result<impl Stream<Item = alloy_rpc_types::Log>> {
        let root = EthStream::root_provider(self).await?;
        eyre::Ok(root.subscribe_logs(&filter).await?.into_stream())
    }
}
