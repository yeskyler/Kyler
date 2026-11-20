//! Local installation transaction. The shell bootstrap only downloads and unpacks bundles.
mod bundle;
mod dependencies;

use super::configure;
use crate::{CliError, Result};

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Output,
};

use tokio::process::Command;

const DEFAULT_CONFIG: &str = r#"[llm]
model = "deepseek-ai/DeepSeek-V4-Flash"
base_url = "https://api.siliconflow.cn/v1"
# Supply credentials during setup or through the process environment.
[llm.capabilities]
images = true
[sandbox]
mode = "workspace-write"
network_access = true
[workspace]
default_directory = "workspace"
[session]
enabled = true
directory = "sessions"
[context]
auto_model_window = true
window_ratio = 0.6
trigger_ratio = 0.8
[server]
bind = "127.0.0.1:30001"
[client]
server_url = "http://127.0.0.1:30001"
[web]
enabled = true
bind = "127.0.0.1:30002"
"#;
const LIFECYCLE_OVERRIDES: &[&str] = &[
    "KYLER_SERVER_BIND",
    "KYLER_SERVER_TOKEN",
    "KYLER_WEB_BIND",
    "KYLER_WEB_TOKEN",
    "KYLER_WEB_ENABLED",
    "KYLER_CLIENT_SERVER_URL",
    "KYLER_CLIENT_TOKEN",
    "KYLER_WEB_BUNDLE",
];
fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(CliError::io)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path).map_err(CliError::io)
    }
}
fn entry(path: &Path) -> Result<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            fs::read_link(path).map(Some).map_err(CliError::io)
        }
        Ok(_) => Err(CliError::new(
            2,
            format!("{} must be a version symlink.", path.display()),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CliError::io(e)),
    }
}
fn atomic_link(target: &Path, destination: &Path) -> Result<()> {
    let stage = tempfile::Builder::new()
        .prefix(".kyler-link-")
        .tempdir_in(destination.parent().unwrap())
        .map_err(CliError::io)?;
    let link = stage.path().join("entry");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, &link).map_err(CliError::io)?;
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        return Err(CliError::new(2, "Installation requires Unix symlinks."));
    }
    fs::rename(link, destination).map_err(CliError::io)
}
fn restore_link(path: &Path, previous: Option<&Path>) -> Result<()> {
    if let Some(previous) = previous {
        atomic_link(previous, path)
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CliError::io(e)),
        }
    }
}
fn contents(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !path.is_symlink() => Ok(None),
        Err(e) => Err(CliError::io(e)),
    }
}
fn restore_contents(path: &Path, previous: Option<&[u8]>) -> Result<()> {
    if let Some(bytes) = previous {
        configure::publish(path, bytes)
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CliError::io(e)),
        }
    }
}
fn command(binary: &Path, path: &std::ffi::OsStr) -> Command {
    let mut c = Command::new(binary);
    c.env("PATH", path)
        .env_remove("KYLER_INTERNAL_TOOL_WORKER")
        .env_remove("KYLER_STARTUP_TRACE")
        .kill_on_drop(true);
    c
}
fn service_command(binary: &Path, config: &Path, path: &std::ffi::OsStr) -> Command {
    let mut c = command(binary, path);
    c.arg("--config").arg(config);
    for name in LIFECYCLE_OVERRIDES {
        c.env_remove(name);
    }
    c
}
async fn checked_output(command: &mut Command, log: &mut File, failure: &str) -> Result<Output> {
    let result = command.output().await.map_err(CliError::io)?;
    log.write_all(&result.stdout).map_err(CliError::io)?;
    log.write_all(&result.stderr).map_err(CliError::io)?;
    if !result.status.success() {
        eprint!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return Err(CliError::new(3, failure));
    }
    Ok(result)
}
struct Transaction {
    binary: PathBuf,
    current: PathBuf,
    config: PathBuf,
    startup: PathBuf,
    old_config: Option<Vec<u8>>,
    old_startup: Option<Vec<u8>>,
    old_entry: Option<PathBuf>,
    old_current: Option<PathBuf>,
    old_binary: Option<tempfile::NamedTempFile>,
    old_running: bool,
    touched: bool,
    path: std::ffi::OsString,
}
impl Transaction {
    fn capture(home: &Path, config: PathBuf, path: std::ffi::OsString) -> Result<Self> {
        let binary = home.join(".local/bin/kyler");
        let current = home.join(".local/share/kyler/current");
        let startup = home.join(
            match std::env::var("SHELL")
                .unwrap_or_default()
                .rsplit('/')
                .next()
            {
                Some("zsh") => ".zshrc",
                Some("bash") => ".bashrc",
                _ => ".profile",
            },
        );
        let old_entry = if binary.is_symlink() {
            fs::read_link(&binary).map(Some).map_err(CliError::io)?
        } else {
            None
        };
        let old_binary = if binary.exists() && old_entry.is_none() {
            if !binary.is_file() {
                return Err(CliError::new(
                    2,
                    "Installed binary path is not a regular file.",
                ));
            }
            let backup = tempfile::NamedTempFile::new().map_err(CliError::io)?;
            fs::copy(&binary, backup.path()).map_err(CliError::io)?;
            Some(backup)
        } else {
            None
        };
        Ok(Self {
            old_config: contents(&config)?,
            old_startup: contents(&startup)?,
            old_current: entry(&current)?,
            binary,
            current,
            config,
            startup,
            old_entry,
            old_binary,
            old_running: false,
            touched: false,
            path,
        })
    }
    fn validate_unchanged(&self) -> Result<()> {
        let mut changed = contents(&self.config)? != self.old_config
            || contents(&self.startup)? != self.old_startup
            || entry(&self.current)? != self.old_current;
        if let Some(target) = &self.old_entry {
            changed |= fs::read_link(&self.binary).ok().as_ref() != Some(target);
        } else if let Some(backup) = &self.old_binary {
            changed |= self.binary.is_symlink()
                || !self.binary.is_file()
                || bundle::digest(&self.binary)? != bundle::digest(backup.path())?;
        } else {
            changed |= self.binary.exists() || self.binary.is_symlink();
        }
        if changed {
            return Err(CliError::new(
                3,
                "Installation or configuration changed during setup. Rerun the installer; no activation was performed.",
            ));
        }
        Ok(())
    }
    async fn lifecycle(&self, json: bool, args: &[&str]) -> Result<Output> {
        let mut command = service_command(&self.binary, &self.config, &self.path);
        if json {
            command.arg("--json");
        }
        let output = command
            .arg("service")
            .args(args)
            .output()
            .await
            .map_err(CliError::io)?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(CliError::new(
                3,
                if message.is_empty() {
                    format!("Service {} failed ({}).", args.join(" "), output.status)
                } else {
                    message
                },
            ));
        }
        Ok(output)
    }
    async fn stop_previous(&mut self) -> Result<()> {
        let directory = self.config.parent().unwrap();
        if self.binary.is_file()
            && (directory.join("server.json").exists() || directory.join("web.json").exists())
        {
            let status = self.lifecycle(true, &["status"]).await?;
            let status: serde_json::Value =
                serde_json::from_slice(&status.stdout).map_err(|_| {
                    CliError::new(
                        3,
                        "Cannot verify existing services; installation was not activated.",
                    )
                })?;
            self.old_running = status["running"] == true
                || status["backend"]["running"] == true
                || status["web"]["running"] == true;
            self.touched = true;
            self.lifecycle(false, &["stop"]).await?;
        }
        Ok(())
    }
    async fn rollback(&self) -> Result<()> {
        eprintln!("Installation failed; restoring the previous installation...");
        if self.binary.is_file() {
            self.lifecycle(false, &["stop"]).await.map_err(|e| CliError::new(3, format!("Cleanup is not confirmed: {e}. Inspect kyler service status and logs before retrying.")))?;
        }
        restore_link(&self.current, self.old_current.as_deref())?;
        if let Some(backup) = &self.old_binary {
            let stage = tempfile::NamedTempFile::new_in(self.binary.parent().unwrap())
                .map_err(CliError::io)?;
            fs::copy(backup.path(), stage.path()).map_err(CliError::io)?;
            stage
                .persist(&self.binary)
                .map_err(|e| CliError::io(e.error))?;
        } else {
            restore_link(&self.binary, self.old_entry.as_deref())?;
        }
        restore_contents(&self.config, self.old_config.as_deref())?;
        restore_contents(&self.startup, self.old_startup.as_deref())?;
        if self.old_running {
            self.lifecycle(false, &["start"]).await?;
        }
        eprintln!("Previous installation restored.");
        Ok(())
    }
}
pub async fn execute(
    explicit: Option<&Path>,
    source: &Path,
    no_start: bool,
    no_configure: bool,
    non_interactive: bool,
) -> Result<()> {
    let source = source.canonicalize().map_err(CliError::io)?;
    let manifest = bundle::verify(&source)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| CliError::new(2, "HOME must be an absolute directory."))?;
    let config = configure::config_path(explicit)?;
    if config.is_dir() {
        return Err(CliError::new(2, "Configuration path is a directory."));
    }
    let versions = home.join(".local/share/kyler/versions");
    private_directory(versions.parent().unwrap())?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(versions.parent().unwrap().join("install.lock"))
        .map_err(CliError::io)?;
    lock.try_lock()
        .map_err(|_| CliError::new(3, "Another installation is in progress."))?;
    let environment = dependencies::Environment::current(non_interactive)?;
    let mut transaction = Transaction::capture(&home, config, environment.path.clone())?;
    let log = tempfile::NamedTempFile::new().map_err(CliError::io)?;
    let mut diagnostics = log.reopen().map_err(CliError::io)?;
    println!(
        "\nKyler · Install\n\nInstall location: {}\nConfiguration: {}",
        home.join(".local/share/kyler").display(),
        transaction.config.display()
    );
    let result = install(
        &source,
        &manifest,
        &mut transaction,
        &versions,
        &mut diagnostics,
        no_start,
        no_configure,
        non_interactive,
        environment,
    )
    .await;
    if let Err(error) = &result {
        let _ = writeln!(diagnostics, "{error}");
    }
    // Only environment diagnostics are retained; never write wizard answers or service tokens.
    if !transaction.config.parent().unwrap().is_file() {
        let logs = transaction.config.parent().unwrap().join("logs");
        if private_directory(&logs).is_ok() {
            let _ = configure::publish(
                &logs.join("install.log"),
                &fs::read(log.path()).unwrap_or_default(),
            );
        }
    }
    result
}
async fn install(
    source: &Path,
    manifest: &str,
    tx: &mut Transaction,
    versions: &Path,
    log: &mut File,
    no_start: bool,
    no_configure: bool,
    non_interactive: bool,
    environment: dependencies::Environment,
) -> Result<()> {
    // Execute only the private verified copy, not a mutable download directory.
    private_directory(versions)?;
    let verified = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(versions)
        .map_err(CliError::io)?;
    bundle::copy(source, verified.path())?;
    if bundle::verify(verified.path())? != manifest {
        return Err(CliError::new(2, "Bundle changed during staging."));
    }
    let source = verified.path();
    println!("\n[1/5] Check environment and dependencies");
    let mut dependency_log = log.try_clone().map_err(CliError::io)?;
    tokio::task::spawn_blocking(move || environment.prepare(&mut dependency_log))
        .await
        .map_err(|e| CliError::new(3, e.to_string()))??;
    println!("\n[2/5] Check application and sandbox");
    let candidate_binary = source.join("bin/kyler");
    let version = checked_output(
        command(&candidate_binary, &tx.path).arg("--version"),
        log,
        "The new binary cannot run; installed version was kept.",
    )
    .await?;
    let version = String::from_utf8_lossy(&version.stdout)
        .trim()
        .strip_prefix("kyler ")
        .filter(|v| {
            v.starts_with(|c: char| c.is_ascii_digit())
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c))
        })
        .map(str::to_owned)
        .ok_or_else(|| CliError::new(2, "File did not report a valid kyler version."))?;
    checked_output(
        command(&source.join("runtime/node"), &tx.path).arg("--version"),
        log,
        "Bundled Node.js cannot run on this machine; previous installation was kept.",
    )
    .await?;
    println!("Checking native sandbox with the new binary as the current user...");
    checked_output(command(&candidate_binary, &tx.path).arg("doctor"), log, "Sandbox self-check failed. Previous installation and configuration were kept. Inspect install.log and run kyler doctor; no host security settings were changed.").await?;
    println!("Sandbox ready.");
    println!("\n[3/5] Configure installation");
    private_directory(tx.config.parent().unwrap())?;
    let seed = tempfile::Builder::new()
        .prefix(".config-seed-")
        .tempfile_in(tx.config.parent().unwrap())
        .map_err(CliError::io)?;
    let draft = tempfile::Builder::new()
        .prefix(".config-install-")
        .tempfile_in(tx.config.parent().unwrap())
        .map_err(CliError::io)?;
    let baseline = if tx.old_config.is_some() {
        println!("Preserved {}", tx.config.display());
        tx.config.as_path()
    } else {
        fs::write(seed.path(), DEFAULT_CONFIG).map_err(CliError::io)?;
        seed.path()
    };
    if !no_configure {
        if let Err(error) = configure::configure(
            Some(baseline),
            non_interactive,
            true,
            true,
            Some(draft.path()),
            false,
        )
        .await
        {
            if error.code == 130 {
                eprintln!("Setup cancelled; installed binary and configuration were kept.");
            }
            return Err(error);
        }
    } else {
        fs::write(
            draft.path(),
            tx.old_config
                .as_deref()
                .unwrap_or(DEFAULT_CONFIG.as_bytes()),
        )
        .map_err(CliError::io)?;
    }
    private_directory(versions)?;
    let destination = versions.join(format!("v{version}-{}", &manifest[..12]));
    if destination.exists() {
        if bundle::verify(&destination)? != manifest {
            return Err(CliError::new(
                2,
                "Installed bundle checksum differs from source.",
            ));
        }
    } else {
        fs::rename(verified.path(), &destination).map_err(CliError::io)?;
    }

    private_directory(tx.binary.parent().unwrap())?;
    let activation = async {
        println!("\n[4/5] Activate installation");
        tx.validate_unchanged()?;
        tx.stop_previous().await?;
        tx.touched = true;
        if no_configure {
            configure::publish(&tx.config, &fs::read(draft.path()).map_err(CliError::io)?)?;
        } else {
            configure::apply(Some(&tx.config), draft.path())?;
        }
        atomic_link(&destination, &tx.current)?;
        atomic_link(&tx.current.join("bin/kyler"), &tx.binary)?;
        const PATH_LINE: &str = "export PATH=\"$HOME/.local/bin:$PATH\"";
        let mut startup = tx.old_startup.clone().unwrap_or_default();
        if !String::from_utf8_lossy(&startup)
            .lines()
            .any(|line| line == PATH_LINE)
        {
            startup.extend_from_slice(format!("\n# Kyler\n{PATH_LINE}\n").as_bytes());
            configure::publish(&tx.startup, &startup)?;
        }
        println!(
            "Installed kyler {version}\nPATH ready for new terminals ({}).\nFor this terminal, run:\n  {PATH_LINE}",
            tx.startup.display()
        );
        println!("\n[5/5] Start and check services");
        if !no_start {
            println!("Starting services...");
            tx.lifecycle(true, &["start", "--restart"]).await.map_err(|e| CliError::new(3, format!("{e}\nServices could not start. Check kyler service status, ~/.kyler/logs/server.log and ~/.kyler/logs/web.log.")))?;
            println!("\nKyler · Service status and management\n");
            let status = tx.lifecycle(false, &["status"]).await?;
            print!("{}", String::from_utf8_lossy(&status.stdout));
            eprint!("{}", String::from_utf8_lossy(&status.stderr));
            match tx.lifecycle(false, &["token"]).await {
                Ok(token) => println!(
                    "\nWeb token: {}",
                    String::from_utf8_lossy(&token.stdout).trim()
                ),
                Err(_) => println!("Web token unavailable. Run kyler configure --web."),
            }
        } else {
            println!("\nKyler · Service status and management\n\nServices not started.");
        }
        Ok(())
    };
    #[cfg(unix)]
    let result = {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt()).map_err(CliError::io)?;
        let mut terminate = signal(SignalKind::terminate()).map_err(CliError::io)?;
        let mut hangup = signal(SignalKind::hangup()).map_err(CliError::io)?;
        tokio::select! {
            result = activation => result,
            _ = interrupt.recv() => Err(CliError::new(130, "Installation cancelled.")),
            _ = terminate.recv() => Err(CliError::new(143, "Installation terminated.")),
            _ = hangup.recv() => Err(CliError::new(129, "Installation interrupted.")),
        }
    };
    #[cfg(not(unix))]
    let result = activation.await;
    if let Err(error) = result {
        if tx.touched {
            tx.rollback().await.map_err(|recovery| {
                CliError::new(
                    3,
                    format!("{error}\nAutomatic recovery needs attention: {recovery}"),
                )
            })?;
        }
        return Err(error);
    }
    println!(
        "\nInstallation complete.\n\nConfiguration: {}\nInstall log: {}\n\nService management:\n  Status:   kyler service status\n  Stop:     kyler service stop\n  Start:    kyler service start\n  Restart:  kyler service start --restart\n  Token:    kyler service token\n  Diagnose: kyler doctor\n  Logs:     kyler logs backend\n  Web logs: kyler logs web\n\nRun in your project:\n  kyler tui",
        tx.config.display(),
        tx.config
            .parent()
            .unwrap()
            .join("logs/install.log")
            .display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_config_edits_prevent_activation_without_overwriting_them() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        fs::write(&config, "original").unwrap();
        let snapshot =
            Transaction::capture(home.path(), config.clone(), Default::default()).unwrap();
        snapshot.validate_unchanged().unwrap();
        fs::write(&config, "external edit").unwrap();
        assert!(snapshot.validate_unchanged().is_err());
        assert_eq!(fs::read_to_string(config).unwrap(), "external edit");
        assert!(!home.path().join(".local/bin/kyler").exists());
    }
}
