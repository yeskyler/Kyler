//! Identity-checked lifecycle for the bundled Next.js service.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::Instant;

use kyler_config::AppConfig;

use super::{StartingChild, failure, io, private_directory, private_file, process_alive};
use crate::{CliError, Result};

#[derive(Serialize, Deserialize)]
struct Record {
    pid: u32,
    instance_id: String,
    secret: String,
    url: String,
    backend_url: String,
    bundle: PathBuf,
}

pub(super) fn check_bundle() -> Result<PathBuf> {
    let root = if let Some(root) = std::env::var_os("KYLER_WEB_BUNDLE") {
        PathBuf::from(root)
    } else {
        let binary = io(io(std::env::current_exe())?.canonicalize())?;
        binary
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| failure("Cannot locate installation bundle"))?
            .to_path_buf()
    };
    let root = io(root.canonicalize())?;
    if !root.join("runtime/node").is_file() || !root.join("web/server.js").is_file() {
        return Err(failure(
            "Web bundle missing. Install the complete Kyler bundle or disable [web].enabled.",
        ));
    }
    Ok(root)
}
fn read(directory: &Path) -> Result<Option<Record>> {
    match fs::read(directory.join("web.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| failure("Invalid Web service record")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CliError::io(e)),
    }
}
async fn probe(record: &Record) -> Result<bool> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|_| failure("Cannot initialize Web management client"))?;
    let response = match client
        .get(format!("{}/api/service/identity", record.url))
        .bearer_auth(&record.secret)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) if error.is_connect() => return Ok(false),
        Err(_) => {
            return Err(failure(
                "Web service could not be verified; inspect ~/.kyler/logs/web.log.",
            ));
        }
    };
    if !response.status().is_success() {
        return Err(failure(
            "Web service identity could not be verified. No unrelated process was stopped.",
        ));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|_| failure("Invalid Web identity response"))?;
    if body["instance_id"] != record.instance_id || body["pid"].as_u64() != Some(record.pid as u64)
    {
        return Err(failure(
            "Web service identity changed. No unrelated process was stopped.",
        ));
    }
    Ok(true)
}
pub(super) async fn status(directory: &Path) -> Result<Value> {
    match read(directory)? {
        Some(record) => {
            Ok(json!({"running":probe(&record).await?,"pid":record.pid,"url":record.url}))
        }
        None => Ok(json!({"running":false})),
    }
}
pub(super) async fn stop(directory: &Path) -> Result<()> {
    let Some(record) = read(directory)? else {
        return Ok(());
    };
    if probe(&record).await? {
        #[cfg(unix)]
        if unsafe { libc::kill(record.pid as i32, libc::SIGTERM) } != 0 {
            return Err(failure("Cannot stop the verified Web service"));
        }
        #[cfg(not(unix))]
        return Err(failure(
            "Managed Web service currently requires macOS or Linux",
        ));
        let deadline = Instant::now() + Duration::from_secs(40);
        while process_alive(record.pid) {
            if Instant::now() >= deadline {
                return Err(failure(
                    "Web shutdown is not confirmed; inspect its log before retrying.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    } else if process_alive(record.pid) {
        return Err(failure(
            "Recorded Web process is alive but unreachable; no unverified process was stopped.",
        ));
    }
    io(fs::remove_file(directory.join("web.json")))?;
    Ok(())
}
pub(super) async fn start(path: &Path, config: &AppConfig, directory: &Path) -> Result<()> {
    let bundle = check_bundle()?;
    let url = kyler_config::web::connect_url(config.web.bind);
    if let Some(record) = read(directory)? {
        if probe(&record).await? {
            if record.url == url
                && record.backend_url == config.client.server_url
                && record.bundle == bundle
            {
                eprintln!("Web already running · {}", record.url);
                return Ok(());
            }
            return Err(failure(
                "Web configuration changed; run kyler service start --restart.",
            ));
        }
        if process_alive(record.pid) {
            return Err(failure(
                "Recorded Web process is alive but unreachable; inspect its log.",
            ));
        }
        io(fs::remove_file(directory.join("web.json")))?;
    }
    // A bind check distinguishes occupied ports from a service that has not started yet.
    let reservation = std::net::TcpListener::bind(config.web.bind).map_err(|_| {
        failure(format!(
            "Web address {} is unavailable. Choose another address or stop its owner.",
            config.web.bind
        ))
    })?;
    drop(reservation);
    let logs = directory.join("logs");
    private_directory(&logs)?;
    let log = private_file(&logs.join("web.log"))?;
    let instance = uuid::Uuid::new_v4().to_string();
    let secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut command = Command::new("/usr/bin/nohup");
    command
        .arg(bundle.join("runtime/node"))
        .arg(bundle.join("web/server.js"))
        .current_dir(bundle.join("web"))
        .env("NODE_ENV", "production")
        .env("HOSTNAME", config.web.bind.ip().to_string())
        .env("PORT", config.web.bind.port().to_string())
        .env("KYLER_CONFIG", path)
        .env("KYLER_WEB_BIND", config.web.bind.to_string())
        .env("KYLER_WEB_TOKEN", config.web.access_token.expose())
        .env("KYLER_WEB_INSTANCE_ID", &instance)
        .env("KYLER_WEB_SERVICE_SECRET", &secret)
        .env("KYLER_CLIENT_SERVER_URL", &config.client.server_url)
        .env(
            "KYLER_CLIENT_TOKEN",
            if config.client.access_token.is_empty()
                && kyler_config::web::is_local_backend(
                    config.server.bind,
                    &config.client.server_url,
                )
            {
                config.server.access_token.expose()
            } else {
                config.client.access_token.expose()
            },
        )
        .env_remove("KYLER_INTERNAL_TOOL_WORKER")
        .env_remove("NODE_OPTIONS")
        .env_remove("KYLER_SERVER_TOKEN")
        .env_remove("KYLER_LLM_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::from(io(log.try_clone())?))
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut starting = StartingChild(Some(io(command.spawn())?));
    let child = starting.0.as_mut().unwrap();
    let record = Record {
        pid: child.id(),
        instance_id: instance,
        secret,
        url,
        backend_url: config.client.server_url.clone(),
        bundle,
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if io(child.try_wait())?.is_some() {
            return Err(failure(
                "Web startup failed; inspect ~/.kyler/logs/web.log.",
            ));
        }
        if probe(&record).await? {
            break;
        }
        if Instant::now() >= deadline {
            return Err(failure(
                "Web startup timed out; inspect ~/.kyler/logs/web.log.",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut temporary = io(tempfile::NamedTempFile::new_in(directory))?;
    io(temporary.write_all(
        &serde_json::to_vec(&record).map_err(|_| failure("Cannot encode Web record"))?,
    ))?;
    io(temporary.as_file().sync_all())?;
    temporary
        .persist(directory.join("web.json"))
        .map_err(|e| CliError::io(e.error))?;
    starting.0.take();
    eprintln!("Web started · {}", record.url);
    Ok(())
}
