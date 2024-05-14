mod utils;
pub use utils::{RethApi, RethDbProvider, RethDebug, RethFilter, RethTrace};

pub struct RethDbApiClient {
    pub reth_api: RethApi,
    pub reth_filter: RethFilter,
    pub reth_trace: RethTrace,
    pub reth_debug: RethDebug,
    pub reth_db_provider: RethDbProvider,
}

impl RethDbApiClient {
    /// `db_directory_path` should be the root directory containging of all reth's libmdbx files/subdirs
    pub fn new(db_directory_path: &str, handle: tokio::runtime::Handle) -> eyre::Result<Self> {
        let (reth_api, reth_filter, reth_trace, reth_debug, reth_db_provider) =
            crate::utils::init_eth(std::path::Path::new(db_directory_path), handle)?;

        Ok(Self {
            reth_api,
            reth_filter,
            reth_trace,
            reth_debug,
            reth_db_provider,
        })
    }
}
