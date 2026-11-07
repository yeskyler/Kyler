//! Process-local and OS execution leases, including live-owner tracking.
use super::validate_id;
use crate::rollout::writer::private_file;

use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
};

use anyhow::{Context, Result};
use tokio::sync::{Mutex, OwnedMutexGuard};

type LockMap = StdMutex<HashMap<String, Weak<Mutex<()>>>>;
static EXECUTION_LOCKS: OnceLock<LockMap> = OnceLock::new();
static STORE_LOCKS: OnceLock<LockMap> = OnceLock::new();
fn lock_for(registry: &OnceLock<LockMap>, key: &str) -> Arc<Mutex<()>> {
    let mut locks = registry
        .get_or_init(|| StdMutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    locks.retain(|_, value| value.strong_count() > 0);
    if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key.into(), Arc::downgrade(&lock));
    lock
}
/// Single-process lease, also used for ephemeral sessions and cloned handles.
pub(crate) fn acquire_session(id: &str) -> Result<OwnedMutexGuard<()>> {
    lock_for(&EXECUTION_LOCKS, id)
        .try_lock_owned()
        .map_err(|_| anyhow::anyhow!("session {id} is busy"))
}

/// Held for the complete runtime execution, not merely one checkpoint.
pub(crate) struct StoreExecutionLease {
    _file: File,
    pub(super) _owner: Option<Arc<()>>,
}

static OWNED_EXECUTIONS: OnceLock<StdMutex<HashMap<PathBuf, Weak<()>>>> = OnceLock::new();

pub(super) fn owned_executions() -> &'static StdMutex<HashMap<PathBuf, Weak<()>>> {
    OWNED_EXECUTIONS.get_or_init(|| StdMutex::new(HashMap::new()))
}

pub(super) fn owns_execution(root: &Path, id: &str) -> bool {
    owned_executions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&root.join(id))
        .and_then(Weak::upgrade)
        .is_some()
}

pub(super) fn execution_lease(root: &Path, id: &str) -> Result<StoreExecutionLease> {
    validate_id(id)?;
    // Stable inode outside the deletable session directory. Never unlink lock files.
    let file = private_file(&root.join(format!(".execution-{id}.lock")), false)?;
    file.try_lock()
        .context("session is executing in another process")?;
    Ok(StoreExecutionLease {
        _file: file,
        _owner: None,
    })
}

pub(super) fn store_lock(root: &Path) -> Arc<Mutex<()>> {
    lock_for(&STORE_LOCKS, &root.to_string_lossy())
}
