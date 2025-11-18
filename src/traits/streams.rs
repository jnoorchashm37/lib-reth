use alloy_primitives::TxHash;
use alloy_rpc_types::{eth::Filter, Log};
use futures::{Future, Stream};

/// `eth_subscribe`
pub trait EthStream {
    type TxEnvelope;

    /// `newHeads`
    fn block_stream(
        &self,
    ) -> impl Future<Output = eyre::Result<impl Stream<Item = alloy_rpc_types_eth::Header> + Send>> + Send;

    /// `newPendingTransactions` (true)
    fn full_pending_transaction_stream(
        &self,
    ) -> impl Future<Output = eyre::Result<impl Stream<Item = Self::TxEnvelope> + Send>> + Send;

    /// `newPendingTransactions` (false)
    fn pending_transaction_hashes_stream(
        &self,
    ) -> impl Future<Output = eyre::Result<impl Stream<Item = TxHash> + Send>> + Send;

    /// `logs`
    fn log_stream(&self, filter: Filter) -> impl Future<Output = eyre::Result<impl Stream<Item = Log> + Send>> + Send;
}
