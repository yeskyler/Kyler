use crate::{
    ClientSettings,
    app::{AppConfig, SecretString},
};

use std::{
    collections::HashMap,
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result};

/// Loads and validates the full application configuration for local services.
pub fn load() -> Result<AppConfig> {
    let config = merged(None, &env::vars().collect(), false)?;
    config.validate()?;
    Ok(config)
}
pub fn load_from(path: impl AsRef<Path>) -> Result<AppConfig> {
    load_from_with_env(path, &env::vars().collect())
}
pub fn load_from_with_env(
    path: impl AsRef<Path>,
    environment: &HashMap<String, String>,
) -> Result<AppConfig> {
    let config = merged(Some(path.as_ref()), environment, false)?;
    config.validate()?;
    Ok(config)
}
/// Only client settings are validated: remote clients need no model credentials.
/// The explicit URL is a command-line override and is applied after file/environment values.
pub fn load_client(path: Option<&Path>, server_url: Option<&str>) -> Result<ClientSettings> {
    load_client_with_env(path, server_url, &env::vars().collect())
}
pub fn load_client_with_env(
    path: Option<&Path>,
    server_url: Option<&str>,
    environment: &HashMap<String, String>,
) -> Result<ClientSettings> {
    let mut client = merged(path, environment, true)?.client;
    if let Some(url) = server_url {
        client.server_url = url.to_owned();
    }
    client.validate()?;
    Ok(client)
}
/// Default user configuration. Independent of the current project directory.
pub fn default_config_path() -> Result<PathBuf> {
    default_config_path_with_env(&env::vars().collect())
}
fn default_config_path_with_env(environment: &HashMap<String, String>) -> Result<PathBuf> {
    let home = environment
        .get("HOME")
        .or_else(|| environment.get("USERPROFILE"))
        .filter(|value| !value.trim().is_empty())
        .context("cannot determine home directory; set HOME or supply --config / KYLER_CONFIG")?;
    let home = Path::new(home);
    anyhow::ensure!(
        home.is_absolute(),
        "home directory must be an absolute path"
    );
    Ok(home.join(".kyler").join("config.toml"))
}
fn merged(
    explicit: Option<&Path>,
    environment: &HashMap<String, String>,
    client_only: bool,
) -> Result<AppConfig> {
    let selected = explicit
        .map(PathBuf::from)
        .or_else(|| environment.get("KYLER_CONFIG").map(PathBuf::from));
    if let Some(path) = selected {
        // Explicit paths must exist. Keep relative paths anchored to the selected config,
        // including the legacy config/default.toml project-base convention.
        let path = fs::canonicalize(path).context("configuration file not found")?;
        let text = fs::read_to_string(&path).context("failed to read configuration file")?;
        let directory = path
            .parent()
            .context("configuration has no parent directory")?;
        let base = if directory.file_name().is_some_and(|name| name == "config") {
            directory.parent().unwrap_or(directory)
        } else {
            directory
        };
        return finish(parse(&text)?, base, environment.clone(), client_only);
    }
    let path = default_config_path_with_env(environment)?;
    let base = path.parent().expect("default configuration has a parent");
    let config = match fs::read_to_string(&path) {
        Ok(text) => parse(&text)?,
        Err(error) if error.kind() == ErrorKind::NotFound => AppConfig::default(),
        Err(error) => return Err(error).context("failed to read default configuration file"),
    };
    // Missing config files still use ~/.kyler for relative application storage.
    finish(config, base, environment.clone(), client_only)
}

pub fn parse(text: &str) -> Result<AppConfig> {
    let mut value: toml::Value =
        toml::from_str(text).map_err(|_| anyhow::anyhow!("invalid configuration TOML syntax"))?;
    if let Some(root) = value.as_table_mut() {
        if let Some(tools) = root.get_mut("tools").and_then(toml::Value::as_table_mut) {
            tools.remove("tavily");
            if tools.is_empty() {
                root.remove("tools");
            }
        }
    }
    value
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid configuration schema"))
}

fn finish(
    mut config: AppConfig,
    base: &Path,
    environment: HashMap<String, String>,
    client_only: bool,
) -> Result<AppConfig> {
    if client_only {
        apply_client_environment(&mut config, &environment);
    } else {
        apply_environment(&mut config, &environment)?;
        config.mcp.resolve_environment(&environment);
        config.models.resolve_environment(&environment);
    }
    let home = environment
        .get("HOME")
        .or_else(|| environment.get("USERPROFILE"))
        .map(Path::new);
    config.workspace.resolve_paths(base, home)?;
    config.sandbox.resolve_paths(base, home)?;
    config.skills.resolve_paths(base, home)?;
    if config.session.directory.is_relative() {
        config.session.directory = base.join(&config.session.directory);
    }
    Ok(config)
}

