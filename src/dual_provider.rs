use std::{marker::PhantomData, sync::Arc};

use crate::{
    reth_libmdbx::{NodeClientSpec, RethNodeClient},
    utils::{RethDbLayer, RethDbProvider},
};
use alloy_eips::{eip1559::Eip1559Estimation, BlockId, BlockNumberOrTag};
use alloy_network::{BlockResponse, Network};

use alloy_provider::{
    fillers::*, utils::Eip1559Estimator, Caller, Empty, EthCall, EthCallMany, EthCallManyParams, EthCallParams, EthGetBlock,
    GetSubscription, Identity, MulticallBuilder, PendingTransactionBuilder, Provider, ProviderBuilder, RpcWithBlock,
};

use reth_rpc::RpcTypes;
use reth_rpc_eth_api::helpers::*;

pub type RethLayerProviderWrapperType<Node, P, N> = FillProvider<
    JoinFill<Identity, <N as RecommendedFillers>::RecommendedFillers>,
    RethDbProvider<P, N, <Node as NodeClientSpec>::DbProvider>,
    N,
>;
use reth_rpc_eth_api::{helpers::EthTransactions, RpcTxReq};

#[derive(Clone)]
pub struct DualRethNodeClient<Node, P, N>
where
    Node: NodeClientSpec,
    P: Provider<N> + Clone,
    N: Network,
{
    node_client: Arc<RethNodeClient<Node>>,
    rpc_provider: P,
    _phantom: PhantomData<N>,
}

impl<Node, P, N> DualRethNodeClient<Node, P, N>
where
    Node: NodeClientSpec,
    P: Provider<N> + Clone,
    N: Network,
{
    pub fn new(node_client: Arc<RethNodeClient<Node>>, rpc_provider: P) -> Self {
        Self { node_client, rpc_provider, _phantom: PhantomData }
    }

    pub fn replace_rpc_provider(&mut self, rpc_provider: P) {
        self.rpc_provider = rpc_provider;
    }

    pub fn node_client(&self) -> Arc<RethNodeClient<Node>> {
        self.node_client.clone()
    }

    pub fn rpc_provider(&self) -> P {
        self.rpc_provider.clone()
    }

    pub fn as_provider_with_db_layer(&self) -> RethLayerProviderWrapperType<Node, P, N>
    where
        N: RecommendedFillers,
        RethDbProvider<P, N, <Node as NodeClientSpec>::DbProvider>: Provider<N>,
    {
        ProviderBuilder::<_, _, N>::default()
            .with_recommended_fillers()
            .layer(RethDbLayer::new(self.node_client.eth_db_provider().clone()))
            .connect_provider(self.rpc_provider())
    }
}

use alloy_consensus::BlockHeader;
use alloy_primitives::{
    Address, BlockHash, BlockNumber, Bytes, Log, StorageKey, StorageValue, TxHash, B256, U128, U256, U64,
};

use alloy_provider::{BoxedFut, ProviderCall, RootProvider};
use alloy_rpc_client::NoParams;
use alloy_rpc_types::{
    pubsub::{Params, SubscriptionKind},
    simulate::{SimulatePayload, SimulatedBlock},
    state::EvmOverrides,
    AccessListResult, Bundle, EIP1186AccountProofResponse, EthCallResponse, Filter, Index,
};
use alloy_transport::{TransportErrorKind, TransportResult};
use reth_rpc_eth_api::EthApiClient;
use reth_rpc_eth_api::EthApiServer;
use reth_rpc_eth_api::{helpers::LoadFee, EthApiTypes};
use std::{future::Future, pin::Pin};
#[derive(Clone)]
struct LocalEthCaller<Api> {
    api: Api,
}

impl<Api> LocalEthCaller<Api> {
    fn new(api: Api) -> Self {
        Self { api }
    }
}

