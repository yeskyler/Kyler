//! Explicit local review, temporary state, and no model/server requests.
use std::process::Stdio;

use serde_json::Value;
use tokio::process::Command;

use kyler_core::memory::{MemoryStatus, MemoryStore};

#[tokio::test]
async fn learning_cli_reviews_exact_memory_revisions_without_credentials_or_server() {
    let root = tempfile::tempdir().unwrap();
    let scope = "a".repeat(64);
    let store = MemoryStore::new(root.path().join("memory"));
    store
        .propose(
            &scope,
            "lesson",
            "Review exact revisions",
            vec![],
            Default::default(),
            Some(0),
            "source",
            "execution",
            "call",
        )
        .unwrap();
    let invoke = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kyler"));
        command
            .args(["--json", "learning", "memory", "--store"])
            .arg(store.directory())
            .args(["--scope", &scope])
            .env("HOME", root.path())
            .stdin(Stdio::null());
        command
    };
    let listed = invoke().arg("list").output().await.unwrap();
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&listed.stdout).unwrap()[0]["status"],
        "candidate"
    );
    let stale = invoke()
        .args(["review", "lesson", "--revision", "0", "--status", "active"])
        .output()
        .await
        .unwrap();
    assert!(!stale.status.success());
    assert!(store.search(&scope, "").unwrap().is_empty());
    let approved = invoke()
        .args(["review", "lesson", "--revision", "1", "--status", "active"])
        .output()
        .await
        .unwrap();
    assert!(
        approved.status.success(),
        "{}",
        String::from_utf8_lossy(&approved.stderr)
    );
    assert_eq!(store.list(&scope).unwrap()[0].status, MemoryStatus::Active);
}

#[tokio::test]
async fn learning_cli_inspects_and_publishes_a_draft_without_model_credentials() {
    use kyler_protocol::api::skills::{SaveSkillRequest, SkillScope};
    let root = tempfile::tempdir().unwrap();
    let settings = kyler_config::SkillSettings {
        user_directory: root.path().join("skills"),
        state_file: root.path().join("state.json"),
        ..Default::default()
    };
    let config = root.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[skills]\nuser_directory = {:?}\nstate_file = {:?}\n",
            settings.user_directory.to_string_lossy(),
            settings.state_file.to_string_lossy()
        ),
    )
    .unwrap();
    let manager = kyler_core::skills::SkillManager::new(settings).unwrap();
    let draft = manager
        .save_draft(SaveSkillRequest {
            name: "workflow".into(),
            scope: SkillScope::User,
            files: [(
                "SKILL.md".into(),
                "---\nname: workflow\ndescription: A reviewed workflow\n---\nValidate the result"
                    .into(),
            )]
            .into(),
            expected_fingerprint: None,
        })
        .await
        .unwrap();
    let invoke = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kyler"));
        command
            .arg("--config")
            .arg(&config)
            .args(["--json", "learning", "drafts"])
            .env("HOME", root.path())
            .stdin(Stdio::null());
        command
    };
    let shown = invoke().args(["show", "workflow"]).output().await.unwrap();
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&shown.stdout).unwrap()["info"]["fingerprint"],
        draft.fingerprint
    );
    let stale = invoke()
        .args(["publish", "workflow", "--fingerprint", "stale"])
        .output()
        .await
        .unwrap();
    assert!(!stale.status.success());
    let published = invoke()
        .args(["publish", "workflow", "--fingerprint", &draft.fingerprint])
        .output()
        .await
        .unwrap();
    assert!(
        published.status.success(),
        "{}",
        String::from_utf8_lossy(&published.stderr)
    );
    assert!(
        manager
            .settings
            .user_directory
            .join("workflow/SKILL.md")
            .exists()
    );
}
