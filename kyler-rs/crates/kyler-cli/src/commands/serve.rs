use crate::{
    CliError, Result,
    args::{SandboxMode, ServeArgs},
};

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::{net::TcpListener, signal::unix::SignalKind};

use kyler_config::loader::LlmOverrides;
use kyler_server::{Server, workspaces::WorkspaceFactory};

pub async fn serve(path: Option<&Path>, settings: &ServeArgs) -> Result<()> {
    let config = kyler_config::loader::load_with_llm_overrides(
        path,
        &LlmOverrides {
            model: settings.model.clone(),
            base_url: settings.base_url.clone(),
            api_key: settings.api_key.clone(),
        },
    )
    .map_err(|e| CliError::new(2, e.to_string()))?;
    kyler_protocol::startup::mark("configuration_ready");
    kyler_logging::init(config.logging.clone()).map_err(|e| CliError::new(2, e.to_string()))?;
    kyler_protocol::startup::mark("logging_ready");
    let mut sandbox = config.sandbox.clone();
    if let Some(mode) = settings.sandbox {
        sandbox.mode = match mode {
            SandboxMode::ReadOnly => kyler_protocol::sandbox::SandboxMode::ReadOnly,
            SandboxMode::WorkspaceWrite => kyler_protocol::sandbox::SandboxMode::WorkspaceWrite,
            SandboxMode::DangerFullAccess => kyler_protocol::sandbox::SandboxMode::DangerFullAccess,
            SandboxMode::ExternalSandbox => kyler_protocol::sandbox::SandboxMode::ExternalSandbox,
        };
    }
    sandbox.allow_commands |= settings.allow_commands;
    if sandbox.mode == kyler_protocol::sandbox::SandboxMode::ReadOnly {
        if settings.allow_network {
            return Err(CliError::new(
                2,
                "--allow-network cannot be used with a read-only sandbox.",
            ));
        }
        sandbox.network_access = false;
    } else {
        sandbox.network_access |= settings.allow_network;
    }
    let factory = WorkspaceFactory::configured(
        std::env::current_exe().map_err(CliError::io)?,
        settings.workspace.iter().cloned().collect(),
        sandbox,
    )
    .map_err(|e| CliError::new(2, format!("Cannot initialize workspace sandboxes: {e:#}")))?
    .with_process_timeouts(
        config.server.command_timeout_ms,
        config.server.background_command_timeout_ms,
    )
    .map_err(|error| CliError::new(2, error.to_string()))?;
    kyler_protocol::startup::mark("sandbox_factory_ready");
    let mut server = Server::from_config_with_workspaces(&config, Arc::new(factory))
        .await
        .map_err(|e| CliError::new(3, e.to_string()))?;
    let source = path
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("KYLER_CONFIG").map(PathBuf::from))
        .map(Ok)
        .unwrap_or_else(kyler_config::loader::default_config_path)
        .map_err(|e| CliError::new(2, format!("Cannot resolve model configuration source: {e}")))?;
    let source = if source.is_absolute() {
        source
    } else {
        std::env::current_dir().map_err(CliError::io)?.join(source)
    };
    server.state.runs.set_model_source(
        source,
        LlmOverrides {
            model: settings.model.clone(),
            base_url: settings.base_url.clone(),
            api_key: settings.api_key.clone(),
        },
    );
    if let Some(id) = &settings.instance_id {
        if !uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == *id) {
            return Err(CliError::new(2, "Invalid service instance ID"));
        }
        server.state.instance_id = id.clone();
    }
    let listener = TcpListener::bind(config.server.bind)
        .await
        .map_err(CliError::io)?;
    eprintln!(
        "Kyler server listening on {}",
        listener.local_addr().map_err(CliError::io)?
    );
    server
        .serve(listener, async {
            #[cfg(unix)]
            {
                match tokio::signal::unix::signal(SignalKind::terminate()) {
                    Ok(mut term) => {
                        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
                    }
                    Err(_) => {
                        let _ = tokio::signal::ctrl_c().await;
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        })
        .await
        .map_err(|e| CliError::new(3, e.to_string()))
}
