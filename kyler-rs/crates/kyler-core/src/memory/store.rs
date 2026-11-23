//! Explicit, project-scoped long-term facts. No automatic extraction of secrets.
use crate::session::Session;

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
};

use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};

pub use kyler_protocol::api::memory::{MemoryEntry, MemoryKind, MemoryMetadata, MemoryStatus};
#[derive(Clone)]
pub struct MemoryStore {
    root: PathBuf,
}
impl MemoryStore {
    pub fn directory(&self) -> &std::path::Path {
        &self.root
    }
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
                    .map(|environment| environment
                        .managed
                        .as_ref()
                        .map(|managed| &managed.root)
                        .or(environment.project_root())
                        .unwrap_or(&environment.cwd)
                        .to_string_lossy()
                        .into_owned())
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
                .take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 2 * 1024 * 1024, "Memory store exceeds 2 MiB");
            serde_json::from_slice(&bytes)?
        } else {
            BTreeMap::new()
        };
        let result = update(&mut entries)?;
        if write {
            let mut file = tempfile::NamedTempFile::new_in(&root)?;
            let bytes = serde_json::to_vec(&entries)?;
            ensure!(bytes.len() <= 2 * 1024 * 1024, "Memory store exceeds 2 MiB");
            file.write_all(&bytes)?;
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
                entries
                    .get(key)
                    .is_none_or(|entry| entry.status == MemoryStatus::Active),
                "Non-active memory requires host review; save cannot activate it"
            );
            ensure!(
                expected.is_none_or(|revision| revision == old),
                "Memory revision conflict"
            );
            ensure!(
                entries.contains_key(key) || entries.len() < 256,
                "Project memory is full"
            );
            ensure!(
                old == 0 || expected == Some(old),
                "Memory updates require expected_revision"
            );
            let entry = MemoryEntry {
                status: MemoryStatus::Active,
                metadata: MemoryMetadata::default(),
                source_execution: None,
                source_tool_call: None,
                reviewed_at: Some(chrono::Utc::now().timestamp_millis()),
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
    /// Inferred experience is stored separately from active context and never replaces an active fact.
    pub fn propose(
        &self,
        scope: &str,
        key: &str,
        content: &str,
        tags: Vec<String>,
        metadata: MemoryMetadata,
        expected: Option<u64>,
        session: &str,
        execution: &str,
        call: &str,
    ) -> Result<MemoryEntry> {
        ensure!(
            !key.is_empty() && key.len() <= 128 && !key.chars().any(char::is_control),
            "Invalid memory key"
        );
        ensure!(
            !content.trim().is_empty()
                && content.len() <= 4000
                && tags.len() <= 16
                && tags.iter().all(|tag| tag.len() <= 64),
            "Memory exceeds limits"
        );
        ensure!(
            !metadata.sensitive,
            "Sensitive memories must be redacted before persistence"
        );
        ensure!(
            metadata.conditions.len() <= 2000
                && metadata.evidence.len() <= 8
                && metadata
                    .evidence
                    .iter()
                    .all(|e| !e.trim().is_empty() && e.len() <= 512),
            "Memory evidence exceeds limits"
        );
        ensure!(
            metadata
                .expires_at
                .is_none_or(|expiry| expiry > chrono::Utc::now().timestamp_millis()),
            "Memory expiry must be in the future"
        );
        self.transaction(scope, true, |entries| {
            let old = entries.get(key);
            ensure!(
                old.is_none_or(|entry| entry.status == MemoryStatus::Candidate),
                "Choose a new candidate key instead of overwriting reviewed memory"
            );
            let revision = old.map_or(0, |entry| entry.revision);
            ensure!(
                if revision == 0 {
                    expected.is_none_or(|v| v == 0)
                } else {
                    expected == Some(revision)
                },
                "Memory revision conflict"
            );
            ensure!(
                old.is_some() || entries.len() < 256,
                "Project memory is full"
            );
            let entry = MemoryEntry {
                key: key.into(),
                content: content.into(),
                tags,
                revision: revision + 1,
                updated_at: chrono::Utc::now().timestamp_millis(),
                source_session: session.into(),
                status: MemoryStatus::Candidate,
                metadata,
                source_execution: Some(execution.into()),
                source_tool_call: Some(call.into()),
                reviewed_at: None,
            };
            entries.insert(key.into(), entry.clone());
            Ok(entry)
        })
    }
    /// Host-only review entry point. The model-facing memory tool cannot invoke this.
    pub fn review(
        &self,
        scope: &str,
        key: &str,
        expected: u64,
        status: MemoryStatus,
    ) -> Result<MemoryEntry> {
        ensure!(
            status != MemoryStatus::Candidate,
            "Review must resolve a candidate or invalidate a fact"
        );
        self.transaction(scope, true, |entries| {
            let entry = entries
                .get_mut(key)
                .ok_or_else(|| anyhow::anyhow!("Memory not found"))?;
            ensure!(entry.revision == expected, "Memory revision conflict");
            ensure!(
                status != MemoryStatus::Active || !entry.metadata.sensitive,
                "Sensitive memories cannot be activated"
            );
            ensure!(
                status != MemoryStatus::Active
                    || entry
                        .metadata
                        .expires_at
                        .is_none_or(|expiry| expiry > chrono::Utc::now().timestamp_millis()),
                "Expired memory cannot be activated"
            );
            entry.status = status;
            entry.revision += 1;
            entry.reviewed_at = Some(chrono::Utc::now().timestamp_millis());
            entry.updated_at = chrono::Utc::now().timestamp_millis();
            Ok(entry.clone())
        })
    }
    pub fn delete_checked(&self, scope: &str, key: &str, expected: u64) -> Result<bool> {
        self.transaction(scope, true, |entries| {
            let entry = entries
                .get(key)
                .ok_or_else(|| anyhow::anyhow!("Memory not found"))?;
            ensure!(entry.revision == expected, "Memory revision conflict");
            Ok(entries.remove(key).is_some())
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

#[cfg(test)]
mod learning_tests {
    use super::*;

    #[test]
    fn candidates_require_exact_review_and_never_replace_active_facts() {
        let root = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(root.path().into());
        let scope = "a".repeat(64);
        let entry = store
            .propose(
                &scope,
                "workflow",
                "Run targeted tests",
                vec![],
                MemoryMetadata {
                    kind: MemoryKind::Procedure,
                    conditions: "Rust changes".into(),
                    evidence: vec!["task:test run passed".into()],
                    ..Default::default()
                },
                Some(0),
                "session",
                "execution",
                "call",
            )
            .unwrap();
        assert_eq!(entry.status, MemoryStatus::Candidate);
        assert_eq!(entry.source_execution.as_deref(), Some("execution"));
        assert!(store.search(&scope, "").unwrap().is_empty());
        assert!(
            store
                .put(
                    &scope,
                    "workflow",
                    "bypass review",
                    vec![],
                    Some(1),
                    "session"
                )
                .is_err()
        );
        assert!(
            store
                .review(&scope, "workflow", 0, MemoryStatus::Active)
                .is_err()
        );
        let active = store
            .review(&scope, "workflow", 1, MemoryStatus::Active)
            .unwrap();
        assert_eq!(active.revision, 2);
        assert_eq!(store.search(&scope, "tests").unwrap().len(), 1);
        assert!(
            store
                .propose(
                    &scope,
                    "workflow",
                    "overwrite",
                    vec![],
                    Default::default(),
                    Some(2),
                    "session",
                    "execution",
                    "call"
                )
                .is_err()
        );
        assert!(
            store
                .put(&scope, "workflow", "overwrite", vec![], None, "session")
                .is_err()
        );
        store
            .review(&scope, "workflow", 2, MemoryStatus::Invalid)
            .unwrap();
        assert!(store.search(&scope, "").unwrap().is_empty());
        assert!(store.delete_checked(&scope, "workflow", 2).is_err());
        assert!(store.delete_checked(&scope, "workflow", 3).unwrap());
    }

    #[test]
    fn legacy_records_remain_active_but_expired_and_sensitive_entries_are_excluded() {
        let root = tempfile::tempdir().unwrap();
        let scope = "b".repeat(64);
        let directory = root.path().join(&scope);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("facts.json"),r#"{"legacy":{"key":"legacy","content":"Existing approved fact","tags":[],"revision":7,"updated_at":0,"source_session":"old"}}"#).unwrap();
        let store = MemoryStore::new(root.path().into());
        assert_eq!(store.search(&scope, "").unwrap()[0].revision, 7);
        assert!(
            store
                .propose(
                    &scope,
                    "secret",
                    "redact",
                    vec![],
                    MemoryMetadata {
                        sensitive: true,
                        ..Default::default()
                    },
                    None,
                    "s",
                    "e",
                    "c"
                )
                .is_err()
        );
        store
            .transaction(&scope, true, |entries| {
                entries.get_mut("legacy").unwrap().metadata.expires_at = Some(1);
                Ok(())
            })
            .unwrap();
        assert!(store.search(&scope, "").unwrap().is_empty());
        assert!(
            store
                .review(&scope, "legacy", 7, MemoryStatus::Active)
                .is_err()
        );
        store
            .transaction(&scope, true, |entries| {
                let entry = entries.get_mut("legacy").unwrap();
                entry.metadata.expires_at = None;
                entry.metadata.sensitive = true;
                Ok(())
            })
            .unwrap();
        assert!(store.search(&scope, "").unwrap().is_empty());
        assert!(
            store
                .review(&scope, "legacy", 7, MemoryStatus::Active)
                .is_err()
        );
        store
            .review(&scope, "legacy", 7, MemoryStatus::Invalid)
            .unwrap();
        assert!(store.search(&"c".repeat(64), "").unwrap().is_empty());
    }

    #[test]
    fn context_loads_active_records_without_a_query_or_search_limit() {
        let root = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(root.path().into());
        let scope = "e".repeat(64);
        for n in 0..18 {
            store
                .put(
                    &scope,
                    &format!("fact{n}"),
                    "project detail",
                    vec![],
                    Some(0),
                    "session",
                )
                .unwrap();
        }
        store
            .propose(
                &scope,
                "unreviewed",
                "candidate content",
                vec![],
                MemoryMetadata::default(),
                None,
                "session",
                "execution",
                "call",
            )
            .unwrap();
        let entries = store.load_context(&scope).unwrap();
        assert_eq!(entries.len(), 18);
        assert!(
            entries
                .iter()
                .all(|entry| entry.status == MemoryStatus::Active)
        );
        assert_eq!(store.search(&scope, "").unwrap().len(), 16);
    }

    #[test]
    fn retrieval_fits_complete_records_within_the_context_budget() {
        let root = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(root.path().into());
        let scope = "d".repeat(64);
        for n in 0..10 {
            store
                .put(
                    &scope,
                    &format!("fact{n}"),
                    &"x".repeat(3900),
                    vec![],
                    Some(0),
                    "session",
                )
                .unwrap();
        }
        let results = store.load_context(&scope).unwrap();
        assert!(!results.is_empty() && results.len() < 10);
        assert!(
            results
                .iter()
                .map(|entry| serde_json::to_vec(entry).unwrap().len())
                .sum::<usize>()
                <= 16384
        );
    }
}
