mod web;

use super::configure::config_path;
use crate::{CliError, Result, args::ServiceCommand, output};

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    net::{Ipv4Addr, Ipv6Addr},
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use kyler_client::{Client, ErrorKind};
use kyler_config::{AppConfig, ClientSettings, loader};

#[derive(Serialize, Deserialize)]
struct Record {
    pid: u32,
    instance_id: String,
    url: String,
}
struct StartingChild(Option<Child>);
impl Drop for StartingChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn failure(message: impl Into<String>) -> CliError {
    CliError::new(3, message)
}
fn io<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(CliError::io)
}
fn client(config: &AppConfig, url: String) -> Result<Client> {
    Ok(Client::new(ClientSettings {
        server_url: url,
        access_token: config.server.access_token.clone(),
        connect_timeout_seconds: 2,
        request_timeout_seconds: 3,
        ..Default::default()
    })?)
}
fn server_url(config: &AppConfig) -> Result<String> {
    let mut address = config.server.bind;
    if address.port() == 0 {
        return Err(CliError::new(
            2,
            "Background service requires a fixed server port.",
        ));
    }
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            Ipv4Addr::LOCALHOST.into()
        } else {
            Ipv6Addr::LOCALHOST.into()
        });
    }
    Ok(format!("http://{address}"))
}
fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;

            io(fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path))?;
        }
        #[cfg(not(unix))]
        {
            io(fs::create_dir_all(path))?;
        }
    }
    Ok(())
}
fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    io(options.open(path))
}
async fn backend_execute(
    explicit: Option<&Path>,
    command: &ServiceCommand,
    json: bool,
) -> Result<()> {
    let path = config_path(explicit)?;
    let directory = path
        .parent()
        .ok_or_else(|| failure("Invalid configuration path"))?;
    private_directory(directory)?;
    let lock = private_file(&directory.join("service.lock"))?;
    lock.try_lock()
        .map_err(|_| failure("Another service operation is in progress."))?;
    // Status/stop must remain available even if model credentials need editing.
    let mut config =
        loader::load_for_setup(Some(&path)).map_err(|e| CliError::new(2, e.to_string()))?;
    let manifest = directory.join("server.json");
    let record: Option<Record> = match fs::read(&manifest) {
        Ok(bytes) => Some(
            serde_json::from_slice(&bytes)
                .map_err(|_| failure("Invalid service record; inspect ~/.kyler/server.json."))?,
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(CliError::io(e)),
    };
    match command {
        ServiceCommand::Status => {
            if let Some(record) = &record {
                let state = probe(&config, record).await?;
                if json {
                    output::json(
                        &serde_json::json!({"running":state,"pid":record.pid,"url":record.url}),
                    )?;
                } else {
                    eprintln!(
                        "Service: {} · PID {} · {}",
                        if state { "running" } else { "stopped" },
                        record.pid,
                        record.url
                    );
                }
            } else if json {
                output::json(&serde_json::json!({"running":false}))?;
            } else {
                eprintln!("Service: stopped (no managed service)");
            }
        }
        ServiceCommand::Stop => {
            if let Some(record) = &record {
                stop(&config, record).await?;
                io(fs::remove_file(&manifest))?;
            }
            if json {
                output::json(&serde_json::json!({"running":false}))?;
            } else {
                eprintln!("Service: stopped");
            }
        }
        ServiceCommand::Start { restart } => {
            // Managed services use the model saved by configure, including on later restarts.
            // A stale shell value must not undo the user's installation answers.
            let saved = loader::parse(&io(fs::read_to_string(&path))?)
                .map_err(|e| CliError::new(2, e.to_string()))?;
            if !saved.llm.model.is_empty()
                && !saved.llm.base_url.is_empty()
                && !saved.llm.api_key.is_empty()
            {
                config.llm = saved.llm;
            }
            config
                .validate()
                .map_err(|e| CliError::new(2, format!("{e}. Run kyler configure first.")))?;
            if !config.session.enabled {
                return Err(CliError::new(
                    2,
                    "session.enabled must be true; run kyler configure.",
                ));
            }
            let url = server_url(&config)?;
            if let Some(record) = &record {
                if probe(&config, record).await? {
                    if !restart {
                        if record.url != url {
                            return Err(failure(
                                "Backend address changed; run kyler service start --restart.",
                            ));
                        }
                        if json {
                            output::json(record)?;
                        } else {
                            eprintln!("Service already running · PID {}", record.pid);
                        }
                        return Ok(());
                    }
                    stop(&config, record).await?;
                }
                if process_alive(record.pid) {
                    return Err(failure(
                        "Recorded service process is still alive but not reachable; inspect its log before restarting.",
                    ));
                }
                io(fs::remove_file(&manifest))?;
            }
            // Never adopt or stop an unrelated foreground server on this port.
            match client(&config, url.clone())?.info().await {
                Ok(_) => {
                    return Err(failure(
                        "A server already uses the configured address. Stop that server before starting the managed service.",
                    ));
                }
                Err(error) if error.kind == ErrorKind::Transport => {}
                Err(_) => {
                    return Err(failure(
                        "The configured server address is occupied or cannot be verified.",
                    ));
                }
            }
            let logs = directory.join("logs");
            private_directory(&logs)?;
            let log_path = logs.join("server.log");
            let log = private_file(&log_path)?;
            let instance_id = uuid::Uuid::new_v4().to_string();
            let mut process = Command::new("/usr/bin/nohup");
            process
                .arg(io(std::env::current_exe())?)
                .arg("--config")
                .arg(&path)
                .arg("serve")
                .arg("--instance-id")
                .arg(&instance_id)
                .current_dir(directory)
                .env_remove("KYLER_INTERNAL_TOOL_WORKER")
                .env("KYLER_LLM_MODEL", &config.llm.model)
                .env("KYLER_LLM_BASE_URL", &config.llm.base_url)
                .env("KYLER_LLM_API_KEY", config.llm.api_key.expose())
                .stdin(Stdio::null())
                .stdout(Stdio::from(io(log.try_clone())?))
                .stderr(Stdio::from(log));
            if !config.server.bind.ip().is_loopback() {
                io(fs::create_dir_all(&config.workspace.default_directory))?;
                process
                    .arg("--workspace")
                    .arg(&config.workspace.default_directory);
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;

                process.process_group(0);
            }
            #[cfg(not(unix))]
            {
                return Err(failure(
                    "Background service currently supports macOS and Linux.",
                ));
            }
            let mut starting = StartingChild(Some(io(process.spawn())?));
            let child = starting.0.as_mut().unwrap();
            let record = Record {
                pid: child.id(),
                instance_id,
                url: url.clone(),
            };
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(status) = io(child.try_wait())? {
                    return Err(failure(format!(
                        "Service startup failed ({status}). Log: {}",
                        log_path.display()
                    )));
                }
                if probe(&config, &record).await? {
                    break;
                }
                if Instant::now() >= deadline {
                    // This handle owns exactly the newly spawned child, never a PID read from disk.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(failure(format!(
                        "Service startup timed out. Log: {}",
                        log_path.display()
                    )));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let mut temporary = io(tempfile::NamedTempFile::new_in(directory))?;
            let encoded =
                serde_json::to_vec(&record).map_err(|_| failure("Cannot encode service record"))?;
            if let Err(error) = temporary
                .write_all(&encoded)
                .and_then(|_| temporary.as_file().sync_all())
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CliError::io(error));
            }
            if let Err(error) = temporary.persist(&manifest) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CliError::io(error.error));
            }
            let _ = starting.0.take(); // Successful readiness and persistence transfer ownership to the user service.
            if json {
                output::json(&record)?;
            } else {
                eprintln!(
                    "Service started · PID {} · {}\nLog: {}",
                    record.pid,
                    record.url,
                    log_path.display()
                );
            }
        }
    }
    Ok(())
}
async fn probe(config: &AppConfig, record: &Record) -> Result<bool> {
    match client(config, record.url.clone())?.info().await {
        Ok(info) if info.instance_id.as_deref() == Some(record.instance_id.as_str()) => Ok(true),
        Ok(_) => Err(failure(
            "Service identity changed. No other process was stopped; inspect the configured address.",
        )),
        Err(error) if error.kind == ErrorKind::Transport => Ok(false),
        Err(_) => Err(failure(
            "Cannot verify the managed service. Check the server access token and service log.",
        )),
    }
}
async fn stop(config: &AppConfig, record: &Record) -> Result<()> {
    if !probe(config, record).await? {
        if process_alive(record.pid) {
            return Err(failure(
                "The recorded service process is alive but not reachable; shutdown was not confirmed.",
            ));
        }
        return Ok(());
    }
    client(config, record.url.clone())?
        .shutdown_instance(&record.instance_id)
        .await?;
    let deadline = Instant::now() + Duration::from_secs(config.server.shutdown_grace_seconds + 5);
    loop {
        match client(config, record.url.clone())?.info().await {
            Ok(info) if info.instance_id.as_deref() != Some(record.instance_id.as_str()) => {
                return Err(failure(
                    "Service identity changed while stopping; no other process was stopped.",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind == ErrorKind::Transport => {
                if !process_alive(record.pid) {
                    return Ok(());
                }
            }
            Err(error) if error.status == Some(503) => {}
            Err(_) => {
                return Err(failure(
                    "Cannot confirm service shutdown; check authentication and the log.",
                ));
            }
        }
        if Instant::now() >= deadline {
            return Err(failure(
                "Service shutdown has not been confirmed; no process was force-killed.",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn process_alive(pid: u32) -> bool {
    Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .ok()
        .is_some_and(|out| {
            out.status.success()
                && String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .chars()
                    .next()
                    .is_some_and(|c| c != 'Z')
        })
}

/// One orchestration lock covers both services; existing backend identity checks remain intact.
pub async fn execute(explicit: Option<&Path>, command: &ServiceCommand, json: bool) -> Result<()> {
    let path = config_path(explicit)?;
    let directory = path
        .parent()
        .ok_or_else(|| failure("Invalid configuration path"))?;
    private_directory(directory)?;
    let lock = private_file(&directory.join("services.lock"))?;
    lock.try_lock()
        .map_err(|_| failure("Another service operation is in progress."))?;
    let config =
        loader::load_for_setup(Some(&path)).map_err(|e| CliError::new(2, e.to_string()))?;
    match command {
        ServiceCommand::Start { restart } => {
            if config.web.enabled {
                web::check_bundle()?;
                config
                    .web
                    .validate(config.server.bind)
                    .map_err(|e| CliError::new(2, e.to_string()))?;
            }
            if *restart {
                let (front, back) = tokio::join!(
                    web::stop(directory),
                    backend_execute(Some(&path), &ServiceCommand::Stop, false)
                );
                front?;
                back?;
            }
            backend_execute(Some(&path), command, false).await?;
            if config.web.enabled {
                web::start(&path, &config, directory).await?;
            } else {
                web::stop(directory).await?;
            }
            if json {
                let backend = backend_status(&config, directory).await?;
                let web = web::status(directory).await?;
                output::json(
                    &serde_json::json!({"running":backend["running"],"backend":backend,"web":web}),
                )?;
            }
        }
        ServiceCommand::Stop => {
            // Signal Web first, then shut down the backend concurrently so active SSE streams can drain.
            let (front, back) = tokio::join!(
                web::stop(directory),
                backend_execute(Some(&path), command, false)
            );
            front?;
            back?;
            if json {
                output::json(
                    &serde_json::json!({"running":false,"backend":{"running":false},"web":{"running":false}}),
                )?;
            }
        }
        ServiceCommand::Status => {
            let backend = backend_status(&config, directory).await?;
            let web = web::status(directory).await?;
            let running =
                backend["running"] == true && (!config.web.enabled || web["running"] == true);
            if json {
                output::json(
                    &serde_json::json!({"running":running,"pid":backend["pid"],"url":backend["url"],"backend":backend,"web":web}),
                )?;
            } else {
                eprintln!(
                    "Backend: {} · {}",
                    if backend["running"] == true {
                        "running"
                    } else {
                        "stopped"
                    },
                    backend["url"].as_str().unwrap_or("not configured")
                );
                eprintln!(
                    "Web: {} · {}",
                    if web["running"] == true {
                        "running"
                    } else if config.web.enabled {
                        "stopped"
                    } else {
                        "disabled"
                    },
                    web["url"].as_str().unwrap_or("not configured")
                );
            }
        }
    }
    Ok(())
}
async fn backend_status(config: &AppConfig, directory: &Path) -> Result<serde_json::Value> {
    let path = directory.join("server.json");
    if !path.is_file() {
        return Ok(serde_json::json!({"running":false}));
    }
    let record: Record = serde_json::from_slice(&io(fs::read(path))?)
        .map_err(|_| failure("Invalid backend service record"))?;
    Ok(
        serde_json::json!({"running":probe(config,&record).await?,"pid":record.pid,"url":record.url}),
    )
}
