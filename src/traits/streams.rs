use alloy_primitives::TxHash;
use alloy_rpc_types::{eth::Filter, Log};
use futures::{Future, Stream};

/// `eth_subscribe`
pub trait EthStream {
    type TxEnvelope;
    type FullBlock;
    type ReceiptLog;

    /// `newHeads`
    fn block_stream(&self) -> impl Future<Output = eyre::Result<impl Stream<Item = Self::FullBlock> + Send>> + Send;

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
