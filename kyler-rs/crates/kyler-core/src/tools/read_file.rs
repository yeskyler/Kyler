use crate::{
    approval::ToolOperation,
    tools::{
        ActivityKind, CompressedToolOutput, Tool, ToolContext, ToolExecutionMode, ToolOutputBudget,
        output::fit_entries,
    },
};

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

use anyhow::Context;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
};

const MAX_LINE_LENGTH: usize = 500;
const TAB_WIDTH: usize = 4;
const COMMENT_PREFIXES: &[&str] = &["#", "//", "--"];

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    pub file_path: String,

    #[serde(default = "default_offset")]
    #[schemars(range(min = 1))]
    pub offset: usize,

    #[serde(default = "default_limit")]
    #[schemars(range(min = 1))]
    pub limit: usize,

    #[serde(default)]
    pub mode: ReadMode,

    #[serde(default)]
    pub indentation: Option<IndentationArgs>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    #[default]
    Slice,
    Indentation,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct IndentationArgs {
    #[serde(default)]
    pub anchor_line: Option<usize>,

    #[serde(default = "default_max_levels")]
    pub max_levels: usize,

    #[serde(default)]
    pub include_siblings: bool,

    #[serde(default = "default_include_header")]
    pub include_header: bool,

    #[serde(default)]
    pub max_lines: Option<usize>,
}

impl Default for IndentationArgs {
    fn default() -> Self {
        Self {
            anchor_line: None,
            max_levels: default_max_levels(),
            include_siblings: false,
            include_header: default_include_header(),
            max_lines: None,
        }
    }
}

#[derive(Debug)]
struct LineRecord {
    number: usize,
    raw: String,
    display: String,
    indent: usize,
}

impl LineRecord {
    fn is_blank(&self) -> bool {
        self.raw.trim().is_empty()
    }

    fn is_comment(&self) -> bool {
        COMMENT_PREFIXES
            .iter()
            .any(|prefix| self.raw.trim_start().starts_with(prefix))
    }
}

fn default_offset() -> usize {
    1
}

fn default_limit() -> usize {
    2_000
}

fn default_max_levels() -> usize {
    0
}

fn default_include_header() -> bool {
    true
}

pub async fn read_file(args: ReadFileArgs) -> anyhow::Result<String> {
    anyhow::ensure!(args.offset > 0, "offset must be a 1-indexed line number");
    anyhow::ensure!(args.limit > 0, "limit must be greater than zero");

    let path = PathBuf::from(&args.file_path);
    anyhow::ensure!(path.is_absolute(), "file_path must be an absolute path");

    let lines = match args.mode {
        ReadMode::Slice => read_slice(&path, args.offset, args.limit).await?,
        ReadMode::Indentation => {
            read_indentation_block(
                &path,
                args.offset,
                args.limit,
                args.indentation.unwrap_or_default(),
            )
            .await?
        }
    };

    Ok(lines.join("\n"))
}

