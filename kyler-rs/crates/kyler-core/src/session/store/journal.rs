//! Authoritative journal replay, legacy import and atomic session commits.
use super::{
    locking::{execution_lease, owns_execution},
    validate_id,
};
use crate::{
    rollout::{
        RolloutPayload, RolloutReader, RolloutWriter,
        replay::{changes, replay_from},
    },
    session::{
        Session,
        snapshot::{SessionSnapshot, sync_directory},
    },
};

use std::{fs as disk, io::ErrorKind, path::Path};

use anyhow::{Context, Result, ensure};

pub(super) struct Loaded {
    pub(super) session: Session,
    pub(super) writer: Option<RolloutWriter>,
}

pub(super) fn read_locked(root: &Path, id: &str) -> Result<Option<Loaded>> {
    validate_id(id)?;
    let directory = root.join(id);
    let log = directory.join("rollout.jsonl");
    if log.try_exists()? {
        let mut writer = RolloutWriter::lock(&log, id)?;
        let cached = match SessionSnapshot::load(&directory, id) {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                if directory.join("snapshot.json").exists() {
                    tracing::warn!(session_id=id, error=%error, "snapshot unavailable; rebuilding from rollout");
                }
                None
            }
        };
        let (base, position) = cached
            .map(|s| (Some(s.session), s.position))
            .unwrap_or_default();
        let mut reader = RolloutReader::from_position(&log, id, position)?;
        let restored = replay_from(&mut reader, base)?;
        writer.resume(reader.position())?;
        if let Some(session) = restored {
            return Ok(Some(Loaded {
                session,
                writer: Some(writer),
            }));
        }
        // Empty/crashed first write can still be recovered from an intact legacy file.
    } else {
        ensure!(
            !directory.join("snapshot.json").exists(),
            "snapshot exists but authoritative rollout is missing"
        );
    }
    match disk::read(root.join(format!("{id}.json"))) {
        Ok(bytes) => {
            let session: Session = serde_json::from_slice(&bytes)?;
            session.validate()?;
            ensure!(
                session.session_id == id,
                "session ID does not match file name"
            );
            Ok(Some(Loaded {
                session,
                writer: None,
            }))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn private_directory(path: &Path) -> Result<()> {
    let mut builder = disk::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            ensure!(path.is_dir(), "session path is not a directory")
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

pub(super) fn commit(root: &Path, next: &mut Session, loaded: Option<Loaded>) -> Result<()> {
    let previous = loaded.as_ref().map(|l| &l.session);
    let mut payloads = changes(previous, next)?;
    let directory = root.join(&next.session_id);
    private_directory(&directory)?;
    sync_directory(root)?;
    let mut writer = match loaded {
        Some(Loaded {
            writer: Some(writer),
            ..
        }) => writer,
        Some(Loaded {
            session,
            writer: None,
        }) => {
            payloads.insert(
                0,
                RolloutPayload::SessionCreated {
                    session: Box::new(session),
                },
            );
            RolloutWriter::open(directory.join("rollout.jsonl"), &next.session_id)?
        }
        None => RolloutWriter::open(directory.join("rollout.jsonl"), &next.session_id)?,
    };
    let revision = next
        .revision
        .checked_add(1)
        .context("session revision overflow")?;
    let updated_at = chrono::Utc::now().timestamp_millis();
    payloads.push(RolloutPayload::CheckpointCommitted {
        revision,
        updated_at,
    });
    let position = writer.append_checkpoint(
        next.execution.as_ref().map(|e| e.execution_id.as_str()),
        payloads,
    )?;
    // Durability includes the newly created rollout directory entry, before tool effects.
    sync_directory(&directory)?;
    next.revision = revision;
    next.updated_at = updated_at;
    // Optional cache: its failure does not invalidate an already durable journal commit.
    if revision == 1 || revision % 16 == 0 || !next.is_running() {
        if let Err(error) = (SessionSnapshot {
            schema_version: 1,
            position,
            session: next.clone(),
        })
        .save(&directory)
        {
            tracing::warn!(session_id=%next.session_id,error=%error,"rollout committed but snapshot cache failed");
        }
    }
    Ok(())
}

pub(super) fn persist(root: &Path, mut session: Session) -> Result<Session> {
    session.validate()?;
    let loaded = read_locked(root, &session.session_id)?;
    match &loaded {
        Some(saved) => ensure!(
            session.revision == saved.session.revision,
            "session changed on disk; reload before saving or running"
        ),
        None => ensure!(
            session.revision == 0,
            "session was deleted; refusing to recreate stale session"
        ),
    }
    // A process may checkpoint its own active run, but cannot mutate another process's run.
    let _external_lease = if owns_execution(root, &session.session_id) {
        None
    } else {
        Some(execution_lease(root, &session.session_id)?)
    };
    commit(root, &mut session, loaded)?;
    Ok(session)
}
