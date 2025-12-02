use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use exe_runners::{TaskSpawner, TokioTaskExecutor};

use reth_db::{mdbx::DatabaseArguments, open_db_read_only};

use crate::reth_libmdbx::node_types::{NodeClientSpec, RethNodeClient};

#[derive(Debug, Clone)]
pub struct RethNodeClientBuilder<N: NodeClientSpec> {
    db_path: String,
    max_tasks: usize,
    db_args: Option<DatabaseArguments>,
    chain: Arc<N::NodeChainSpec>,
    ipc_path_or_rpc_url: Option<String>,
}

impl<N: NodeClientSpec> RethNodeClientBuilder<N> {
    pub fn new(db_path: &str, max_tasks: usize, chain: Arc<N::NodeChainSpec>, ipc_path_or_rpc_url: Option<String>) -> Self {
        Self { db_path: db_path.to_string(), max_tasks, db_args: None, chain, ipc_path_or_rpc_url }
    }

    pub fn with_db_args(mut self, db_args: DatabaseArguments) -> Self {
        self.db_args = Some(db_args);
        self
    }

    pub fn build(self) -> eyre::Result<RethNodeClient<N>> {
        let (db_path, static_files) = self.db_paths()?;

        let db = Arc::new(open_db_read_only(
            db_path,
            self.db_args
                .unwrap_or(DatabaseArguments::new(Default::default())),
        )?);

        N::new_with_db(db, self.max_tasks, TokioTaskExecutor::default(), static_files, self.chain, self.ipc_path_or_rpc_url)
    }

    pub fn build_with_task_executor<T: TaskSpawner + Clone + 'static>(
        self,
        task_executor: T,
    ) -> eyre::Result<RethNodeClient<N>> {
        let (db_path, static_files) = self.db_paths()?;

        let db = Arc::new(open_db_read_only(
            db_path,
            self.db_args
                .unwrap_or(DatabaseArguments::new(Default::default())),
        )?);

        N::new_with_db(db, self.max_tasks, task_executor, static_files, self.chain, self.ipc_path_or_rpc_url)
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
