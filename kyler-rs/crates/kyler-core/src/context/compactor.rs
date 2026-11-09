//! Select complete interaction groups and summarize evicted history.
use super::{Summarizer, policy::ContextPolicy};
use crate::{
    context::{ContentItem, Event},
    session::Session,
};

use std::collections::HashSet;

use anyhow::{Context, ensure};

use kyler_protocol::{api::models::Tokenizer, prompts::SUMMARY};

pub struct ContextCompactor {
    policy: ContextPolicy,
    tokenizer: Tokenizer,
}
impl ContextCompactor {
    pub fn new(policy: ContextPolicy) -> anyhow::Result<Self> {
        policy.validate()?;
        Ok(Self {
            policy,
            tokenizer: Default::default(),
        })
    }
    pub fn with_tokenizer(mut self, tokenizer: Tokenizer) -> Self {
        self.tokenizer = tokenizer;
        self
    }
    /// UTF-8 bytes provide a conservative estimate for byte-based text tokenizers.
    /// JSON counts tool schemas, call arguments and message framing too.
    pub fn count(&self, value: &(impl serde::Serialize + ?Sized)) -> anyhow::Result<usize> {
        // Image tokenization is provider-specific. Reserve a conservative visual budget
        // instead of charging Base64 characters as text tokens.
        fn project(value: &mut serde_json::Value, images: &mut usize) {
            match value {
                serde_json::Value::Array(items) => {
                    for item in items {
                        project(item, images);
                    }
                }
                serde_json::Value::Object(items) => {
                    if let Some(url) = items
                        .get_mut("image_url")
                        .and_then(|image| image.get_mut("url"))
                    {
                        if url
                            .as_str()
                            .is_some_and(|text| text.starts_with("data:image/"))
                        {
                            *images += 1;
                            *url = serde_json::Value::String("[image]".into());
                        }
                    }
                    for item in items.values_mut() {
                        project(item, images);
                    }
                }
                _ => {}
            }
        }
        let mut value = serde_json::to_value(value)?;
        let mut images = 0;
        project(&mut value, &mut images);
        let text = serde_json::to_string(&value)?;
        let tokens = match self.tokenizer {
            Tokenizer::Conservative => text.len(),
            Tokenizer::Cl100kBase => tiktoken_rs::cl100k_base_singleton()
                .encode_ordinary(&text)
                .len(),
            Tokenizer::O200kBase => tiktoken_rs::o200k_base_singleton()
                .encode_ordinary(&text)
                .len(),
        };
        Ok(tokens + 32 + images * 4096)
    }
    pub fn input_limit(&self) -> usize {
        self.policy.input_limit()
    }

