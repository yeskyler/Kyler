use std::{convert::Infallible, path::PathBuf};

use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};

use kyler_config::SecretString;

#[derive(Debug, Parser)]
#[command(name = "kyler", version, about = "Kyler assistant client and server")]
pub struct Args {
    /// Server URL (overrides configuration and environment).
    #[arg(long, global = true)]
    pub server: Option<String>,
    /// Configuration file (default: ~/.kyler/config.toml).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Write machine-readable JSON lines to stdout.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install, inspect or remove user skills without a model call.
    Skills {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Run a JSON task suite against the configured server (may incur model usage).
    Eval {
        #[arg(long)]
        suite: PathBuf,
        #[arg(long, default_value = "eval-report.json")]
        report: PathBuf,
    },
    /// Interactive conversation (the default command).
    Chat {
        #[arg(long)]
        session: Option<String>,
    },
    /// Configure model credentials; existing values can be kept with Enter.
    Configure {
        #[arg(long)]
        non_interactive: bool,
        /// Enable the bundled Web service and configure both listening addresses.
        #[arg(long)]
        web: bool,
        /// Configure backend listening and connection addresses, even without Web.
        #[arg(long)]
        addresses: bool,
        /// Write a validated candidate beside the selected configuration without replacing it.
        #[arg(long, conflicts_with = "apply")]
        output: Option<PathBuf>,
        /// Atomically publish a validated candidate configuration.
        #[arg(long, hide=true, conflicts_with_all=["web","addresses"])]
        apply: Option<PathBuf>,
    },
    /// Manage the background server; each client session selects its own workspace.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Open the full-screen terminal interface.
    Tui {
        #[arg(long)]
        session: Option<String>,
    },
    /// Execute one turn and wait for its final result.
    Run {
        #[arg(long)]
        session: Option<String>,
        /// Keep this key to safely repeat admission after an uncertain network response.
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long, conflicts_with = "json")]
        final_only: bool,
        #[arg(required = true, num_args = 1..)]
        input: Vec<String>,
    },
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Runs {
        #[command(subcommand)]
        command: RunCommand,
    },
    /// Start the local HTTP/SSE server; requires complete application configuration.
    Serve(ServeArgs),
}
#[derive(Debug, Subcommand)]
pub enum SkillCommand {
    /// Upload a local directory containing SKILL.md to the server's user skills.
    Install { path: PathBuf },
    /// List user and system skills.
    List,
    /// Show a skill's instructions and resources.
    Show { name: String },
    /// Remove a user skill; system skills cannot be removed.
    Remove { name: String },
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    List {
        #[arg(long, default_value_t = 0)]
        cursor: usize,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Show {
        id: String,
    },
    Messages {
        id: String,
        #[arg(long, default_value_t = 0)]
        cursor: usize,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Delete {
        id: String,
    },
}
#[derive(Debug, Subcommand)]
pub enum RunCommand {
    Show {
        id: String,
    },
    Watch {
        id: String,
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
    Cancel {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
    ExternalSandbox,
}
#[derive(Debug, ClapArgs)]
pub struct ServeArgs {
    #[arg(long, hide = true)]
    pub instance_id: Option<String>,
    /// Model identifier; overrides environment and TOML configuration.
    #[arg(long)]
    pub model: Option<String>,
    /// OpenAI-compatible model API base URL.
    #[arg(long)]
    pub base_url: Option<String>,
    /// Model API key. KYLER_LLM_API_KEY / OPENAI_API_KEY are also supported.
    #[arg(long, value_parser = parse_api_key)]
    pub api_key: Option<SecretString>,
    /// Optional allowed root for client-selected session workspaces.
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    #[arg(long, value_enum)]
    pub sandbox: Option<SandboxMode>,
    /// Permit sandboxed commands in addition to built-in file tools.
    #[arg(long)]
    pub allow_commands: bool,
    /// Permit outbound network access from sandboxed tools.
    #[arg(long)]
    pub allow_network: bool,
}

fn parse_api_key(value: &str) -> Result<SecretString, Infallible> {
    Ok(SecretString::new(value))
}

#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    Start {
        #[arg(long)]
        restart: bool,
    },
    Stop,
    Status,
}
