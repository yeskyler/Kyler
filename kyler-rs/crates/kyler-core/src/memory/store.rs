//! Explicit, project-scoped long-term facts. No automatic extraction of secrets.
use crate::session::Session;

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub key: String,
    pub content: String,
    pub tags: Vec<String>,
    pub revision: u64,
    pub updated_at: i64,
    pub source_session: String,
}
#[derive(Clone)]
pub struct MemoryStore {
    root: PathBuf,
}
impl MemoryStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn list(&self, scope: &str) -> Result<Vec<MemoryEntry>> {
        self.transaction(scope, false, |entries| {
            Ok(entries.values().cloned().collect())
        })
    }
    pub fn scope(session: &Session) -> String {
        format!(
            "{:x}",
            Sha256::digest(
                session
                    .environment
                    .as_ref()
                    .map(|environment| environment.workspace_root.to_string_lossy().into_owned())
                    .unwrap_or_else(|| session.session_id.clone())
            )
        )
    }
    pub(super) fn transaction<T>(
        &self,
        scope: &str,
        write: bool,
        update: impl FnOnce(&mut BTreeMap<String, MemoryEntry>) -> Result<T>,
    ) -> Result<T> {
        ensure!(
            scope.len() == 64 && scope.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Invalid memory scope"
        );
        let root = self.root.join(scope);
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            let _ = OpenOptions::new()
                .write(true)
                .create(true)
                .mode(0o600)
                .open(root.join("lock"))?;
        }
        let lock = OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .truncate(false)
            .open(root.join("lock"))?;
        lock.lock()?;
        let path = root.join("facts.json");
        let mut entries = if path.exists() {
            let mut bytes = Vec::new();
            File::open(&path)?
                .take(2 * 1024 * 1024)
                .read_to_end(&mut bytes)?;
            serde_json::from_slice(&bytes)?
        } else {
            BTreeMap::new()
        };
        let result = update(&mut entries)?;
        if write {
            let mut file = tempfile::NamedTempFile::new_in(&root)?;
            serde_json::to_writer(&mut file, &entries)?;
            file.flush()?;
            file.as_file().sync_all()?;
            file.persist(path).map_err(|error| error.error)?;
        }
        Ok(result)
    }
    pub fn put(
        &self,
        scope: &str,
        key: &str,
        content: &str,
        tags: Vec<String>,
        expected: Option<u64>,
        session: &str,
    ) -> Result<MemoryEntry> {
        ensure!(
            !key.is_empty() && key.len() <= 128 && !key.chars().any(char::is_control),
            "Invalid memory key"
        );
        ensure!(
            !content.is_empty()
                && content.len() <= 4000
                && tags.len() <= 16
                && tags.iter().all(|tag| tag.len() <= 64),
            "Memory exceeds limits"
        );
        self.transaction(scope, true, |entries| {
            let old = entries.get(key).map(|entry| entry.revision).unwrap_or(0);
            ensure!(
                expected.is_none_or(|revision| revision == old),
                "Memory revision conflict"
            );
            ensure!(
                entries.contains_key(key) || entries.len() < 256,
                "Project memory is full"
            );
            let entry = MemoryEntry {
                key: key.into(),
                content: content.into(),
                tags,
                revision: old + 1,
                updated_at: chrono::Utc::now().timestamp_millis(),
                source_session: session.into(),
            };
            entries.insert(key.into(), entry.clone());
            Ok(entry)
        })
    }
    pub fn delete(&self, scope: &str, key: &str) -> Result<bool> {
        self.transaction(scope, true, |entries| Ok(entries.remove(key).is_some()))
    }
}
#[cfg(test)]
mod tests {
    use super::MemoryStore;

    #[test]
    fn project_facts_survive_reopen_have_revisions_and_do_not_cross_scopes() {
        let root = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(root.path().into());
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let fact = store
            .put(
                &a,
                "language",
                "Use English in code",
                vec!["preference".into()],
                Some(0),
                "session",
            )
            .unwrap();
        assert_eq!(fact.revision, 1);
        assert!(
            store
                .put(&a, "language", "changed", Vec::new(), Some(0), "session")
                .is_err()
        );
        let reopened = MemoryStore::new(root.path().into());
        assert_eq!(reopened.search(&a, "code").unwrap().len(), 1);
        assert!(reopened.search(&b, "code").unwrap().is_empty());
        assert!(reopened.delete(&a, "language").unwrap());
        assert!(reopened.search(&a, "").unwrap().is_empty());
    }
}
