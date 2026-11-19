use anyhow::Result;
use kyler_protocol::api::{preferences::PreferenceUpdate, session::SessionSummary};
use kyler_store::{
    Database,
    catalog::{Catalog, SessionRecord},
};
use uuid::Uuid;

fn record(draft: bool) -> SessionRecord {
    SessionRecord {
        summary: SessionSummary {
            session_id: Uuid::new_v4().to_string(),
            title: Some("Persistent session".into()),
            cwd: None,
            project: None,
            created_at: 1,
            updated_at: 2,
            event_count: 3,
            execution_count: 0,
            status: None,
            draft: false,
        },
        revision: 1,
        signature: "reviewed journal".into(),
        last_message_preview: None,
        draft,
    }
}

#[test]
fn scopes_drafts_and_preferences_survive_reopen_and_removal() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("app.sqlite3");
    let a = Catalog::open(&path, "a".into())?;
    let b = Catalog::open(&path, "b".into())?;
    let record = record(true);
    let id = &record.summary.session_id;
    a.upsert(&record)?;
    assert!(a.page(0, 10)?.sessions.is_empty());
    assert!(b.list(false)?.is_empty());
    assert!(
        a.update_preferences(PreferenceUpdate {
            last_session_id: Some(Some(id.clone())),
            ..Default::default()
        })
        .is_err()
    );
    a.draft(id, false)?;
    a.update_preferences(PreferenceUpdate {
        last_session_id: Some(Some(id.clone())),
        sidebar_collapsed: Some(true),
    })?;
    drop(a);
    let a = Catalog::open(&path, "a".into())?;
    assert_eq!(
        a.preferences()?.last_session_id.as_deref(),
        Some(id.as_str())
    );
    assert!(a.preferences()?.sidebar_collapsed);
    assert_eq!(a.signature(id)?.as_deref(), Some("reviewed journal"));
    assert_eq!(a.page(0, 10)?.sessions[0].session_id, *id);
    assert!(a.page(2, 10).is_err());
    a.remove(id)?;
    assert!(a.ids()?.is_empty());
    assert_eq!(a.preferences()?.last_session_id, None);
    assert_eq!(b.preferences()?.revision, 0);
    Ok(())
}

#[test]
fn old_catalogs_upgrade_without_losing_preferences_or_partial_schema() -> Result<()> {
    for version in [1, 2] {
        let root = tempfile::tempdir()?;
        let path = root.path().join("app.sqlite3");
        let database = Database::open(&path)?;
        let schema = include_str!("../src/catalog.sql")
            .replace(
                "cwd TEXT, project TEXT,",
                if version == 1 {
                    "workspace_root TEXT, workspace_mode TEXT,"
                } else {
                    "cwd TEXT, workspace_root TEXT, workspace_mode TEXT,"
                },
            )
            .replace("user_version=3", &format!("user_version={version}"));
        database.with(|connection| {
            connection.execute_batch(&schema)?;
            connection.execute("INSERT INTO ui_preferences VALUES('scope',NULL,1,7)", [])?;
            Ok(())
        })?;
        let catalog = Catalog::open(&path, "scope".into())?;
        assert_eq!(catalog.preferences()?.revision, 7);
        assert!(catalog.preferences()?.sidebar_collapsed);
        database.with(|connection| {
            assert_eq!(
                connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?,
                3
            );
            connection.prepare("SELECT cwd,project FROM sessions")?;
            assert!(
                connection
                    .prepare("SELECT workspace_root FROM sessions")
                    .is_err()
            );
            Ok(())
        })?;
    }
    Ok(())
}
