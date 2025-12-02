mod node_types;
pub use node_types::*;

mod builder;
pub use builder::*;

// pub(crate) mod state_stream;

// #[derive(Debug, Clone, Copy, Hash)]
// pub(crate) enum SupportedChains {
//     Mainnet,
//     Base,
//     Unichain,
//     Sepolia,
// }

// impl SupportedChains {
//     pub fn from_chain_id(id: u64) -> Option<Self> {
//         match id {
//             1 => Some(Self::Mainnet),
//             130 => Some(Self::Unichain),
//             8453 => Some(Self::Base),
//             11155111 => Some(Self::Sepolia),
//             _ => None,
//         }
//     }

//     pub(crate) fn get_default_poll_time_ms_for_chain_id(id: u64) -> usize {
//         Self::from_chain_id(id)
//             .expect(&format!("chain id not supported: {id}"))
//             .get_default_poll_time_ms_for_chain()
//     }

//     pub(crate) fn get_default_poll_time_ms_for_chain(&self) -> usize {
//         match self {
//             SupportedChains::Mainnet => 500,
//             SupportedChains::Base => 100,
//             SupportedChains::Unichain => 75,
//             SupportedChains::Sepolia => 500,
//         }
//     }
// }
