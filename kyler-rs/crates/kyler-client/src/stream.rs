use crate::{
    Client,
    decoder::{Decoder, Frame},
    error::{Error, ErrorKind, Result},
};

use std::{collections::VecDeque, pin::Pin, time::Duration};

use futures::{Stream, StreamExt};
use tokio::time::Instant;

use kyler_protocol::api::{
    ApiErrorBody,
    event::{EventPayload, ServerEvent},
    run::{RunInfo, RunStatus},
};

type BytesStream = Pin<Box<dyn Stream<Item = std::result::Result<Vec<u8>, reqwest::Error>> + Send>>;
#[derive(Debug, Clone)]
pub enum ClientEvent {
    Server(ServerEvent),
    Reconnecting { attempt: u32, last_event_id: u64 },
    Gap { message: String },
    Result(RunInfo),
}
/// Owns a subscription, not an execution. Dropping it never cancels the server run.
pub struct RunStream {
    client: Client,
    id: String,
    cursor: u64,
    base: RunInfo,
    transport: Option<BytesStream>,
    decoder: Decoder,
    frames: VecDeque<Frame>,
    attempts: u32,
    retry_at: Option<Instant>,
    poll_at: Option<Instant>,
    polling: bool,
    pending_result: Option<RunInfo>,
    query_terminal: bool,
    ended: bool,
}
impl Client {
    pub async fn watch(&self, id: &str, after: u64) -> Result<RunStream> {
        let base = self.run(id).await?;
        if base.run_id != id {
            return Err(Error::protocol(
                "Run response ID does not match the request.",
            ));
        }
        let pending_result = if base.status.is_terminal() && after >= base.last_event_seq {
            Some(base.clone())
        } else {
            None
        };
        Ok(RunStream {
            client: self.clone(),
            id: id.into(),
            cursor: after,
            base,
            transport: None,
            decoder: Decoder::new(self.settings.max_event_bytes),
            frames: VecDeque::new(),
            attempts: 0,
            retry_at: None,
            poll_at: None,
            polling: false,
            pending_result,
            query_terminal: false,
            ended: false,
        })
    }
}
impl RunStream {
    pub fn cursor(&self) -> u64 {
        self.cursor
    }
    pub fn run_id(&self) -> &str {
        &self.id
    }
    /// Cancellation-safe between calls. Retain this object if next() is interrupted by Ctrl+C.
    pub async fn next(&mut self) -> Result<Option<ClientEvent>> {
        if self.ended {
            return Ok(None);
        }
        let result = self.next_inner().await;
        if result.is_err() {
            self.ended = true;
            self.transport = None;
        }
        result
    }
    async fn next_inner(&mut self) -> Result<Option<ClientEvent>> {
        loop {
            if let Some(result) = self.pending_result.take() {
                self.ended = true;
                return Ok(Some(ClientEvent::Result(result)));
            }
            if let Some(deadline) = self.retry_at {
                tokio::time::sleep_until(deadline).await;
                self.retry_at = None;
            }
            if self.query_terminal {
                let info = match self.client.run(&self.id).await {
                    Ok(info) => info,
                    Err(error) if retryable(&error) => return self.retry().await,
                    Err(error) => return Err(error),
                };
                if !info.status.is_terminal() {
                    return Err(Error::protocol(
                        "A terminal event was followed by a nonterminal run state.",
                    ));
                }
                self.query_terminal = false;
                self.pending_result = Some(info);
                continue;
            }
            if self.polling {
                if let Some(deadline) = self.poll_at {
                    tokio::time::sleep_until(deadline).await;
                    self.poll_at = None;
                }
                match self.client.run(&self.id).await {
                    Ok(info) => {
                        if info.status.is_terminal() {
                            self.pending_result = Some(info);
                            continue;
                        }
                        self.base = info;
                        self.attempts = 0;
                        self.poll_at = Some(
                            Instant::now()
                                + Duration::from_millis(
                                    self.client.settings.reconnect_delay_ms.clamp(100, 1000),
                                ),
                        );
                    }
                    Err(error) if retryable(&error) => return self.retry().await,
                    Err(error) => return Err(error),
                }
                continue;
            }
            if let Some(frame) = self.frames.pop_front() {
                if frame.name == "stream_error" {
                    let error = serde_json::from_str::<ApiErrorBody>(&frame.data)
                        .map_err(|_| Error::protocol("Invalid stream error event."))?;
                    if error.code == "event_cursor_expired" {
                        return Ok(Some(self.gap()));
                    }
                    return Err(Error::protocol("The server terminated its event stream."));
                }
                let event: ServerEvent = serde_json::from_str(&frame.data)
                    .map_err(|_| Error::protocol("Invalid server event JSON."))?;
                let id = frame
                    .id
                    .as_deref()
                    .and_then(|id| id.parse::<u64>().ok())
                    .ok_or_else(|| Error::protocol("An SSE event is missing a valid ID."))?;
                if event.run_id != self.id || id != event.seq || frame.name != event.payload.name()
                {
                    return Err(Error::protocol(
                        "SSE event identity does not match its payload.",
                    ));
                }
                if event.seq <= self.cursor {
                    continue;
                }
                if self.cursor.checked_add(1) != Some(event.seq) {
                    return Err(Error::protocol(
                        "Noncontiguous SSE event sequence; output was not joined across the gap.",
                    ));
                }
                self.cursor = event.seq;
                self.attempts = 0;
                self.base.execution_id = event.execution_id.clone();
                self.base.last_event_seq = event.seq;
                match &event.payload {
                    EventPayload::RunCompleted {
                        output,
                        output_truncated,
                        usage,
                    } => {
                        if *output_truncated {
                            self.query_terminal = true;
                        } else {
                            let mut result = self.base.clone();
                            result.status = RunStatus::Completed;
                            result.output = Some(output.clone());
                            result.usage = Some(usage.clone());
                            result.finished_at = Some(event.timestamp_ms);
                            result.error = None;
                            self.pending_result = Some(result);
                        }
                    }
                    EventPayload::RunFailed { .. } | EventPayload::RunCancelled => {
                        self.query_terminal = true
                    }
                    _ => {}
                }
                return Ok(Some(ClientEvent::Server(event)));
            }
            if self.transport.is_none() {
                let request = self
                    .client
                    .http
                    .get(self.client.url(&format!("/v1/runs/{}/events", self.id)))
                    .header("accept", "text/event-stream")
                    .header("last-event-id", self.cursor.to_string());
                let connected = tokio::time::timeout(
                    Duration::from_secs(self.client.settings.request_timeout_seconds),
                    async {
                        let response = request.send().await.map_err(|_| Error::transport())?;
                        let response = self.client.checked(response).await?;
                        if !response
                            .headers()
                            .get("content-type")
                            .and_then(|v| v.to_str().ok())
                            .is_some_and(|v| {
                                v.split(';').next().is_some_and(|s| {
                                    s.trim().eq_ignore_ascii_case("text/event-stream")
                                })
                            })
                        {
                            return Err(Error::protocol("Expected a text/event-stream response."));
                        }
                        Ok(response)
                    },
                )
                .await
                .unwrap_or_else(|_| Err(Error::transport()));
                match connected {
                    Ok(response) => {
                        self.decoder = Decoder::new(self.client.settings.max_event_bytes);
                        self.transport = Some(Box::pin(
                            response.bytes_stream().map(|r| r.map(|b| b.to_vec())),
                        ));
                    }
                    Err(error)
                        if error.status == Some(410) && error.code == "event_cursor_expired" =>
                    {
                        return Ok(Some(self.gap()));
                    }
                    Err(error) if retryable(&error) => return self.retry().await,
                    Err(error) => return Err(error),
                }
            }
            let chunk = tokio::time::timeout(
                Duration::from_secs(self.client.settings.stream_idle_timeout_seconds),
                self.transport.as_mut().expect("connected stream").next(),
            )
            .await;
            match chunk {
                Ok(Some(Ok(bytes))) => self.frames.extend(self.decoder.push(&bytes)?),
                _ => return self.retry().await,
            }
        }
    }
    fn gap(&mut self) -> ClientEvent {
        self.transport = None;
        self.frames.clear();
        self.polling = true;
        self.attempts = 0;
        ClientEvent::Gap {message:"Incremental output is unavailable. Waiting for the authoritative final result; missing text will not be silently joined.".into()}
    }
    async fn retry(&mut self) -> Result<Option<ClientEvent>> {
        self.transport = None;
        self.frames.clear();
        if self.attempts >= self.client.settings.reconnect_max_attempts {
            let info = self.client.run(&self.id).await?;
            if info.status.is_terminal() {
                self.pending_result = Some(info);
                return Ok(Some(ClientEvent::Gap {message:"The stream ended before its terminal event; the final result was recovered from run status.".into()}));
            }
            return Err(Error::new(
                ErrorKind::Transport,
                "stream_unavailable",
                "Reconnection attempts were exhausted. The run may still be active; use `kyler runs show` or `kyler runs watch` with its ID.",
            ));
        }
        self.attempts += 1;
        let delay = self
            .client
            .settings
            .reconnect_delay_ms
            .saturating_mul(1u64 << ((self.attempts - 1).min(6)))
            .min(30000);
        self.retry_at = Some(Instant::now() + Duration::from_millis(delay));
        Ok(Some(ClientEvent::Reconnecting {
            attempt: self.attempts,
            last_event_id: self.cursor,
        }))
    }
}
fn retryable(error: &Error) -> bool {
    error.kind == ErrorKind::Transport
        || error
            .status
            .is_some_and(|status| status == 429 || status >= 500)
}
