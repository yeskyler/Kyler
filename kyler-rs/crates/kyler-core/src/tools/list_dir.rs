use crate::{
    approval::ToolOperation,
    tools::{
        ActivityKind, CompressedToolOutput, Tool, ToolContext, ToolExecutionMode, ToolOutputBudget,
        output::fit_entries,
    },
};

use std::{
    collections::VecDeque,
    ffi::OsStr,
    fs::FileType,
    path::{Path, PathBuf},
};

use anyhow::Context;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;

const MAX_ENTRY_LENGTH: usize = 500;
const INDENTATION_SPACES: usize = 2;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListDirArgs {
    pub dir_path: String,

    #[serde(default = "default_offset")]
    #[schemars(range(min = 1))]
    pub offset: usize,

    #[serde(default = "default_limit")]
    #[schemars(range(min = 1))]
    pub limit: usize,

    #[serde(default = "default_depth")]
    #[schemars(range(min = 1))]
    pub depth: usize,
}

fn default_offset() -> usize {
    1
}

fn default_limit() -> usize {
    25
}

fn default_depth() -> usize {
    2
}

pub async fn list_dir(args: ListDirArgs) -> anyhow::Result<String> {
    anyhow::ensure!(args.offset > 0, "offset must be a 1-indexed entry number");
    anyhow::ensure!(args.limit > 0, "limit must be greater than zero");
    anyhow::ensure!(args.depth > 0, "depth must be greater than zero");

    let path = PathBuf::from(&args.dir_path);
    anyhow::ensure!(path.is_absolute(), "dir_path must be an absolute path");

    let entries = list_dir_slice(&path, args.offset, args.limit, args.depth).await?;
    let mut output = Vec::with_capacity(entries.len() + 1);
    output.push(format!("Absolute path: {}", path.display()));
    output.extend(entries);

    Ok(output.join("\n"))
}

async fn list_dir_slice(
    path: &Path,
    offset: usize,
    limit: usize,
    depth: usize,
) -> anyhow::Result<Vec<String>> {
    let mut entries = Vec::new();
    collect_entries(path, Path::new(""), depth, &mut entries).await?;

    if entries.is_empty() {
        return Ok(Vec::new());
    }

    entries.sort_unstable_by(|left, right| left.sort_key.cmp(&right.sort_key));

    let start_index = offset
        .checked_sub(1)
        .context("offset must be a 1-indexed entry number")?;
    anyhow::ensure!(
        start_index < entries.len(),
        "offset exceeds directory entry count"
    );

    let end_index = start_index.saturating_add(limit).min(entries.len());
    let mut formatted = entries[start_index..end_index]
        .iter()
        .map(format_entry_line)
        .collect::<Vec<_>>();

    if end_index < entries.len() {
        formatted.push(format!(
            "More than {} entries found",
            end_index - start_index
        ));
    }

    Ok(formatted)
}

async fn collect_entries(
    dir_path: &Path,
    relative_prefix: &Path,
    depth: usize,
    entries: &mut Vec<DirEntry>,
) -> anyhow::Result<()> {
    let mut queue =
        VecDeque::from([(dir_path.to_path_buf(), relative_prefix.to_path_buf(), depth)]);

    while let Some((current_dir, prefix, remaining_depth)) = queue.pop_front() {
        let mut read_dir = fs::read_dir(&current_dir)
            .await
            .with_context(|| format!("failed to read directory {}", current_dir.display()))?;
        let mut current_entries = Vec::new();

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .with_context(|| format!("failed to read directory {}", current_dir.display()))?
        {
            let file_type = entry
                .file_type()
                .await
                .with_context(|| format!("failed to inspect entry {}", entry.path().display()))?;
            let file_name = entry.file_name();
            let relative_path = if prefix.as_os_str().is_empty() {
                PathBuf::from(&file_name)
            } else {
                prefix.join(&file_name)
            };
            let kind = DirEntryKind::from(&file_type);

            current_entries.push((
                entry.path(),
                relative_path.clone(),
                kind,
                DirEntry {
                    sort_key: format_entry_name(&relative_path),
                    display_name: format_entry_component(&file_name),
                    depth: prefix.components().count(),
                    kind,
                },
            ));
        }

        current_entries.sort_unstable_by(|left, right| left.3.sort_key.cmp(&right.3.sort_key));

        for (entry_path, relative_path, kind, entry) in current_entries {
            if kind == DirEntryKind::Directory && remaining_depth > 1 {
                queue.push_back((entry_path, relative_path, remaining_depth - 1));
            }
            entries.push(entry);
        }
    }

    Ok(())
}

