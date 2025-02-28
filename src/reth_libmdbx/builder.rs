use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use reth_db::{mdbx::DatabaseArguments, open_db_read_only};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};

use crate::reth_libmdbx::RethLibmdbxClient;

#[derive(Debug, Clone)]
pub struct RethLibmdbxClientBuilder {
    db_path: String,
    max_tasks: usize,
    db_args: Option<DatabaseArguments>,
}

impl RethLibmdbxClientBuilder {
    pub fn new(db_path: &str, max_tasks: usize) -> Self {
        Self { db_path: db_path.to_string(), max_tasks, db_args: None }
    }

    pub fn with_db_args(mut self, db_args: DatabaseArguments) -> Self {
        self.db_args = Some(db_args);
        self
    }

    pub fn build(self) -> eyre::Result<RethLibmdbxClient> {
        let (db_path, static_files) = self.db_paths()?;

        let db = Arc::new(open_db_read_only(
            db_path,
            self.db_args
                .unwrap_or(DatabaseArguments::new(Default::default())),
        )?);

        crate::reth_libmdbx::init::new_with_db(db, self.max_tasks, TokioTaskExecutor::default(), static_files)
    }

    pub fn build_with_task_executor<T: TaskSpawner + Clone + 'static>(
        self,
        task_executor: T,
    ) -> eyre::Result<RethLibmdbxClient> {
        let (db_path, static_files) = self.db_paths()?;

        let db = Arc::new(open_db_read_only(
            db_path,
            self.db_args
                .unwrap_or(DatabaseArguments::new(Default::default())),
        )?);

        crate::reth_libmdbx::init::new_with_db(db, self.max_tasks, task_executor, static_files)
    }

    /// (db_path, static_files)
    fn db_paths(&self) -> eyre::Result<(PathBuf, PathBuf)> {
        let db_path = Path::new(&self.db_path);

        if !db_path.join("db").exists() {
            eyre::bail!("no 'db' subdirectory found in directory '{db_path:?}'")
        }

        if !db_path.join("static_files").exists() {
            eyre::bail!("no 'static_files' subdirectory found in directory '{db_path:?}'")
        }

        Ok((db_path.join("db"), db_path.join("static_files")))
    }
}
