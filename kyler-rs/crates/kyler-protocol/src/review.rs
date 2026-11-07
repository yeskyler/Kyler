//! Provider-independent review targets and validated report representation.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    #[default]
    General,
    Review,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewTarget {
    Uncommitted,
    BaseBranch { branch: String },
    Commit { commit: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    pub target: ReviewTarget,
    /// Optional workspace-relative files or directories. Empty means the workspace.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Include non-ignored untracked files in uncommitted reviews.
    #[serde(default = "include_untracked")]
    pub include_untracked: bool,
}
fn include_untracked() -> bool {
    true
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAssessment {
    IssuesFound,
    NoFindings,
    Inconclusive,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCoverageStatus {
    Complete,
    Partial,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ReviewPriority {
    P0,
    P1,
    P2,
    P3,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewLocation {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewFinding {
    pub priority: ReviewPriority,
    pub title: String,
    pub explanation: String,
    pub location: ReviewLocation,
    pub confidence: f64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewCoverage {
    pub status: ReviewCoverageStatus,
    pub limitations: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewReport {
    pub assessment: ReviewAssessment,
    pub summary: String,
    pub findings: Vec<ReviewFinding>,
    pub coverage: ReviewCoverage,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSnapshot {
    pub target: ReviewTarget,
    pub head: String,
    pub base: Option<String>,
    /// None denotes raw working-tree content; otherwise an immutable commit ID.
    pub revision: Option<String>,
    pub fingerprint: String,
    pub files: Vec<String>,
    pub stale: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewResult {
    pub report: ReviewReport,
    pub snapshot: ReviewSnapshot,
}
