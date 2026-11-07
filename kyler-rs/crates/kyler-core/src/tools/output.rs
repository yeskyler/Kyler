//! Deterministic output budgets and helpers shared by tool-specific compressors.
use anyhow::{Result, ensure};

use kyler_config::AgentSettings;

#[derive(Debug, Clone, Copy)]
pub struct ToolOutputBudget {
    pub max_bytes: usize,
}
impl Default for ToolOutputBudget {
    fn default() -> Self {
        Self {
            max_bytes: AgentSettings::default().tool_output_max_bytes,
        }
    }
}
impl ToolOutputBudget {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.max_bytes > 0,
            "tool output byte budget must be positive"
        );
        Ok(())
    }
}

#[derive(Debug)]
pub struct CompressedToolOutput {
    pub content: String,
    pub compressed: bool,
}
impl CompressedToolOutput {
    pub fn unchanged(output: &str) -> Self {
        Self {
            content: output.to_string(),
            compressed: false,
        }
    }
    pub fn compressed(original: &str, content: String, budget: &ToolOutputBudget) -> Result<Self> {
        ensure!(
            !content.trim().is_empty()
                && content.len() <= budget.max_bytes
                && content.len() < original.len(),
            "compressed tool output must fit the budget and be smaller than the original"
        );
        Ok(Self {
            content,
            compressed: true,
        })
    }
}

/// Only select whole entries. `render(0)` must truthfully describe omitted data.
pub(crate) fn fit_entries(
    total: usize,
    budget: &ToolOutputBudget,
    render: impl Fn(usize) -> String,
) -> Result<String> {
    budget.validate()?;
    let mut best = render(0);
    ensure!(
        best.len() <= budget.max_bytes,
        "tool metadata and omission notice cannot fit the output budget"
    );
    let (mut low, mut high) = (0, total);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let candidate = render(mid);
        if candidate.len() <= budget.max_bytes {
            low = mid;
            best = candidate;
        } else {
            high = mid - 1;
        }
    }
    Ok(best)
}

