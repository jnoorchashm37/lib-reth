mod init;

use alloy_consensus::TxEnvelope;
use alloy_primitives::{TxHash, U256};
use alloy_rpc_types::{
    eth::{Filter, Log},
    Header,
};
use futures::{Stream, StreamExt};
use init::{RethApi, RethDbProvider, RethDebug, RethFilter, RethTrace, RethTxPool};

use reth_provider::CanonStateSubscriptions;

use reth_rpc_eth_types::logs_utils;
use reth_transaction_pool::TransactionPool;
use tokio_stream::wrappers::BroadcastStream;

mod builder;
pub use builder::*;

use crate::traits::EthStream;

/// direct libmdbx database connection to a reth node
pub struct RethLibmdbxClient {
    api: RethApi,
    filter: RethFilter,
    trace: RethTrace,
    debug: RethDebug,
    tx_pool: RethTxPool,
    db_provider: RethDbProvider,
}

impl RethLibmdbxClient {
    pub fn eth_api(&self) -> RethApi {
        self.api.clone()
    }

    pub fn eth_filter(&self) -> RethFilter {
        self.filter.clone()
    }

    pub fn eth_trace(&self) -> RethTrace {
        self.trace.clone()
    }

    pub fn eth_debug(&self) -> RethDebug {
        self.debug.clone()
    }

    pub fn eth_tx_pool(&self) -> RethTxPool {
        self.tx_pool.clone()
    }

    pub fn eth_db_provider(&self) -> &RethDbProvider {
        &self.db_provider
    }
}

impl EthStream for RethLibmdbxClient {
    async fn block_stream(&self) -> eyre::Result<impl Stream<Item = alloy_rpc_types_eth::Header> + Send> {
        let stream = self
            .api
            .provider()
            .canonical_state_stream()
            .flat_map(|new_chain| {
                let sealed_blocks = new_chain
                    .committed()
                    .blocks_iter()
                    .map(|sealed_block| Header {
                        hash: sealed_block.hash(),
                        total_difficulty: Some(sealed_block.difficulty),
                        size: Some(U256::from(sealed_block.size())),
                        inner: alloy_consensus::Header {
                            parent_hash: sealed_block.parent_hash,
                            ommers_hash: sealed_block.ommers_hash,
                            beneficiary: sealed_block.beneficiary,
                            state_root: sealed_block.state_root,
                            transactions_root: sealed_block.transactions_root,
                            receipts_root: sealed_block.receipts_root,
                            logs_bloom: sealed_block.logs_bloom,
                            difficulty: sealed_block.difficulty,
                            number: sealed_block.number,
                            gas_limit: sealed_block.gas_limit,
                            gas_used: sealed_block.gas_used,
                            timestamp: sealed_block.timestamp,
                            extra_data: sealed_block.extra_data.clone(),
                            mix_hash: sealed_block.mix_hash,
                            nonce: sealed_block.nonce,
                            base_fee_per_gas: sealed_block.base_fee_per_gas,
                            withdrawals_root: sealed_block.withdrawals_root,
                            blob_gas_used: sealed_block.blob_gas_used,
                            excess_blob_gas: sealed_block.excess_blob_gas,
                            parent_beacon_block_root: sealed_block.parent_beacon_block_root,
                            requests_hash: sealed_block.requests_hash,
                        },
                    })
                    .collect::<Vec<_>>();
                futures::stream::iter(sealed_blocks)
            });

        Ok(stream)
    }

    async fn full_pending_transaction_stream(&self) -> eyre::Result<impl Stream<Item = TxEnvelope> + Send> {
        let stream = self
            .api
            .pool()
            .new_pending_pool_transactions_listener()
            .map(|pooled_tx| TxEnvelope::from(pooled_tx.transaction.transaction.transaction.clone_inner()));

        Ok(stream)
    }

    async fn pending_transaction_hashes_stream(&self) -> eyre::Result<impl Stream<Item = TxHash> + Send> {
        let stream = self
            .api
            .pool()
            .new_pending_pool_transactions_listener()
            .map(|tx| *tx.transaction.hash());

        Ok(stream)
    }

    async fn log_stream(&self, filter: Filter) -> eyre::Result<impl Stream<Item = Log> + Send> {
        let stream = BroadcastStream::new(self.api.provider().subscribe_to_canonical_state())
            .map(move |canon_state| {
                canon_state
                    .expect("new block subscription never ends")
                    .block_receipts()
            })
            .flat_map(futures::stream::iter)
            .flat_map(move |(block_receipts, removed)| {
                let all_logs = logs_utils::matching_block_logs_with_tx_hashes(
                    &filter,
                    block_receipts.block,
                    block_receipts.timestamp,
                    block_receipts
                        .tx_receipts
                        .iter()
                        .map(|(tx, receipt)| (*tx, receipt)),
                    removed,
                );
                futures::stream::iter(all_logs)
            });

        Ok(stream)
    }
}

#[cfg(feature = "revm")]
impl crate::traits::EthRevm for RethLibmdbxClient {
    type InnerDb = crate::traits::reth_revm_utils::RethLibmdbxDatabaseRef;

    fn make_inner_db(&self, block_number: u64) -> eyre::Result<Self::InnerDb> {
        let state = reth_rpc_eth_api::helpers::state::LoadState::state_at_block_id(&self.eth_api(), block_number.into())?;
        let this = reth_revm::database::StateProviderDatabase::new(state);
        Ok(crate::traits::reth_revm_utils::RethLibmdbxDatabaseRef::new(this))
    }
}

#[cfg(test)]
mod tests {

    use reth_chainspec::MAINNET;

    use super::*;

    async fn stream_timeout<O>(stream: impl Stream<Item = O> + Unpin, values: usize, timeout: u64) -> eyre::Result<()> {
        let mut sub_stream = stream.take(values);
        let f = async { while let Some(_) = sub_stream.next().await {} };

        tokio::time::timeout(std::time::Duration::from_secs(timeout), f).await?;

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn can_build() {
        let builder = RethLibmdbxClientBuilder::new("/home/data/reth/db", 1000, MAINNET.clone());
        assert!(builder.build().is_ok())
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial]
    async fn can_stream() {
        let builder = RethLibmdbxClientBuilder::new("/home/data/reth/db", 1000, MAINNET.clone());
        let client = builder.build().unwrap();

        // let block_stream = client.block_stream().await.unwrap();
        // assert!(stream_timeout(block_stream, 2, 30).await.is_ok());

        let mempool_hash_stream = client.pending_transaction_hashes_stream().await.unwrap();
        assert!(stream_timeout(mempool_hash_stream, 2, 30).await.is_ok());

        let mempool_full_stream = client.full_pending_transaction_stream().await.unwrap();
        assert!(stream_timeout(mempool_full_stream, 2, 30).await.is_ok());

        let log_stream = client.log_stream(Filter::new()).await.unwrap();
        assert!(stream_timeout(log_stream, 2, 30).await.is_ok());
    }
}
