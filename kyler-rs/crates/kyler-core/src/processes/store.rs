//! Persist process state and mark interrupted work on restart.
use super::manager::{Entry, Inner, ProcessManager};

use std::{
    cmp::Reverse,
    collections::HashMap,
    fs::Permissions,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use tokio::sync::broadcast;

use kyler_protocol::api::processes::ProcessInfo;

pub(super) fn persist(inner: &Inner, entry: &Entry) -> Result<()> {
    if let Some(root) = &inner.root {
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        serde_json::to_writer(&mut file, &(&entry.session, &entry.info))?;
        file.as_file().sync_all()?;
        file.persist(root.join(format!("{}.json", entry.info.id)))
            .map_err(|error| error.error)?;
    }
    Ok(())
}

impl ProcessManager {
    pub fn persistent(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&root, Permissions::from_mode(0o700))?;
        }
        let manager = Self {
            inner: Arc::new(Inner {
                root: Some(root.clone()),
                entries: Mutex::new(HashMap::new()),
                events: broadcast::channel(256).0,
            }),
        };
        let mut files = std::fs::read_dir(root)?
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .collect::<Vec<_>>();
        files.sort_by_key(|entry| Reverse(entry.metadata().and_then(|m| m.modified()).ok()));
        for file in files.into_iter().take(128) {
            let (session, mut info): (String, ProcessInfo) =
                serde_json::from_slice(&std::fs::read(file.path())?)?;
            if info.status == "running" {
                info.status = "interrupted".into();
                info.error=Some("Server restarted; process outcome is unknown. Inspect state before repeating side effects.".into());
            }
            let (input, _) = std::sync::mpsc::sync_channel(1);
            let entry = Entry {
                session,
                info,
                input,
                abort: None,
                interaction: Arc::new(tokio::sync::Mutex::new(0)),
                command_key: None,
                stdin_closed: true,
            };
            persist(&manager.inner, &entry)?;
            manager
                .inner
                .entries
                .lock()
                .unwrap()
                .insert(entry.info.id.clone(), entry);
        }
        Ok(manager)
    }
}
