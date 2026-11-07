use super::record::{ROLLOUT_SCHEMA_VERSION, RolloutPayload, RolloutPosition, RolloutRecord};

use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

/// An exclusive OS file lock owns the writer, across processes as well as threads.
/// Store uses the synchronous methods on a blocking I/O task.
pub struct RolloutWriter {
    file: File,
    path: PathBuf,
    session_id: String,
    position: RolloutPosition,
    initialized: bool,
    poisoned: bool,
}

pub(crate) fn private_file(path: &Path, create_new: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

impl RolloutWriter {
    pub fn open(path: impl AsRef<Path>, session_id: &str) -> Result<Self> {
        let mut writer = Self::lock(path.as_ref(), session_id)?;
        let mut reader = super::RolloutReader::open(&writer.path, session_id)?;
        while reader.read_next()?.is_some() {}
        writer.resume(reader.position())?;
        Ok(writer)
    }

    pub(crate) fn lock(path: &Path, session_id: &str) -> Result<Self> {
        let file = private_file(path, false)?;
        file.try_lock().context("rollout already has a writer")?;
        Ok(Self {
            file,
            path: path.into(),
            session_id: session_id.into(),
            position: RolloutPosition::default(),
            initialized: false,
            poisoned: false,
        })
    }

    /// Discard only a verified unfinished tail while holding the exclusive log lock.
    pub(crate) fn resume(&mut self, position: RolloutPosition) -> Result<()> {
        ensure!(
            position.log_offset <= self.file.metadata()?.len(),
            "invalid rollout position"
        );
        if position.log_offset < self.file.metadata()?.len() {
            self.file.set_len(position.log_offset)?;
            self.file.sync_all()?;
        }
        self.file.seek(SeekFrom::End(0))?;
        self.position = position;
        self.initialized = true;
        Ok(())
    }

    pub fn position(&self) -> RolloutPosition {
        self.position
    }

    /// Commits a whole checkpoint and fsyncs it before returning. Never append individual
    /// records outside a transaction. Reopen after an I/O error to recover the durable prefix.
    pub fn append_checkpoint(
        &mut self,
        execution_id: Option<&str>,
        payloads: Vec<RolloutPayload>,
    ) -> Result<RolloutPosition> {
        ensure!(
            self.initialized && !self.poisoned,
            "rollout writer must be recovered/reopened"
        );
        ensure!(
            matches!(
                payloads.last(),
                Some(RolloutPayload::CheckpointCommitted { .. })
            ),
            "checkpoint requires a commit record"
        );
        ensure!(
            !payloads[..payloads.len() - 1]
                .iter()
                .any(|p| matches!(p, RolloutPayload::CheckpointCommitted { .. })),
            "checkpoint contains multiple commits"
        );
        let mut bytes = Vec::new();
        let mut seq = self.position.last_seq;
        for payload in payloads {
            seq = seq.checked_add(1).context("rollout sequence overflow")?;
            let record = RolloutRecord {
                schema_version: ROLLOUT_SCHEMA_VERSION,
                session_id: self.session_id.clone(),
                execution_id: execution_id.map(str::to_owned),
                seq,
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
                payload,
            };
            serde_json::to_writer(&mut bytes, &record)?;
            bytes.push(b'\n');
        }
        self.poisoned = true;
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        self.position = RolloutPosition {
            last_seq: seq,
            log_offset: self.file.stream_position()?,
        };
        self.poisoned = false;
        Ok(self.position)
    }
}
