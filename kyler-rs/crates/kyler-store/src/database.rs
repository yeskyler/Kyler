//! Shared SQLite policy. Call synchronous methods only from blocking contexts.
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use anyhow::{Result, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior};

#[derive(Clone)]
pub struct Database {
    connection: Arc<Mutex<Connection>>,
}
impl Database {
    /// Open a private local database with the common durability policy.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let mut options = fs::OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.mode(0o600);
            options.open(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        options.open(path)?;
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;",
        )?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }
    pub fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Database unavailable"))
    }
    pub fn with<T>(&self, operation: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        operation(&mut *self.lock()?)
    }
    /// Keep connection ownership inside the blocking task, including after caller cancellation.
    pub async fn with_async<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let database = self.clone();
        tokio::task::spawn_blocking(move || database.with(operation)).await?
    }
    /// Recheck schema under a write transaction so concurrent openers cannot migrate twice.
    /// The callback owns schema SQL; it must not begin or commit another transaction.
    pub fn migrate(
        &self,
        target: u32,
        migration: impl FnOnce(&Transaction<'_>, u32) -> Result<()>,
    ) -> Result<()> {
        self.with(|connection| {
            let current: u32 =
                connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
            ensure!(
                current <= target,
                "Database schema is newer than this binary"
            );
            if current == target {
                return Ok(());
            }
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: u32 =
                transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
            ensure!(
                current <= target,
                "Database schema is newer than this binary"
            );
            if current < target {
                migration(&transaction, current)?;
                transaction.pragma_update(None, "user_version", target)?;
            }
            transaction.commit()?;
            Ok(())
        })
    }
}
