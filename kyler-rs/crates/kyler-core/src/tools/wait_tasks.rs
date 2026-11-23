use crate::{
    approval::ToolOperation,
    subagent::{SubagentTask, WaitMode, compress_task_json},
    tools::{CompressedToolOutput, Tool, ToolContext, ToolKind, ToolOutputBudget, fit_entries},
};

use anyhow::{Context, ensure};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitTasksArgs {
    #[serde(default)]
    pub task_ids: Vec<String>,
    pub task_id: Option<String>,
    pub review_section: Option<ReviewSection>,
    #[serde(default)]
    pub review_offset: usize,
    #[serde(default = "page_limit")]
    pub review_limit: usize,
    #[serde(default)]
    pub mode: WaitMode,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}
fn default_timeout() -> u64 {
    20
}

fn page_limit() -> usize {
    5
}
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSection {
    Overview,
    Findings,
    Files,
    Limitations,
}

fn review_page(
    task: SubagentTask,
    section: ReviewSection,
    offset: usize,
    limit: usize,
) -> anyhow::Result<Value> {
    ensure!((1..=10).contains(&limit), "review_limit must be in 1..=10");
    let result = task
        .review
        .context("This task has no validated review result")?;
    let overview = json!({"assessment":result.report.assessment,"summary":result.report.summary,"coverage":result.report.coverage.status,"findings_total":result.report.findings.len(),"files_total":result.snapshot.files.len(),"limitations_total":result.report.coverage.limitations.len(),"snapshot":{"target":result.snapshot.target,"head":result.snapshot.head,"base":result.snapshot.base,"revision":result.snapshot.revision,"fingerprint":result.snapshot.fingerprint,"stale":result.snapshot.stale}});
    let (section, entries) = match section {
        ReviewSection::Overview => {
            ensure!(offset == 0, "Overview has no offset");
            return Ok(
                json!({"task_id":task.task_id,"author":task.author,"agent":task.agent,"status":task.status,"overview":overview}),
            );
        }
        ReviewSection::Findings => ("findings", serde_json::to_value(result.report.findings)?),
        ReviewSection::Files => ("files", serde_json::to_value(result.snapshot.files)?),
        ReviewSection::Limitations => (
            "limitations",
            serde_json::to_value(result.report.coverage.limitations)?,
        ),
    };
    let entries = entries.as_array().context("Invalid review entries")?;
    ensure!(
        offset <= entries.len(),
        "review_offset exceeds the result length"
    );
    let total = entries.len();
    let page = entries
        .iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    let next = offset + page.len();
    Ok(
        json!({"task_id":task.task_id,"author":task.author,"agent":task.agent,"status":task.status,"fingerprint":result.snapshot.fingerprint,"review_page":{"section":section,"offset":offset,"total":total,"entries":page,"next_offset":(next<total).then_some(next)}}),
    )
}
fn compress_page(
    original: &str,
    value: Value,
    budget: &ToolOutputBudget,
) -> anyhow::Result<CompressedToolOutput> {
    let page = &value["review_page"];
    let entries = page["entries"]
        .as_array()
        .context("Missing review page entries")?;
    let offset = page["offset"]
        .as_u64()
        .context("Invalid review page offset")? as usize;
    let total = page["total"]
        .as_u64()
        .context("Invalid review page total")? as usize;
    let mut kept = 0;
    let content = fit_entries(entries.len(), budget, |count| {
        let mut value = value.clone();
        value["review_page"]["entries"] = Value::Array(entries[..count].to_vec());
        value["review_page"]["next_offset"] =
            json!((offset + count < total).then_some(offset + count));
        value["review_page"]["truncated"] = Value::Bool(count < entries.len());
        value.to_string()
    })?;
    if let Ok(value) = serde_json::from_str::<Value>(&content) {
        kept = value["review_page"]["entries"]
            .as_array()
            .map_or(0, Vec::len);
    }
    ensure!(
        entries.is_empty() || kept > 0,
        "One review entry exceeds the configured tool output budget"
    );
    CompressedToolOutput::compressed(original, content, budget)
}

