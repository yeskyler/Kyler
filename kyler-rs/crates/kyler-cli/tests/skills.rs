//! A real client directory is uploaded to an independent service-owned user directory.
use std::{path::Path, process::Output};

use serde_json::Value;
use tokio::{net::TcpListener, process::Command};

use kyler_config::{AppConfig, SecretString, SkillSettings};
use kyler_server::Server;

async fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kyler"))
        .arg("--config")
        .arg(root.join("client.toml"))
        .arg("--json")
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("HOME", root)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .await
        .unwrap()
}
#[tokio::test]
async fn skills_install_list_show_remove_need_no_session_or_model_request() {
    let root = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.llm.api_key = SecretString::new("unused-model-key");
    config.llm.base_url = "http://127.0.0.1:1/v1".into();
    config.llm.model = "unused-model".into();
    config.session.enabled = true;
    config.session.directory = root.path().join("server-sessions");
    config.skills = SkillSettings {
        user_directory: root.path().join("server-skills"),
        state_file: root.path().join("skills-state.json"),
        ..Default::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = Server::from_config(&config).await.unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = stopped.await;
    }));
    std::fs::write(
        root.path().join("client.toml"),
        format!("[client]\nserver_url='{url}'\n"),
    )
    .unwrap();
    std::fs::create_dir(root.path().join("input")).unwrap();
    std::fs::write(
        root.path().join("input/SKILL.md"),
        "---\nname: cli-fixture\ndescription: CLI fixture skill\n---\nHelp with local tasks.",
    )
    .unwrap();
    let installed = cli(root.path(), &["skills", "install", "./input"]).await;
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let info: Value = serde_json::from_slice(&installed.stdout).unwrap();
    assert_eq!(info["source"], "user");
    assert!(
        root.path()
            .join("server-skills/cli-fixture/SKILL.md")
            .is_file()
    );
    assert!(root.path().join("input/SKILL.md").is_file());
    let duplicate = cli(root.path(), &["skills", "install", "./input"]).await;
    assert!(!duplicate.status.success());
    let list = cli(root.path(), &["skills", "list"]).await;
    assert!(list.status.success());
    let listed: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|skill| skill["name"] == "cli-fixture")
    );
    let details = cli(root.path(), &["skills", "show", "cli-fixture"]).await;
    assert!(details.status.success());
    assert!(String::from_utf8_lossy(&details.stdout).contains("Help with local tasks"));
    assert!(
        !cli(root.path(), &["skills", "remove", "skill-installer"])
            .await
            .status
            .success()
    );
    assert!(
        cli(root.path(), &["skills", "remove", "cli-fixture"])
            .await
            .status
            .success()
    );
    assert!(!root.path().join("server-skills/cli-fixture").exists());
    assert!(
        !cli(root.path(), &["skills", "show", "cli-fixture"])
            .await
            .status
            .success()
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
