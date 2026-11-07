//! Application configuration: typed defaults, explicit loading, overrides and validation.
//! Does not initialize logging, mutate global environment variables, or start runtime services.
pub mod app;
pub mod loader;
pub mod mcp;
pub mod models;
pub mod sandbox;
pub mod skills;
pub mod validation;
pub mod web;
pub mod workspace;

pub use app::{
    AgentSettings, AppConfig, ClientSettings, ContextSettings, LlmSettings, LoggingConfig,
    SecretString, ServerSettings, SessionSettings, SubagentSettings,
};
pub use loader::{load, load_from};
pub use mcp::{McpServerSettings, McpSettings, McpTransport};
pub use models::{ModelProfile, ModelSettings, OutputTokenParameter, ProviderSettings};
pub use sandbox::SandboxSettings;
pub use skills::SkillSettings;
pub use web::WebSettings;
pub use workspace::WorkspaceSettings;
