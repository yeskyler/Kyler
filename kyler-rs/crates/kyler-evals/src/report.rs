use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn fingerprint(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("Missing output directory")?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temporary, value)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}
pub struct RunLock {
    _file: File,
}
impl RunLock {
    pub fn acquire(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(".lock"))?;
        file.try_lock()
            .context("Evaluation run is already in use")?;
        Ok(Self { _file: file })
    }
}
pub fn create_run(root: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let path = root.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(path.canonicalize()?)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: String,
    pub attempt: usize,
    pub level: u8,
    pub execution_status: String,
    pub answer_correct: Option<bool>,
    pub answer_format_error: bool,
    pub answer: Option<String>,
    pub reference: Option<String>,
    pub error: Option<String>,
    pub elapsed_ms: f64,
    pub setup_ms: f64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub model_requests: usize,
    pub tool_calls: usize,
    pub tool_errors: usize,
    pub tool_elapsed_ms: f64,
    pub first_text_ms: Option<f64>,
    pub session_id: Option<String>,
    pub attachment_mode: Option<String>,
}
pub fn task_results(directory: &Path) -> Result<Vec<TaskResult>> {
    let mut results = Vec::new();
    let tasks = directory.join("tasks");
    if !tasks.exists() {
        return Ok(results);
    }
    for task in std::fs::read_dir(tasks)? {
        let task = task?;
        if !task.file_type()?.is_dir() {
            continue;
        }
        for attempt in std::fs::read_dir(task.path())? {
            let path = attempt?.path().join("result.json");
            if path.is_file() {
                results.push(serde_json::from_slice(&std::fs::read(path)?)?);
            }
        }
    }
    results.sort_by(|a, b| a.task_id.cmp(&b.task_id).then(a.attempt.cmp(&b.attempt)));
    Ok(results)
}
pub fn latest(results: &[TaskResult]) -> Vec<&TaskResult> {
    let mut values = BTreeMap::<&str, &TaskResult>::new();
    for result in results {
        let item = values.entry(&result.task_id).or_insert(result);
        if result.attempt > item.attempt {
            *item = result;
        }
    }
    values.into_values().collect()
}
pub fn distribution(values: impl IntoIterator<Item = f64>) -> Value {
    let mut values: Vec<_> = values.into_iter().filter(|v| v.is_finite()).collect();
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return json!({"samples":0});
    }
    let percentile = |p: f64| {
        values[((values.len() as f64 * p).ceil() as usize)
            .saturating_sub(1)
            .min(values.len() - 1)]
    };
    json!({"samples":values.len(),"min":values[0],"p50":percentile(0.5),"p95":percentile(0.95),"max":values[values.len()-1]})
}
pub fn gaia_summary(directory: &Path, expected: usize) -> Result<Value> {
    let all = task_results(directory)?;
    let latest = latest(&all);
    let summarize = |tasks: Vec<&TaskResult>| {
        let scored = tasks.iter().filter(|t| t.answer_correct.is_some()).count();
        let correct = tasks
            .iter()
            .filter(|t| t.answer_correct == Some(true))
            .count();
        let mut statuses = BTreeMap::<&str, usize>::new();
        for task in &tasks {
            *statuses.entry(&task.execution_status).or_default() += 1;
        }
        json!({"evaluated":tasks.len(),"scored":scored,"correct":correct,"accuracy":(scored>0).then(|| correct as f64 / scored as f64),"status_counts":statuses,"format_errors":tasks.iter().filter(|t|t.answer_format_error).count(),"elapsed_ms":distribution(tasks.iter().map(|t|t.elapsed_ms)),"correct_elapsed_ms":distribution(tasks.iter().filter(|t|t.answer_correct==Some(true)).map(|t|t.elapsed_ms)),"total_tokens":tasks.iter().map(|t|t.total_tokens).sum::<u64>(),"model_requests":tasks.iter().map(|t|t.model_requests).sum::<usize>(),"tool_calls":tasks.iter().map(|t|t.tool_calls).sum::<usize>(),"tool_errors":tasks.iter().map(|t|t.tool_errors).sum::<usize>(),"tool_elapsed_ms":distribution(tasks.iter().map(|t|t.tool_elapsed_ms)),"first_text_ms":distribution(tasks.iter().filter_map(|t|t.first_text_ms)),"correct_total_tokens":distribution(tasks.iter().filter(|t|t.answer_correct==Some(true)).map(|t|t.total_tokens as f64))})
    };
    let summary = json!({"kind":"gaia","expected":expected,"finished":latest.len(),"complete":latest.len()==expected,"attempts":all.len(),"selection_policy":"latest attempt; retries retained, never best-of","overall":summarize(latest.clone()),"levels":(1..=3).map(|level|(level.to_string(),summarize(latest.iter().copied().filter(|t|t.level==level).collect()))).collect::<BTreeMap<_,_>>()});
    write_json(&directory.join("summary.json"), &summary)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    for result in all {
        serde_json::to_writer(&mut file, &result)?;
        writeln!(file)?;
    }
    file.as_file().sync_all()?;
    file.persist(directory.join("results.jsonl"))?;
    Ok(summary)
}
pub fn compare(baseline: &Path, candidate: &Path) -> Result<Value> {
    let a: Value = serde_json::from_slice(&std::fs::read(baseline.join("manifest.json"))?)?;
    let b: Value = serde_json::from_slice(&std::fs::read(candidate.join("manifest.json"))?)?;
    ensure!(
        a["kind"] == b["kind"],
        "Cannot compare different evaluation kinds"
    );
    let same = if a["kind"] == "gaia" {
        a["dataset_fingerprint"] == b["dataset_fingerprint"]
            && a["task_ids"] == b["task_ids"]
            && a["attachment_fingerprints"] == b["attachment_fingerprints"]
            && a["scorer_version"] == b["scorer_version"]
    } else {
        a["scenario"] == b["scenario"] && a["model_mode"] == b["model_mode"]
    };
    ensure!(same, "Incompatible task selection or startup scenario");
    let a_summary: Value = serde_json::from_slice(&std::fs::read(baseline.join("summary.json"))?)?;
    let b_summary: Value = serde_json::from_slice(&std::fs::read(candidate.join("summary.json"))?)?;
    let paths = if a["kind"] == "gaia" {
        vec![
            "/overall/accuracy",
            "/overall/elapsed_ms/p50",
            "/overall/elapsed_ms/p95",
            "/overall/total_tokens",
        ]
    } else {
        vec![
            "/process_to_ready_ms/p50",
            "/process_to_ready_ms/p95",
            "/turn_to_request_ms/p50",
            "/turn_to_first_text_ms/p50",
        ]
    };
    let deltas: BTreeMap<_, _> = paths
        .into_iter()
        .map(|path| {
            let delta = a_summary
                .pointer(path)
                .and_then(Value::as_f64)
                .zip(b_summary.pointer(path).and_then(Value::as_f64))
                .map(|(a, b)| b - a);
            (path, delta)
        })
        .collect();
    Ok(
        json!({"baseline":a_summary,"candidate":b_summary,"candidate_minus_baseline":deltas,"same_configuration":a["configuration_fingerprint"]==b["configuration_fingerprint"],"same_platform":a["platform"]==b["platform"],"note":"Check sample counts, completion, model, host and configuration before interpreting differences. Deltas for partial GAIA runs are provisional."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn percentile_uses_nearest_rank_and_discards_nonfinite_values() {
        let result = distribution([20.0, 10.0, 30.0, f64::NAN]);
        assert_eq!(result["samples"], 3);
        assert_eq!(result["p50"], 20.0);
        assert_eq!(result["p95"], 30.0);
        assert_eq!(distribution([])["samples"], 0);
    }
    #[test]
    fn run_lock_excludes_concurrent_writers_and_survives_reopen() {
        let root = tempfile::tempdir().unwrap();
        let first = RunLock::acquire(root.path()).unwrap();
        assert!(RunLock::acquire(root.path()).is_err());
        drop(first);
        assert!(RunLock::acquire(root.path()).is_ok());
    }
}
