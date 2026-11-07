mod input;

use crate::{CliError, Result};
use input::Prompts;

use std::{
    fs::Permissions,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use kyler_config::loader;

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
pub fn configure(
    explicit: Option<&Path>,
    non_interactive: bool,
    enable_web: bool,
    addresses: bool,
    output: Option<&Path>,
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
    let interactive = io::stdin().is_terminal() && !non_interactive;
    if enable_web {
        config.web.enabled = true;
    }
    if interactive {
        let mut prompts = Prompts::default();
        prompts.section("Configure model");
        eprintln!("Enter keeps current values. API keys are hidden.");
        eprintln!("OPENAI_BASE_URL must use an OpenAI-compatible Chat Completions API.");
        if config.models.providers.is_empty() {
            config.llm.base_url = prompts.api_url("OPENAI_BASE_URL", &config.llm.base_url)?;
            config.llm.api_key = prompts.key("OPENAI_API_KEY", &config.llm.api_key, false)?;
            config.llm.model = prompts.model(&config.llm.model)?;
        } else {
            for (name, provider) in &mut config.models.providers {
                provider.base_url =
                    prompts.api_url(&format!("{name} OPENAI_BASE_URL"), &provider.base_url)?;
                provider.api_key =
                    prompts.key(&format!("{name} OPENAI_API_KEY"), &provider.api_key, false)?;
            }
        }
        if addresses || enable_web || config.web.enabled {
            prompts.section("Configure service addresses");
            loop {
                config.server.bind = prompts.address("Backend address", config.server.bind)?;
                if address_available(config.server.bind)
                    || (config.server.bind == old_backend
                        && path.parent().unwrap().join("server.json").is_file())
                {
                    break;
                }
                eprintln!(
                    "Backend address is unavailable. Choose another address or stop its owner."
                );
                prompts.repeat_last();
            }
            config.client.server_url = kyler_config::web::connect_url(config.server.bind);
            config.client.access_token = config.server.access_token.clone();
            if config.web.enabled {
                loop {
                    config.web.bind = prompts.address("Web address", config.web.bind)?;
                    if !kyler_config::web::addresses_overlap(config.server.bind, config.web.bind)
                        && (address_available(config.web.bind)
                            || (config.web.bind == config_before_web(&values)
                                && path.parent().unwrap().join("web.json").is_file()))
                    {
                        break;
                    }
                    eprintln!(
                        "Backend and Web addresses conflict. Choose a different address or port."
                    );
                    prompts.repeat_last();
                }
                if !config.web.bind.ip().is_loopback() || !config.web.access_token.is_empty() {
                    config.web.access_token =
                        prompts.key("Web access password", &config.web.access_token, false)?;
                }
            }
            if !config.server.bind.ip().is_loopback() && config.server.access_token.is_empty() {
                // A private service token is generated locally, never displayed or put in shell arguments.
                config.server.access_token = kyler_config::SecretString::new(format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                ));
            }
        }
    }
    if addresses || enable_web || config.web.enabled {
        if !interactive && automatic_connection {
            config.client.server_url = kyler_config::web::connect_url(config.server.bind);
        }
        if kyler_config::web::is_local_backend(config.server.bind, &config.client.server_url)
            && !config.server.access_token.is_empty()
            && config.client.access_token.is_empty()
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
    let root = values
        .as_table_mut()
        .ok_or_else(|| CliError::new(2, "Configuration must be a TOML table"))?;
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
        let llm = table(root, "llm")?;
        llm.insert("model".into(), toml::Value::String(config.llm.model));
        llm.insert("base_url".into(), toml::Value::String(config.llm.base_url));
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
                "base_url".into(),
                toml::Value::String(provider.base_url.clone()),
            );
            value.insert(
                "api_key".into(),
                toml::Value::String(provider.api_key.expose().into()),
            );
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
        eprintln!("Configuration prepared.");
        return Ok(());
    }
    publish(&path, encoded.as_bytes())?;
    eprintln!("Saved: {}", path.display());
    Ok(())
}
fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
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
