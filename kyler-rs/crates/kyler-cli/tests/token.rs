use std::process::Command;

use serde_json::Value;

#[test]
fn local_token_lifecycle_requires_no_model_credentials_and_never_lists_secrets() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(&config, "[session]\ndirectory='sessions'\n").unwrap();
    let run = |arguments: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_kyler"))
            .args(["--config", config.to_str().unwrap(), "--json", "token"])
            .args(arguments)
            .env_remove("KYLER_CONFIG")
            .env_remove("KYLER_SESSION_DIR")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let created = run(&["create", "--name", "local-test"]);
    let id = created["info"]["id"].as_str().unwrap();
    let secret = created["token"].as_str().unwrap();
    assert!(secret.starts_with("kyl_"));
    let list = run(&["list"]);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(secret));
    let rotated = run(&["rotate", id]);
    assert_eq!(
        created["info"]["principal_id"],
        rotated["info"]["principal_id"]
    );
    assert_ne!(created["token"], rotated["token"]);
    run(&["revoke", rotated["info"]["id"].as_str().unwrap()]);
    assert!(
        run(&["list"])
            .as_array()
            .unwrap()
            .iter()
            .all(|token| !token["revoked_at"].is_null())
    );
    assert!(!std::fs::read_to_string(config).unwrap().contains(secret));
}

#[test]
fn token_management_rejects_remote_override_and_invalid_expiry() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(&config, "[session]\ndirectory='sessions'\n").unwrap();
    for arguments in [
        vec!["--server", "http://127.0.0.1:1", "token", "list"],
        vec![
            "token",
            "create",
            "--name",
            "test",
            "--expires-in-seconds",
            "0",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_kyler"))
            .args(["--config", config.to_str().unwrap()])
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("kyl_"));
    }
}
