//! Opt-in latency measurements. Never record requests, credentials, or tool arguments.
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use tokio::sync::mpsc;

pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("KYLER_TRACE_LATENCY").is_ok_and(|value| value == "1"))
}

pub(crate) struct ChannelTiming {
    channel: &'static str,
    session_id: String,
    sends: AtomicU64,
    wait_us: AtomicU64,
    max_wait_us: AtomicU64,
    full_observations: AtomicU64,
}

impl ChannelTiming {
    pub fn new(channel: &'static str, session_id: &str) -> Self {
        Self {
            channel,
            session_id: if enabled() {
                session_id.to_owned()
            } else {
                String::new()
            },
            sends: AtomicU64::new(0),
            wait_us: AtomicU64::new(0),
            max_wait_us: AtomicU64::new(0),
            full_observations: AtomicU64::new(0),
        }
    }

    pub async fn send<T>(
        &self,
        sender: &mpsc::Sender<T>,
        value: T,
    ) -> Result<(), mpsc::error::SendError<T>> {
        if !enabled() {
            return sender.send(value).await;
        }
        let started = Instant::now();
        if sender.capacity() == 0 {
            self.full_observations.fetch_add(1, Ordering::Relaxed);
        }
        let result = sender.send(value).await;
        let wait_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        self.sends.fetch_add(1, Ordering::Relaxed);
        self.wait_us.fetch_add(wait_us, Ordering::Relaxed);
        self.max_wait_us.fetch_max(wait_us, Ordering::Relaxed);
        if wait_us >= 10_000 {
            tracing::info!(target: "kyler_latency", channel=self.channel, session_id=%self.session_id,
                wait_us, "slow event send");
        }
        result
    }
}

impl Drop for ChannelTiming {
    fn drop(&mut self) {
        if enabled() {
            tracing::info!(target: "kyler_latency", channel=self.channel, session_id=%self.session_id,
                sends=self.sends.load(Ordering::Relaxed), wait_us=self.wait_us.load(Ordering::Relaxed),
                max_wait_us=self.max_wait_us.load(Ordering::Relaxed),
                full_observations=self.full_observations.load(Ordering::Relaxed), "event channel summary");
        }
    }
}

pub(crate) struct ModelTiming {
    started: Instant,
    session_id: String,
    execution_id: String,
    step: u32,
    pub chunks: u64,
    pub read_wait_us: u64,
    pub max_read_wait_us: u64,
    pub reasoning_bytes: usize,
    pub text_bytes: usize,
    pub tool_argument_bytes: usize,
    pub usage: Option<serde_json::Value>,
    pub complete: bool,
}

impl ModelTiming {
    pub fn new(session: &crate::Session) -> Option<Self> {
        if !enabled() {
            return None;
        }
        let execution = session.execution.as_ref()?;
        let timing = Self {
            started: Instant::now(),
            session_id: session.session_id.clone(),
            execution_id: execution.execution_id.clone(),
            step: execution.current_step,
            chunks: 0,
            read_wait_us: 0,
            max_read_wait_us: 0,
            reasoning_bytes: 0,
            text_bytes: 0,
            tool_argument_bytes: 0,
            usage: None,
            complete: false,
        };
        timing.stage("request_started");
        Some(timing)
    }

    pub fn stage(&self, stage: &'static str) {
        tracing::info!(target: "kyler_latency", session_id=%self.session_id,
            execution_id=%self.execution_id, step=self.step, stage,
            elapsed_ms=self.started.elapsed().as_secs_f64()*1000.0, "model latency");
    }
}

impl Drop for ModelTiming {
    fn drop(&mut self) {
        let usage = self.usage.as_ref();
        let number = |path: &str| {
            usage
                .and_then(|usage| usage.pointer(path))
                .and_then(|v| v.as_u64())
        };
        tracing::info!(target: "kyler_latency", session_id=%self.session_id,
            execution_id=%self.execution_id, step=self.step, complete=self.complete,
            elapsed_ms=self.started.elapsed().as_secs_f64()*1000.0, chunks=self.chunks,
            read_wait_us=self.read_wait_us, max_read_wait_us=self.max_read_wait_us,
            reasoning_bytes=self.reasoning_bytes, text_bytes=self.text_bytes,
            tool_argument_bytes=self.tool_argument_bytes,
            prompt_tokens=?number("/prompt_tokens"), completion_tokens=?number("/completion_tokens"),
            cached_tokens=?number("/prompt_tokens_details/cached_tokens"),
            reasoning_tokens=?number("/completion_tokens_details/reasoning_tokens"), "model latency summary");
    }
}
