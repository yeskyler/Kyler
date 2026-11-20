//! Explicit facts and reviewable experience; candidates never enter automatic context.
use crate::{
    approval::ToolOperation,
    memory::{MemoryMetadata, MemoryStatus, MemoryStore},
    skills::SkillManager,
    tools::{Tool, ToolContext, ToolExecutionMode},
};

use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use kyler_protocol::api::skills::{SaveSkillRequest, SkillScope};

pub struct MemoryTool {
    pub store: Arc<MemoryStore>,
    pub scope: String,
    pub skills: Option<Arc<SkillManager>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    key: String,
    revision: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    action: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    tags: Vec<String>,
    expected_revision: Option<u64>,
    metadata: Option<MemoryMetadata>,
    status: Option<MemoryStatus>,
    #[serde(default)]
    offset: usize,
    #[serde(default = "page_limit")]
    limit: usize,
    name: Option<String>,
    instructions: Option<String>,
    #[serde(default)]
    sources: Vec<Source>,
    expected_fingerprint: Option<String>,
}
fn page_limit() -> usize {
    8
}
#[async_trait::async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }
    fn description(&self) -> &str {
        "Search active project facts; save only when the user explicitly asks to remember an approved fact. Use propose for inferred lessons/preferences, with applicability conditions and evidence. Candidates are excluded from automatic context until the user reviews their exact revision in Memory or with the local learning CLI. List inspects scoped entries; delete requires expected_revision. draft_skill turns exact source memory revisions into an uninstalled user skill draft: provide complete SKILL.md as instructions, name and sources. It never enables or executes the draft. Never persist credentials, sensitive data, temporary progress or claims of verification without evidence."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{
            "action":{"type":"string","enum":["search","save","propose","list","delete","draft_skill"]},
            "query":{"type":"string"},"key":{"type":"string"},"content":{"type":"string"},
            "tags":{"type":"array","items":{"type":"string"}},"expected_revision":{"type":"integer","minimum":0},
            "metadata":{"type":"object","properties":{"kind":{"type":"string","enum":["fact","preference","lesson","procedure"]},"conditions":{"type":"string"},"evidence":{"type":"array","items":{"type":"string"}},"expires_at":{"type":["integer","null"]},"sensitive":{"type":"boolean"}},"additionalProperties":false},
            "status":{"type":"string","enum":["candidate","active","rejected","invalid"]},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":16},
            "name":{"type":"string"},"instructions":{"type":"string"},
            "sources":{"type":"array","maxItems":16,"items":{"type":"object","properties":{"key":{"type":"string"},"revision":{"type":"integer","minimum":1}},"required":["key","revision"],"additionalProperties":false}},"expected_fingerprint":{"type":["string","null"]}
        },"required":["action"],"additionalProperties":false})
    }
    fn approval_identity(&self) -> String {
        format!(
            "memory:{}:{:?}:{:?}",
            self.scope,
            self.store.directory(),
            self.skills
                .as_ref()
                .map(|manager| &manager.settings.user_directory)
        )
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn approval_requirement(&self, args: &Value) -> Result<ToolOperation> {
        Ok(
            if matches!(args["action"].as_str(), Some("search" | "list")) {
                ToolOperation::Read
            } else {
                ToolOperation::Write
            },
        )
    }
    async fn execute(&self, args: &str, context: &mut ToolContext<'_>) -> Result<String> {
        let args: Args = serde_json::from_str(args)?;
        let store = self.store.clone();
        let scope = self.scope.clone();
        let session = context.session_id.to_owned();
        let execution = context.execution.execution_id.clone();
        let call = context.tool_call_id.to_owned();
        if args.action == "draft_skill" {
            ensure!(
                !args.sources.is_empty() && args.sources.len() <= 16,
                "Skill drafts require 1..=16 exact source references"
            );
            let sources =
                tokio::task::spawn_blocking(move || draft_sources(&store, &scope, args.sources))
                    .await??;
            let manager = self.skills.as_ref().context("Skills are unavailable")?;
            let mut files = BTreeMap::new();
            files.insert(
                "SKILL.md".into(),
                args.instructions
                    .context("instructions must contain complete SKILL.md")?,
            );
            files.insert("references/learning-source.json".into(),serde_json::to_string_pretty(&json!({"sources":sources,"created_by_session":session,"created_by_execution":execution,"created_by_tool_call":call}))?);
            let draft = manager
                .save_draft(SaveSkillRequest {
                    name: args.name.context("Skill draft name is required")?,
                    scope: SkillScope::User,
                    files,
                    expected_fingerprint: args.expected_fingerprint,
                })
                .await?;
            return Ok(json!({"draft":draft,"note":"Draft is not installed or enabled. The conversation displays a skill review card. Ask the user to inspect its files and choose Install skill, Request changes, or Discard. Never publish without their confirmation. The local kyler learning drafts CLI is also available."}).to_string());
        }
        tokio::task::spawn_blocking(move || {
            local_action(&store, &scope, &session, &execution, &call, args)
                .map(|value| value.to_string())
        })
        .await?
    }
}

