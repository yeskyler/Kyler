//! Optional acceleration cache. The committed rollout remains authoritative.
use crate::{
    rollout::{RolloutPayload, RolloutPosition, RolloutRecord, writer::private_file},
    session::Session,
};

use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub schema_version: u32,
    pub position: RolloutPosition,
    pub session: Session,
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}

impl SessionSnapshot {
    /// Check that the cached cursor points to the same committed revision in the log.
    pub(crate) fn load(directory: &Path, id: &str) -> Result<Self> {
        let snapshot: Self = serde_json::from_slice(&fs::read(directory.join("snapshot.json"))?)?;
        ensure!(
            snapshot.schema_version == 1 && snapshot.session.session_id == id,
            "invalid snapshot metadata"
        );
        snapshot.session.validate()?;
        let mut file = File::open(directory.join("rollout.jsonl"))?;
        let offset = snapshot.position.log_offset;
        ensure!(
            offset > 0 && offset <= file.metadata()?.len(),
            "invalid snapshot cursor"
        );
        let start = offset.saturating_sub(4096);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![0; (offset - start) as usize];
        file.read_exact(&mut bytes)?;
        ensure!(
            bytes.last() == Some(&b'\n'),
            "snapshot cursor is not a record boundary"
        );
        let line_start = bytes[..bytes.len() - 1]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        let record: RolloutRecord = serde_json::from_slice(&bytes[line_start..])?;
        ensure!(
            record.schema_version == 1
                && record.session_id == id
                && record.seq == snapshot.position.last_seq,
            "snapshot/log position mismatch"
        );
        ensure!(
            matches!(record.payload, RolloutPayload::CheckpointCommitted { revision, updated_at } if revision == snapshot.session.revision && updated_at == snapshot.session.updated_at),
            "snapshot/log revision mismatch"
        );
        Ok(snapshot)
    }

    pub(crate) fn save(&self, directory: &Path) -> Result<()> {
        let temporary = directory.join(format!(".snapshot-{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = private_file(&temporary, true)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, directory.join("snapshot.json"))?;
            sync_directory(directory).context("snapshot directory sync failed")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}