impl<Api> Caller<<Api as EthApiTypes>::NetworkTypes, Bytes> for LocalEthCaller<Api>
where
    Api: reth_rpc_eth_api::helpers::EthCall + EthApiTypes + Clone + Send + Sync + 'static,
    <Api as EthApiTypes>::NetworkTypes: RpcTypes + Network + Unpin,
{
    fn call(
        &self,
        params: EthCallParams<<Api as EthApiTypes>::NetworkTypes>,
    ) -> TransportResult<ProviderCall<EthCallParams<<Api as EthApiTypes>::NetworkTypes>, Bytes>> {
        let api = self.api.clone();
        let fut = async move {
            let block = params
                .block()
                .unwrap_or_else(|| BlockNumberOrTag::Pending.into());
            let overrides = params.overrides().cloned();
            let block_overrides = params.block_overrides().cloned().map(Box::new);
            let evm_overrides = EvmOverrides::new(overrides, block_overrides);
            let tx = params.into_data();

            reth_rpc_eth_api::helpers::EthCall::call(&api, tx, Some(block), evm_overrides)
                .await
                .map_err(TransportErrorKind::custom)
        };

        Ok(ProviderCall::BoxedFuture(Box::pin(fut)))
    }

    fn estimate_gas(
        &self,
        _params: alloy_provider::EthCallParams<<Api as EthApiTypes>::NetworkTypes>,
    ) -> TransportResult<ProviderCall<EthCallParams<<Api as EthApiTypes>::NetworkTypes>, Bytes>> {
        Err(TransportErrorKind::custom_str("estimate_gas not supported for local caller"))
    }

    fn call_many(
        &self,
        _params: alloy_provider::EthCallManyParams<'_>,
    ) -> TransportResult<ProviderCall<EthCallManyParams<'static>, Bytes>> {
        Err(TransportErrorKind::custom_str("call_many not supported for byte responses"))
    }
}

impl<Api> Caller<<Api as EthApiTypes>::NetworkTypes, Vec<Vec<EthCallResponse>>> for LocalEthCaller<Api>
where
    Api: reth_rpc_eth_api::helpers::EthCall + EthApiTypes + Clone + Send + Sync + 'static,
    <Api as EthApiTypes>::NetworkTypes: RpcTypes + Network + Unpin,
{
    fn call(
        &self,
        _params: alloy_provider::EthCallParams<<Api as EthApiTypes>::NetworkTypes>,
    ) -> TransportResult<ProviderCall<EthCallParams<<Api as EthApiTypes>::NetworkTypes>, Vec<Vec<EthCallResponse>>>> {
        Err(TransportErrorKind::custom_str("eth_call not supported for call_many response"))
    }

    fn estimate_gas(
        &self,
        _params: alloy_provider::EthCallParams<<Api as EthApiTypes>::NetworkTypes>,
    ) -> TransportResult<ProviderCall<EthCallParams<<Api as EthApiTypes>::NetworkTypes>, Vec<Vec<EthCallResponse>>>> {
        Err(TransportErrorKind::custom_str("estimate_gas not supported for call_many response"))
    }

    fn call_many(
        &self,
        params: alloy_provider::EthCallManyParams<'_>,
    ) -> TransportResult<ProviderCall<EthCallManyParams<'static>, Vec<Vec<EthCallResponse>>>> {
        let api = self.api.clone();
        let bundles = params.bundles().to_vec();
        let context = params.context().cloned();
        let overrides = params.overrides().cloned();

        let fut = async move {
            reth_rpc_eth_api::helpers::EthCall::call_many(&api, bundles, context, overrides)
                .await
                .map_err(TransportErrorKind::custom)
        };

        Ok(ProviderCall::BoxedFuture(Box::pin(fut)))
    }
}