fn number<T: FromStr>(values: &HashMap<String, String>, key: &str) -> Result<Option<T>> {
    values
        .get(key)
        .map(|v| {
            v.parse()
                .map_err(|_| anyhow::anyhow!("invalid value for {key}"))
        })
        .transpose()
}
fn apply_environment(config: &mut AppConfig, values: &HashMap<String, String>) -> Result<()> {
    apply_client_environment(config, values);
    if let Some(v) = values.get("KYLER_SERVER_TOKEN") {
        config.server.access_token = SecretString::new(v);
    }
    if let Some(v) = values.get("KYLER_SERVER_BIND") {
        config.server.bind = v
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid KYLER_SERVER_BIND"))?;
    }
    if let Some(v) = values.get("KYLER_WEB_BIND") {
        config.web.bind = v
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid KYLER_WEB_BIND"))?;
    }
    if let Some(v) = values.get("KYLER_WEB_TOKEN") {
        config.web.access_token = SecretString::new(v);
    }
    if let Some(v) = number(values, "KYLER_WEB_ENABLED")? {
        config.web.enabled = v;
    }
    if let Some(v) = values.get("KYLER_LLM_BASE_URL") {
        config.llm.base_url = v.clone();
    }
    if let Some(v) = values.get("KYLER_LLM_MODEL") {
        config.llm.model = v.clone();
    }
    if let Some(v) = values
        .get("KYLER_LLM_API_KEY")
        .or_else(|| values.get("OPENAI_API_KEY"))
    {
        config.llm.api_key = SecretString::new(v);
    }
    if let Some(v) = values.get("KYLER_SESSION_DIR") {
        config.session.directory = PathBuf::from(v);
    }
    if let Some(v) = values.get("KYLER_LOG_FILTER") {
        config.logging.filter = v.clone();
    }
    if let Some(v) = number(values, "KYLER_MAX_STEPS")? {
        config.agent.max_steps = v;
    }
    if let Some(v) = number(values, "KYLER_SESSION_ENABLED")? {
        config.session.enabled = v;
    }
    Ok(())
}

fn apply_client_environment(config: &mut AppConfig, values: &HashMap<String, String>) {
    if let Some(v) = values.get("KYLER_CLIENT_SERVER_URL") {
        config.client.server_url = v.clone();
    }
    if let Some(v) = values
        .get("KYLER_CLIENT_TOKEN")
        .or_else(|| values.get("KYLER_SERVER_TOKEN"))
    {
        config.client.access_token = SecretString::new(v);
    }
}

/// Explicit launch-time values override process environment and configuration files.
#[derive(Debug, Clone, Default)]
pub struct LlmOverrides {
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<SecretString>,
}
pub fn load_with_llm_overrides(path: Option<&Path>, overrides: &LlmOverrides) -> Result<AppConfig> {
    load_with_llm_overrides_and_env(path, overrides, &env::vars().collect())
}
pub fn load_with_llm_overrides_and_env(
    path: Option<&Path>,
    overrides: &LlmOverrides,
    environment: &HashMap<String, String>,
) -> Result<AppConfig> {
    let mut config = merged(path, environment, false)?;
    if let Some(model) = &overrides.model {
        config.llm.model = model.clone();
    }
    if let Some(base_url) = &overrides.base_url {
        config.llm.base_url = base_url.clone();
    }
    if let Some(api_key) = &overrides.api_key {
        config.llm.api_key = api_key.clone();
    }
    if !config.models.profiles.is_empty() {
        if let Some(profile) = config.models.profiles.get_mut(&config.models.default) {
            if let Some(model) = &overrides.model {
                profile.model = model.clone();
            }
            if let Some(provider) = config.models.providers.get_mut(&profile.provider) {
                if let Some(base_url) = &overrides.base_url {
                    provider.base_url = base_url.clone();
                }
                if let Some(api_key) = &overrides.api_key {
                    provider.api_key = api_key.clone();
                }
            }
        }
    }
    config.validate()?;
    Ok(config)
}

/// Setup may fill missing credentials before calling AppConfig::validate.
pub fn load_for_setup(path: Option<&Path>) -> Result<AppConfig> {
    let environment = env::vars().collect();
    if let Some(path) = path.filter(|path| !path.exists()) {
        return finish(
            AppConfig::default(),
            path.parent().context("configuration has no parent")?,
            environment,
            false,
        );
    }
    merged(path, &environment, false)
}
