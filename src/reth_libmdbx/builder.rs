use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use alloy_network::Ethereum;
use alloy_provider::{Provider, RootProvider};
use reth_db::{mdbx::DatabaseArguments, open_db_read_only};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};

use crate::reth_libmdbx::RethLibmdbxClient;

#[derive(Debug, Clone)]
pub struct RethLibmdbxClientBuilder<P> {
    db_path: String,
    max_tasks: usize,
    db_args: Option<DatabaseArguments>,
    provider: P,
}

impl<P: Provider> RethLibmdbxClientBuilder<P> {
    pub fn new(db_path: &str, max_tasks: usize, provider: P) -> Self {
        Self { provider, db_path: db_path.to_string(), max_tasks, db_args: None }
    }

    pub fn with_db_args(mut self, db_args: DatabaseArguments) -> Self {
        self.db_args = Some(db_args);
        self
    }

    pub fn build(self) -> eyre::Result<RethLibmdbxClient<P>> {
        let (db_path, static_files) = self.db_paths()?;

        let db = Arc::new(open_db_read_only(
            db_path,
            self.db_args
                .unwrap_or(DatabaseArguments::new(Default::default())),
        )?);

        crate::reth_libmdbx::init::new_with_db(db, self.max_tasks, TokioTaskExecutor::default(), static_files, self.provider)
    }

    pub fn build_with_task_executor<T: TaskSpawner + Clone + 'static>(
        self,
        task_executor: T,
    ) -> eyre::Result<RethLibmdbxClient<P>> {
        let (db_path, static_files) = self.db_paths()?;

        let db = Arc::new(open_db_read_only(
            db_path,
            self.db_args
                .unwrap_or(DatabaseArguments::new(Default::default())),
        )?);

        crate::reth_libmdbx::init::new_with_db(db, self.max_tasks, task_executor, static_files, self.provider)
    }

    /// (db_path, static_files)
    fn db_paths(&self) -> eyre::Result<(PathBuf, PathBuf)> {
        let db_dir = Path::new(&self.db_path);

        let db_path = db_dir.join("db");
        let static_files_path = db_dir.join("static_files");

        if !db_path.exists() {
            eyre::bail!("no 'db' subdirectory found in directory '{db_dir:?}'")
        }

        if !static_files_path.exists() {
            eyre::bail!("no 'static_files' subdirectory found in directory '{db_dir:?}'")
        }

        Ok((db_path, static_files_path))
    }
}

impl RethLibmdbxClientBuilder<RootProvider<Ethereum>> {
    pub fn new_no_root_provider(db_path: &str, max_tasks: usize) -> Self {
        let provider = alloy_provider::ProviderBuilder::<_, _, alloy_network::Ethereum>::default();
        RethLibmdbxClientBuilder::new(
            db_path,
            max_tasks,
            provider.connect_mocked_client(alloy_provider::mock::Asserter::new()),
        )
    }
}
