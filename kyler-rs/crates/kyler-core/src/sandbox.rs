//! Trusted host policies and isolated worker execution through MXC's Rust API.
pub mod diagnostics;
pub mod direct;
pub mod mxc;
pub mod paths;
pub mod policy;
pub mod worker;

use crate::environment::SessionEnvironment;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, mpsc::Receiver},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

pub use mxc::MxcSandbox;
pub use policy::{FilesystemPolicy, NetworkAccess, ProcessPolicy, SandboxMode, SandboxPolicy};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRequest {
    #[serde(default)]
    pub stream_output: bool,
    pub tool: String,
    pub arguments: Value,
    pub environment: SessionEnvironment,
    pub filesystem: FilesystemPolicy,
    pub allow_commands: bool,
    pub max_output_bytes: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerResponse {
    pub output: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessChunk {
    pub stream: String,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInput {
    pub data: String,
    #[serde(default)]
    pub close: bool,
}
pub struct SandboxRequest {
    pub progress: Option<UnboundedSender<ProcessChunk>>,
    pub input: Option<Receiver<ProcessInput>>,
    pub worker_request: WorkerRequest,
    pub policy: SandboxPolicy,
    pub worker_path: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub temporary_directory: Arc<tempfile::TempDir>,
}
#[async_trait::async_trait]
pub trait Sandbox: Send + Sync {
    async fn run(&self, request: SandboxRequest) -> Result<WorkerResponse>;
}
