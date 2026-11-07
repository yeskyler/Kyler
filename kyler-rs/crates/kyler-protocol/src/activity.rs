use serde::{Deserialize, Serialize};

/// Presentation category, independent of execution permissions and sandbox policy.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Explore,
    Execute,
    Edit,
    Search,
    #[default]
    Think,
}

impl ActivityKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Explore => "Explore",
            Self::Execute => "Execute",
            Self::Edit => "Edit",
            Self::Search => "Search",
            Self::Think => "Think",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolSource {
    Skill {
        name: String,
        fingerprint: String,
    },
    #[default]
    Builtin,
    Mcp {
        server_id: String,
        remote_name: String,
    },
}