    pub async fn prepare<S, F>(
        &self,
        context: &mut Session,
        summarizer: &mut S,
        count_request: F,
    ) -> anyhow::Result<()>
    where
        S: Summarizer,
        F: Fn(&Session) -> anyhow::Result<usize>,
    {
        let before = count_request(context)?;
        if before <= self.input_limit() {
            context.memory.last_input_tokens = before;
            return Ok(());
        }
        // Work on a candidate: failed summary/validation cannot discard the active history.
        let mut candidate = context.clone();
        let start = candidate.memory.window_start.min(candidate.events.len());
        let question = context.memory.current_question_index;
        // Previous pinned questions join the next summary instead of disappearing
        // when the active question changes on a new turn.
        candidate.memory.retained_questions.clear();
        let groups = history_groups(&candidate.events, start)?;
        let window_budget = self.policy.sliding_window_tokens();
        let mut window_size = 0;
        let mut cut = candidate.events.len();
        for &(a, b) in groups.iter().rev() {
            let group: Vec<_> = candidate.events[a..b]
                .iter()
                .map(Event::model_view)
                .filter(|event| !event.content.is_empty())
                .collect();
            let size = self.count(&group)?;
            if window_size + size > window_budget {
                break;
            }
            window_size += size;
            cut = a;
        }
        // Remove enough complete groups to fit fixed content + a worst-case summary.
        candidate.memory.summary = Some("x".repeat(self.policy.summary_tokens));
        candidate.memory.window_start = cut;
        while count_request(&candidate)? > self.input_limit() {
            match groups.iter().find(|(a, _)| *a >= cut) {
                Some(&(_, end)) => {
                    cut = end;
                    candidate.memory.window_start = cut;
                }
                None => anyhow::bail!(
                    "fixed question, instructions, plan and tool definitions exceed context budget"
                ),
            }
        }
        let mut summary = context.memory.summary.clone().unwrap_or_default();
        ensure!(
            cut > start || !summary.is_empty(),
            "no history available to compress"
        );
        let mut removed_indices: Vec<_> = context
            .memory
            .retained_questions
            .iter()
            .copied()
            .filter(|&i| i < start && i != question)
            .collect();
        removed_indices.extend((start..cut).filter(|&i| i != question));
        removed_indices.sort_unstable();
        removed_indices.dedup();
        let removed: Vec<_> = removed_indices
            .iter()
            .map(|&i| candidate.events[i].model_view())
            .filter(|event| !event.content.is_empty())
            .collect();
        let history = serde_json::to_string(&removed)?;
        // Bound every summary request too. Large individual tool results are chunked as
        // historical text here; the live tool call/result protocol is never split.
        let chunk_budget = self.policy.context_tokens
            - self.policy.safety_tokens
            - self.policy.summary_tokens * 2
            - SUMMARY.len()
            - 1024;
        ensure!(chunk_budget > 0, "insufficient summary input budget");
        let chunks = chunks(&history, chunk_budget);
        for chunk in chunks {
            // JSON escaping can expand the fragment. Split recursively before any request.
            let mut pending = vec![chunk.to_string()];
            while let Some(fragment) = pending.pop() {
                let input = serde_json::json!({ "previous_summary": summary, "history_fragment": fragment }).to_string();
                if input.len()
                    + SUMMARY.len()
                    + self.policy.summary_tokens
                    + self.policy.safety_tokens
                    + 256
                    > self.policy.context_tokens
                {
                    ensure!(
                        fragment.chars().count() > 1,
                        "summary request cannot fit context budget"
                    );
                    let mid = fragment
                        .char_indices()
                        .nth(fragment.chars().count() / 2)
                        .unwrap()
                        .0;
                    pending.push(fragment[mid..].to_string());
                    pending.push(fragment[..mid].to_string());
                    continue;
                }
                let result = summarizer
                    .summarize(&input, self.policy.summary_tokens as u32)
                    .await
                    .context("context summarization failed")?;
                if let Some((p, c, t)) = result.usage {
                    context.add_usage(p, c, t);
                }
                ensure!(!result.text.trim().is_empty(), "empty context summary");
                ensure!(
                    result.text.len() <= self.policy.summary_tokens,
                    "summary exceeds configured conservative token budget"
                );
                summary = result.text;
            }
        }
        candidate.memory.summary = Some(summary);
        candidate.memory.window_start = cut;
        if question < cut {
            candidate.memory.retained_questions.push(question);
        }
        candidate.memory.last_input_tokens = count_request(&candidate)?;
        ensure!(
            candidate.memory.last_input_tokens <= self.input_limit(),
            "compressed request exceeds context budget"
        );
        candidate.memory.compression_count += 1;
        tracing::info!(
            before,
            after = candidate.memory.last_input_tokens,
            window_start = cut,
            "context compressed"
        );
        context.memory = candidate.memory;
        Ok(())
    }
}

