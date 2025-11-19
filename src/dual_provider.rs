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
