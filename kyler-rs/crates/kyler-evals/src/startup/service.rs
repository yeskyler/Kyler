use std::collections::BTreeMap;

use anyhow::{Result, ensure};

use super::{probe::Measurement, profile::Prepared};

pub async fn check_instrumentation(binary: &std::path::Path) -> Result<()> {
    let mut command = tokio::process::Command::new(binary);
    command
        .arg("--version")
        .env(kyler_protocol::startup::TRACE_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(10), command.output()).await??;
    let instrumented = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix(kyler_protocol::startup::PREFIX))
        .filter_map(|line| serde_json::from_str::<kyler_protocol::startup::StartupEvent>(line).ok())
        .any(|event| event.event == "main_entered");
    ensure!(
        output.status.success() && instrumented,
        "Executable lacks startup instrumentation; rebuild kyler-cli or select an instrumented baseline"
    );
    Ok(())
}

pub async fn health(address: &str) -> Result<()> {
    let address: std::net::SocketAddr = address.parse()?;
    ensure!(
        address.ip().is_loopback(),
        "Startup service must bind loopback"
    );
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?
        .get(format!("http://{address}/healthz"))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

pub fn measurement(
    prepared: &Prepared,
    timeline: &BTreeMap<String, f64>,
    resources: &serde_json::Value,
) -> Result<Measurement> {
    let mut phases = BTreeMap::new();
    for (name, start, end) in [
        ("async_runtime", "main_entered", "async_runtime_ready"),
        (
            "configuration_and_argument_parsing",
            "async_runtime_ready",
            "configuration_ready",
        ),
        ("logging", "configuration_ready", "logging_ready"),
        ("sandbox_factory", "logging_ready", "sandbox_factory_ready"),
        (
            "storage_and_workspace",
            "sandbox_factory_ready",
            "storage_ready",
        ),
        ("agent_construction", "storage_ready", "agent_ready"),
        ("server_state", "agent_ready", "server_state_ready"),
        ("http_ready", "server_state_ready", "service_ready"),
    ] {
        if let Some((a, b)) = timeline.get(start).zip(timeline.get(end)) {
            phases.insert(name.into(), b - a);
        }
    }
    Ok(Measurement{configuration:prepared.configuration.clone(),configuration_fingerprint:crate::report::fingerprint(&prepared.configuration)?,phases_ms:phases,timings:Default::default(),first_text_ms:None,turn_ms:None,peak_rss_bytes:resources["peak_rss_bytes"].as_u64(),cpu_seconds:resources["cpu_seconds"].as_f64(),ready_scope:"Actual kyler serve process passed HTTP health. Session capabilities remain lazy and are not claimed ready.".into(),warmup_turn_ms:None,restore_ms:None,capabilities_evaluated:false,degraded:Vec::new()})
}