fn format_entry_name(path: &Path) -> String {
    truncate_entry(path.to_string_lossy().replace('\\', "/"))
}

fn format_entry_component(name: &OsStr) -> String {
    truncate_entry(name.to_string_lossy().into_owned())
}

fn truncate_entry(value: String) -> String {
    if value.len() <= MAX_ENTRY_LENGTH {
        return value;
    }

    let mut end = MAX_ENTRY_LENGTH;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn format_entry_line(entry: &DirEntry) -> String {
    let indent = " ".repeat(entry.depth * INDENTATION_SPACES);
    let suffix = match entry.kind {
        DirEntryKind::Directory => "/",
        DirEntryKind::File => "",
        DirEntryKind::Symlink => "@",
        DirEntryKind::Other => "?",
    };
    format!("{indent}{}{suffix}", entry.display_name)
}

#[derive(Clone)]
struct DirEntry {
    sort_key: String,
    display_name: String,
    depth: usize,
    kind: DirEntryKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DirEntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

impl From<&FileType> for DirEntryKind {
    fn from(file_type: &FileType) -> Self {
        if file_type.is_symlink() {
            Self::Symlink
        } else if file_type.is_dir() {
            Self::Directory
        } else if file_type.is_file() {
            Self::File
        } else {
            Self::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ListDirArgs, list_dir};

    use std::path::PathBuf;

    fn temporary_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kyler-list-dir-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn lists_nested_entries_in_sorted_order() {
        let directory = temporary_directory("nested");
        let nested = directory.join("nested");
        tokio::fs::create_dir_all(&nested)
            .await
            .expect("create nested directory");
        tokio::fs::write(directory.join("root.txt"), b"root")
            .await
            .expect("write root file");
        tokio::fs::write(nested.join("child.txt"), b"child")
            .await
            .expect("write child file");

        let output = list_dir(ListDirArgs {
            dir_path: directory.to_string_lossy().into_owned(),
            offset: 1,
            limit: 10,
            depth: 2,
        })
        .await
        .expect("list directory");

        assert!(output.contains("nested/\n  child.txt\nroot.txt"));
        tokio::fs::remove_dir_all(directory)
            .await
            .expect("remove temporary directory");
    }

    #[tokio::test]
    async fn rejects_relative_paths() {
        let error = list_dir(ListDirArgs {
            dir_path: "relative/path".to_string(),
            offset: 1,
            limit: 25,
            depth: 2,
        })
        .await
        .expect_err("relative paths must fail");

        assert!(error.to_string().contains("absolute path"));
    }
}

pub struct ListDirTool;

#[async_trait::async_trait]
impl Tool for ListDirTool {
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
        "list_dir"
    }

    fn description(&self) -> &str {
        "List directory entries. In a sandbox, relative paths use the session working directory; otherwise use an absolute path. Supports pagination and bounded recursive depth."
    }

    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(ListDirArgs))
            .expect("ListDirArgs schema is always serializable")
    }

    fn compress_output(
        &self,
        output: &str,
        budget: &ToolOutputBudget,
    ) -> anyhow::Result<CompressedToolOutput> {
        if output.len() <= budget.max_bytes {
            return Ok(CompressedToolOutput::unchanged(output));
        }
        let mut lines = output.lines();
        let header = lines.next().unwrap_or("");
        let mut entries: Vec<_> = lines.collect();
        let footer = if entries
            .last()
            .is_some_and(|line| line.starts_with("More than "))
        {
            entries.pop()
        } else {
            None
        };
        let content = fit_entries(entries.len().saturating_sub(1), budget, |keep| {
            let mut result = vec![header.to_string()];
            result.extend(entries[..keep].iter().map(|s| s.to_string()));
            result.push(format!("[Partial listing: omitted {} returned entries. Use list_dir with offset/limit; reduce depth if needed.]", entries.len()-keep));
            if let Some(footer) = footer {
                result.push(footer.to_string());
            }
            result.join("\n")
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
        let args: ListDirArgs =
            serde_json::from_str(args_json).context("failed to parse list_dir arguments")?;
        list_dir(args).await
    }
}