fn draft_sources(store: &MemoryStore, scope: &str, sources: Vec<Source>) -> Result<Vec<Value>> {
    let entries = store.list(scope)?;
    sources
        .into_iter()
        .map(|source| {
            let entry = entries
                .iter()
                .find(|entry| entry.key == source.key && entry.revision == source.revision)
                .context("Source memory changed or is missing")?;
            ensure!(
                matches!(entry.status, MemoryStatus::Active | MemoryStatus::Candidate)
                    && !entry.metadata.sensitive
                    && entry
                        .metadata
                        .expires_at
                        .is_none_or(|expiry| expiry > chrono::Utc::now().timestamp_millis()),
                "Source memory is unavailable"
            );
            Ok(
                json!({"scope":scope,"key":entry.key,"revision":entry.revision,
            "content_sha256":format!("{:x}",Sha256::digest(entry.content.as_bytes())),
            "source_session":entry.source_session,"source_execution":entry.source_execution}),
            )
        })
        .collect()
}

fn local_action(
    store: &MemoryStore,
    scope: &str,
    session: &str,
    execution: &str,
    call: &str,
    args: Args,
) -> Result<Value> {
    match args.action.as_str() {
        "search" => Ok(serde_json::to_value(store.search(scope, &args.query)?)?),
        "save" => {
            ensure!(
                args.metadata.is_none(),
                "Use propose to attach evidence and applicability metadata before review"
            );
            Ok(serde_json::to_value(store.put(
                scope,
                &args.key,
                &args.content,
                args.tags,
                args.expected_revision,
                session,
            )?)?)
        }
        "propose" => Ok(
            json!({"store":store.directory(),"scope":scope,"entry":store.propose(scope,&args.key,&args.content,args.tags,args.metadata.unwrap_or_default(),args.expected_revision,session,execution,call)?}),
        ),
        "list" => {
            ensure!(
                (1..=16).contains(&args.limit),
                "Memory list limit must be in 1..=16"
            );
            let entries = store
                .list(scope)?
                .into_iter()
                .filter(|entry| args.status.is_none_or(|status| entry.status == status))
                .collect::<Vec<_>>();
            ensure!(
                args.offset <= entries.len(),
                "Memory offset exceeds result length"
            );
            let mut page = Vec::new();
            let mut bytes = 0;
            for entry in entries.iter().skip(args.offset).take(args.limit) {
                let size = serde_json::to_vec(entry)?.len();
                if bytes + size > 16384 {
                    break;
                }
                bytes += size;
                page.push(entry);
            }
            let next = args.offset + page.len();
            ensure!(
                next > args.offset || args.offset == entries.len(),
                "One memory exceeds the model page budget; inspect it with the local learning CLI"
            );
            Ok(
                json!({"store":store.directory(),"scope":scope,"entries":page,"total":entries.len(),"next_offset":(next<entries.len()).then_some(next)}),
            )
        }
        "delete" => Ok(
            json!({"deleted":store.delete_checked(scope,&args.key,args.expected_revision.context("delete requires expected_revision")?)?}),
        ),
        _ => anyhow::bail!("Invalid memory action"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ExecutionContext;

    #[tokio::test]
    async fn experience_can_create_only_a_hidden_draft_with_exact_scoped_sources() {
        let root = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            SkillManager::new(kyler_config::SkillSettings {
                user_directory: root.path().join("skills"),
                state_file: root.path().join("skills-state.json"),
                ..Default::default()
            })
            .unwrap(),
        );
        let store = Arc::new(MemoryStore::new(root.path().join("memory")));
        let scope = "a".repeat(64);
        store
            .propose(
                &scope,
                "lesson",
                "Use exact revisions",
                vec![],
                Default::default(),
                None,
                "source",
                "source-execution",
                "source-call",
            )
            .unwrap();
        let tool = MemoryTool {
            store: store.clone(),
            scope: scope.clone(),
            skills: Some(manager.clone()),
        };
        let mut execution = ExecutionContext::new();
        let mut state = Default::default();
        let mut context = ToolContext {
            processes: None,
            execution: &mut execution,
            state: &mut state,
            session_id: "session",
            tool_call_id: "draft-call",
            subagents: None,
            environment: None,
        };
        let args = |revision| {
            json!({"action":"draft_skill","name":"workflow","instructions":"---\nname: workflow\ndescription: A reusable workflow\n---\nValidate first", "sources":[{"key":"lesson","revision":revision}]}).to_string()
        };
        assert!(tool.execute(&args(0), &mut context).await.is_err());
        tool.execute(&args(1), &mut context).await.unwrap();
        assert!(
            !manager
                .snapshot(None)
                .await
                .unwrap()
                .available
                .contains_key("workflow")
        );
        let draft = manager.draft_details("workflow").await.unwrap();
        let provenance: Value =
            serde_json::from_str(&draft.files["references/learning-source.json"]).unwrap();
        assert_eq!(provenance["sources"][0]["scope"], scope);
        assert_eq!(provenance["sources"][0]["source_session"], "source");
        assert!(store.search(&scope, "").unwrap().is_empty());
    }
}
