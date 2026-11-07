//! Review tools expose scoped evidence, never commands or mutable application state.
use super::{repository::Repository, validate_path};
use crate::{
    approval::ToolOperation,
    tools::{ActivityKind, Tool, ToolBox, ToolContext, ToolExecutionMode, output::text_prefix},
};

use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

struct ReadFile(Arc<Repository>);
struct ListFiles(Arc<Repository>);
struct SearchFiles(Arc<Repository>);
pub(super) fn toolbox(repository: Arc<Repository>) -> ToolBox {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ReadFile(repository.clone())),
        Arc::new(ListFiles(repository.clone())),
        Arc::new(SearchFiles(repository)),
    ];
    tools
        .into_iter()
        .map(|tool| (tool.name().into(), tool))
        .collect()
}
fn number(args: &Value, key: &str, default: usize, max: usize) -> Result<usize> {
    let value = match args.get(key) {
        None => default,
        Some(value) => usize::try_from(
            value
                .as_u64()
                .context("Review offsets and limits must be nonnegative integers")?,
        )?,
    };
    ensure!(value <= max, "Review {key} exceeds {max}");
    Ok(value)
}
fn prefix(args: &Value) -> Result<&str> {
    let path = args
        .get("path")
        .map(|path| path.as_str().context("Review path must be a string"))
        .transpose()?
        .unwrap_or("");
    if !path.is_empty() && path != "." {
        validate_path(path)?;
    }
    Ok(if path == "." {
        ""
    } else {
        path.trim_end_matches('/')
    })
}
macro_rules! read_contract {
    () => {
        fn execution_mode(&self) -> ToolExecutionMode {
            ToolExecutionMode::Host
        }
        fn approval_requirement(&self, _: &Value) -> Result<ToolOperation> {
            Ok(ToolOperation::Read)
        }
        fn activity_kind(&self) -> ActivityKind {
            ActivityKind::Explore
        }
    };
}
#[async_trait::async_trait]
impl Tool for ReadFile {
    read_contract!();
    fn name(&self) -> &str {
        "read_review_file"
    }
    fn description(&self) -> &str {
        "Read bounded, numbered UTF-8 lines from the captured review revision (or current worktree for uncommitted review). Deleted files use base content. Paths are workspace-relative and cannot escape the workspace. Inspect project instructions, changed files and relevant callers. Output states omitted lines explicitly."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","additionalProperties":false,"required":["path"],"properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1,"maximum":1000}}})
    }
    async fn execute(&self, args: &str, _: &mut ToolContext<'_>) -> Result<String> {
        let args: Value = serde_json::from_str(args)?;
        let path = args["path"].as_str().context("Missing review path")?;
        let offset = number(&args, "offset", 1, 10000000)?;
        let limit = number(&args, "limit", 200, 1000)?;
        ensure!(
            offset > 0 && limit > 0,
            "Review offset and limit must be positive"
        );
        let file = self.0.read(path, true).await?;
        let total = file.content.lines().count();
        let mut lines = Vec::new();
        let mut bytes = 0;
        for (index, line) in file
            .content
            .lines()
            .enumerate()
            .skip(offset - 1)
            .take(limit)
        {
            let preview = text_prefix(line, 4000);
            if bytes + preview.len() > 12000 && !lines.is_empty() {
                break;
            }
            bytes += preview.len();
            lines.push(
                json!({"line":index+1,"text":preview,"truncated":preview.len() != line.len()}),
            );
        }
        let next = offset.saturating_add(lines.len());
        let truncated = next <= total || lines.iter().any(|line| line["truncated"] == true);
        if !lines.iter().any(|line| line["truncated"] == true) {
            self.0
                .note_visible(path, offset, next.saturating_sub(1), total);
        }
        if lines.iter().any(|line| line["truncated"] == true) {
            self.0.limit(format!(
                "A long line in {path} exceeded the preview budget."
            ));
        }
        Ok(json!({"path":path,"revision":self.0.captured.revision,"deleted":file.deleted,"total_lines":total,"lines":lines,"truncated":truncated,"next_line":(next<=total).then_some(next)}).to_string())
    }
}
#[async_trait::async_trait]
impl Tool for ListFiles {
    read_contract!();
    fn name(&self) -> &str {
        "list_review_files"
    }
    fn description(&self) -> &str {
        "List captured workspace-relative review/context files, optionally under a file or directory prefix. Pagination uses offset and limit. The immutable revision is used for commit/branch targets. review.paths limits file discovery; explicit reads can inspect other authorized workspace files."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","additionalProperties":false,"properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":200}}})
    }
    async fn execute(&self, args: &str, _: &mut ToolContext<'_>) -> Result<String> {
        let args: Value = serde_json::from_str(args)?;
        let files = self.0.files(prefix(&args)?);
        let offset = number(&args, "offset", 0, 1000000)?;
        let limit = number(&args, "limit", 100, 200)?;
        ensure!(limit > 0, "Review limit must be positive");
        let selected = files.iter().skip(offset).take(limit).collect::<Vec<_>>();
        let next = offset + selected.len();
        Ok(json!({"files":selected,"total":files.len(),"next_offset":(next<files.len()).then_some(next)}).to_string())
    }
}
#[async_trait::async_trait]
impl Tool for SearchFiles {
    read_contract!();
    fn name(&self) -> &str {
        "search_review_files"
    }
    fn description(&self) -> &str {
        "Search literal text in at most 64 captured files per call, within an optional workspace-relative prefix. Returns numbered matches with bounded snippets. Uses the captured revision for commit/branch reviews. Continue using next_file_offset; unreadable/binary files are reported as skipped, not searched successfully."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","additionalProperties":false,"required":["query"],"properties":{"query":{"type":"string"},"path":{"type":"string"},"case_sensitive":{"type":"boolean"},"file_offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}})
    }
    async fn execute(&self, args: &str, _: &mut ToolContext<'_>) -> Result<String> {
        let args: Value = serde_json::from_str(args)?;
        let query = args["query"].as_str().context("Missing search query")?;
        ensure!(
            !query.is_empty() && query.len() <= 512,
            "Review search query must contain 1 to 512 bytes"
        );
        let sensitive = args["case_sensitive"].as_bool().unwrap_or(false);
        let query = if sensitive {
            query.into()
        } else {
            query.to_lowercase()
        };
        let offset = number(&args, "file_offset", 0, 1000000)?;
        let limit = number(&args, "limit", 20, 100)?;
        ensure!(limit > 0, "Review limit must be positive");
        let files = self.0.files(prefix(&args)?);
        let mut matches = Vec::new();
        let mut skipped = Vec::new();
        let mut scanned = 0;
        let mut truncated = false;
        for path in files.iter().skip(offset).take(64) {
            scanned += 1;
            match self.0.read(path, true).await {
                Ok(file) => {
                    for (index, line) in file.content.lines().enumerate() {
                        let contains = if sensitive {
                            line.contains(&query)
                        } else {
                            line.to_lowercase().contains(&query)
                        };
                        if contains {
                            if matches.len() == limit {
                                truncated = true;
                                break;
                            }
                            matches.push(
                                json!({"path":path,"line":index+1,"text":text_prefix(line,300)}),
                            );
                        }
                    }
                }
                Err(_) => skipped.push(*path),
            }
            if truncated {
                break;
            }
        }
        let next = offset + scanned;
        Ok(json!({"matches":matches,"skipped_files":skipped,"matches_truncated":truncated,"next_file_offset":(next<files.len()).then_some(next),"files_total":files.len()}).to_string())
    }
}
