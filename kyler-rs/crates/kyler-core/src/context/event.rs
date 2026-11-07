use crate::attachments::image::UserImage;

use std::fmt::{Display, Formatter};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use kyler_protocol::api::attachments::AttachmentInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

impl Display for Role {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventAuthor {
    pub name: String,
}

impl Default for EventAuthor {
    fn default() -> Self {
        Self {
            name: kyler_protocol::agents::PRIMARY_AGENT_NAME.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentItem {
    #[serde(rename = "attachment")]
    Attachment { attachment: AttachmentInfo },
    #[serde(rename = "image")]
    Image { image: UserImage },
    #[serde(rename = "reasoning")]
    Reasoning { text: String },
    #[serde(rename = "message")]
    Message { role: Role, content: String },

    #[serde(rename = "tool_call")]
    ToolCall {
        tool_call_id: String,
        name: String,
        arguments: Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_call_id: String,
        name: String,
        status: ToolResultStatus,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_content: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub execution_id: String,
    pub timestamp: i64,
    pub author: EventAuthor,
    pub content: Vec<ContentItem>,
}

impl Event {
    /// Projection for model requests and summaries; never serializes both copies.
    pub fn model_view(&self) -> Self {
        Self {
            id: self.id.clone(),
            execution_id: self.execution_id.clone(),
            timestamp: self.timestamp,
            author: self.author.clone(),
            content: self
                .content
                .iter()
                .filter(|item| !matches!(item, ContentItem::Reasoning { .. }))
                .map(|item| match item {
                    ContentItem::ToolResult {
                        tool_call_id,
                        name,
                        status,
                        content,
                        model_content,
                    } => ContentItem::ToolResult {
                        tool_call_id: tool_call_id.clone(),
                        name: name.clone(),
                        status: *status,
                        content: model_content.as_ref().unwrap_or(content).clone(),
                        model_content: None,
                    },
                    ContentItem::Attachment { attachment } => ContentItem::Message {
                        role: Role::User,
                        content: format!("[Attachment: {} id={}]", attachment.name, attachment.id),
                    },
                    ContentItem::Image { image } => ContentItem::Message {
                        role: Role::User,
                        content: format!(
                            "[Image: {} ({}x{})]",
                            image.name, image.width, image.height
                        ),
                    },
                    other => other.clone(),
                })
                .collect(),
        }
    }

    pub fn new(
        execution_id: impl Into<String>,
        author: EventAuthor,
        content: Vec<ContentItem>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            execution_id: execution_id.into(),
            timestamp: chrono::Utc::now().timestamp(),
            author,
            content,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentItem, Event, EventAuthor, ToolResultStatus};

    #[test]
    fn legacy_tool_results_load_and_new_results_preserve_original() {
        let original = r#"{"type":"tool_result","tool_call_id":"a","name":"read","status":"success","content":"original"}"#;
        let legacy: ContentItem = serde_json::from_str(original).unwrap();
        assert!(matches!(
            legacy,
            ContentItem::ToolResult {
                model_content: None,
                ..
            }
        ));
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("model_content")
        );
        let event = Event::new(
            "run",
            EventAuthor {
                name: "agent".into(),
            },
            vec![ContentItem::ToolResult {
                tool_call_id: "a".into(),
                name: "read".into(),
                status: ToolResultStatus::Success,
                content: "original".into(),
                model_content: Some("compact".into()),
            }],
        );
        assert!(serde_json::to_string(&event).unwrap().contains("original"));
        let projected = serde_json::to_string(&event.model_view()).unwrap();
        assert!(projected.contains("compact"));
        assert!(!projected.contains("original"));
    }
}
