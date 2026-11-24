mod channels;
mod input;
mod models;

use super::ui::{self, Progress};
use crate::{CliError, Result};
use input::Prompts;

use std::{
    fs::Permissions,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use kyler_config::loader;
use kyler_core::context::ContextPolicy;

pub(super) fn config_path(explicit: Option<&Path>) -> Result<PathBuf> {
    let path = explicit
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("KYLER_CONFIG").map(PathBuf::from));
    let path = match path {
        Some(path) => path,
        None => loader::default_config_path().map_err(|e| CliError::new(2, e.to_string()))?,
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir().map_err(CliError::io)?.join(path))
    }
}
pub async fn configure(
    explicit: Option<&Path>,
    non_interactive: bool,
    enable_web: bool,
    addresses: bool,
    output: Option<&Path>,
    advanced: bool,
    install_setup: bool,
) -> Result<()> {
    let path = config_path(explicit)?;
    // An explicit missing file is a new setup, not a load error.
    let mut config =
        loader::load_for_setup(Some(&path)).map_err(|e| CliError::new(2, e.to_string()))?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(CliError::io(e)),
    };
    let mut values: toml::Value = toml::from_str(&text).map_err(|_| {
        CliError::new(
            2,
            "Invalid configuration TOML; the existing file was preserved.",
        )
    })?;
    if config.models.providers.is_empty() && config.llm.model.is_empty() {
        config.llm.model = "deepseek-ai/DeepSeek-V4-Flash".into();
        if config.llm.base_url.is_empty() {
            config.llm.base_url = "https://api.siliconflow.cn/v1".into();
        }
    }
    // An absent window is automatic; an existing explicit budget remains authoritative.
    if values
        .get("context")
        .and_then(|v| v.get("context_tokens"))
        .is_none()
        && values
            .get("context")
            .and_then(|v| v.get("auto_model_window"))
            .is_none()
    {
        config.context.auto_model_window = true;
    }
    config.session.enabled = true;
    let mut discovered = Vec::new();
    let can_start = output.is_none()
        && std::env::current_exe()
            .ok()
            .and_then(|worker| kyler_core::sandbox::paths::bundled_runtime_root(&worker))
            .is_some_and(|runtime| {
                runtime.join("node").is_file()
                    && runtime.parent().unwrap().join("web/server.js").is_file()
            });
    let mut start_after_save = false;

    let old_backend = config.server.bind;
    let saved_backend = values
        .get("server")
        .and_then(|v| v.get("bind"))
        .and_then(toml::Value::as_str)
        .and_then(|v| v.parse().ok())
        .unwrap_or(kyler_config::ServerSettings::default().bind);
    let automatic_connection = std::env::var_os("KYLER_CLIENT_SERVER_URL").is_none()
        && (config.client.server_url == kyler_config::web::connect_url(saved_backend)
            || values
                .get("client")
                .and_then(|v| v.get("server_url"))
                .is_none());
    let interactive = io::stdin().is_terminal() && ui::terminal_controls() && !non_interactive;
    if enable_web {
        config.web.enabled = true;
    }
    if interactive {
        #[derive(Clone, Copy)]
        enum Page {
            Model,
            Access,
            Channels,
            Review,
        }
        let mut prompts = Prompts::new().await?;
        if install_setup && can_start && !advanced && !text.is_empty() && config.validate().is_ok()
        {
            match prompts
                .select(
                    "Configuration",
                    vec![
                        "Keep current configuration and start".into(),
                        "Edit configuration".into(),
                    ],
                    0,
                )
                .map_err(|error| {
                    if error.code == input::BACK {
                        input::cancelled()
                    } else {
                        error
                    }
                })? {
                0 => {
                    ui::field("Configuration", "Saved settings kept");
                    let mut progress = Progress::new();
                    progress.begin("Configure Kyler");
                    progress.complete();
                    return launch(&path, &config).await;
                }
                _ => {}
            }
        }
        let mut wizard = models::Wizard::new(&config, &values);
        let scope = addresses || enable_web || config.web.enabled;
        let mut lan = !config.server.bind.ip().is_loopback()
            || (config.web.enabled && !config.web.bind.ip().is_loopback());
        let mut page = Page::Model;
        loop {
            match page {
                Page::Model => {
                    let channel_settings = config.channels.clone();
                    discovered = wizard
                        .run(&mut prompts, &mut config, &mut values, advanced)
                        .await?;
                    config.channels = channel_settings;
                    page = if scope { Page::Access } else { Page::Channels };
                }
                Page::Access => {
                    match prompts.access_scope(lan) {
                        Ok(value) => lan = value,
                        Err(e) if e.code == input::BACK => {
                            page = Page::Model;
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                    let ip = if lan {
                        std::net::Ipv4Addr::UNSPECIFIED
                    } else {
                        std::net::Ipv4Addr::LOCALHOST
                    };
                    config.server.bind.set_ip(ip.into());
                    config.web.bind.set_ip(ip.into());
                    for (label, address, old, record) in [
                        ("Backend", config.server.bind, old_backend, "server.json"),
                        (
                            "Web",
                            config.web.bind,
                            config_before_web(&values),
                            "web.json",
                        ),
                    ] {
                        if label == "Web" && !config.web.enabled {
                            continue;
                        }
                        if !address_available(address)
                            && !(address.port() == old.port()
                                && path.parent().unwrap().join(record).is_file())
                        {
                            return Err(CliError::new(
                                2,
                                format!(
                                    "{label} port {} is unavailable. Stop its owner and run setup again.",
                                    address.port()
                                ),
                            ));
                        }
                    }
                    config.client.server_url = kyler_config::web::connect_url(config.server.bind);

                    page = Page::Channels;
                }
                Page::Channels => match channels::run(&mut prompts, &mut config) {
                    Ok(()) => page = Page::Review,
                    Err(e) if e.code == input::BACK => {
                        page = if scope { Page::Access } else { Page::Model };
                    }
                    Err(e) => return Err(e),
                },
                Page::Review => {
                    models::review(
                        &config,
                        advanced,
                        scope.then_some(if lan {
                            "Local network"
                        } else {
                            "This machine only"
                        }),
                        &discovered,
                    )?;
                    let mut choices = vec![
                        if can_start {
                            "Save and start".into()
                        } else {
                            "Save configuration".into()
                        },
                        "Edit model".into(),
                    ];
                    if scope {
                        choices.push("Edit access scope".into());
                    }
                    let edit_channels = choices.len();
                    choices.push("Edit channels".into());
                    let save_only = can_start.then_some(choices.len());
                    if can_start {
                        choices.push("Save without starting".into());
                    }
                    choices.push("Cancel setup".into());
                    match prompts.select("Finish setup", choices, 0) {
                        Ok(0) => {
                            start_after_save = can_start;
                            break;
                        }
                        Ok(1) => {
                            wizard.edit_model();
                            page = Page::Model;
                        }
                        Ok(2) if scope => page = Page::Access,
                        Ok(index) if index == edit_channels => page = Page::Channels,
                        Ok(index) if Some(index) == save_only => break,
                        Ok(_) => return Err(input::cancelled()),
                        Err(e) if e.code == input::BACK => {
                            page = Page::Channels;
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }

    if addresses || enable_web || config.web.enabled {
        if config.web.enabled && config.web.access_token.is_empty() {
            config.web.access_token = super::access::generate_token()?;
        }
        if !interactive && automatic_connection {
            config.client.server_url = kyler_config::web::connect_url(config.server.bind);
        }
        if kyler_config::web::is_local_backend(config.server.bind, &config.client.server_url)
            && !config.server.access_token.is_empty()
        {
            config.client.access_token = config.server.access_token.clone();
        }
    }
    if config.models.providers.is_empty()
        && config.llm.model == "deepseek-ai/DeepSeek-V4.1-Flash"
        && config.llm.capabilities.images.is_none()
    {
        config.llm.capabilities.images = Some(true);
    }
    config.session.enabled = true;
    config.validate().map_err(|e|CliError::new(2,format!("Configuration not saved: {e}. Run kyler configure to fix it; non-interactive setup requires credentials in the config or environment.")))?;
    for profile in config.effective_models().profiles.values() {
        let mut policy = ContextPolicy::from(&config.context);
        policy.context_tokens = profile.context_tokens;
        policy.output_tokens = profile.output_tokens;
        policy
            .validate()
            .map_err(|error| CliError::new(2, format!("Configuration not saved: {error}")))?;
    }
    channels::validate(&config)?;
    super::token::configure(&mut config).await?;
    let root = values
        .as_table_mut()
        .ok_or_else(|| CliError::new(2, "Configuration must be a TOML table"))?;
    channels::write(&config, root);
    table(root, "server")?.insert(
        "access_token".into(),
        config.server.access_token.expose().into(),
    );
    table(root, "client")?.insert(
        "access_token".into(),
        config.client.access_token.expose().into(),
    );
    if addresses || enable_web || config.web.enabled {
        table(root, "server")?.insert(
            "bind".into(),
            toml::Value::String(config.server.bind.to_string()),
        );
        if !config.server.access_token.is_empty() {
            table(root, "server")?.insert(
                "access_token".into(),
                toml::Value::String(config.server.access_token.expose().into()),
            );
        }
        table(root, "client")?.insert(
            "server_url".into(),
            toml::Value::String(config.client.server_url.clone()),
        );
        if !config.client.access_token.is_empty() {
            table(root, "client")?.insert(
                "access_token".into(),
                toml::Value::String(config.client.access_token.expose().into()),
            );
        }
        if config.web.enabled {
            let web = table(root, "web")?;
            web.insert("enabled".into(), toml::Value::Boolean(true));
            web.insert(
                "bind".into(),
                toml::Value::String(config.web.bind.to_string()),
            );
            if !config.web.access_token.is_empty() {
                web.insert(
                    "access_token".into(),
                    toml::Value::String(config.web.access_token.expose().into()),
                );
            }
        }
    }
    if let Some(tools) = root.get_mut("tools").and_then(toml::Value::as_table_mut) {
        tools.remove("tavily");
        if tools.is_empty() {
            root.remove("tools");
        }
    }
    if config.models.providers.is_empty() {
        models::write_window(
            table(root, "context")?,
            config.context.auto_model_window,
            config.context.context_tokens,
            "auto_model_window",
        )?;
        let llm = table(root, "llm")?;
        llm.insert(
            "protocol".into(),
            serde_json::to_value(config.llm.protocol)
                .unwrap()
                .as_str()
                .unwrap()
                .into(),
        );
        llm.insert(
            "adapter".into(),
            toml::Value::String(
                serde_json::to_value(config.llm.adapter)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .into(),
            ),
        );
        llm.insert(
            "model".into(),
            toml::Value::String(config.llm.model.clone()),
        );
        llm.insert(
            "base_url".into(),
            toml::Value::String(config.llm.base_url.clone()),
        );
        llm.insert(
            "api_key".into(),
            toml::Value::String(config.llm.api_key.expose().into()),
        );
        if let Some(images) = config.llm.capabilities.images {
            table(llm, "capabilities")?.insert("images".into(), toml::Value::Boolean(images));
        }
    } else {
        let providers = table(table(root, "models")?, "providers")?;
        for (name, provider) in &config.models.providers {
            let value = table(providers, name)?;
            value.insert(
                "protocol".into(),
                serde_json::to_value(provider.protocol)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .into(),
            );
            value.insert(
                "base_url".into(),
                toml::Value::String(provider.base_url.clone()),
            );
            value.insert(
                "adapter".into(),
                serde_json::to_value(provider.adapter)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .into(),
            );
            if provider.api_key_env.is_none() {
                value.remove("api_key_env");
                value.insert("api_key".into(), provider.api_key.expose().into());
            }
        }
    }
    if !config.models.profiles.is_empty() {
        table(root, "models")?.insert("default".into(), config.models.default.clone().into());
        let profiles = table(table(root, "models")?, "profiles")?;
        for (name, profile) in &config.models.profiles {
            let value = table(profiles, name)?;
            value.insert("provider".into(), profile.provider.clone().into());
            value.insert("model".into(), profile.model.clone().into());
            value.insert(
                "output_tokens".into(),
                i64::from(profile.output_tokens).into(),
            );
            models::write_window(
                value,
                profile.auto_context_window,
                profile.context_tokens,
                "auto_context_window",
            )?;
        }
    }
    table(root, "workspace")?
        .entry("default_directory")
        .or_insert_with(|| toml::Value::String("workspace".into()));
    std::fs::create_dir_all(&config.workspace.default_directory).map_err(CliError::io)?;
    root.entry("session")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| CliError::new(2, "Invalid session configuration"))?
        .insert("enabled".into(), toml::Value::Boolean(true));
    let encoded = toml::to_string_pretty(&values)
        .map_err(|_| CliError::new(2, "Could not serialize configuration"))?;
    let directory = path
        .parent()
        .ok_or_else(|| CliError::new(2, "Invalid configuration path"))?;
    std::fs::create_dir_all(directory).map_err(CliError::io)?;
    if let Some(output) = output {
        let output = if output.is_absolute() {
            output.to_path_buf()
        } else {
            std::env::current_dir().map_err(CliError::io)?.join(output)
        };
        if output.parent() != path.parent() {
            return Err(CliError::new(
                2,
                "Candidate configuration must be beside the selected configuration",
            ));
        }
        publish(&output, encoded.as_bytes())?;
        return Ok(());
    }
    publish(&path, encoded.as_bytes())?;
    models::save_cache(&config, discovered);
    let mut progress = Progress::new();
    progress.begin("Configure Kyler");
    progress.complete();
    if start_after_save {
        launch(&path, &config).await?;
    }
    Ok(())
}
async fn launch(path: &Path, config: &kyler_config::AppConfig) -> Result<()> {
    let mut progress = Progress::new();
    progress.begin("Launch");
    let output = tokio::process::Command::new(std::env::current_exe().map_err(CliError::io)?)
        .arg("--config")
        .arg(path)
        .args(["--json", "service", "start", "--restart"])
        .env_remove("KYLER_INTERNAL_TOOL_WORKER")
        .env_remove("KYLER_INSTALL_SETUP")
        .kill_on_drop(true)
        .output()
        .await
        .map_err(CliError::io)?;
    if !output.status.success() {
        progress.fail();
        let logs = path.parent().unwrap().join("logs");
        std::fs::create_dir_all(&logs).map_err(CliError::io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&logs, Permissions::from_mode(0o700)).map_err(CliError::io)?;
        }
        let log = logs.join("launch.log");
        let mut diagnostics = output.stdout;
        diagnostics.extend_from_slice(&output.stderr);
        publish(&log, &diagnostics)?;
        return Err(CliError::new(
            3,
            format!("Could not start services. Log:{}", log.display()),
        ));
    }
    progress.complete();
    let saved = loader::load_for_setup(Some(path)).ok();
    let config = saved.as_ref().unwrap_or(config);
    let primary = super::service::addresses::primary_connection();
    let mut details = Vec::new();
    details.push(match &primary {
        Some((name, address)) => format!("Interface  {name} · {address}"),
        None => "Interface  Primary network connection unavailable".into(),
    });
    if config.web.enabled {
        let url = match &primary {
            Some((_, address))
                if config.web.bind.ip().is_ipv4() && config.web.bind.ip().is_unspecified() =>
            {
                format!("http://{address}:{}", config.web.bind.port())
            }
            _ => kyler_config::web::connect_url(config.web.bind),
        };
        details.push(format!("Web        {url}"));
        details.push(format!(
            "Token      {}",
            if config.web.access_token.is_empty() {
                "Run kyler service token"
            } else {
                config.web.access_token.expose()
            }
        ));
    }
    if config.web.enabled && config.web.bind.ip().is_loopback() {
        details.push("Access     This machine only".into());
    }
    ui::connection(&details.join("\n")).map_err(CliError::io)?;
    Ok(())
}

pub(super) fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    let destination = if path.exists() {
        std::fs::canonicalize(&path).map_err(CliError::io)?
    } else {
        path.to_path_buf()
    };
    let mut temporary =
        tempfile::NamedTempFile::new_in(destination.parent().unwrap()).map_err(CliError::io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        temporary
            .as_file()
            .set_permissions(Permissions::from_mode(0o600))
            .map_err(CliError::io)?;
    }
    temporary.write_all(bytes).map_err(CliError::io)?;
    temporary.as_file().sync_all().map_err(CliError::io)?;
    temporary
        .persist(&destination)
        .map_err(|e| CliError::io(e.error))?;
    Ok(())
}
pub fn apply(explicit: Option<&Path>, candidate: &Path) -> Result<()> {
    let path = config_path(explicit)?;
    let text = std::fs::read_to_string(candidate).map_err(CliError::io)?;
    let mut config = loader::parse(&text)
        .map_err(|e| CliError::new(2, format!("Candidate configuration is invalid: {e}")))?;
    let environment = std::env::vars().collect();
    config.mcp.resolve_environment(&environment);
    config.models.resolve_environment(&environment);
    config
        .validate()
        .map_err(|e| CliError::new(2, e.to_string()))?;
    channels::validate(&config)?;
    let bytes = std::fs::read(candidate).map_err(CliError::io)?;
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| CliError::new(2, "Invalid config path"))?,
    )
    .map_err(CliError::io)?;
    publish(&path, &bytes)
}
fn table<'a>(root: &'a mut toml::Table, key: &str) -> Result<&'a mut toml::Table> {
    root.entry(key)
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| CliError::new(2, format!("Invalid {key} configuration")))
}

fn address_available(address: std::net::SocketAddr) -> bool {
    std::net::TcpListener::bind(address).is_ok()
}
fn config_before_web(values: &toml::Value) -> std::net::SocketAddr {
    values
        .get("web")
        .and_then(|v| v.get("bind"))
        .and_then(toml::Value::as_str)
        .and_then(|v| v.parse().ok())
        .unwrap_or(kyler_config::WebSettings::default().bind)
}
