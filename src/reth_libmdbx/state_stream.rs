use alloy_consensus::{BlockHeader, Sealable, TxReceipt};
use alloy_primitives::BlockNumber;
use alloy_rpc_types::Log;
use parking_lot::RwLock;
use reth_node_types::NodePrimitives;
use reth_provider::{BlockNumReader, BlockReader, HeaderProvider, ReceiptProvider};
use reth_rpc_eth_api::RpcNodeCore;
use std::time::Duration;
use tokio::{sync::broadcast, task::JoinHandle};

use crate::reth_libmdbx::{NodeClientSpec, SupportedChains};

pub type NodeLogs = Vec<Log>;
pub type NodeBlock<N> = Option<<<<N as NodeClientSpec>::Api as RpcNodeCore>::Primitives as NodePrimitives>::Block>;

const DEFAULT_CHANNEL_CAPACITY: usize = 1024;

pub struct LiveStateStream<N: NodeClientSpec> {
    db_api: N::Api,
    chain: SupportedChains,
    logs_stream: RwLock<Option<StreamHandle<NodeLogs>>>,
    blocks_stream: RwLock<Option<StreamHandle<NodeBlock<N>>>>,
}

impl<N: NodeClientSpec> LiveStateStream<N> {
    pub fn new(db_api: N::Api, chain: SupportedChains) -> Self {
        Self { db_api, chain, logs_stream: RwLock::new(None), blocks_stream: RwLock::new(None) }
    }

    pub fn subscribe_logs(&self) -> broadcast::Receiver<Result<NodeLogs, LiveStateStreamError>> {
        self.ensure_logs_stream();
        let guard = self.logs_stream.read();
        guard
            .as_ref()
            .expect("log stream should be initialized")
            .subscribe()
    }

    pub fn subscribe_blocks(&self) -> broadcast::Receiver<Result<NodeBlock<N>, LiveStateStreamError>> {
        self.ensure_blocks_stream();
        let guard = self.blocks_stream.read();
        guard
            .as_ref()
            .expect("block stream should be initialized")
            .subscribe()
    }

    fn ensure_logs_stream(&self) {
        let needs_restart = {
            let guard = self.logs_stream.read();
            guard
                .as_ref()
                .map(|stream| stream.is_finished())
                .unwrap_or(true)
        };

        if needs_restart {
            let mut guard = self.logs_stream.write();
            if guard
                .as_ref()
                .map(|stream| stream.is_finished())
                .unwrap_or(true)
            {
                *guard = Some(Self::spawn_stream(
                    LiveStateStreamKind::Logs,
                    self.db_api.clone(),
                    self.chain,
                    get_latest_logs::<N>,
                ));
            }
        }
    }

    fn ensure_blocks_stream(&self) {
        let needs_restart = {
            let guard = self.blocks_stream.read();
            guard
                .as_ref()
                .map(|stream| stream.is_finished())
                .unwrap_or(true)
        };

        if needs_restart {
            let mut guard = self.blocks_stream.write();
            if guard
                .as_ref()
                .map(|stream| stream.is_finished())
                .unwrap_or(true)
            {
                *guard = Some(Self::spawn_stream(
                    LiveStateStreamKind::Blocks,
                    self.db_api.clone(),
                    self.chain,
                    get_latest_block::<N>,
                ));
            }
        }
    }

    fn spawn_stream<T: Clone + Send + 'static>(
        kind: LiveStateStreamKind,
        db_api: N::Api,
        chain: SupportedChains,
        fetch_fn: fn(&N::Api, BlockNumber) -> Result<T, LiveStateStreamError>,
    ) -> StreamHandle<T> {
        let (tx, _) = broadcast::channel(DEFAULT_CHANNEL_CAPACITY);
        let poll_time_ms = chain.get_default_poll_time_ms_for_chain();
        let stream = StateStream::<N, T>::new(db_api, fetch_fn, tx.clone(), poll_time_ms);
        let join = tokio::spawn(async move {
            stream.run().await;
        });

        StreamHandle { _kind: kind, tx, join }
    }
}

