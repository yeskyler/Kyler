use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};

use kyler_protocol::startup::{self, StartupEvent};

use super::{ModelMode, Sample, Scenario, Settings, probe, profile, service, statistics};
use crate::{gaia::dataset::digest_file, report};

pub async fn run(
    mut settings: Settings,
    app_config: Option<PathBuf>,
    root: PathBuf,
    eval_config: Option<PathBuf>,
    worker: PathBuf,
) -> Result<PathBuf> {
    settings.validate()?;
    if settings.scenario == Scenario::ProcessCold {
        let path = settings
            .kyler_bin
            .clone()
            .unwrap_or_else(|| worker.parent().unwrap_or(Path::new(".")).join("kyler"));
        settings.kyler_bin = Some(
            path.canonicalize()
                .context("Kyler binary missing; build kyler-cli or pass --kyler-bin")?,
        );
    }
    if let Some(path) = &mut settings.baseline_bin {
        *path = path
            .canonicalize()
            .context("Baseline Kyler binary missing")?;
    }
    if settings.scenario == Scenario::ProcessCold {
        for binary in [&settings.kyler_bin, &settings.baseline_bin]
            .into_iter()
            .flatten()
        {
            service::check_instrumentation(binary).await?;
        }
    }
    let directory = report::create_run(&root)?;
    let _lock = report::RunLock::acquire(&directory)?;
    let mock = if settings.model_mode == ModelMode::Mock {
        Some(probe::mock_server().await?)
    } else {
        None
    };
    let app_config = app_config.map(|p| p.canonicalize()).transpose()?;
    let eval_config = eval_config.map(|p| p.canonicalize()).transpose()?;
    let settings_path = directory.join("settings.json");
    report::write_json(&settings_path, &settings)?;
    let mut manifest = json!({
        "kind":"startup","schema_version":2,"scenario":settings.scenario,"profile":settings.profile,
        "model_mode":settings.model_mode,"settings":settings,"platform":format!("{}-{}",std::env::consts::OS,std::env::consts::ARCH),
        "logical_cpus":std::thread::available_parallelism().ok().map(|n|n.get()),
        "scope":if settings.scenario==Scenario::ProcessCold {"Actual kyler serve executable, from process creation through HTTP health and graceful shutdown"}else{"Fresh evaluation subprocess using the real Agent/runtime; session-cold performs no capability prewarming"},
        "clock":if startup::monotonic_ns().is_some(){"cross-process CLOCK_MONOTONIC"}else{"parent-observed event receipt (includes transport latency)"},
        "memory_scope":"measured process peak RSS only; excludes live workers and MCP servers",
        "cache_policy":"fresh process/private storage per sample; OS file cache retained",
        "executable_fingerprint":digest_file(&worker)?,
        "kyler_fingerprint":settings.kyler_bin.as_ref().map(|p|digest_file(p)).transpose()?,
        "baseline_fingerprint":settings.baseline_bin.as_ref().map(|p|digest_file(p)).transpose()?,
        "build_profile":if cfg!(debug_assertions){"debug"}else{"release"},
        "configuration_fingerprint":report::fingerprint(&settings)?,
    });
    report::write_json(&directory.join("manifest.json"), &manifest)?;
    let mut samples = Vec::new();
    let mut cancelled = false;
    for round in 0..settings.warmup + settings.repeat {
        let mut variants = vec!["candidate"];
        if settings.baseline_bin.is_some() {
            variants.push("baseline");
            if round % 2 == 1 {
                variants.reverse();
            }
        }
        for variant in variants {
            let private = directory.join("samples").join(format!("{round}-{variant}"));
            std::fs::create_dir_all(&private)?;
            // Synthetic services and files are prepared outside the measured interval.
            let prepared = profile::prepare(
                &settings,
                app_config.as_deref(),
                eval_config.as_deref(),
                mock.as_ref().map(|m| m.url.as_str()),
            )
            .await?;
            let configuration_fingerprint = report::fingerprint(&prepared.configuration)?;
            if let Some(previous) = manifest.get("runtime_configuration_fingerprint") {
                ensure!(
                    previous.as_str() == Some(configuration_fingerprint.as_str()),
                    "Configuration or fixtures changed between samples"
                );
            } else {
                manifest["runtime_configuration"] = prepared.configuration.clone();
                manifest["runtime_configuration_fingerprint"] =
                    configuration_fingerprint.clone().into();
                manifest["configuration_fingerprint"]=report::fingerprint(&json!({"scenario":settings.scenario,"profile":settings.profile,"mode":settings.model_mode,"runtime":configuration_fingerprint,"restore_turns":settings.restore_turns,"restore_snapshot":settings.restore_snapshot,"required_mcp":settings.required_mcp}))?.into();
                report::write_json(&directory.join("manifest.json"), &manifest)?;
            }
            let sample = sample(
                &settings,
                &worker,
                &settings_path,
                &prepared,
                &private,
                round,
                variant,
            )
            .await?;
            cancelled = sample.status == "cancelled";
            eprintln!(
                "{} {}/{} · {} · {}",
                if sample.warmup { "Warmup" } else { "Sample" },
                round + 1,
                settings.repeat + settings.warmup,
                variant,
                sample.status
            );
            report::write_json(&private.join("result.json"), &sample)?;
            manifest["measured_build_profiles"][variant] = json!(sample.build_profile);
            report::write_json(&directory.join("manifest.json"), &manifest)?;
            samples.push(sample);
            report::write_json(&directory.join("results.json"), &samples)?;
            report::write_json(
                &directory.join("summary.json"),
                &statistics::summarize(&samples),
            )?;
            if cancelled {
                break;
            }
        }
        if cancelled {
            break;
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&statistics::summarize(&samples))?
    );
    println!("Results: {}", directory.display());
    ensure!(
        !cancelled
            && samples
                .iter()
                .filter(|s| !s.warmup)
                .all(|s| s.status == "completed"),
        "Some startup samples failed or were cancelled; inspect saved results"
    );
    Ok(directory)
}

