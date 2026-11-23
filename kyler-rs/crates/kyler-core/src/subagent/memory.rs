//! Extract reviewable memory from the parent session's committed history.
use crate::{
    Agent, ExecutionEnvironment,
    context::{ContentItem, Role},
    memory::{MemoryKind, MemoryMetadata, MemoryStatus, MemoryStore},
    rollout::{RolloutPayload, RolloutReader},
    session::{Session, SessionStore},
    subagent::{ParentSessionRef, SubagentExecutor},
};

use std::{collections::VecDeque, sync::Arc};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

const MAX_EVENTS: usize = 32;
const MAX_MESSAGE_BYTES: usize = 1200;
const MAX_HISTORY_BYTES: usize = 16 * 1024;
const MAX_CANDIDATES: usize = 6;

pub(crate) struct MemoryExecutor {
    child: Agent,
    environment: Arc<ExecutionEnvironment>,
    history: Arc<SessionStore>,
    memory: Arc<MemoryStore>,
}

#[derive(Serialize)]
struct Evidence {
    seq: u64,
    execution: String,
    messages: Vec<(Role, String)>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Fact,
    Preference,
    Procedure,
}
impl From<Kind> for MemoryKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Fact => Self::Fact,
            Kind::Preference => Self::Preference,
            Kind::Procedure => Self::Procedure,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    kind: Kind,
    content: String,
    conditions: String,
    evidence: Vec<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    candidates: Vec<Candidate>,
}

