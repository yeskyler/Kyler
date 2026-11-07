use std::process::Command;

use kyler_logging::{LogFormat, LoggingConfig};

#[test]
fn subprocess_logger() {
    if std::env::var_os("KYLER_LOGGING_TEST_CHILD").is_none() {
        return;
    }
    kyler_logging::init(LoggingConfig {
        filter: "info".into(),
        format: LogFormat::Json,
        ansi: true,
    })
    .unwrap();
    tracing::info!(
        session_id = "test-session",
        tool = "read_file",
        "logging probe"
    );
    assert!(kyler_logging::init(LoggingConfig::default()).is_err());
}

#[test]
fn json_logging_uses_stderr_and_global_installation_is_explicit() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "subprocess_logger", "--nocapture"])
        .env("KYLER_LOGGING_TEST_CHILD", "1")
        .env("RUST_LOG", "info")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stderr.contains("\"session_id\":\"test-session\"") && stderr.contains("logging probe"));
    assert!(!stderr.contains('\u{1b}'));
    assert!(!stdout.contains("logging probe"));
}

#[test]
fn invalid_filter_is_an_error() {
    let config = LoggingConfig {
        filter: "info[".into(),
        ..Default::default()
    };
    assert!(config.validate().is_err());
    assert!(kyler_logging::dispatch(&config).is_err());
}
