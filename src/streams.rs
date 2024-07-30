use alloy_primitives::TxHash;
use alloy_rpc_types::{eth::Filter, Block, Log, Transaction};
use futures::{Future, Stream};

/// `eth_subscribe`
pub trait EthStream<'a> {
    /// `newHeads`
    fn block_stream(&'a self) -> impl Future<Output = eyre::Result<impl Stream<Item = Block> + Send + 'a>> + Send;

    /// `newPendingTransactions` (true)
    fn full_pending_transaction_stream(
        &'a self
    ) -> impl Future<Output = eyre::Result<impl Stream<Item = Transaction> + Send + 'a>> + Send;

    /// `newPendingTransactions` (false)
    fn pending_transaction_hashes_stream(
        &'a self
    ) -> impl Future<Output = eyre::Result<impl Stream<Item = TxHash> + Send + 'a>> + Send;

    /// `logs`
    fn log_stream(
        &'a self,
        filter: &Filter
    ) -> impl Future<Output = eyre::Result<impl Stream<Item = Log> + Send + 'a>> + Send;
}
