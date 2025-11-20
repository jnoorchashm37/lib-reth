use std::{marker::PhantomData, sync::Arc};

use crate::{
    reth_libmdbx::{NodeClientSpec, RethNodeClient},
    utils::{RethDbLayer, RethDbProvider},
};
use alloy_network::Network;

use alloy_provider::{fillers::*, Identity, Provider, ProviderBuilder};

pub type RethLayerProviderWrapperType<Node, P, N> =
    FillProvider<Identity, RethDbProvider<P, N, <Node as NodeClientSpec>::DbProvider>, N>;

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

    pub fn as_provider_with_db_layer(&self) -> RethDbProvider<P, N, <Node as NodeClientSpec>::DbProvider>
    where
        RethDbProvider<P, N, <Node as NodeClientSpec>::DbProvider>: Provider<N>,
    {
        ProviderBuilder::<_, _, N>::default()
            .layer(RethDbLayer::new(self.node_client.eth_db_provider().clone()))
            .connect_provider(self.rpc_provider())
    }
}
