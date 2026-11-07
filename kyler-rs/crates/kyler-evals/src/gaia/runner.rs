use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{sync::watch, task::JoinSet};

use kyler_config::AppConfig;
use kyler_core::{AgentStreamEvent, Session, context::ToolResultStatus, runtime::UserTurn};

use super::{
    INSTRUCTIONS, answer,
    dataset::{Dataset, Task, digest_file},
    scorer,
};
use crate::{
    config::EvalConfig,
    report::{self, TaskResult},
    runtime::{Runtime, Timings},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub kind: String,
    pub schema_version: u32,
    pub scorer_version: String,
    pub dataset_fingerprint: String,
    pub attachment_fingerprints: BTreeMap<String, String>,
    pub task_ids: Vec<String>,
    pub split: String,
    pub eval: EvalConfig,
    pub configuration: Value,
    pub configuration_fingerprint: String,
    pub revision: String,
    pub platform: String,
    pub executable_fingerprint: String,
    pub evaluation_prompt_fingerprint: String,
}
fn revision() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".into())
}
pub fn check(eval: &EvalConfig, level: Option<u8>, limit: Option<usize>) -> Result<Value> {
    let dataset = Dataset::load(&eval.dataset.directory, &eval.dataset.split)?;
    let tasks = dataset.select(level, limit)?;
    let mut attachments = 0;
    let mut binary_media = 0;
    for task in &tasks {
        if let Some(path) = dataset.attachment(task)? {
            attachments += 1;
            if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("mp3" | "mp4" | "wav" | "zip")
            ) {
                binary_media += 1;
            }
        }
    }
    Ok(
        json!({"split":eval.dataset.split,"tasks":tasks.len(),"split_total":dataset.tasks.len(),"attachments":attachments,"binary_media_or_archives":binary_media,"dataset_fingerprint":dataset.fingerprint,"mcp_enabled":eval.agent.enable_mcp,"note":"No built-in web search. Configure trusted search/browsing MCP explicitly. Media/archive inputs are copied to the workspace when the attachment API cannot ingest them. This data check does not establish provider or connector readiness."}),
    )
}
fn manifest(
    eval: EvalConfig,
    app: &AppConfig,
    dataset: &Dataset,
    indices: &[usize],
    worker: &Path,
) -> Result<Manifest> {
    let configuration = crate::runtime::public_configuration(app, &eval)?;
    let mut attachments = BTreeMap::new();
    for &index in indices {
        let task = &dataset.tasks[index];
        if let Some(path) = dataset.attachment(task)? {
            attachments.insert(task.task_id.clone(), digest_file(&path)?);
        }
    }
    Ok(Manifest {
        kind: "gaia".into(),
        schema_version: 1,
        scorer_version: scorer::VERSION.into(),
        dataset_fingerprint: dataset.fingerprint.clone(),
        attachment_fingerprints: attachments,
        task_ids: indices
            .iter()
            .map(|&i| dataset.tasks[i].task_id.clone())
            .collect(),
        split: eval.dataset.split.clone(),
        configuration_fingerprint: report::fingerprint(&configuration)?,
        configuration,
        eval,
        revision: revision(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        executable_fingerprint: digest_file(worker)?,
        evaluation_prompt_fingerprint: report::fingerprint(&INSTRUCTIONS)?,
    })
}
pub async fn run(
    eval: EvalConfig,
    app: AppConfig,
    level: Option<u8>,
    limit: Option<usize>,
    worker: PathBuf,
) -> Result<PathBuf> {
    let dataset = Dataset::load(&eval.dataset.directory, &eval.dataset.split)?;
    let selected = dataset.select(level, limit)?;
    let ids: Vec<_> = selected.iter().map(|t| t.task_id.as_str()).collect();
    let indices: Vec<_> = dataset
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| ids.contains(&t.task_id.as_str()))
        .map(|(i, _)| i)
        .collect();
    let manifest = manifest(eval.clone(), &app, &dataset, &indices, &worker)?;
    let directory = report::create_run(&eval.run.results_directory)?;
    let _lock = report::RunLock::acquire(&directory)?;
    report::write_json(&directory.join("manifest.json"), &manifest)?;
    execute(
        directory.clone(),
        manifest,
        app,
        dataset,
        indices,
        worker,
        false,
    )
    .await?;
    Ok(directory)
}
pub async fn resume(
    directory: PathBuf,
    app: AppConfig,
    worker: PathBuf,
    retry_failed: bool,
) -> Result<()> {
    let directory = directory.canonicalize()?;
    let _lock = report::RunLock::acquire(&directory)?;
    let stored: Manifest =
        serde_json::from_slice(&std::fs::read(directory.join("manifest.json"))?)?;
    ensure!(
        stored.schema_version == 1
            && stored.kind == "gaia"
            && stored.scorer_version == scorer::VERSION,
        "Unsupported evaluation manifest or scorer version"
    );
    let dataset = Dataset::load(&stored.eval.dataset.directory, &stored.split)?;
    let mut indices = Vec::new();
    for id in &stored.task_ids {
        indices.push(
            dataset
                .tasks
                .iter()
                .position(|t| &t.task_id == id)
                .context("Selected task is missing from dataset")?,
        );
    }
    let current = manifest(stored.eval.clone(), &app, &dataset, &indices, &worker)?;
    ensure!(
        stored.dataset_fingerprint == current.dataset_fingerprint
            && stored.attachment_fingerprints == current.attachment_fingerprints,
        "Dataset changed; start a new run"
    );
    ensure!(
        stored.configuration_fingerprint == current.configuration_fingerprint
            && stored.revision == current.revision
            && stored.executable_fingerprint == current.executable_fingerprint
            && stored.evaluation_prompt_fingerprint == current.evaluation_prompt_fingerprint,
        "Configuration or code revision changed; start a new run"
    );
    execute(
        directory,
        stored,
        app,
        dataset,
        indices,
        worker,
        retry_failed,
    )
    .await
}
async fn execute(
    directory: PathBuf,
    manifest: Manifest,
    app: AppConfig,
    dataset: Dataset,
    indices: Vec<usize>,
    worker: PathBuf,
    retry_failed: bool,
) -> Result<()> {
    let dataset = Arc::new(dataset);
    let app = Arc::new(app);
    let eval = Arc::new(manifest.eval);
    let existing = report::task_results(&directory)?;
    let latest: HashMap<_, _> = report::latest(&existing)
        .into_iter()
        .map(|r| (r.task_id.clone(), r.clone()))
        .collect();
    let mut pending = indices.into_iter().filter(|&i| {
        latest.get(&dataset.tasks[i].task_id).is_none_or(|r| {
            retry_failed
                && (r.execution_status != "completed"
                    || r.answer_correct == Some(false)
                    || r.answer_format_error)
        })
    });
    let (cancel, cancelled) = watch::channel(false);
    let mut jobs = JoinSet::new();
    let mut stopped = false;
    loop {
        while !stopped && jobs.len() < eval.run.concurrency {
            let Some(index) = pending.next() else {
                break;
            };
            let task_directory = directory.join("tasks").join(&dataset.tasks[index].task_id);
            let attempt = next_attempt(&task_directory)?;
            let (dataset, app, eval, directory, worker, cancelled) = (
                dataset.clone(),
                app.clone(),
                eval.clone(),
                directory.clone(),
                worker.clone(),
                cancelled.clone(),
            );
            let expected_configuration = manifest.configuration_fingerprint.clone();
            let expected_attachment = manifest
                .attachment_fingerprints
                .get(&dataset.tasks[index].task_id)
                .cloned();
            jobs.spawn(async move {
                execute_task(
                    &dataset,
                    &dataset.tasks[index],
                    &app,
                    &eval,
                    &directory,
                    &worker,
                    attempt,
                    cancelled,
                    &expected_configuration,
                    expected_attachment.as_deref(),
                )
                .await
            });
        }
        if jobs.is_empty() {
            break;
        }
        tokio::select! {
            result=jobs.join_next()=>{
                let result = result.context("Missing evaluation worker")??;
                if let Err(error) = result {
                    cancel.send_replace(true);
                    while jobs.join_next().await.is_some() {}
                    return Err(error);
                }
                report::gaia_summary(&directory, manifest.task_ids.len())?;
            }
            signal=tokio::signal::ctrl_c(), if !stopped => { signal?; stopped=true; cancel.send_replace(true); eprintln!("Cancelling active evaluations; remaining tasks can be resumed."); }
        }
    }
    let summary = report::gaia_summary(&directory, manifest.task_ids.len())?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    println!("Results: {}", directory.display());
    Ok(())
}
async fn execute_task(
    dataset: &Dataset,
    task: &Task,
    app: &AppConfig,
    eval: &EvalConfig,
    run: &Path,
    worker: &Path,
    attempt: usize,
    mut cancelled: watch::Receiver<bool>,
    expected_configuration: &str,
    expected_attachment: Option<&str>,
) -> Result<()> {
    let directory = run
        .join("tasks")
        .join(&task.task_id)
        .join(format!("attempt-{attempt}"));
    ensure!(
        !directory.exists(),
        "Attempt directory already exists; preserve previous evidence"
    );
    std::fs::create_dir_all(&directory)?;
    let started = Instant::now();
    let mut result = TaskResult {
        task_id: task.task_id.clone(),
        attempt,
        level: task.level,
        execution_status: "infrastructure_error".into(),
        answer_correct: (eval.dataset.split == "validation").then_some(false),
        answer_format_error: false,
        answer: None,
        reference: if eval.dataset.split == "validation" {
            task.answer.clone()
        } else {
            None
        },
        error: None,
        elapsed_ms: 0.0,
        setup_ms: 0.0,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        model_requests: 0,
        tool_calls: 0,
        tool_errors: 0,
        tool_elapsed_ms: 0.0,
        first_text_ms: None,
        session_id: None,
        attachment_mode: None,
    };
    let unchanged = (|| -> Result<()> {
        ensure!(
            report::fingerprint(&crate::runtime::public_configuration(app, eval)?)?
                == expected_configuration,
            "Configuration or skills changed during the run"
        );
        if let Some(expected) = expected_attachment {
            let path = dataset
                .attachment(task)?
                .context("Task attachment disappeared")?;
            ensure!(
                digest_file(&path)? == expected,
                "Task attachment changed during the run"
            );
        }
        Ok(())
    })();
    if let Err(error) = unchanged {
        result.error = Some(error.to_string());
        result.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        report::write_json(&directory.join("result.json"), &result)?;
        return Ok(());
    }
    let runtime = match tokio::time::timeout(
        Duration::from_secs(eval.run.timeout_seconds),
        Runtime::build(app, eval, &directory, worker),
    )
    .await
    {
        Ok(Ok(runtime)) => runtime,
        other => {
            result.error = Some(match other {
                Ok(Err(error)) => format!("Runtime setup failed: {error}"),
                _ => "Runtime setup timed out".into(),
            });
            result.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            report::write_json(&directory.join("result.json"), &result)?;
            return Ok(());
        }
    };
    let mut session = Session::new();
    result.session_id = Some(session.session_id.clone());
    session.environment = runtime.agent.workspace_environment().cloned();
    let mut turn = UserTurn {
        text: format!("{}\n\n{}", task.question, INSTRUCTIONS),
        ..Default::default()
    };
    let prepared = (|| -> Result<()> {
        if let Some(path) = dataset.attachment(task)? {
            ensure!(
                std::fs::metadata(&path)?.len() <= 256 * 1024 * 1024,
                "Task attachment exceeds 256 MiB"
            );
            let target = runtime.workspace.path().join(&task.file_name);
            std::fs::copy(&path, &target)?;
            turn.text.push_str(&format!(
                "\nTask attachment is available at {}.",
                target.display()
            ));
            let ingested = if std::fs::metadata(&path)?.len() <= 20 * 1024 * 1024 {
                runtime
                    .attachments
                    .put(&session.session_id, &task.file_name, &std::fs::read(path)?)
            } else {
                Err(anyhow::anyhow!("Attachment exceeds API upload limit"))
            };
            match ingested {
                Ok(info) => {
                    result.attachment_mode = Some("attachment_api_and_workspace".into());
                    turn.attachments.push(info);
                }
                Err(_) => {
                    result.attachment_mode = Some("workspace_only".into());
                    turn.text.push_str("\nThis file is not supported by the attachment API. Inspect it using available command or connector tools; do not claim to have read it without doing so.");
                }
            }
        }
        Ok(())
    })();
    result.setup_ms = started.elapsed().as_secs_f64() * 1000.0;
    let timings = Timings::new();
    let agent = runtime.agent.clone().with_observer(timings.clone());
    let mut response = String::new();
    let mut calls = HashMap::new();
    let execution = async {
        prepared?;
        let mut stream = agent.run_turn_in_session(session,turn).await?;
        let deadline = tokio::time::Instant::now()+Duration::from_secs(eval.run.timeout_seconds).saturating_sub(started.elapsed());
        loop {
            tokio::select! {
                event=stream.next()=>match event {
                    Some(AgentStreamEvent::TextDelta {text,..})=>{ if !text.is_empty() { result.first_text_ms.get_or_insert(started.elapsed().as_secs_f64()*1000.0); } }
                    Some(AgentStreamEvent::ToolStarted {call_id,..})=>{ result.tool_calls+=1; calls.insert(call_id,Instant::now()); }
                    Some(AgentStreamEvent::ToolFinished {call_id,status,..})=>{ if let Some(start)=calls.remove(&call_id) { result.tool_elapsed_ms+=start.elapsed().as_secs_f64()*1000.0; } if status==ToolResultStatus::Error {result.tool_errors+=1;} }
                    Some(_)=>{}, None=>break,
                },
                _=tokio::time::sleep_until(deadline)=>{ result.execution_status="timed_out".into(); stream.cancel(); break; }
                _=cancelled.wait_for(|v|*v)=>{result.execution_status="cancelled".into(); stream.cancel(); break;}
            }
        }
        let outcome = tokio::time::timeout(Duration::from_secs(30),stream.finish_with_session()).await.context("Cancellation cleanup timed out")??;
        result.prompt_tokens=outcome.session.usage.prompt_tokens as u64+outcome.session.subagent_usage.prompt_tokens as u64;
        result.completion_tokens=outcome.session.usage.completion_tokens as u64+outcome.session.subagent_usage.completion_tokens as u64;
        result.total_tokens=outcome.session.usage.total_tokens as u64+outcome.session.subagent_usage.total_tokens as u64;
        if matches!(result.execution_status.as_str(),"cancelled"|"timed_out") {return Ok::<_,anyhow::Error>(());}
        match outcome.result { Ok(output)=>{response=output.output;result.execution_status="completed".into();}, Err(error)=>{result.execution_status="failed".into();result.error=Some(error.to_string());} }
        Ok(())
    }.await;
    if let Err(error) = execution {
        result.error = Some(error.to_string());
    }
    if tokio::time::timeout(Duration::from_secs(15), runtime.shutdown())
        .await
        .is_err()
    {
        result.execution_status = "infrastructure_error".into();
        result.error = Some("Runtime shutdown timed out; inspect rollout before retry".into());
    }
    result.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    result.model_requests = timings.snapshot().model_requests;
    if result.execution_status == "completed" {
        result.answer = answer::extract(&response);
        result.answer_format_error = result.answer.is_none();
        if eval.dataset.split == "validation" {
            result.answer_correct =
                Some(result.answer.as_ref().is_some_and(|a| {
                    scorer::correct(a, task.answer.as_deref().unwrap_or_default())
                }));
        }
    }
    std::fs::write(directory.join("response.txt"), response)?;
    report::write_json(&directory.join("timings.json"), &timings.snapshot())?;
    // Copy only regular files after all agent work has stopped. Never follow model-created links.
    if let Err(error) = snapshot_workspace(
        runtime.workspace.path(),
        &directory.join("workspace"),
        &mut 0usize,
        &mut 0u64,
    ) {
        report::write_json(
            &directory.join("workspace-snapshot-error.json"),
            &json!({"error":error.to_string()}),
        )?;
    }
    report::write_json(&directory.join("result.json"), &result)?;
    eprintln!(
        "{} {} · {}",
        task.task_id,
        result.execution_status,
        result
            .answer_correct
            .map_or("unscored", |v| if v { "correct" } else { "incorrect" })
    );
    Ok(())
}
fn snapshot_workspace(
    source: &Path,
    target: &Path,
    count: &mut usize,
    bytes: &mut u64,
) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        *count += 1;
        ensure!(*count <= 10000, "Workspace snapshot exceeds file limit");
        if kind.is_dir() {
            snapshot_workspace(&entry.path(), &target.join(entry.file_name()), count, bytes)?;
        } else if kind.is_file() {
            *bytes += entry.metadata()?.len();
            ensure!(
                *bytes <= 256 * 1024 * 1024,
                "Workspace snapshot exceeds 256 MiB"
            );
            std::fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn next_attempt(directory: &Path) -> Result<usize> {
    if !directory.exists() {
        return Ok(1);
    }
    let maximum = std::fs::read_dir(directory)?
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_prefix("attempt-")?.parse::<usize>().ok()
        })
        .max()
        .unwrap_or(0);
    maximum.checked_add(1).context("Attempt counter exhausted")
}

#[cfg(test)]
mod tests {
    #[test]
    fn interrupted_attempt_directories_are_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(super::next_attempt(root.path()).unwrap(), 1);
        std::fs::create_dir(root.path().join("attempt-1")).unwrap();
        std::fs::create_dir(root.path().join("attempt-3")).unwrap();
        assert_eq!(super::next_attempt(root.path()).unwrap(), 4);
    }
}