pub(crate) fn text_prefix(text: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::ToolOutputBudget;
    use crate::tools::{
        Tool, apply_patch::ApplyPatchTool, grep_files::GrepFilesTool, list_dir::ListDirTool,
        read_file::ReadFileTool,
    };

    #[test]
    fn text_tools_keep_complete_entries_and_explicit_omissions() {
        let budget = ToolOutputBudget { max_bytes: 512 };
        let file = format!(
            "Absolute path: /tmp/\u{4e2d}\u{6587}.rs\n{}",
            (1..=100)
                .map(|n| format!("L{n}: \u{4e2d}\u{6587} code line {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let result = ReadFileTool.compress_output(&file, &budget).unwrap();
        assert!(result.compressed && result.content.len() <= budget.max_bytes);
        assert!(
            result
                .content
                .starts_with("Absolute path: /tmp/\u{4e2d}\u{6587}.rs\nL1:")
        );
        assert!(
            result
                .content
                .ends_with("L100: \u{4e2d}\u{6587} code line 100")
        );
        assert!(result.content.contains("omitted"));
        let paths = (0..100)
            .map(|n| format!("/project/\u{6587}\u{4ef6}_{n}.rs"))
            .collect::<Vec<_>>()
            .join("\n");
        let grep = GrepFilesTool.compress_output(&paths, &budget).unwrap();
        assert!(grep.content.len() <= budget.max_bytes);
        for line in grep.content.lines().filter(|s| !s.starts_with('[')) {
            assert!(paths.lines().any(|original| original == line));
        }
        assert!(grep.content.contains("returned paths"));
        let listing = format!("Absolute path: /project\n{paths}\nMore than 100 entries found");
        let listing = ListDirTool.compress_output(&listing, &budget).unwrap();
        assert!(listing.content.len() <= budget.max_bytes);
        assert!(listing.content.starts_with("Absolute path: /project"));
        assert!(listing.content.ends_with("More than 100 entries found"));
        assert!(listing.content.contains("offset/limit"));
    }
    #[test]
    fn small_outputs_and_action_results_remain_unchanged() {
        let budget = ToolOutputBudget { max_bytes: 256 };
        let result = ReadFileTool.compress_output("L1: small", &budget).unwrap();
        assert!(!result.compressed);
        assert_eq!(result.content, "L1: small");
        let action = "Updated /a.rs\n".repeat(100);
        let result = ApplyPatchTool.compress_output(&action, &budget).unwrap();
        assert!(!result.compressed);
        assert_eq!(result.content, action);
        assert!(
            ListDirTool
                .compress_output(
                    &format!("Absolute path: {}", "x".repeat(100)),
                    &ToolOutputBudget { max_bytes: 1 }
                )
                .is_err()
        );
    }
}

/// Preserve task identity/status/error/usage while bounding result text in valid JSON.
pub(crate) fn compress_task_json(
    output: &str,
    budget: &ToolOutputBudget,
) -> Result<CompressedToolOutput> {
    if output.len() <= budget.max_bytes {
        return Ok(CompressedToolOutput::unchanged(output));
    }
    let original: serde_json::Value = serde_json::from_str(output)?;
    fn trim_task(value: &mut serde_json::Value, cap: usize) {
        if let Some(review) = value.get_mut("review").filter(|value| value.is_object()) {
            if let Some(findings) = review
                .pointer_mut("/report/findings")
                .and_then(|value| value.as_array_mut())
            {
                let count = findings.len();
                let keep = findings
                    .iter()
                    .take_while(|finding| {
                        serde_json::to_string(finding)
                            .is_ok_and(|text| text.len() <= cap / count.max(1))
                    })
                    .count();
                findings.truncate(keep);
                if keep < count {
                    review["findings_omitted"] = (count - keep).into();
                    review["retrieval"] =
                        "Use get_task with this task ID and review_section=overview/findings/files/limitations; continue pages with review_offset=next_offset"
                            .into();
                }
            }
            if let Some(files) = review
                .pointer_mut("/snapshot/files")
                .and_then(|value| value.as_array_mut())
            {
                let count = files.len();
                files.truncate(if cap == 0 { 0 } else { count });
                if files.len() < count {
                    review["files_omitted"] = count.into();
                }
            }
        }
        if let Some(text) = value
            .get("output")
            .and_then(|v| v.as_str())
            .filter(|s| s.len() > cap)
            .map(str::to_string)
        {
            value["output"] = serde_json::Value::String(format!(
                "{}[... output omitted; full result is retained by task ID]",
                text_prefix(&text, cap)
            ));
            value["output_truncated"] = serde_json::Value::Bool(true);
        }
    }
    let render = |cap| -> Result<String> {
        let mut value = original.clone();
        if let Some(tasks) = value.get_mut("tasks").and_then(|v| v.as_array_mut()) {
            for task in tasks {
                trim_task(task, cap);
            }
        } else {
            trim_task(&mut value, cap);
        }
        Ok(serde_json::to_string(&value)?)
    };
    let mut content = render(0)?;
    ensure!(
        content.len() <= budget.max_bytes,
        "task metadata and errors exceed compression budget"
    );
    let (mut low, mut high) = (0, output.len());
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let candidate = render(mid)?;
        if candidate.len() <= budget.max_bytes {
            low = mid;
            content = candidate;
        } else {
            high = mid - 1;
        }
    }
    CompressedToolOutput::compressed(output, content, budget)
}

#[cfg(test)]
mod subagent_output_tests {
    use super::{ToolOutputBudget, compress_task_json};

    #[test]
    fn task_results_stay_json_and_keep_status_ids_and_errors() {
        let raw=serde_json::json!({"tasks":[{"task_id":"first","status":"completed","output":"\u{4e2d}\u{6587}\u{5185}\u{5bb9}".repeat(1000),"error":null},
            {"task_id":"second","status":"failed","output":null,"error":"exact failure"}],"timed_out":false}).to_string();
        let compressed = compress_task_json(&raw, &ToolOutputBudget { max_bytes: 512 }).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&compressed.content).unwrap();
        assert!(compressed.content.len() <= 512);
        assert_eq!(parsed["tasks"][0]["task_id"], "first");
        assert_eq!(parsed["tasks"][0]["status"], "completed");
        assert_eq!(parsed["tasks"][0]["output_truncated"], true);
        assert_eq!(parsed["tasks"][1]["error"], "exact failure");
        assert_eq!(parsed["timed_out"], false);
    }
}
