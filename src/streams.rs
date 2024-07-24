use alloy_primitives::TxHash;
use alloy_rpc_types::{eth::Filter, Block, Log, Transaction};
use futures::{Future, Stream};

/// `eth_subscribe`
pub trait EthStream {
    /// `newHeads`
    fn block_stream(&self) -> impl Future<Output = eyre::Result<impl Stream<Item = Block>>>;

    /// `newPendingTransactions` (true)
    fn full_pending_transaction_stream(&self) -> impl Future<Output = eyre::Result<impl Stream<Item = Transaction>>>;

    /// `newPendingTransactions` (false)
    fn pending_transaction_hashes_stream(&self) -> impl Future<Output = eyre::Result<impl Stream<Item = TxHash>>>;

    /// `logs`
    fn log_stream(&self, filter: &Filter) -> impl Future<Output = eyre::Result<impl Stream<Item = Log>>>;
}