async fn sample(
    settings: &Settings,
    worker: &Path,
    settings_path: &Path,
    prepared: &profile::Prepared,
    private: &Path,
    index: usize,
    variant: &str,
) -> Result<Sample> {
    let actual = settings.scenario == Scenario::ProcessCold;
    let executable = if actual {
        if variant == "baseline" {
            settings.baseline_bin.as_deref().unwrap()
        } else {
            settings.kyler_bin.as_deref().unwrap()
        }
    } else {
        worker
    };
    let mut command = tokio::process::Command::new(executable);
    if actual {
        command
            .arg("--config")
            .arg(&prepared.config_path)
            .arg("serve")
            .arg("--workspace")
            .arg(&prepared.workspace);
    } else {
        command
            .arg("__probe")
            .arg("--scenario")
            .arg(settings.scenario.argument())
            .arg("--mode")
            .arg(if settings.model_mode == ModelMode::Mock {
                "mock"
            } else {
                "live"
            })
            .arg("--private")
            .arg(private)
            .arg("--startup-settings")
            .arg(settings_path)
            .arg("--public-configuration")
            .arg(&prepared.public_path)
            .arg("--app-config")
            .arg(&prepared.config_path)
            .arg("--eval-config")
            .arg(&prepared.eval_path);
    }
    for key in profile::ISOLATION_OVERRIDES {
        command.env_remove(key);
    }
    if settings.model_mode == ModelMode::Mock {
        for key in profile::MODEL_OVERRIDES {
            command.env_remove(key);
        }
    }
    command
        .env(startup::TRACE_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let origin_ns = startup::monotonic_ns();
    let started = Instant::now();
    let mut child = command
        .spawn()
        .context("Could not start measurement subprocess")?;
    let pid = child.id().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut sample = Sample {
        index,
        variant: variant.into(),
        warmup: index < settings.warmup,
        status: "failed".into(),
        process_to_ready_ms: None,
        process_total_ms: 0.0,
        timeline_ms: BTreeMap::new(),
        build_profile: None,
        measurement: None,
        error: None,
    };
    let mut stopped = false;
    let mut resources = serde_json::Value::Null;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(settings.timeout_seconds);
    let read = async {
        while let Some(line) = lines.next_line().await? {
            let Some(json) = line.strip_prefix(startup::PREFIX) else {
                continue;
            };
            let event: StartupEvent =
                serde_json::from_str(json).context("Malformed startup event")?;
            if event.process_id != pid {
                continue;
            }
            let at = origin_ns
                .zip(event.monotonic_ns)
                .and_then(|(a, b)| b.checked_sub(a))
                .map(|n| n as f64 / 1e6)
                .unwrap_or_else(|| started.elapsed().as_secs_f64() * 1000.0);
            sample.timeline_ms.entry(event.event.clone()).or_insert(at);
            match event.event.as_str() {
                "main_entered" => {
                    sample.build_profile = event.data["build_profile"].as_str().map(str::to_owned);
                }
                "ready" => {
                    sample.process_to_ready_ms = Some(at);
                }
                "result" => {
                    sample.measurement =
                        Some(serde_json::from_value(event.data["measurement"].clone())?);
                }
                "error" => {
                    sample.error = Some(
                        event.data["message"]
                            .as_str()
                            .unwrap_or("Probe failed")
                            .into(),
                    );
                }
                "process_finished" => {
                    resources = event.data;
                }
                "service_listening" if actual && !stopped => {
                    let address = event.data["address"]
                        .as_str()
                        .context("Missing service address")?;
                    service::health(address).await?;
                    let ready = started.elapsed().as_secs_f64() * 1000.0;
                    sample.timeline_ms.insert("service_ready".into(), ready);
                    sample.process_to_ready_ms = Some(ready);
                    #[cfg(unix)]
                    {
                        ensure!(
                            unsafe { libc::kill(pid as i32, libc::SIGTERM) } == 0,
                            "Could not request graceful service shutdown"
                        );
                    }
                    #[cfg(not(unix))]
                    {
                        anyhow::bail!("Graceful process-cold shutdown requires Unix");
                    }
                    stopped = true;
                }
                _ => {}
            }
        }
        let status = child.wait().await?;
        ensure!(
            status.success(),
            "Measured subprocess exited unsuccessfully"
        );
        if actual {
            ensure!(
                sample.timeline_ms.contains_key("main_entered")
                    && sample.timeline_ms.contains_key("server_state_ready")
                    && sample.timeline_ms.contains_key("service_ready")
                    && resources.is_object(),
                "Executable lacks startup instrumentation or failed readiness/shutdown; use an instrumented Kyler build"
            );
            sample.measurement = Some(service::measurement(
                prepared,
                &sample.timeline_ms,
                &resources,
            )?);
        }
        ensure!(
            sample.measurement.is_some()
                && sample.process_to_ready_ms.is_some()
                && sample.error.is_none(),
            "Measurement did not complete successfully"
        );
        Ok::<_, anyhow::Error>(())
    };
    let outcome = tokio::select! {
        result=tokio::time::timeout_at(deadline,read)=>result.map_err(|_|anyhow::anyhow!("Startup measurement timed out")).and_then(|r|r),
        _=tokio::signal::ctrl_c()=>{sample.status="cancelled".into();Err(anyhow::anyhow!("Startup measurement cancelled"))},
    };
    match outcome {
        Ok(()) => sample.status = "completed".into(),
        Err(error) => {
            if error.to_string() == "Startup measurement timed out" {
                sample.status = "timed_out".into();
            }
            if sample.error.is_none() {
                sample.error = Some(error.to_string());
            }
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
    sample.process_total_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(sample)
}