#[cfg_attr(target_family = "wasm", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait::async_trait)]
impl<Node, P, N> Provider<N> for DualRethNodeClient<Node, P, N>
where
    Node: NodeClientSpec,
    P: Provider<N> + Clone,
    N: Network,
{
    fn root(&self) -> &RootProvider<N> {
        self.rpc_provider.root()
    }

    fn get_blob_base_fee(&self) -> ProviderCall<NoParams, U128, u128> {
        let api = self.node_client.eth_api();
        let f = async move {
            LoadFee::blob_base_fee(&api)
                .await
                .map(|v| v.to::<u128>())
                .map_err(TransportErrorKind::custom)
        };

        ProviderCall::BoxedFuture(Box::pin(f) as BoxedFut<u128>)
    }

    fn get_block_number(&self) -> ProviderCall<NoParams, U64, BlockNumber> {
        let api = self.node_client.eth_api();
        let f = async move {
            EthApiSpec::chain_info(&api)
                .map(|info| info.best_number)
                .map_err(TransportErrorKind::custom)
        };

        ProviderCall::BoxedFuture(Box::pin(f) as BoxedFut<BlockNumber>)
    }

    async fn get_block_number_by_id(&self, block_id: BlockId) -> TransportResult<Option<BlockNumber>> {
        match block_id {
            BlockId::Number(BlockNumberOrTag::Number(num)) => Ok(Some(num)),
            BlockId::Number(BlockNumberOrTag::Latest) => self.get_block_number().await.map(Some),
            _ => {
                let block = self.get_block(block_id).await?;
                Ok(block.map(|b| b.header().number()))
            }
        }
    }

    fn call(&self, tx: N::TransactionRequest) -> EthCall<N, Bytes> {
        EthCall::call(LocalEthCaller::new(self.node_client.eth_api()), tx).block(BlockNumberOrTag::Pending.into())
    }

    fn call_many<'req>(&self, bundles: &'req [Bundle]) -> EthCallMany<'req, N, Vec<Vec<EthCallResponse>>> {
        self.node_client
            .eth_api()
            .call_many(bundles.to_vec(), BlockNumberOrTag::Pending.into(), None);
    }

    fn simulate<'req>(
        &self,
        payload: &'req SimulatePayload,
    ) -> RpcWithBlock<&'req SimulatePayload, Vec<SimulatedBlock<N::BlockResponse>>> {
        let api = self.node_client.eth_api();
        RpcWithBlock::new_provider(|block| {
            let f = async move {
                reth_rpc_eth_api::helpers::EthCall::simulate_v1(&api, payload, block)
                    .await
                    .map_err(TransportErrorKind::custom)
            };

            ProviderCall::BoxedFuture(Box::pin(f))
        })
    }

    fn get_chain_id(&self) -> ProviderCall<NoParams, U64, u64> {
        ProviderCall::Ready(Some(Ok(EthApiSpec::chain_id(&self.node_client.eth_api()).to())))
    }

    fn create_access_list<'a>(
        &self,
        request: &'a N::TransactionRequest,
    ) -> RpcWithBlock<&'a N::TransactionRequest, AccessListResult> {
        RpcWithBlock::new_provider(|block| {
            let api = self.node_client.eth_api();
            let f = async move {
                api.create_access_list(request, block, None)
                    .await
                    .map_err(TransportErrorKind::custom)
            };
            ProviderCall::BoxedFuture(Box::pin(f))
        })
    }

    fn estimate_gas(&self, tx: N::TransactionRequest) -> EthCall<N, U64, u64> {
        self.node_client
            .eth_api()
            .estimate_gas(tx, BlockNumberOrTag::Pending.into(), None);
    }

    fn get_gas_price(&self) -> ProviderCall<NoParams, U128, u128> {
        let api = self.node_client().eth_api();
        let f = async move { api.gas_price().await.map_err(TransportErrorKind::custom) };
        ProviderCall::BoxedFuture(f)
    }

    fn get_account_info(&self, address: Address) -> RpcWithBlock<Address, alloy_rpc_types_eth::AccountInfo> {
        self.client().request("eth_getAccountInfo", address).into()
    }

    fn get_account(&self, address: Address) -> RpcWithBlock<Address, alloy_consensus::Account> {
        let api = self.node_client.eth_api();
        RpcWithBlock::new_provider(|block| {
            let f = async move {
                api.get_account(address, block)
                    .await
                    .map_err(TransportErrorKind::custom)
            };
            ProviderCall::BoxedFuture(Box::pin(f))
        })
    }

    fn get_balance(&self, address: Address) -> RpcWithBlock<Address, U256, U256> {
        let api = self.node_client.eth_api();
        RpcWithBlock::new_provider(move |block| {
            let api = api.clone();
            let f = async move {
                api.balance(address, Some(block))
                    .await
                    .map_err(TransportErrorKind::custom)
            };
            ProviderCall::BoxedFuture(Box::pin(f))
        })
    }

    fn get_block(&self, block: BlockId) -> EthGetBlock<N::BlockResponse> {
        match block {
            BlockId::Hash(hash) => self.get_block_by_hash(hash),
            BlockId::Number(number) => self.get_block_by_number(number),
        }
    }

    fn get_block_by_hash(&self, hash: BlockHash) -> EthGetBlock<N::BlockResponse> {
        self.node_client.eth_api().block_by_hash(hash, false)
    }

    fn get_block_by_number(&self, number: BlockNumberOrTag) -> EthGetBlock<N::BlockResponse> {
        self.node_client.eth_api().block_by_number(number, false)
    }

    async fn get_block_transaction_count_by_hash(&self, hash: BlockHash) -> EthGetBlock<N::BlockResponse> {
        self.node_client
            .eth_api()
            .block_transaction_count_by_hash(hash)
    }

    async fn get_block_transaction_count_by_number(&self, number: BlockNumberOrTag) -> EthGetBlock<N::BlockResponse> {
        self.node_client
            .eth_api()
            .block_transaction_count_by_number(hash)
    }

    // can fut
    fn get_block_receipts(&self, block: BlockId) -> ProviderCall<(BlockId,), Option<Vec<N::ReceiptResponse>>> {
        self.node_client.eth_api().block_receipts(block)
    }

    fn get_code_at(&self, address: Address) -> RpcWithBlock<Address, Bytes> {
        let api = self.node_client().eth_api();

        RpcWithBlock::new_provider(move |block_id| {
            let f = async move {
                api.get_code(address, block_id)
                    .await
                    .map_err(TransportErrorKind::custom)
            };
            ProviderCall::BoxedFuture(f)
        })
    }

    fn get_proof(
        &self,
        address: Address,
        keys: Vec<StorageKey>,
    ) -> RpcWithBlock<(Address, Vec<StorageKey>), EIP1186AccountProofResponse> {
        let api = self.node_client().eth_api();

        RpcWithBlock::new_provider(move |block_id| {
            let f = async move {
                api.get_proof(address, keys, block_id)
                    .await
                    .map_err(TransportErrorKind::custom)
            };
            ProviderCall::BoxedFuture(f)
        })
    }

    fn get_storage_at(&self, address: Address, key: U256) -> RpcWithBlock<(Address, U256), StorageValue> {
        let api = self.node_client().eth_api();

        RpcWithBlock::new_provider(move |block_id| {
            let f = async move {
                api.storage_at(address, key, block_id)
                    .await
                    .map_err(TransportErrorKind::custom)
            };
            ProviderCall::BoxedFuture(f)
        })
    }

    fn get_transaction_by_sender_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> ProviderCall<(Address, U64), Option<N::TransactionResponse>> {
        let api = self.node_client().eth_api();
        let f = async move {
            api.transaction_by_sender_and_nonce(sender, nonce)
                .await
                .map_err(TransportErrorKind::custom)
        };
        ProviderCall::BoxedFuture(f)
    }

    fn get_transaction_by_hash(&self, hash: TxHash) -> ProviderCall<(TxHash,), Option<N::TransactionResponse>> {
        let api = self.node_client().eth_api();
        let f = async move {
            api.transaction_by_hash(hash)
                .await
                .map_err(TransportErrorKind::custom)
        };
        ProviderCall::BoxedFuture(f)
    }

    fn get_transaction_by_block_hash_and_index(
        &self,
        block_hash: B256,
        index: usize,
    ) -> ProviderCall<(B256, Index), Option<N::TransactionResponse>> {
        let api = self.node_client().eth_api();
        let f = async move {
            api.transaction_by_block_hash_and_index(block_hash, index)
                .await
                .map_err(TransportErrorKind::custom)
        };
        ProviderCall::BoxedFuture(f)
    }

    fn get_raw_transaction_by_block_hash_and_index(
        &self,
        block_hash: B256,
        index: usize,
    ) -> ProviderCall<(B256, Index), Option<Bytes>> {
        let api = self.node_client().eth_api();
        let f = async move {
            api.raw_transaction_by_block_hash_and_index(block_hash, index)
                .await
                .map_err(TransportErrorKind::custom)
        };
        ProviderCall::BoxedFuture(f)
    }

    fn get_transaction_by_block_number_and_index(
        &self,
        block_number: BlockNumberOrTag,
        index: usize,
    ) -> ProviderCall<(BlockNumberOrTag, Index), Option<N::TransactionResponse>> {
        let api = self.node_client().eth_api();
        let f = async move {
            api.transaction_by_block_number_and_index(block_number, index)
                .await
                .map_err(TransportErrorKind::custom)
        };
        ProviderCall::BoxedFuture(f)
    }

    fn get_raw_transaction_by_block_number_and_index(
        &self,
        block_number: BlockNumberOrTag,
        index: usize,
    ) -> ProviderCall<(BlockNumberOrTag, Index), Option<Bytes>> {
        let api = self.node_client().eth_api();
        let f = async move {
            api.raw_transaction_by_block_number_and_index(block_number, index)
                .await
                .map_err(TransportErrorKind::custom)
        };
        ProviderCall::BoxedFuture(f)
    }

    fn get_raw_transaction_by_hash(&self, hash: TxHash) -> ProviderCall<(TxHash,), Option<Bytes>> {
        let api = self.node_client().eth_api();
        let f = async move {
            api.raw_transaction_by_hash(hash)
                .await
                .map_err(TransportErrorKind::custom)
        };
        ProviderCall::BoxedFuture(f)
    }

    fn get_transaction_count(&self, address: Address) -> RpcWithBlock<Address, U64, u64> {
        let api = self.node_client().eth_api();

        RpcWithBlock::new_provider(move |block_id| {
            let f = async move {
                api.transaction_count(address, block_id)
                    .await
                    .map_err(TransportErrorKind::custom)
            };
            ProviderCall::BoxedFuture(f)
        })
    }

    fn get_transaction_receipt(&self, hash: TxHash) -> ProviderCall<(TxHash,), Option<N::ReceiptResponse>> {
        let api = self.node_client().eth_api();
        let f = async move {
            api.transaction_receipt(hash)
                .await
                .map_err(TransportErrorKind::custom)
        };
        ProviderCall::BoxedFuture(f)
    }

    fn get_max_priority_fee_per_gas(&self) -> ProviderCall<NoParams, U128, u128> {
        let api = self.node_client().eth_api();
        let f = async move {
            api.max_priority_fee_per_gas()
                .await
                .map_err(TransportErrorKind::custom)
        };
        ProviderCall::BoxedFuture(f)
    }

    async fn send_raw_transaction(&self, tx: N::TransactionRequest) -> TransportResult<N::ReceiptResponse> {
        self.node_client.eth_api().send_raw_transaction(tx);
    }

    async fn send_raw_transaction_sync(&self, tx: N::TransactionRequest) -> TransportResult<N::ReceiptResponse> {
        self.node_client.eth_api().send_raw_transaction_sync(tx);
    }

    async fn send_transaction(&self, tx: N::TransactionRequest) -> TransportResult<N::ReceiptResponse> {
        self.node_client.eth_api().send_transaction(tx);
    }

    async fn send_tx_envelope(&self, tx: N::TxEnvelope) -> TransportResult<PendingTransactionBuilder<N>> {
        self.node_client.eth_api().send_transaction(tx);
    }

    async fn send_transaction_sync(&self, tx: N::TransactionRequest) -> TransportResult<N::ReceiptResponse> {
        self.node_client.eth_api().send_transaction(tx);
    }

    async fn sign_transaction(&self, tx: N::TransactionRequest) -> TransportResult<Bytes> {
        self.node_client.eth_api().sign_transaction(&tx);
    }
}
