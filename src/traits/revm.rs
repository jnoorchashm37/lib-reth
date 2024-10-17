use revm::{
    database_interface::{async_db::DatabaseAsyncRef, WrapDatabaseAsync},
    DatabaseRef, Evm
};
use revm_database::CacheDB;
use revm_wiring::EthereumWiring;

/// revm utils
pub trait EthRevm {
    type InnerDb: DatabaseRef;

    /// `makes the inner database fetcher`
    fn make_inner_db(&self) -> eyre::Result<Self::InnerDb>;

    /// `makes a new cache db`
    fn make_cache_db(&self) -> eyre::Result<CacheDB<Self::InnerDb>> {
        Ok(CacheDB::new(self.make_inner_db()?))
    }

    /// `makes a new cache db`
    fn make_empty_evm(&self) -> eyre::Result<Evm<'_, EthereumWiring<CacheDB<Self::InnerDb>, ()>>> {
        let cache = self.make_cache_db()?;
        let evm = Evm::builder().with_db(cache).build();
        Ok(evm)
    }
}

/// async revm utils
pub trait AsyncEthRevm {
    type InnerDb: DatabaseAsyncRef;

    /// `makes the inner database fetcher`
    fn make_inner_db(&self) -> eyre::Result<WrapDatabaseAsync<Self::InnerDb>>;

    /// `makes a new cache db`
    fn make_cache_db(&self) -> eyre::Result<CacheDB<WrapDatabaseAsync<Self::InnerDb>>> {
        Ok(CacheDB::new(self.make_inner_db()?))
    }

    /// `makes a new cache db`
    fn make_empty_evm(&self) -> eyre::Result<Evm<'_, EthereumWiring<CacheDB<WrapDatabaseAsync<Self::InnerDb>>, ()>>> {
        let cache = self.make_cache_db()?;
        let evm = Evm::builder().with_db(cache).build();
        Ok(evm)
    }
}
