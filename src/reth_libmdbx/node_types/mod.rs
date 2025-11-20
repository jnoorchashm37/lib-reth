use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "revm")]
use crate::traits::BlockNumberOrHash;
use crate::traits::EthStream;

use alloy_consensus::BlockHeader;
use alloy_primitives::{TxHash, U256};
use alloy_rpc_types::{
    eth::{Filter, Log},
    Header,
};
use futures::{Stream, StreamExt};
use reth_db::DatabaseEnv;
use reth_node_types::NodeTypes;
use reth_primitives_traits::size::InMemorySize;
use reth_provider::{BlockNumReader, DatabaseProviderFactory, StateProviderFactory, TryIntoHistoricalStateProvider};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;

use reth_provider::CanonStateSubscriptions;
use reth_rpc_eth_api::{helpers::FullEthApi, EthApiTypes, FullEthApiServer, RpcNodeCore};
use tokio_stream::wrappers::BroadcastStream;

pub mod node;
#[cfg(feature = "op-reth-libmdbx")]
pub mod op_node;

pub trait NodeClientSpec: NodeTypes + Send + Sync {
    type NodeChainSpec: Clone + Send + Sync;
    type Api: FullEthApi + FullEthApiServer + EthApiTypes + RpcNodeCore + Clone + Send + Sync;
    type Filter: Clone + Send + Sync;
    type Trace: Clone + Send + Sync;
    type Debug: Clone + Send + Sync;
    type TxPool: Clone + Send + Sync;
    type DbProvider: DatabaseProviderFactory<Provider: TryIntoHistoricalStateProvider + BlockNumReader>
        + StateProviderFactory
        + CanonStateSubscriptions
        + Send
        + Sync
        + Clone
        + 'static;

    fn new_with_db<T: TaskSpawner + Clone + 'static>(
        db: Arc<DatabaseEnv>,
        max_tasks: usize,
        task_executor: T,
        static_files_path: PathBuf,
        chain: Arc<Self::NodeChainSpec>,
    ) -> eyre::Result<RethNodeClient<Self>>;
}

pub struct RethNodeClient<N: NodeClientSpec> {
    api: N::Api,
    filter: N::Filter,
    trace: N::Trace,
    debug: N::Debug,
    tx_pool: N::TxPool,
    db_provider: N::DbProvider,
    chain_spec: Arc<<N as NodeClientSpec>::NodeChainSpec>,
}

impl<N: NodeClientSpec> RethNodeClient<N> {
    pub fn chain_spec(&self) -> Arc<<N as NodeClientSpec>::NodeChainSpec> {
        self.chain_spec.clone()
    }

    pub fn eth_api(&self) -> N::Api {
        self.api.clone()
    }

    pub fn eth_filter(&self) -> N::Filter {
        self.filter.clone()
    }

    pub fn eth_trace(&self) -> N::Trace {
        self.trace.clone()
    }

    pub fn eth_debug(&self) -> N::Debug {
        self.debug.clone()
    }

    pub fn eth_tx_pool(&self) -> N::TxPool {
        self.tx_pool.clone()
    }

    pub fn eth_db_provider(&self) -> &N::DbProvider {
        &self.db_provider
    }
}

impl<N: NodeClientSpec> EthStream for RethNodeClient<N> {
    type TxEnvelope = <<<N as NodeClientSpec>::Api as RpcNodeCore>::Pool as TransactionPool>::Transaction;

    async fn block_stream(&self) -> eyre::Result<impl Stream<Item = Header> + Send> {
        let stream = self
            .eth_db_provider()
            .canonical_state_stream()
            .flat_map(|new_chain| {
                let sealed_blocks = new_chain
                    .committed()
                    .blocks_iter()
                    .map(|sealed_block| Header {
                        hash: sealed_block.hash(),
                        total_difficulty: Some(sealed_block.difficulty()),
                        size: Some(U256::from(sealed_block.size())),
                        inner: alloy_consensus::Header {
                            parent_hash: sealed_block.parent_hash(),
                            ommers_hash: sealed_block.ommers_hash(),
                            beneficiary: sealed_block.beneficiary(),
                            state_root: sealed_block.state_root(),
                            transactions_root: sealed_block.transactions_root(),
                            receipts_root: sealed_block.receipts_root(),
                            logs_bloom: sealed_block.logs_bloom(),
                            difficulty: sealed_block.difficulty(),
                            number: sealed_block.number(),
                            gas_limit: sealed_block.gas_limit(),
                            gas_used: sealed_block.gas_used(),
                            timestamp: sealed_block.timestamp(),
                            extra_data: sealed_block.extra_data().clone(),
                            mix_hash: sealed_block.mix_hash().unwrap(),
                            nonce: sealed_block.nonce().unwrap(),
                            base_fee_per_gas: sealed_block.base_fee_per_gas(),
                            withdrawals_root: sealed_block.withdrawals_root(),
                            blob_gas_used: sealed_block.blob_gas_used(),
                            excess_blob_gas: sealed_block.excess_blob_gas(),
                            parent_beacon_block_root: sealed_block.parent_beacon_block_root(),
                            requests_hash: sealed_block.requests_hash(),
                        },
                    })
                    .collect::<Vec<_>>();
                futures::stream::iter(sealed_blocks)
            });

        Ok(stream)
    }

    async fn full_pending_transaction_stream(&self) -> eyre::Result<impl Stream<Item = Self::TxEnvelope> + Send> {
        let stream = self
            .eth_api()
            .pool()
            .new_pending_pool_transactions_listener()
            .map(|pooled_tx| pooled_tx.transaction.transaction.clone());

        Ok(stream)
    }

    async fn pending_transaction_hashes_stream(&self) -> eyre::Result<impl Stream<Item = TxHash> + Send> {
        let stream = self
            .eth_api()
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
                let all_logs = reth_rpc_eth_types::logs_utils::matching_block_logs_with_tx_hashes(
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
impl<N: NodeClientSpec> crate::traits::EthRevm for RethNodeClient<N> {
    type InnerDb = crate::traits::reth_revm_utils::RethLibmdbxDatabaseRef;

    fn make_inner_db<T: Into<BlockNumberOrHash>>(&self, block: T) -> eyre::Result<Self::InnerDb> {
        use reth_provider::StateProviderFactory;

        let block: BlockNumberOrHash = block.into();

        let state_provider = match block {
            BlockNumberOrHash::Number(num) => self
                .eth_db_provider()
                .state_by_block_number_or_tag(num.into())?,
            BlockNumberOrHash::Hash(hash) => self.eth_db_provider().state_by_block_hash(hash)?,
        };

        let this = reth_revm::database::StateProviderDatabase::new(state_provider);
        Ok(crate::traits::reth_revm_utils::RethLibmdbxDatabaseRef::new(this))
    }
}
