//! Real CLI launch path, isolated from the user's service and model credentials.
use std::path::PathBuf;

use serde_json::Value;

use kyler_evals::startup::{self, ModelMode, Profile, Scenario, Settings};

#[tokio::test]
async fn actual_cli_cold_start_passes_health_and_alternates_baseline_candidate() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("app.toml");
    std::fs::write(&config, "[skills]\nenabled = false\n").unwrap();
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_kyler"));
    let settings = Settings {
        scenario: Scenario::ProcessCold,
        profile: Profile::Minimal,
        model_mode: ModelMode::Mock,
        repeat: 2,
        warmup: 1,
        timeout_seconds: 30,
        kyler_bin: Some(executable.clone()),
        baseline_bin: Some(executable.clone()),
        ..Default::default()
    };
    let directory = startup::run(
        settings,
        Some(config),
        root.path().join("results"),
        None,
        executable,
    )
    .await
    .unwrap();
    let samples: Vec<startup::Sample> =
        serde_json::from_slice(&std::fs::read(directory.join("results.json")).unwrap()).unwrap();
    assert_eq!(
        samples
            .iter()
            .map(|s| s.variant.as_str())
            .collect::<Vec<_>>(),
        vec![
            "candidate",
            "baseline",
            "baseline",
            "candidate",
            "candidate",
            "baseline"
        ]
    );
    for sample in &samples {
        assert_eq!(sample.status, "completed");
        assert!(sample.timeline_ms["main_entered"] <= sample.timeline_ms["configuration_ready"]);
        assert!(sample.timeline_ms["configuration_ready"] <= sample.timeline_ms["agent_ready"]);
        assert!(sample.timeline_ms["agent_ready"] <= sample.timeline_ms["service_ready"]);
        assert!(sample.timeline_ms["service_ready"] <= sample.timeline_ms["process_finished"]);
        assert!(sample.build_profile.is_some());
        assert!(!sample.measurement.as_ref().unwrap().capabilities_evaluated);
    }
    let summary: Value =
        serde_json::from_slice(&std::fs::read(directory.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["variants"]["candidate"]["samples"], 2);
    assert_eq!(summary["variants"]["baseline"]["samples"], 2);
    assert_eq!(summary["service_cold_ms"]["samples"], 2);
    assert_eq!(summary["capabilities_evaluated"], 0);
}

#[test]
fn normal_cli_does_not_emit_startup_trace() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_kyler"))
        .arg("--version")
        .env_remove(kyler_protocol::startup::TRACE_ENV)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains(kyler_protocol::startup::PREFIX)
    );
}