impl WaitTasksArgs {
    fn ids(&self) -> anyhow::Result<Vec<String>> {
        ensure!(
            self.task_id.is_none() || self.task_ids.is_empty(),
            "Use task_id or task_ids, not both"
        );
        let ids = self
            .task_id
            .as_ref()
            .map(|id| vec![id.clone()])
            .unwrap_or_else(|| self.task_ids.clone());
        if self.review_section.is_some() {
            ensure!(
                ids.len() == 1 && self.timeout_secs == 0,
                "Review pagination requires exactly one task ID and timeout_secs=0"
            );
            ensure!(
                (1..=10).contains(&self.review_limit),
                "review_limit must be in 1..=10"
            );
        } else {
            ensure!(
                self.review_offset == 0 && self.review_limit == page_limit(),
                "Review pagination requires review_section"
            );
        }
        Ok(ids)
    }
}

pub struct WaitTasksTool;
#[async_trait::async_trait]
impl Tool for WaitTasksTool {
    fn approval_requirement(&self, _: &Value) -> anyhow::Result<ToolOperation> {
        Ok(ToolOperation::State)
    }

    fn kind(&self) -> ToolKind {
        ToolKind::SubagentManagement
    }
    fn name(&self) -> &str {
        "wait_tasks"
    }
    fn description(&self) -> &str {
        "List, inspect or wait for child tasks owned by this parent execution. Omit IDs to list immediately. Set task_ids (or a single task_id) and timeout_secs=0 to inspect without waiting; positive timeout waits for mode=any/all, default 20 seconds, capped by host policy. Timeout does not cancel tasks. Use exactly one ID, timeout_secs=0 and review_section=overview/findings/files/limitations for review results. Continue pages with review_offset=next_offset, review_limit defaults to 5 (1..=10). Collect required findings and limitations before summarizing a review."
    }
    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(WaitTasksArgs)).expect("tool schema is serializable")
    }
    fn concurrency(&self) -> crate::tools::ToolConcurrency {
        crate::tools::ToolConcurrency::Wait
    }
    async fn execute(
        &self,
        args_json: &str,
        context: &mut ToolContext<'_>,
    ) -> anyhow::Result<String> {
        self.execute_parallel(args_json, &context.parallel()).await
    }
    async fn execute_parallel(
        &self,
        args_json: &str,
        context: &crate::tools::ParallelToolContext<'_>,
    ) -> anyhow::Result<String> {
        let args: WaitTasksArgs = serde_json::from_str(args_json)?;
        let scope = context
            .subagents
            .context("subagent manager is not configured")?;
        let ids = args.ids()?;
        if let Some(section) = args.review_section {
            return Ok(review_page(
                scope.get(&ids[0])?,
                section,
                args.review_offset,
                args.review_limit,
            )?
            .to_string());
        }
        if ids.is_empty() {
            return Ok(json!({"tasks":scope.list(),"timed_out":false}).to_string());
        }
        let mut result = scope.wait(&ids, args.mode, args.timeout_secs).await?;
        if args.timeout_secs == 0 {
            result.timed_out = false;
        }
        Ok(serde_json::to_string(&result)?)
    }

    fn compress_output(
        &self,
        output: &str,
        budget: &ToolOutputBudget,
    ) -> anyhow::Result<CompressedToolOutput> {
        if output.len() > budget.max_bytes {
            let value: Value = serde_json::from_str(output)?;
            if value.get("review_page").is_some() {
                return compress_page(output, value, budget);
            }
        }
        compress_task_json(output, budget)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn merged_query_arguments_and_registration_are_unambiguous() {
        let parse = |value| {
            serde_json::from_value::<WaitTasksArgs>(value)
                .unwrap()
                .ids()
        };
        assert!(parse(json!({})).unwrap().is_empty());
        assert_eq!(
            parse(json!({"task_id":"one","timeout_secs":0})).unwrap(),
            vec!["one"]
        );
        assert!(parse(json!({"task_id":"one","task_ids":["two"]})).is_err());
        assert!(
            parse(json!({"task_ids":["one","two"],"review_section":"findings","timeout_secs":0}))
                .is_err()
        );
        assert!(parse(json!({"task_ids":["one"],"review_section":"findings"})).is_err());
        assert!(parse(json!({"task_ids":["one"],"review_offset":1})).is_err());
        assert!(
            parse(json!({"task_ids":["one"],"review_section":"findings","timeout_secs":0})).is_ok()
        );
        let names = crate::tools::subagent_tools()
            .into_iter()
            .map(|tool| tool.name().to_owned())
            .collect::<Vec<_>>();
        assert!(!names.contains(&"get_task".into()));
        assert!(names.contains(&"wait_tasks".into()));
    }

    use super::*;
    use kyler_protocol::review::*;
    fn task() -> SubagentTask {
        let findings = (0..5)
            .map(|index| ReviewFinding {
                priority: ReviewPriority::P1,
                title: format!("Finding {index}"),
                explanation: "Evidence and impact. ".repeat(80),
                location: ReviewLocation {
                    path: "code.rs".into(),
                    start_line: 1,
                    end_line: 1,
                },
                confidence: 0.9,
            })
            .collect();
        SubagentTask {
            author: Default::default(),
            agent: AgentKind::Review,
            review: Some(ReviewResult {
                report: ReviewReport {
                    assessment: ReviewAssessment::IssuesFound,
                    summary: "Findings require attention.".into(),
                    findings,
                    coverage: ReviewCoverage {
                        status: ReviewCoverageStatus::Complete,
                        limitations: vec!["Tests were not run.".into()],
                    },
                },
                snapshot: ReviewSnapshot {
                    target: ReviewTarget::Uncommitted,
                    head: "a".repeat(40),
                    base: Some("a".repeat(40)),
                    revision: None,
                    fingerprint: "b".repeat(64),
                    files: vec!["code.rs".into()],
                    stale: false,
                },
            }),
            review_target: None,
            task_id: "task".into(),
            parent_session_id: "parent".into(),
            parent_execution_id: "execution".into(),
            parent_tool_call_id: "call".into(),
            session_id: "child".into(),
            execution_id: None,
            status: crate::subagent::SubagentStatus::Completed,
            output: Some("Review completed.".into()),
            error: None,
            usage: Default::default(),
            created_at: 0,
            started_at: Some(0),
            finished_at: Some(1),
        }
    }
    #[test]
    fn compressed_review_pages_advance_without_losing_or_duplicating_findings() {
        let task = task();
        let mut offset = 0;
        let mut found = Vec::new();
        loop {
            let raw = review_page(task.clone(), ReviewSection::Findings, offset, 5)
                .unwrap()
                .to_string();
            let compact = WaitTasksTool
                .compress_output(&raw, &ToolOutputBudget { max_bytes: 3000 })
                .unwrap();
            assert!(compact.content.len() <= 3000);
            let value: Value = serde_json::from_str(&compact.content).unwrap();
            found.extend(
                value["review_page"]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| entry["title"].as_str().unwrap().to_owned()),
            );
            match value["review_page"]["next_offset"].as_u64() {
                Some(next) => {
                    assert!(next as usize > offset);
                    offset = next as usize;
                }
                None => break,
            }
        }
        assert_eq!(
            found,
            (0..5)
                .map(|index| format!("Finding {index}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            review_page(task, ReviewSection::Overview, 0, 5).unwrap()["overview"]["findings_total"],
            5
        );
    }
    #[test]
    fn report_previews_keep_identity_and_explicit_retrieval_instructions() {
        let raw = serde_json::to_string(&task()).unwrap();
        let compact = WaitTasksTool
            .compress_output(&raw, &ToolOutputBudget { max_bytes: 3000 })
            .unwrap();
        let value: Value = serde_json::from_str(&compact.content).unwrap();
        assert!(compact.compressed && compact.content.len() <= 3000);
        assert_eq!(value["task_id"], "task");
        assert!(value["review"]["findings_omitted"].as_u64().unwrap() > 0);
        assert!(
            value["review"]["retrieval"]
                .as_str()
                .unwrap()
                .contains("review_section")
        );
    }
}
