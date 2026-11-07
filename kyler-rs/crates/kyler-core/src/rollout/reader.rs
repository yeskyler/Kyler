use super::record::{ROLLOUT_SCHEMA_VERSION, RolloutPayload, RolloutPosition, RolloutRecord};

use std::{
    collections::VecDeque,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Take},
    path::Path,
};

use anyhow::{Context, Result, ensure};

/// Reads committed transactions only. An unfinished final transaction is ignored.
/// A complete malformed line or a sequence gap is an error, including at EOF.
pub struct RolloutReader {
    inner: Option<Reader>,
}

struct Reader {
    file: BufReader<Take<File>>,
    offset: u64,
    session_id: String,
    next_seq: u64,
    position: RolloutPosition,
    ready: VecDeque<RolloutRecord>,
    ready_position: Option<RolloutPosition>,
    ended: bool,
    failed: bool,
}

impl RolloutReader {
    pub fn open(path: impl AsRef<Path>, session_id: &str) -> Result<Self> {
        Self::from_position(path, session_id, RolloutPosition::default())
    }

    pub(crate) fn from_position(
        path: impl AsRef<Path>,
        session_id: &str,
        position: RolloutPosition,
    ) -> Result<Self> {
        let mut file = File::open(path)?;
        ensure!(
            position.log_offset <= file.metadata()?.len(),
            "snapshot extends beyond rollout"
        );
        let remaining = file.metadata()?.len() - position.log_offset;
        file.seek(SeekFrom::Start(position.log_offset))?;
        Ok(Self {
            inner: Some(Reader {
                file: BufReader::new(file.take(remaining)),
                offset: position.log_offset,
                session_id: session_id.into(),
                next_seq: position
                    .last_seq
                    .checked_add(1)
                    .context("rollout sequence overflow")?,
                position,
                ready: VecDeque::new(),
                ready_position: None,
                ended: false,
                failed: false,
            }),
        })
    }

    pub fn position(&self) -> RolloutPosition {
        self.inner.as_ref().expect("reader available").position
    }

    /// Async reads run on the blocking I/O pool. If cancelled, discard this reader and reopen.
    pub async fn next(&mut self) -> Result<Option<RolloutRecord>> {
        let mut inner = self
            .inner
            .take()
            .context("reader was cancelled; reopen it")?;
        let (inner, result) = tokio::task::spawn_blocking(move || {
            let result = inner.next();
            (inner, result)
        })
        .await?;
        self.inner = Some(inner);
        result
    }

    pub(crate) fn read_next(&mut self) -> Result<Option<RolloutRecord>> {
        self.inner.as_mut().context("reader unavailable")?.next()
    }
}

impl Reader {
    fn next(&mut self) -> Result<Option<RolloutRecord>> {
        ensure!(
            !self.failed,
            "rollout reader failed; reopen after investigating corruption"
        );
        let result = self.next_inner();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn pop_ready(&mut self) -> Option<RolloutRecord> {
        let record = self.ready.pop_front();
        if self.ready.is_empty() {
            if let Some(position) = self.ready_position.take() {
                self.position = position;
            }
        }
        record
    }

    fn next_inner(&mut self) -> Result<Option<RolloutRecord>> {
        if !self.ready.is_empty() {
            return Ok(self.pop_ready());
        }
        if self.ended {
            return Ok(None);
        }
        let mut pending = VecDeque::new();
        loop {
            let mut line = Vec::new();
            let count = self.file.read_until(b'\n', &mut line)?;
            self.offset += count as u64;
            if count == 0 || !line.ends_with(b"\n") {
                self.ended = true;
                return Ok(None);
            }
            let record: RolloutRecord = serde_json::from_slice(&line)
                .with_context(|| format!("invalid rollout record at sequence {}", self.next_seq))?;
            ensure!(
                record.schema_version == ROLLOUT_SCHEMA_VERSION,
                "unsupported rollout schema {}",
                record.schema_version
            );
            ensure!(
                record.session_id == self.session_id,
                "rollout session ID mismatch"
            );
            ensure!(
                record.seq == self.next_seq,
                "rollout sequence gap: expected {}, found {}",
                self.next_seq,
                record.seq
            );
            self.next_seq = self
                .next_seq
                .checked_add(1)
                .context("rollout sequence overflow")?;
            let committed = matches!(record.payload, RolloutPayload::CheckpointCommitted { .. });
            pending.push_back(record);
            if committed {
                self.ready_position = Some(RolloutPosition {
                    last_seq: self.next_seq - 1,
                    log_offset: self.offset,
                });
                self.ready = pending;
                return Ok(self.pop_ready());
            }
        }
    }
}
