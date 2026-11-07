//! Opt-in, payload-free startup timestamps shared by the CLI and performance harness.
use std::{io::Write, sync::OnceLock, time::Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TRACE_ENV: &str = "KYLER_STARTUP_TRACE";
pub const PREFIX: &str = "KYLER_STARTUP_EVENT ";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupEvent {
    pub event: String,
    pub process_id: u32,
    pub elapsed_ms: f64,
    pub monotonic_ns: Option<u64>,
    pub data: Value,
}

/// CLOCK_MONOTONIC has the same origin in a parent and its local child processes.
pub fn monotonic_ns() -> Option<u64> {
    #[cfg(unix)]
    {
        let mut time = std::mem::MaybeUninit::<libc::timespec>::zeroed();
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, time.as_mut_ptr()) } == 0 {
            let time = unsafe { time.assume_init() };
            return (time.tv_sec as u64)
                .checked_mul(1_000_000_000)?
                .checked_add(time.tv_nsec as u64);
        }
    }
    None
}

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os(TRACE_ENV).is_some_and(|value| value == "1"))
}

/// Disabled in normal operation. Never include configuration, credentials or message content.
pub fn mark(event: &str) {
    emit(event, Value::Null);
}
pub fn emit(event: &str, data: Value) {
    if !enabled() {
        return;
    }
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = ORIGIN.get_or_init(Instant::now);
    let record = StartupEvent {
        event: event.into(),
        process_id: std::process::id(),
        elapsed_ms: origin.elapsed().as_secs_f64() * 1000.0,
        monotonic_ns: monotonic_ns(),
        data,
    };
    let mut output = std::io::stdout().lock();
    if let Ok(json) = serde_json::to_string(&record) {
        let _ = writeln!(output, "{PREFIX}{json}");
        let _ = output.flush();
    }
}

pub fn resources() -> Value {
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } == 0 {
            let usage = unsafe { usage.assume_init() };
            let rss = usage.ru_maxrss as u64;
            let rss = if cfg!(target_os = "macos") {
                rss
            } else {
                rss * 1024
            };
            return serde_json::json!({"peak_rss_bytes":rss,"cpu_seconds":usage.ru_utime.tv_sec as f64+usage.ru_utime.tv_usec as f64/1e6+usage.ru_stime.tv_sec as f64+usage.ru_stime.tv_usec as f64/1e6});
        }
    }
    Value::Null
}
