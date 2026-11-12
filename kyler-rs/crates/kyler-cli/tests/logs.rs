#![cfg(unix)]
use std::{io::Write, os::unix::process::CommandExt, path::Path, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, ChildStdout, Command},
};

struct Viewer {
    child: Child,
    output: BufReader<ChildStdout>,
}
impl Drop for Viewer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // Tests own this process group so failures cannot leak viewers.
            if let Some(pid) = self.child.id() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
        }
    }
}
impl Viewer {
    async fn start(home: &Path, config: Option<&Path>, web: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kyler"));
        command
            .env("HOME", home)
            .env_remove("KYLER_CONFIG")
            .env_remove("KYLER_INTERNAL_TOOL_WORKER")
            .env_remove("OPENAI_API_KEY")
            .env_remove("KYLER_LLM_API_KEY");
        if let Some(config) = config {
            command.arg("--config").arg(config);
        }
        command.arg("logs");
        if web {
            command.arg("web");
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        command.as_std_mut().process_group(0);
        let mut child = command.spawn().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self { child, output }
    }
    async fn line(&mut self) -> String {
        let mut text = String::new();
        let count = tokio::time::timeout(Duration::from_secs(15), self.output.read_line(&mut text))
            .await
            .unwrap()
            .unwrap();
        assert!(count > 0, "viewer closed before the expected log line");
        text
    }
    async fn stop(&mut self) {
        let pid = self.child.id().unwrap();
        // Exiting the viewer must close its output without touching the log writer.
        assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGINT) }, 0);
        let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success(), "{status}");
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), self.output.read_to_end(&mut rest))
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn backend_logs_follow_append_truncation_and_rotation_without_loading_config() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".kyler/logs");
    std::fs::create_dir_all(&directory).unwrap();
    let config = home.path().join(".kyler/config.toml");
    std::fs::write(&config, "invalid TOML [").unwrap();
    let log = directory.join("server.log");
    std::fs::write(
        &log,
        (0..120)
            .map(|i| format!("line{i:03}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let mut viewer = Viewer::start(home.path(), None, false).await;
    for i in 20..120 {
        assert_eq!(viewer.line().await, format!("line{i:03}\n"));
    }
    let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    writeln!(file, "live 中文").unwrap();
    assert_eq!(viewer.line().await, "live 中文\n");
    drop(file);
    std::fs::write(&log, "after truncation\n").unwrap();
    assert_eq!(viewer.line().await, "after truncation\n");
    std::fs::rename(&log, directory.join("server.log.1")).unwrap();
    std::fs::write(&log, "after rotation\n").unwrap();
    assert_eq!(viewer.line().await, "after rotation\n");
    viewer.stop().await;
    assert_eq!(std::fs::read_to_string(config).unwrap(), "invalid TOML [");
    assert!(!home.path().join(".kyler/server.json").exists());
}

#[tokio::test]
async fn web_logs_use_explicit_config_directory_and_leave_writer_running() {
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join("custom/logs");
    std::fs::create_dir_all(&directory).unwrap();
    let config = directory.parent().unwrap().join("absent.toml");
    let log = directory.join("web.log");
    let mut writer = std::fs::File::create(&log).unwrap();
    writeln!(writer, "web initial").unwrap();
    let mut viewer = Viewer::start(home.path(), Some(&config), true).await;
    assert_eq!(viewer.line().await, "web initial\n");
    writeln!(writer, "web live").unwrap();
    assert_eq!(viewer.line().await, "web live\n");
    viewer.stop().await;
    writeln!(writer, "writer still active").unwrap();
    assert!(
        std::fs::read_to_string(log)
            .unwrap()
            .ends_with("writer still active\n")
    );
    assert!(!config.exists());
}

#[tokio::test]
async fn missing_log_and_json_mode_fail_clearly_without_creating_files() {
    let home = tempfile::tempdir().unwrap();
    for arguments in [vec!["logs"], vec!["--json", "logs"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_kyler"))
            .args(&arguments)
            .env("HOME", home.path())
            .env_remove("KYLER_CONFIG")
            .env_remove("KYLER_INTERNAL_TOOL_WORKER")
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        if arguments[0] == "logs" {
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(error.contains("Log file not found:"));
            assert!(error.contains("logs/server.log"));
        } else {
            let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .contains("--json cannot be combined with logs")
            );
        }
    }
    assert!(!home.path().join(".kyler").exists());
}