async fn read_slice(path: &Path, offset: usize, limit: usize) -> anyhow::Result<Vec<String>> {
    let file = File::open(path)
        .await
        .with_context(|| format!("failed to read file {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    let mut line_number = 0;
    let mut lines = Vec::with_capacity(limit);

    loop {
        buffer.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut buffer)
            .await
            .with_context(|| format!("failed to read file {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }

        trim_line_ending(&mut buffer);
        line_number += 1;

        if line_number < offset {
            continue;
        }
        if lines.len() == limit {
            break;
        }

        lines.push(format!("L{line_number}: {}", format_line(&buffer)));
    }

    anyhow::ensure!(line_number >= offset, "offset exceeds file length");
    Ok(lines)
}

async fn read_indentation_block(
    path: &Path,
    offset: usize,
    limit: usize,
    options: IndentationArgs,
) -> anyhow::Result<Vec<String>> {
    let anchor_line = options.anchor_line.unwrap_or(offset);
    anyhow::ensure!(
        anchor_line > 0,
        "anchor_line must be a 1-indexed line number"
    );

    let guard_limit = options.max_lines.unwrap_or(limit);
    anyhow::ensure!(guard_limit > 0, "max_lines must be greater than zero");

    let records = collect_file_lines(path).await?;
    anyhow::ensure!(
        anchor_line <= records.len(),
        "anchor_line exceeds file length"
    );

    let anchor_index = anchor_line - 1;
    let effective_indents = compute_effective_indents(&records);
    let anchor_indent = effective_indents[anchor_index];
    let min_indent = if options.max_levels == 0 {
        0
    } else {
        anchor_indent.saturating_sub(options.max_levels * TAB_WIDTH)
    };
    let final_limit = limit.min(guard_limit).min(records.len());

    if final_limit == 1 {
        return Ok(vec![format!(
            "L{}: {}",
            records[anchor_index].number, records[anchor_index].display
        )]);
    }

    let mut output = VecDeque::with_capacity(final_limit);
    output.push_back(&records[anchor_index]);

    let mut up_index = anchor_index.checked_sub(1);
    let mut down_index = anchor_index + 1;
    let mut up_base_count = 0;
    let mut down_base_count = 0;

    while output.len() < final_limit {
        let mut progressed = false;

        if let Some(index) = up_index {
            if effective_indents[index] >= min_indent {
                let record = &records[index];
                let allow_header = options.include_header && record.is_comment();
                if effective_indents[index] != min_indent
                    || options.include_siblings
                    || allow_header
                    || up_base_count == 0
                {
                    output.push_front(record);
                    progressed = true;
                    if effective_indents[index] == min_indent && !allow_header {
                        up_base_count += 1;
                    }
                    up_index = index.checked_sub(1);
                } else {
                    up_index = None;
                }
            } else {
                up_index = None;
            }
        }

        if output.len() >= final_limit {
            break;
        }

        if down_index < records.len() {
            if effective_indents[down_index] >= min_indent {
                let record = &records[down_index];
                if effective_indents[down_index] != min_indent
                    || options.include_siblings
                    || down_base_count == 0
                {
                    output.push_back(record);
                    progressed = true;
                    if effective_indents[down_index] == min_indent {
                        down_base_count += 1;
                    }
                    down_index += 1;
                } else {
                    down_index = records.len();
                }
            } else {
                down_index = records.len();
            }
        }

        if !progressed {
            break;
        }
    }

    trim_empty_lines(&mut output);
    Ok(output
        .into_iter()
        .map(|record| format!("L{}: {}", record.number, record.display))
        .collect())
}

async fn collect_file_lines(path: &Path) -> anyhow::Result<Vec<LineRecord>> {
    let file = File::open(path)
        .await
        .with_context(|| format!("failed to read file {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    let mut records = Vec::new();
    let mut line_number = 0;

    loop {
        buffer.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut buffer)
            .await
            .with_context(|| format!("failed to read file {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }

        trim_line_ending(&mut buffer);
        line_number += 1;
        let raw = String::from_utf8_lossy(&buffer).into_owned();
        records.push(LineRecord {
            number: line_number,
            indent: measure_indent(&raw),
            display: format_line(&buffer),
            raw,
        });
    }

    Ok(records)
}

fn trim_line_ending(buffer: &mut Vec<u8>) {
    if buffer.last() == Some(&b'\n') {
        buffer.pop();
        if buffer.last() == Some(&b'\r') {
            buffer.pop();
        }
    }
}

fn compute_effective_indents(records: &[LineRecord]) -> Vec<usize> {
    let mut previous_indent = 0;
    records
        .iter()
        .map(|record| {
            if !record.is_blank() {
                previous_indent = record.indent;
            }
            previous_indent
        })
        .collect()
}

fn measure_indent(line: &str) -> usize {
    line.chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .map(|character| if character == '\t' { TAB_WIDTH } else { 1 })
        .sum()
}

fn format_line(bytes: &[u8]) -> String {
    truncate_line(String::from_utf8_lossy(bytes).into_owned())
}

fn truncate_line(value: String) -> String {
    if value.len() <= MAX_LINE_LENGTH {
        return value;
    }

    let mut end = MAX_LINE_LENGTH;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn trim_empty_lines(lines: &mut VecDeque<&LineRecord>) {
    while matches!(lines.front(), Some(line) if line.is_blank()) {
        lines.pop_front();
    }
    while matches!(lines.back(), Some(line) if line.is_blank()) {
        lines.pop_back();
    }
}

#[cfg(test)]
mod tests {
    use super::{IndentationArgs, ReadFileArgs, ReadMode, read_file};

    use std::path::PathBuf;

    fn temporary_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kyler-read-file-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn reads_requested_line_slice() {
        let path = temporary_file("slice");
        tokio::fs::write(&path, "alpha\nbeta\ngamma\n")
            .await
            .expect("write temporary file");

        let output = read_file(ReadFileArgs {
            file_path: path.to_string_lossy().into_owned(),
            offset: 2,
            limit: 2,
            mode: ReadMode::Slice,
            indentation: None,
        })
        .await
        .expect("read file");

        assert_eq!(output, "L2: beta\nL3: gamma");
        tokio::fs::remove_file(path)
            .await
            .expect("remove temporary file");
    }

    #[tokio::test]
    async fn reads_indented_block() {
        let path = temporary_file("indentation");
        tokio::fs::write(
            &path,
            "fn outer() {\n    if ready {\n        run();\n    }\n}\n",
        )
        .await
        .expect("write temporary file");

        let output = read_file(ReadFileArgs {
            file_path: path.to_string_lossy().into_owned(),
            offset: 3,
            limit: 10,
            mode: ReadMode::Indentation,
            indentation: Some(IndentationArgs {
                anchor_line: Some(3),
                max_levels: 1,
                include_siblings: false,
                include_header: true,
                max_lines: None,
            }),
        })
        .await
        .expect("read indentation block");

        assert_eq!(output, "L2:     if ready {\nL3:         run();\nL4:     }");
        tokio::fs::remove_file(path)
            .await
            .expect("remove temporary file");
    }
}

pub struct ReadFileTool;

#[async_trait::async_trait]
impl Tool for ReadFileTool {
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
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a slice of a file with line numbers (sandbox-relative paths use the session working directory), or read an indentation-aware code block around an anchor line."
    }

    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(ReadFileArgs))
            .expect("ReadFileArgs schema is always serializable")
    }

    fn compress_output(
        &self,
        output: &str,
        budget: &ToolOutputBudget,
    ) -> anyhow::Result<CompressedToolOutput> {
        if output.len() <= budget.max_bytes {
            return Ok(CompressedToolOutput::unchanged(output));
        }
        let mut lines: Vec<_> = output.lines().collect();
        let header = if lines
            .first()
            .is_some_and(|s| s.starts_with("Absolute path: "))
        {
            Some(lines.remove(0))
        } else {
            None
        };
        anyhow::ensure!(!lines.is_empty(), "file output has no lines to compress");
        let label = |line: &str| {
            line.split_once(':')
                .map_or("unknown", |(label, _)| label)
                .to_string()
        };
        let content = fit_entries(lines.len().saturating_sub(1), budget, |keep| {
            let head = keep.div_ceil(2);
            let tail = keep / 2;
            let mut result = Vec::new();
            if let Some(header) = header {
                result.push(header.to_string());
            }
            result.extend(lines[..head].iter().map(|s| s.to_string()));
            result.push(format!("[Partial file: omitted {} returned lines ({}–{}). Use read_file with offset/limit to read this range.]", lines.len()-keep, label(lines[head]), label(lines[lines.len()-tail-1])));
            result.extend(lines[lines.len() - tail..].iter().map(|s| s.to_string()));
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
        let args: ReadFileArgs =
            serde_json::from_str(args_json).context("failed to parse read_file arguments")?;
        let path = args.file_path.clone();
        Ok(format!("Absolute path: {path}\n{}", read_file(args).await?))
    }
}