// Drop secret-shaped messages instead of passing their raw text to the model.
fn eligible(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ![
        "authorization:",
        "bearer ",
        "api_key",
        "api key",
        "api-key",
        "apikey",
        "password",
        "secret",
        "access_token",
        "token:",
        "token=",
        "private key",
        "sk-",
        "ghp_",
        "github_pat_",
        "akia",
        "密码",
        "密钥",
        "令牌",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        && !text
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
            .any(|word| word.len() >= 32 && word.bytes().any(|byte| byte.is_ascii_digit()))
}

fn history(mut reader: RolloutReader, parent_execution: &str) -> Result<Vec<Evidence>> {
    let mut selected = VecDeque::<(usize, Evidence)>::new();
    let mut bytes = 0;
    while let Some(record) = reader.read_next()? {
        let RolloutPayload::EventRecorded { event } = record.payload else {
            continue;
        };
        if event.execution_id == parent_execution {
            continue;
        }
        let messages: Vec<_> = event
            .content
            .into_iter()
            .filter_map(|item| match item {
                ContentItem::Message { role, content }
                    if !content.trim().is_empty() && eligible(&content) =>
                {
                    let end = content.floor_char_boundary(MAX_MESSAGE_BYTES.min(content.len()));
                    Some((role, content[..end].to_owned()))
                }
                _ => None,
            })
            .take(2)
            .collect();
        if messages.is_empty() {
            continue;
        }
        let entry = Evidence {
            seq: record.seq,
            execution: event.execution_id,
            messages,
        };
        let size = serde_json::to_vec(&entry)?.len();
        if size > MAX_HISTORY_BYTES {
            continue;
        }
        while selected.len() == MAX_EVENTS || bytes + size > MAX_HISTORY_BYTES {
            if let Some((removed, _)) = selected.pop_front() {
                bytes -= removed;
            }
        }
        selected.push_back((size, entry));
        bytes += size;
    }
    Ok(selected.into_iter().map(|(_, entry)| entry).collect())
}

fn validate(report: &Report, history: &[Evidence]) -> Result<()> {
    ensure!(
        report.candidates.len() <= MAX_CANDIDATES,
        "Too many memory candidates"
    );
    for candidate in &report.candidates {
        ensure!(
            !candidate.content.trim().is_empty()
                && candidate.content.len() <= 900
                && candidate.conditions.len() <= 400
                && eligible(&candidate.content)
                && eligible(&candidate.conditions),
            "Memory candidate is too large or contains sensitive content"
        );
        ensure!(
            (1..=4).contains(&candidate.evidence.len())
                && candidate
                    .evidence
                    .iter()
                    .all(|id| history.iter().any(|entry| entry.seq == *id)),
            "Memory candidate references unavailable history"
        );
        if matches!(candidate.kind, Kind::Preference) {
            ensure!(
                history.iter().any(|entry| {
                    candidate.evidence.contains(&entry.seq)
                        && entry.messages.iter().any(|(role, _)| *role == Role::User)
                }),
                "Memory preference must cite a user message"
            );
        }
    }
    Ok(())
}

fn persist(
    store: &MemoryStore,
    scope: &str,
    owner: &ParentSessionRef,
    report: Report,
    history: &[Evidence],
) -> Result<String> {
    let mut saved = Vec::new();
    let mut skipped = 0;
    let mut failed = 0;
    for candidate in report.candidates {
        let kind = MemoryKind::from(candidate.kind);
        let fingerprint = Sha256::digest(serde_json::to_vec(&(
            kind,
            candidate.content.trim(),
            candidate.conditions.trim(),
        ))?);
        let key = format!("history-{fingerprint:x}");
        let mut cited: Vec<_> = history
            .iter()
            .filter(|entry| candidate.evidence.contains(&entry.seq))
            .collect();
        cited.sort_by_key(|entry| entry.seq);
        let tags = if kind == MemoryKind::Preference {
            vec!["preference".into()]
        } else {
            Vec::new()
        };
        let metadata = MemoryMetadata {
            kind,
            conditions: candidate.conditions,
            evidence: cited
                .iter()
                .map(|entry| {
                    format!(
                        "rollout:{}:{}:{}",
                        owner.session_id, entry.execution, entry.seq
                    )
                })
                .collect(),
            ..Default::default()
        };
        let entry = match store.propose(
            scope,
            &key,
            candidate.content.trim(),
            tags,
            metadata,
            None,
            &owner.session_id,
            &owner.execution_id,
            &owner.tool_call_id,
        ) {
            Ok(entry) => Some(entry),
            Err(_) => {
                let existing = store
                    .list(scope)?
                    .into_iter()
                    .find(|entry| entry.key == key);
                if existing.is_some() {
                    skipped += 1;
                } else {
                    failed += 1;
                }
                existing.filter(|entry| {
                    matches!(entry.status, MemoryStatus::Active | MemoryStatus::Candidate)
                        && !entry.metadata.sensitive
                        && entry
                            .metadata
                            .expires_at
                            .is_none_or(|expires| expires > chrono::Utc::now().timestamp_millis())
                })
            }
        };
        if let Some(entry) = entry
            && !saved
                .iter()
                .any(|row: &serde_json::Value| row["key"] == entry.key)
        {
            saved.push(json!({
                "key":entry.key,"revision":entry.revision,"kind":kind,
                "content":entry.content,"conditions":entry.metadata.conditions
            }));
        }
    }
    Ok(json!({"candidates":saved,"duplicates_skipped":skipped,"writes_failed":failed}).to_string())
}

impl MemoryExecutor {
    pub(crate) fn new(
        agent: &Agent,
        history: Arc<SessionStore>,
        memory: Arc<MemoryStore>,
    ) -> Result<Self> {
        let environment = Arc::new(
            agent
                .execution_environment()
                .context("Memory analysis requires an authorized workspace")?
                .read_only(),
        );
        Ok(Self {
            child: agent.memory_runtime(environment.clone()),
            environment,
            history,
            memory,
        })
    }
}

#[async_trait::async_trait]
impl SubagentExecutor for MemoryExecutor {
    fn session_environment(&self) -> Option<crate::environment::SessionEnvironment> {
        Some(self.environment.session_environment().clone())
    }

    async fn execute(&self, session: &mut Session, input: &str) -> Result<String> {
        let owner = session.parent.clone().context("Missing parent session")?;
        let scope = MemoryStore::scope(session);
        let reader = self.history.rollout(&owner.session_id).await?;
        let execution = owner.execution_id.clone();
        let evidence = tokio::task::spawn_blocking(move || history(reader, &execution)).await??;
        if evidence.is_empty() {
            return Ok(json!({"candidates":[],"note":"No eligible prior messages in the committed session history."}).to_string());
        }
        let focus: serde_json::Value = serde_json::from_str(input)?;
        let focus = focus["task"]
            .as_str()
            .filter(|text| text.len() <= 1000 && eligible(text));
        let request = json!({"focus":focus,"history":evidence}).to_string();
        let result = self.child.run_in_session(session, &request).await?;
        ensure!(
            result.output.len() <= 24000,
            "Memory report exceeds the output limit"
        );
        let report: Report = serde_json::from_str(&result.output)
            .map_err(|_| anyhow::anyhow!("Memory report did not match the required JSON schema"))?;
        validate(&report, &evidence)?;
        let store = self.memory.clone();
        tokio::task::spawn_blocking(move || persist(&store, &scope, &owner, report, &evidence))
            .await?
    }
}
