use crate::error::{ApiError, ApiResult};

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use axum::http::StatusCode;
use tokio::sync::{Semaphore, watch};

use kyler_protocol::api::{
    event::{EventPayload, ServerEvent},
    run::RunInfo,
};

pub struct Entry {
    pub inner: Mutex<RunData>,
    pub changed: watch::Sender<u64>,
    pub cancel: watch::Sender<bool>,
    pub subscribers: Arc<Semaphore>,
}
pub struct RunData {
    pub info: RunInfo,
    pub events: VecDeque<(Arc<ServerEvent>, usize)>,
    bytes: usize,
    max_events: usize,
    max_bytes: usize,
    /// True on restart: transient token events were not persisted.
    pub cache_lost: bool,
}
impl Entry {
    pub fn new(
        info: RunInfo,
        max_events: usize,
        max_bytes: usize,
        cache_lost: bool,
        max_subscribers: usize,
    ) -> Self {
        Self {
            inner: Mutex::new(RunData {
                info,
                events: VecDeque::new(),
                bytes: 0,
                max_events,
                max_bytes,
                cache_lost,
            }),
            changed: watch::channel(0).0,
            cancel: watch::channel(false).0,
            subscribers: Arc::new(Semaphore::new(max_subscribers)),
        }
    }
    pub fn info(&self) -> RunInfo {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .info
            .clone()
    }
    pub fn publish(&self, payload: EventPayload) {
        let seq = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(payload);
        self.changed.send_replace(seq);
    }
    pub fn complete(&self, mut info: RunInfo, payload: EventPayload) {
        let seq = {
            let mut data = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            info.last_event_seq = data.info.last_event_seq;
            data.info = info;
            data.push(payload)
        };
        self.changed.send_replace(seq);
    }
    pub fn after(&self, cursor: u64) -> ApiResult<(Vec<Arc<ServerEvent>>, bool)> {
        let data = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if cursor > data.info.last_event_seq {
            return Err(ApiError::bad("Event cursor is ahead of this run."));
        }
        let oldest = data
            .events
            .front()
            .map_or(data.info.last_event_seq + 1, |e| e.0.seq);
        if data.cache_lost || cursor.saturating_add(1) < oldest {
            return Err(ApiError::new(
                StatusCode::GONE,
                "event_cursor_expired",
                "Incremental events are unavailable; fetch the run status and result.",
            ));
        }
        let batch = data
            .events
            .iter()
            .filter(|e| e.0.seq > cursor)
            .take(32)
            .map(|e| e.0.clone())
            .collect::<Vec<_>>();
        let delivered = batch.last().map_or(cursor, |e| e.seq);
        Ok((
            batch,
            data.info.status.is_terminal() && delivered == data.info.last_event_seq,
        ))
    }
}

pub fn prefix(text: &str, max_bytes: usize) -> String {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

impl RunData {
    fn push(&mut self, payload: EventPayload) -> u64 {
        let event = Arc::new(ServerEvent {
            run_id: self.info.run_id.clone(),
            execution_id: self.info.execution_id.clone(),
            seq: self.info.last_event_seq + 1,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            payload,
        });
        let bytes = serde_json::to_vec(event.as_ref())
            .expect("serializable event")
            .len();
        self.info.last_event_seq = event.seq;
        if bytes <= self.max_bytes {
            while self.events.len() >= self.max_events || self.bytes + bytes > self.max_bytes {
                if let Some((_, bytes)) = self.events.pop_front() {
                    self.bytes -= bytes;
                } else {
                    break;
                }
            }
            self.bytes += bytes;
            self.events.push_back((event, bytes));
        } else {
            self.events.clear();
            self.bytes = 0;
        }
        self.info.last_event_seq
    }
}
