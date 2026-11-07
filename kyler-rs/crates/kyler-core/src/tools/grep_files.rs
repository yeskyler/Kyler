use crate::{
    approval::ToolOperation,
    tools::{
        ActivityKind, CompressedToolOutput, Tool, ToolContext, ToolExecutionMode, ToolOutputBudget,
        output::fit_entries,
    },
};

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;
use tokio::{process::Command, time::timeout};

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 2_000;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GrepFilesArgs {
    pub pattern: String,

    #[serde(default)]
    pub include: Option<String>,

    #[serde(default)]
    pub path: Option<String>,

    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 2000))]
    pub limit: usize,
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

pub async fn grep_files(args: GrepFilesArgs) -> anyhow::Result<String> {
    let pattern = args.pattern.trim();
    anyhow::ensure!(!pattern.is_empty(), "pattern must not be empty");
    anyhow::ensure!(args.limit > 0, "limit must be greater than zero");

    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let search_path = resolve_search_path(args.path.as_deref(), &cwd)?;
    verify_path_exists(&search_path).await?;

    let include = args
        .include
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let files = run_rg_search(
        pattern,
        include,
        &search_path,
        args.limit.min(MAX_LIMIT),
        &cwd,
    )
    .await?;

    if files.is_empty() {
        return Ok("No matches found.".to_string());
    }

    Ok(files.join("\n"))
}

fn resolve_search_path(path: Option<&str>, cwd: &Path) -> anyhow::Result<PathBuf> {
    match path {
        None => Ok(cwd.to_path_buf()),
        Some(path) if path.trim().is_empty() => Ok(cwd.to_path_buf()),
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                Ok(path)
            } else {
                Ok(cwd.join(path))
            }
        }
    }
}

async fn verify_path_exists(path: &Path) -> anyhow::Result<()> {
    tokio::fs::metadata(path)
        .await
        .with_context(|| format!("unable to access `{}`", path.display()))?;
    Ok(())
}

async fn run_rg_search(
    pattern: &str,
    include: Option<&str>,
    search_path: &Path,
    limit: usize,
    cwd: &Path,
) -> anyhow::Result<Vec<String>> {
    let mut command = Command::new("rg");
    command
        .current_dir(cwd)
        .arg("--files-with-matches")
        .arg("--sortr=modified")
        .arg("--regexp")
        .arg(pattern)
        .arg("--no-messages");

    if let Some(glob) = include {
        command.arg("--glob").arg(glob);
    }

    command.arg("--").arg(search_path);

    let output = timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| anyhow::anyhow!("rg timed out after 30 seconds"))?
        .context("failed to launch rg; ensure ripgrep is installed and on PATH")?;

    if output.status.success() {
        return Ok(parse_results(&output.stdout, limit));
    }

    if output.status.code() == Some(1) {
        return Ok(Vec::new());
    }

    anyhow::bail!(
        "rg failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn parse_results(stdout: &[u8], limit: usize) -> Vec<String> {
    stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| std::str::from_utf8(line).ok())
        .filter(|line| !line.is_empty())
        .take(limit)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{GrepFilesArgs, grep_files, parse_results};

    use std::path::PathBuf;

    fn temporary_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kyler-grep-files-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn parses_and_limits_results() {
        let parsed = parse_results(b"/tmp/a.rs\n/tmp/b.rs\n/tmp/c.rs\n", 2);
        assert_eq!(parsed, vec!["/tmp/a.rs", "/tmp/b.rs"]);
    }

    #[tokio::test]
    async fn searches_files_with_matching_content() {
        let directory = temporary_directory("search");
        tokio::fs::create_dir_all(&directory)
            .await
            .expect("create temporary directory");
        tokio::fs::write(directory.join("match.rs"), "needle")
            .await
            .expect("write matching file");
        tokio::fs::write(directory.join("other.txt"), "haystack")
            .await
            .expect("write non-matching file");

        let output = grep_files(GrepFilesArgs {
            pattern: "needle".to_string(),
            include: Some("*.rs".to_string()),
            path: Some(directory.to_string_lossy().into_owned()),
            limit: 10,
        })
        .await
        .expect("search files");

        assert!(output.ends_with("match.rs"));
        tokio::fs::remove_dir_all(directory)
            .await
            .expect("remove temporary directory");
    }
}

pub struct GrepFilesTool;

#[async_trait::async_trait]
impl Tool for GrepFilesTool {
    fn approval_requirement(&self, _: &Value) -> anyhow::Result<ToolOperation> {
        Ok(ToolOperation::Read)
    }

    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Explore
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Worker
    }
    fn name(&self) -> &str {
        "grep_files"
    }

    fn description(&self) -> &str {
        "Find files whose contents match a ripgrep regular expression. Supports an optional path, file glob, and result limit."
    }

    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(GrepFilesArgs))
            .expect("GrepFilesArgs schema is always serializable")
    }

    fn compress_output(
        &self,
        output: &str,
        budget: &ToolOutputBudget,
    ) -> anyhow::Result<CompressedToolOutput> {
        if output.len() <= budget.max_bytes {
            return Ok(CompressedToolOutput::unchanged(output));
        }
        let lines: Vec<_> = output.lines().collect();
        let content = fit_entries(lines.len().saturating_sub(1), budget, |keep| {
            let mut kept = lines[..keep].join("\n");
            if !kept.is_empty() {
                kept.push('\n');
            }
            kept.push_str(&format!("[Partial results: omitted {} returned paths. Refine pattern/path/include or adjust limit.]", lines.len()-keep));
            kept
        })?;
        CompressedToolOutput::compressed(output, content, budget)
    }

    async fn execute(
        &self,
        args_json: &str,
        _context: &mut ToolContext<'_>,
    ) -> anyhow::Result<String> {
        if let Some(environment) = _context.environment {
            return environment.run_tool(self.name(), args_json).await;
        }
        let args: GrepFilesArgs =
            serde_json::from_str(args_json).context("failed to parse grep_files arguments")?;
        grep_files(args).await
    }
}
