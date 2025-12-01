use alloy_primitives::TxHash;
use alloy_rpc_types::eth::Filter;
use futures::{Future, Stream};

use crate::reth_libmdbx::state_stream::LiveStateStreamError;

/// `eth_subscribe`
pub trait EthStream {
    type TxEnvelope;
    type FullBlock;

    /// `newHeads`
    fn block_stream(
        &self,
    ) -> impl Future<Output = eyre::Result<impl Stream<Item = Result<Self::FullBlock, LiveStateStreamError>> + Send>> + Send;

    // /// `newPendingTransactions` (true)
    // fn full_pending_transaction_stream(
    //     &self,
    // ) -> impl Future<Output = eyre::Result<impl Stream<Item = Self::TxEnvelope> + Send>> + Send;

    // /// `newPendingTransactions` (false)
    // fn pending_transaction_hashes_stream(
    //     &self,
    // ) -> impl Future<Output = eyre::Result<impl Stream<Item = TxHash> + Send>> + Send;

    /// `logs`
    fn log_stream(
        &self,
        filter: Filter,
    ) -> impl Future<Output = eyre::Result<impl Stream<Item = Result<alloy_rpc_types::Log, LiveStateStreamError>> + Send>> + Send;
}