/// Find safe boundaries, including calls/results spread across several events.
fn history_groups(events: &[Event], start: usize) -> anyhow::Result<Vec<(usize, usize)>> {
    let mut groups = Vec::new();
    let mut pending = HashSet::new();
    let mut begin = start;
    for (index, event) in events.iter().enumerate().skip(start) {
        for item in &event.content {
            match item {
                ContentItem::ToolCall { tool_call_id, .. } => {
                    ensure!(
                        pending.insert(tool_call_id.clone()),
                        "duplicate tool call ID"
                    );
                }
                ContentItem::ToolResult { tool_call_id, .. } => {
                    ensure!(
                        pending.remove(tool_call_id),
                        "tool result has no matching call"
                    );
                }
                _ => {}
            }
        }
        if pending.is_empty() {
            groups.push((begin, index + 1));
            begin = index + 1;
        }
    }
    ensure!(
        pending.is_empty(),
        "cannot prepare context with pending tool calls"
    );
    Ok(groups)
}
fn chunks(text: &str, max_bytes: usize) -> Vec<&str> {
    let mut result = Vec::new();
    let mut rest = text;
    while rest.len() > max_bytes {
        let mut end = max_bytes;
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            end = rest.char_indices().nth(1).map_or(rest.len(), |(i, _)| i);
        }
        result.push(&rest[..end]);
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        result.push(rest);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        super::{ContextPolicy, Summarizer},
        ContextCompactor, chunks, history_groups,
    };
    use crate::{
        context::{
            ContentItem, Event, EventAuthor, ExecutionStatus, Role, SummaryResult, ToolResultStatus,
        },
        session::Session,
    };

    use anyhow::ensure;

    fn event(items: Vec<ContentItem>) -> Event {
        Event::new(
            "test",
            EventAuthor {
                name: "test".into(),
            },
            items,
        )
    }
    fn context() -> Session {
        let mut c = Session::new();
        c.add_event(event(vec![ContentItem::Message {
            role: Role::User,
            content: "original question".into(),
        }]));
        for i in 0..8 {
            c.add_event(event(vec![ContentItem::ToolCall {
                tool_call_id: i.to_string(),
                name: "read".into(),
                arguments: serde_json::json!({}),
            }]));
            c.add_event(event(vec![ContentItem::ToolResult {
                tool_call_id: i.to_string(),
                name: "read".into(),
                status: ToolResultStatus::Success,
                model_content: None,
                content: "data".repeat(400),
            }]));
        }
        c
    }
    fn manager() -> ContextCompactor {
        ContextCompactor::new(ContextPolicy {
            context_tokens: 8000,
            output_tokens: 500,
            safety_tokens: 256,
            summary_tokens: 256,
            ..Default::default()
        })
        .unwrap()
    }
    fn size(c: &Session) -> anyhow::Result<usize> {
        let visible: Vec<_> = c
            .visible_event_indices()
            .iter()
            .map(|&i| &c.events[i])
            .collect();
        Ok(serde_json::to_vec(&visible)?.len() + c.memory.summary.as_ref().map_or(0, String::len))
    }
    struct Fake {
        fail: bool,
        calls: usize,
    }
    #[async_trait::async_trait]
    impl Summarizer for Fake {
        async fn summarize(&mut self, input: &str, _: u32) -> anyhow::Result<SummaryResult> {
            self.calls += 1;
            ensure!(!self.fail, "offline");
            assert!(input.contains("previous_summary"));
            Ok(SummaryResult {
                text: "Facts and progress".into(),
                usage: Some((10, 5, 15)),
            })
        }
    }
    #[tokio::test]
    async fn compression_preserves_question_events_and_complete_groups() {
        let m = manager();
        let mut c = context();
        let original = serde_json::to_string(&c.events).unwrap();
        let mut fake = Fake {
            fail: false,
            calls: 0,
        };
        m.prepare(&mut c, &mut fake, size).await.unwrap();
        assert_eq!(original, serde_json::to_string(&c.events).unwrap());
        assert!(c.memory.window_start > 1);
        assert_eq!(c.memory.window_start % 2, 1);
        assert!(size(&c).unwrap() <= m.input_limit());
        assert!(c.usage.total_tokens > 0);
        let calls = fake.calls;
        m.prepare(&mut c, &mut fake, size).await.unwrap();
        assert_eq!(calls, fake.calls);
    }
    #[tokio::test]
    async fn failed_summary_does_not_move_window() {
        let mut c = context();
        assert!(
            manager()
                .prepare(
                    &mut c,
                    &mut Fake {
                        fail: true,
                        calls: 0
                    },
                    size
                )
                .await
                .is_err()
        );
        assert_eq!(c.memory.window_start, 0);
        assert!(c.memory.summary.is_none());
    }
    #[tokio::test]
    async fn fixed_content_overflow_fails_before_summary() {
        let mut c = context();
        let mut fake = Fake {
            fail: false,
            calls: 0,
        };
        assert!(
            manager()
                .prepare(&mut c, &mut fake, |c| Ok(size(c)? + 9000))
                .await
                .is_err()
        );
        assert_eq!(fake.calls, 0);
    }
    #[test]
    fn rejects_incomplete_parallel_call_group() {
        let mut c = context();
        c.events.pop();
        assert!(history_groups(&c.events, 1).is_err());
    }
    #[test]
    fn utf8_chunks_are_lossless_and_bounded() {
        let text = "\u{4e2d}\u{6587}abcdef".repeat(100);
        let parts = chunks(&text, 19);
        assert_eq!(parts.concat(), text);
        assert!(parts.iter().all(|s| s.len() <= 19));
    }
    #[tokio::test]
    async fn later_compression_merges_previous_summary() {
        let mut c = context();
        let m = manager();
        let mut fake = Fake {
            fail: false,
            calls: 0,
        };
        m.prepare(&mut c, &mut fake, size).await.unwrap();
        let previous_start = c.memory.window_start;
        for i in 8..16 {
            c.add_event(event(vec![ContentItem::ToolCall {
                tool_call_id: i.to_string(),
                name: "read".into(),
                arguments: serde_json::json!({}),
            }]));
            c.add_event(event(vec![ContentItem::ToolResult {
                tool_call_id: i.to_string(),
                name: "read".into(),
                status: ToolResultStatus::Success,
                model_content: None,
                content: "data".repeat(400),
            }]));
        }
        struct Merge;
        #[async_trait::async_trait]
        impl Summarizer for Merge {
            async fn summarize(&mut self, input: &str, _: u32) -> anyhow::Result<SummaryResult> {
                assert!(input.contains("Facts and progress"));
                Ok(SummaryResult {
                    text: "Facts and progress, updated".into(),
                    usage: None,
                })
            }
        }
        m.prepare(&mut c, &mut Merge, size).await.unwrap();
        assert!(c.memory.window_start > previous_start);
        assert_eq!(c.memory.compression_count, 2);
    }

    #[tokio::test]
    async fn huge_single_result_is_summarized_in_bounded_requests() {
        let mut c = context();
        c.events.truncate(3);
        if let ContentItem::ToolResult { content, .. } = &mut c.events[2].content[0] {
            *content = "\u{4e2d}\u{6587}\\\"".repeat(10000);
        }
        let mut fake = Fake {
            fail: false,
            calls: 0,
        };
        manager().prepare(&mut c, &mut fake, size).await.unwrap();
        assert_eq!(c.memory.window_start, 3);
        assert!(fake.calls > 1);
    }

    #[tokio::test]
    async fn prior_pinned_question_survives_next_turn_until_summarized() {
        let mut session = Session::new();
        session.begin_execution("FIRST_QUESTION").unwrap();
        let append = |s: &mut Session, id: &str| {
            let execution_id = s.current_execution().unwrap().execution_id.clone();
            s.add_event(Event::new(
                execution_id.clone(),
                EventAuthor {
                    name: "agent".into(),
                },
                vec![ContentItem::ToolCall {
                    tool_call_id: id.into(),
                    name: "read".into(),
                    arguments: serde_json::json!({}),
                }],
            ));
            s.add_event(Event::new(
                execution_id,
                EventAuthor {
                    name: "agent".into(),
                },
                vec![ContentItem::ToolResult {
                    tool_call_id: id.into(),
                    name: "read".into(),
                    status: ToolResultStatus::Success,
                    model_content: None,
                    content: "large result".repeat(1000),
                }],
            ));
        };
        struct Capture {
            inputs: String,
        }
        #[async_trait::async_trait]
        impl Summarizer for Capture {
            async fn summarize(&mut self, input: &str, _: u32) -> anyhow::Result<SummaryResult> {
                self.inputs.push_str(input);
                Ok(SummaryResult {
                    text: "facts".into(),
                    usage: None,
                })
            }
        }
        append(&mut session, "first");
        let mut first = Capture {
            inputs: String::new(),
        };
        manager()
            .prepare(&mut session, &mut first, size)
            .await
            .unwrap();
        assert!(!first.inputs.contains("FIRST_QUESTION"));
        assert_eq!(session.memory.retained_questions, vec![0]);
        session
            .finish(ExecutionStatus::Completed, Some("done".into()), None)
            .unwrap();
        session.begin_execution("SECOND_QUESTION").unwrap();
        assert!(session.visible_event_indices().contains(&0));
        append(&mut session, "second");
        let mut second = Capture {
            inputs: String::new(),
        };
        manager()
            .prepare(&mut session, &mut second, size)
            .await
            .unwrap();
        assert!(second.inputs.contains("FIRST_QUESTION"));
        assert!(!second.inputs.contains("SECOND_QUESTION"));
        assert!(!session.visible_event_indices().contains(&0));
        assert!(
            session
                .visible_event_indices()
                .contains(&session.memory.current_question_index)
        );
        session.validate().unwrap();
    }

    #[tokio::test]
    async fn summaries_use_compact_content_without_reintroducing_original() {
        let mut c = context();
        for event in &mut c.events {
            for item in &mut event.content {
                if let ContentItem::ToolResult {
                    content,
                    model_content,
                    ..
                } = item
                {
                    *content = "RAW_SECRET_MARKER".repeat(1000);
                    *model_content = Some("[Partial result; omitted raw content]".into());
                }
            }
        }
        struct Check;
        #[async_trait::async_trait]
        impl Summarizer for Check {
            async fn summarize(&mut self, input: &str, _: u32) -> anyhow::Result<SummaryResult> {
                assert!(!input.contains("RAW_SECRET_MARKER"));
                assert!(input.contains("Partial result"));
                Ok(SummaryResult {
                    text: "summary".into(),
                    usage: None,
                })
            }
        }
        // Fixed overhead forces eviction even though the projected history is small.
        manager()
            .prepare(&mut c, &mut Check, |s| Ok(size(s)? + 5800))
            .await
            .unwrap();
        assert!(
            serde_json::to_string(&c.events)
                .unwrap()
                .contains("RAW_SECRET_MARKER")
        );
    }

    #[test]
    fn invalid_policy_is_rejected() {
        assert!(
            ContextCompactor::new(ContextPolicy {
                window_ratio: 0.9,
                ..Default::default()
            })
            .is_err()
        );
    }
}

#[cfg(test)]
mod tokenizer_tests {
    use super::{super::ContextPolicy, ContextCompactor};

    #[test]
    fn declared_tokenizers_count_text_without_charging_base64_as_text() {
        let counter = ContextCompactor::new(ContextPolicy::default())
            .unwrap()
            .with_tokenizer(kyler_protocol::api::models::Tokenizer::Cl100kBase);
        let text = "hello world ".repeat(100);
        let value = serde_json::json!({"content":text});
        let tokens = counter.count(&value).unwrap();
        assert!(tokens < text.len() / 2);
        let image = serde_json::json!({"type":"image_url","image_url":{"url":format!("data:image/png;base64,{}","A".repeat(100000))}});
        assert!(counter.count(&image).unwrap() < 5000);
    }
}
