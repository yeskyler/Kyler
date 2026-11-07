//! Durable clarification requests shared by all clients.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionType {
    SingleSelect,
    MultiSelect,
    Text,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub recommended: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub question: String,
    #[serde(rename = "type")]
    pub kind: QuestionType,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskQuestionArgs {
    pub questions: Vec<Question>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionAnswer {
    #[serde(default)]
    pub selected: Vec<String>,
    #[serde(default)]
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuestionResponse {
    Submit {
        answers: BTreeMap<String, QuestionAnswer>,
    },
    Cancel,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStatus {
    Pending,
    Answered,
    Dismissed,
    Cancelled,
    Interrupted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRecord {
    pub id: String,
    pub session_id: String,
    pub execution_id: String,
    pub tool_call_id: String,
    pub questions: Vec<Question>,
    pub status: QuestionStatus,
    pub answers: BTreeMap<String, QuestionAnswer>,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}
