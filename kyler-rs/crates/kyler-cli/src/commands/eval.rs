//! Task-level evaluations against the configured server; never runs during installation.
use crate::{CliError, Result};

use std::{
    path::Path,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use kyler_client::{Client, ClientEvent};
use kyler_protocol::api::{
    event::EventPayload,
    run::{CreateRunRequest, RunStatus},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    input: String,
    #[serde(default)]
    answer_contains: Vec<String>,
    #[serde(default)]
    expected_tools: Vec<String>,
    max_tokens: Option<u32>,
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
}
fn default_timeout() -> u64 {
    120
}
#[derive(Debug, Serialize)]
struct CaseResult {
    name: String,
    passed: bool,
    run_id: String,
    tools: Vec<String>,
    total_tokens: u32,
    elapsed_ms: u128,
    failures: Vec<String>,
}
#[derive(Debug, Serialize)]
struct Report {
    passed: usize,
    total: usize,
    cases: Vec<CaseResult>,
}
pub async fn run(client: &Client, suite: &Path, report: &Path) -> Result<()> {
    let bytes = std::fs::read(suite).map_err(CliError::io)?;
    if bytes.len() > 1024 * 1024 {
        return Err(CliError::new(2, "Evaluation suite exceeds 1 MiB"));
    }
    let cases: Vec<Case> =
        serde_json::from_slice(&bytes).map_err(|error| CliError::new(2, error.to_string()))?;
    if cases.is_empty() || cases.len() > 100 {
        return Err(CliError::new(2, "Suite must contain 1 to 100 cases"));
    }
    let mut results = Vec::new();
    for case in cases {
        let session = client
            .create_session_in(&std::env::current_dir().map_err(CliError::io)?)
            .await?;
        let start = Instant::now();
        let accepted = client
            .start_run(
                &session.session_id,
                &CreateRunRequest {
                    input: case.input.clone(),
                    images: Vec::new(),
                    attachment_ids: Vec::new(),
                    resume: false,
                    client_request_id: Some(uuid::Uuid::new_v4().to_string()),
                },
            )
            .await?;
        let mut stream = client.watch(&accepted.run_id, 0).await?;
        let mut tools = Vec::new();
        let outcome = tokio::time::timeout(
            Duration::from_secs(case.timeout_seconds.clamp(1, 3600)),
            async {
                while let Some(event) = stream.next().await? {
                    match event {
                        ClientEvent::Server(event) => {
                            if let EventPayload::ToolStarted { name, .. } = event.payload {
                                tools.push(name);
                            }
                        }
                        ClientEvent::Result(info) => {
                            return Ok::<_, kyler_client::Error>(Some(info));
                        }
                        _ => {}
                    }
                }
                Ok(None)
            },
        )
        .await;
        let mut failures = Vec::new();
        let mut total_tokens = 0;
        match outcome {
            Ok(Ok(Some(info))) => {
                if info.status != RunStatus::Completed {
                    failures.push(format!("Run ended as {:?}", info.status));
                }
                let answer = info.output.unwrap_or_default();
                for expected in &case.answer_contains {
                    if !answer.contains(expected) {
                        failures.push(format!("Missing answer fragment: {expected}"));
                    }
                }
                total_tokens = info.usage.map(|usage| usage.total_tokens).unwrap_or(0);
                if case.max_tokens.is_some_and(|limit| total_tokens > limit) {
                    failures.push("Token budget exceeded".into());
                }
            }
            _ => {
                let _ = client.cancel(&accepted.run_id).await;
                failures.push("Evaluation timed out or lost its terminal result".into());
            }
        }
        for tool in &case.expected_tools {
            if !tools.contains(tool) {
                failures.push(format!("Missing expected tool: {tool}"));
            }
        }
        let result = CaseResult {
            name: case.name,
            passed: failures.is_empty(),
            run_id: accepted.run_id,
            tools,
            total_tokens,
            elapsed_ms: start.elapsed().as_millis(),
            failures,
        };
        eprintln!(
            "{} {}",
            if result.passed { "PASS" } else { "FAIL" },
            result.name
        );
        results.push(result);
    }
    let report_data = Report {
        passed: results.iter().filter(|case| case.passed).count(),
        total: results.len(),
        cases: results,
    };
    // Preserve the previous report if writing or serialization fails.
    let parent = report
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(CliError::io)?;
    serde_json::to_writer_pretty(&mut temp, &report_data)
        .map_err(|error| CliError::new(3, error.to_string()))?;
    temp.persist(report)
        .map_err(|error| CliError::io(error.error))?;
    if report_data.passed != report_data.total {
        return Err(CliError::new(
            1,
            "Some evaluation cases failed; inspect the report",
        ));
    }
    Ok(())
}