struct StreamHandle<T> {
    _kind: LiveStateStreamKind,
    tx: broadcast::Sender<Result<T, LiveStateStreamError>>,
    join: JoinHandle<()>,
}

impl<T> StreamHandle<T> {
    fn subscribe(&self) -> broadcast::Receiver<Result<T, LiveStateStreamError>> {
        self.tx.subscribe()
    }

    fn is_finished(&self) -> bool {
        self.join.is_finished()
    }
}

struct StateStream<N: NodeClientSpec, T> {
    db_api: N::Api,
    fetch_fn: Box<dyn Fn(&N::Api, BlockNumber) -> Result<T, LiveStateStreamError> + Send + Sync + 'static>,
    tx: broadcast::Sender<Result<T, LiveStateStreamError>>,
    sleep_interval_ms: usize,
}

impl<N: NodeClientSpec, T: Clone + Send + 'static> StateStream<N, T> {
    fn new(
        db_api: N::Api,
        fetch_fn: fn(&N::Api, BlockNumber) -> Result<T, LiveStateStreamError>,
        tx: broadcast::Sender<Result<T, LiveStateStreamError>>,
        sleep_interval_ms: usize,
    ) -> Self {
        Self { db_api, fetch_fn: Box::new(fetch_fn), tx, sleep_interval_ms }
    }

    async fn run(self) {
        let mut last_block: Option<BlockNumber> = None;
        let sleep_duration = Duration::from_millis(self.sleep_interval_ms as u64);

        loop {
            if self.tx.receiver_count() == 0 {
                tokio::time::sleep(sleep_duration).await;
                continue;
            }

            match self.next_best_block() {
                Some(next_block_number) => {
                    if last_block != Some(next_block_number) {
                        let data = (self.fetch_fn)(&self.db_api, next_block_number);
                        let _ = self.tx.send(data);
                        last_block = Some(next_block_number);
                    }
                }
                None => {}
            }

            tokio::time::sleep(sleep_duration).await;
        }
    }

    fn next_best_block(&self) -> Option<BlockNumber> {
        match self.db_api.provider().last_block_number() {
            Ok(v) => Some(v),
            Err(err) => {
                let _ = self
                    .tx
                    .send(Err(LiveStateStreamError::FailedToFetchBestBlock(err.to_string())));
                None
            }
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum LiveStateStreamError {
    #[error("data store disconnected")]
    NoData,
    #[error("failed to fetch best block: {0}")]
    FailedToFetchBestBlock(String),
    #[error("failed to fetch data for best block, {0}, {1}")]
    FailedToGetData(u64, String),
    #[error("stream error: {0}")]
    StreamError(String),
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
enum LiveStateStreamKind {
    Logs,
    Blocks,
}

fn get_latest_logs<N: NodeClientSpec>(db_api: &N::Api, block_number: BlockNumber) -> Result<NodeLogs, LiveStateStreamError> {
    let Some(block_header) = db_api
        .provider()
        .header_by_number(block_number.into())
        .map_err(|e| LiveStateStreamError::FailedToGetData(block_number, e.to_string()))?
    else {
        return Ok(Vec::new());
    };

    db_api
        .provider()
        .receipts_by_block(block_number.into())
        .map(|receipts| {
            receipts
                .unwrap_or_default()
                .into_iter()
                .flat_map(|receipt| {
                    receipt
                        .logs()
                        .iter()
                        .enumerate()
                        .map(|(i, log)| Log {
                            inner: log.clone(),
                            block_hash: Some(block_header.hash_slow()),
                            block_number: Some(block_number as u64),
                            block_timestamp: Some(block_header.timestamp()),
                            transaction_hash: None,
                            transaction_index: None,
                            log_index: Some(i as u64),
                            removed: false,
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .map_err(|e| LiveStateStreamError::FailedToGetData(block_number, e.to_string()))
}

fn get_latest_block<N: NodeClientSpec>(
    db_api: &N::Api,
    block_number: BlockNumber,
) -> Result<NodeBlock<N>, LiveStateStreamError> {
    db_api
        .provider()
        .block_by_number(block_number.into())
        .map_err(|e| LiveStateStreamError::FailedToGetData(block_number, e.to_string()))
}
