//! Session interpretation and journal-driven index repair stay in core.
use super::{journal::read_locked, validate_id};
use crate::{
    Session,
    context::{ContentItem, ExecutionStatus},
};

use std::{
    collections::BTreeSet,
    fs,
    ops::Deref,
    path::{Path, PathBuf},
};

use anyhow::Result;

use kyler_protocol::api::{run::RunStatus, session::SessionSummary};
use kyler_store::catalog::SessionRecord;
pub use kyler_store::catalog::{PreferenceUpdate, UiPreferences};

pub struct Catalog {
    inner: kyler_store::catalog::Catalog,
    pub path: PathBuf,
}
impl Deref for Catalog {
    type Target = kyler_store::catalog::Catalog;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

pub fn summary(session: &Session) -> SessionSummary {
    let title = session
        .events
        .iter()
        .flat_map(|event| &event.content)
        .find_map(|item| match item {
            ContentItem::Message { role, content } if role.as_str() == "user" => {
                let value = content.split_whitespace().collect::<Vec<_>>().join(" ");
                (!value.is_empty()).then(|| value.chars().take(80).collect())
            }
            _ => None,
        });
    SessionSummary {
        cwd: session
            .environment
            .as_ref()
            .map(|e| e.cwd.to_string_lossy().into_owned()),
        title,
        draft: false,
        project: session
            .environment
            .as_ref()
            .and_then(|e| e.project_root())
            .map(|path| path.to_string_lossy().into_owned()),
        session_id: session.session_id.clone(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        event_count: session.events.len(),
        execution_count: session.executions.len() + usize::from(session.execution.is_some()),
        status: session.execution.as_ref().map(|e| match e.status {
            ExecutionStatus::Running => RunStatus::Running,
            ExecutionStatus::Completed => RunStatus::Completed,
            ExecutionStatus::Failed => RunStatus::Failed,
            ExecutionStatus::Cancelled => RunStatus::Cancelled,
        }),
    }
}
impl Catalog {
    pub fn open(root: &Path) -> Result<Self> {
        let parent = if root.file_name().is_some_and(|name| name == "sessions") {
            root.parent().unwrap_or(root)
        } else {
            root
        };
        let path = parent.join("app.sqlite3");
        let inner =
            kyler_store::catalog::Catalog::open(&path, root.to_string_lossy().into_owned())?;
        Ok(Self { inner, path })
    }
    fn signature(root: &Path, id: &str) -> Result<String> {
        let log = root.join(id).join("rollout.jsonl");
        let path = if log.exists() {
            log
        } else {
            root.join(format!("{id}.json"))
        };
        let meta = fs::metadata(path)?;
        Ok(format!("{}:{:?}", meta.len(), meta.modified()?))
    }
    pub fn upsert(&self, root: &Path, session: &Session) -> Result<()> {
        let item = summary(session);
        let preview = session
            .events
            .iter()
            .rev()
            .flat_map(|e| e.content.iter().rev())
            .find_map(|item| match item {
                ContentItem::Message { role, content }
                    if matches!(role.as_str(), "user" | "assistant") =>
                {
                    Some(content.chars().take(160).collect::<String>())
                }
                _ => None,
            });
        self.inner.upsert(&SessionRecord {
            summary: item,
            revision: session.revision,
            signature: Self::signature(root, &session.session_id)?,
            last_message_preview: preview,
            draft: root
                .join("_runs")
                .join(format!("{}.draft", session.session_id))
                .exists(),
        })
    }
    pub fn reconcile(&self, root: &Path) -> Result<()> {
        let mut ids = BTreeSet::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            let candidate = if path.is_dir() {
                path.file_name()
            } else if path.extension().is_some_and(|e| e == "json") {
                path.file_stem()
            } else {
                None
            };
            if let Some(id) = candidate
                .and_then(|s| s.to_str())
                .filter(|id| validate_id(id).is_ok())
            {
                ids.insert(id.to_owned());
            }
        }
        for id in &ids {
            if !root.join(id).join("rollout.jsonl").exists()
                && !root.join(format!("{id}.json")).exists()
            {
                continue;
            }
            let signature = Self::signature(root, id)?;
            let saved = self.inner.signature(id)?;
            if saved.as_ref() != Some(&signature) {
                if let Some(loaded) = read_locked(root, id)? {
                    self.upsert(root, &loaded.session)?;
                }
            }
        }
        let known = self.ids()?;
        for id in known {
            if !ids.contains(&id) {
                self.remove(&id)?;
            }
        }
        // Draft markers are authoritative when recovering an interrupted publication.
        for id in ids {
            self.inner
                .draft(&id, root.join("_runs").join(format!("{id}.draft")).exists())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionStore;
    use rusqlite::Connection;
    #[tokio::test]
    async fn indexes_migrate_repair_and_preferences_survive_without_loading_history() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sessions");
        let store = SessionStore::new(&directory).await.unwrap();
        let mut session = store.create().await.unwrap();
        session.begin_execution("Review my project").unwrap();
        store.save(&mut session).await.unwrap();
        let catalog = store.catalog();
        let preferences = catalog
            .update_preferences(PreferenceUpdate {
                last_session_id: Some(Some(session.session_id.clone())),
                sidebar_collapsed: Some(true),
            })
            .unwrap();
        assert_eq!(preferences.revision, 1);
        catalog
            .update_preferences(PreferenceUpdate {
                sidebar_collapsed: Some(false),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            catalog.preferences().unwrap().last_session_id,
            Some(session.session_id.clone())
        );
        assert_eq!(
            catalog.page(0, 50).unwrap().sessions[0].title.as_deref(),
            Some("Review my project")
        );
        // A missing/corrupted acceleration snapshot is not needed by the indexed list.
        fs::write(
            directory.join(&session.session_id).join("snapshot.json"),
            b"invalid cache",
        )
        .unwrap();
        assert_eq!(catalog.page(0, 50).unwrap().sessions.len(), 1);
        // Simulate a committed journal whose index update was interrupted.
        Connection::open(&catalog.path)
            .unwrap()
            .execute("UPDATE sessions SET signature='stale',title='stale'", [])
            .unwrap();
        drop(catalog);
        drop(store);
        let store = SessionStore::new(&directory).await.unwrap();
        assert_eq!(
            store.summaries(false).await.unwrap()[0].title.as_deref(),
            Some("Review my project")
        );
        assert!(!store.catalog().preferences().unwrap().sidebar_collapsed);
        let mut restored = store.load(&session.session_id).await.unwrap();
        restored.recover_interrupted().unwrap();
        store.save(&mut restored).await.unwrap();
        store.delete(&session.session_id).await.unwrap();
        assert!(store.summaries(false).await.unwrap().is_empty());
        assert_eq!(store.catalog().preferences().unwrap().last_session_id, None);
    }
    #[tokio::test]
    async fn draft_filter_scope_and_nullable_patches_are_independent() {
        let root = tempfile::tempdir().unwrap();
        let a = SessionStore::new(root.path()).await.unwrap();
        let b = SessionStore::new(root.path().join("sessions"))
            .await
            .unwrap();
        let session = a.create().await.unwrap();
        a.catalog().draft(&session.session_id, true).unwrap();
        assert!(a.catalog().page(0, 50).unwrap().sessions.is_empty());
        assert!(b.list().await.unwrap().is_empty());
        assert!(
            a.catalog()
                .update_preferences(PreferenceUpdate {
                    last_session_id: Some(Some(session.session_id.clone())),
                    ..Default::default()
                })
                .is_err()
        );
        a.catalog().draft(&session.session_id, false).unwrap();
        a.catalog()
            .update_preferences(PreferenceUpdate {
                last_session_id: Some(Some(session.session_id)),
                ..Default::default()
            })
            .unwrap();
        let patch: PreferenceUpdate = serde_json::from_str("{\"last_session_id\":null}").unwrap();
        assert_eq!(patch.last_session_id, Some(None));
        a.catalog().update_preferences(patch).unwrap();
        assert_eq!(a.catalog().preferences().unwrap().last_session_id, None);
        assert!(a.catalog().page(99, 50).is_err());
    }
    #[tokio::test]
    async fn existing_legacy_sessions_are_imported_and_deleted_catalog_rows_rebuild() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sessions");
        fs::create_dir(&directory).unwrap();
        let session = Session::new();
        fs::write(
            directory.join(format!("{}.json", session.session_id)),
            serde_json::to_vec(&session).unwrap(),
        )
        .unwrap();
        let store = SessionStore::new(&directory).await.unwrap();
        assert_eq!(
            store.page(0, 50).await.unwrap().sessions[0].session_id,
            session.session_id
        );
        store.load(&session.session_id).await.unwrap();
        assert!(
            directory
                .join(&session.session_id)
                .join("rollout.jsonl")
                .exists()
        );
        let catalog = store.catalog();
        Connection::open(&catalog.path)
            .unwrap()
            .execute(
                "DELETE FROM sessions WHERE scope=?",
                [directory.canonicalize().unwrap().to_string_lossy().as_ref()],
            )
            .unwrap();
        drop(catalog);
        drop(store);
        let restored = SessionStore::new(&directory).await.unwrap();
        assert_eq!(restored.list().await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn old_indexes_drop_obsolete_columns_without_losing_preferences() {
        for version in [1, 2] {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().join("sessions");
            fs::create_dir(&directory).unwrap();
            let directory = directory.canonicalize().unwrap();
            let database = root.path().join("app.sqlite3");
            let connection = Connection::open(&database).unwrap();
            let old_schema = include_str!("../../../../kyler-store/src/catalog.sql")
                .replace(
                    "cwd TEXT, project TEXT,",
                    if version == 1 {
                        "workspace_root TEXT, workspace_mode TEXT,"
                    } else {
                        "cwd TEXT, workspace_root TEXT, workspace_mode TEXT,"
                    },
                )
                .replace("user_version=3", &format!("user_version={version}"));
            connection.execute_batch(&old_schema).unwrap();
            connection
                .execute(
                    "INSERT INTO ui_preferences(scope,sidebar_collapsed,revision) VALUES(?,1,7)",
                    [directory.to_string_lossy().as_ref()],
                )
                .unwrap();
            drop(connection);
            let mut session = Session::new();
            session.environment = Some(
                serde_json::from_value(serde_json::json!({
                    "cwd":root.path(),
                    "sandbox":{"base_directory":root.path(),"policy_id":"fixture"}
                }))
                .unwrap(),
            );
            fs::write(
                directory.join(format!("{}.json", session.session_id)),
                serde_json::to_vec(&session).unwrap(),
            )
            .unwrap();
            let store = SessionStore::new(&directory).await.unwrap();
            assert_eq!(
                store.page(0, 50).await.unwrap().sessions[0].cwd.as_deref(),
                root.path().to_str()
            );
            assert!(store.catalog().preferences().unwrap().sidebar_collapsed);
            assert_eq!(store.catalog().preferences().unwrap().revision, 7);
            let connection = Connection::open(database).unwrap();
            assert_eq!(
                connection
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                3
            );
            connection
                .prepare("SELECT cwd,project FROM sessions")
                .unwrap();
            assert!(
                connection
                    .prepare("SELECT workspace_root FROM sessions")
                    .is_err()
            );
            assert!(
                connection
                    .prepare("SELECT workspace_mode FROM sessions")
                    .is_err()
            );
            drop(store);
            assert!(SessionStore::new(&directory).await.is_ok());
        }
    }
}
