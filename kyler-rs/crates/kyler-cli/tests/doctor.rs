//! Native checks must work without configuration, credentials or a backend service.
#![cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;

#[test]
fn doctor_checks_native_isolation_without_reading_configuration() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("invalid.toml");
    std::fs::write(&config, "not valid TOML [").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kyler"))
        .args(["--json", "--config"])
        .arg(&config)
        .arg("doctor")
        .env("HOME", home.path())
        .env("TMPDIR", home.path())
        .env_remove("KYLER_INTERNAL_TOOL_WORKER")
        .env_remove("OPENAI_API_KEY")
        .env_remove("KYLER_LLM_API_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["type"], "sandbox_diagnostics");
    let checks = report["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 2);
    for (check, mode) in checks.iter().zip(["offline", "outbound"]) {
        assert_eq!(check["mode"], mode);
        for field in [
            "child_process",
            "workspace_read_write",
            "readonly_enforced",
            "denied_read_enforced",
            "outside_workspace_enforced",
            "host_loopback_blocked",
        ] {
            assert_eq!(check[field], true, "{mode}: {field}");
        }
    }
    assert_eq!(std::fs::read_to_string(config).unwrap(), "not valid TOML [");
    assert!(!home.path().join(".kyler").exists());
    assert_eq!(
        std::fs::read_dir(home.path()).unwrap().count(),
        1,
        "temporary sandbox fixtures must be removed"
    );
}
