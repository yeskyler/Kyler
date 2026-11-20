//! Transport-independent gateway identity, delegation and audit contracts.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Admin,
    #[default]
    Member,
    Viewer,
}
impl Role {
    pub fn administrative(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
    pub fn executes(self) -> bool {
        self != Self::Viewer
    }
}

/// Sessions require ownership or an explicit session grant. Nonempty workspace and
/// model lists further constrain access; scoped creation requires a workspace grant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scope {
    pub workspaces: Vec<String>,
    pub sessions: Vec<String>,
    /// Catalog profile IDs, not provider model names.
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateToken {
    pub name: String,
    #[serde(default)]
    pub role: Role,
    #[serde(default)]
    pub scope: Scope,
    /// Unix milliseconds. None is explicitly non-expiring.
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub id: String,
    pub principal_id: String,
    pub name: String,
    pub role: Role,
    pub scope: Scope,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Principal {
    pub id: String,
    pub token_id: Option<String>,
    pub role: Role,
    pub scope: Scope,
}
impl Principal {
    /// Only trusted in-process hosts may construct this identity; never deserialize it.
    pub fn local() -> Self {
        Self {
            id: "host:local".into(),
            token_id: None,
            role: Role::Owner,
            scope: Scope::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub id: i64,
    pub request_id: String,
    pub principal_id: Option<String>,
    pub token_id: Option<String>,
    pub action: String,
    pub resource: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: Option<u16>,
}
