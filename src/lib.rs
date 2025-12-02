#![allow(ambiguous_glob_reexports)]

#[cfg(feature = "reth-libmdbx")]
pub mod reth_libmdbx;

pub mod rpc;
pub mod traits;

#[cfg(feature = "op-reth-libmdbx")]
pub mod op_reth {
    pub use reth_optimism_chainspec::*;
    pub use reth_optimism_node::*;
    pub use reth_optimism_primitives::*;
}

#[cfg(feature = "reth-libmdbx")]
pub use regular_reth::*;

#[cfg(feature = "reth-libmdbx")]
mod regular_reth {
    pub use reth_chainspec::*;
    pub use reth_node_ethereum::EthereumNode;
    pub use reth_rpc_eth_api::*;
    pub use reth_storage_api::*;
    pub use revm::*;
}

#[cfg(test)]
pub mod test_utils;
