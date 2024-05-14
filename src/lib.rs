mod utils;
pub use utils::{RethDbProvider, RethDebug, RethEthApi, RethFilter, RethTrace};

pub struct RethDbApiClient {
    pub eth_api: RethEthApi,
    pub filter: RethFilter,
    pub trace: RethTrace,
    pub debug: RethDebug,
    pub db_provider: RethDbProvider,
}

impl RethDbApiClient {
    /// `db_directory_path` should be the root directory containging of all reth's libmdbx files/subdirs
    pub fn new(db_directory_path: &str, handle: tokio::runtime::Handle) -> eyre::Result<Self> {
        let (eth_api, filter, trace, debug, db_provider) =
            crate::utils::init_eth(std::path::Path::new(db_directory_path), handle)?;

        Ok(Self {
            eth_api,
            filter,
            trace,
            debug,
            db_provider,
        })
    }
}
