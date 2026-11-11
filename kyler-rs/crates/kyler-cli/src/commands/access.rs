//! Web token management. Secrets are printed only by an explicit token command.
use super::configure;
use crate::{CliError, Result};

use std::path::Path;

use kyler_config::SecretString;

pub(super) fn generate_token() -> Result<SecretString> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| CliError::new(3, "Cannot generate a secure access token"))?;
    Ok(SecretString::new(
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    ))
}

fn save(path: &Path, token: &SecretString) -> Result<()> {
    let text = std::fs::read_to_string(path).map_err(CliError::io)?;
    let mut values: toml::Value =
        toml::from_str(&text).map_err(|_| CliError::new(2, "Invalid configuration TOML"))?;
    let root = values
        .as_table_mut()
        .ok_or_else(|| CliError::new(2, "Invalid configuration table"))?;
    let web = root
        .entry("web")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| CliError::new(2, "Invalid Web configuration"))?;
    if web.get("access_token").and_then(toml::Value::as_str) == Some(token.expose()) {
        return Ok(());
    }
    web.insert(
        "access_token".into(),
        toml::Value::String(token.expose().into()),
    );
    let text = toml::to_string_pretty(&values)
        .map_err(|_| CliError::new(2, "Cannot encode configuration"))?;
    configure::publish(path, text.as_bytes())
}

pub(super) fn ensure_web_token(path: &Path, current: &SecretString) -> Result<SecretString> {
    if std::env::var("KYLER_WEB_TOKEN").is_ok_and(|token| token.is_empty()) {
        return Err(CliError::new(2, "KYLER_WEB_TOKEN must not be empty"));
    }
    let token = if current.is_empty() {
        generate_token()?
    } else {
        current.clone()
    };
    if !token.expose().bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(CliError::new(
            2,
            "Web token must contain printable ASCII without whitespace",
        ));
    }
    save(path, &token)?;
    Ok(token)
}

pub(super) fn reset_web_token(path: &Path) -> Result<SecretString> {
    if std::env::var_os("KYLER_WEB_TOKEN").is_some() {
        return Err(CliError::new(
            2,
            "Unset KYLER_WEB_TOKEN before resetting the saved Web token",
        ));
    }
    let token = generate_token()?;
    save(path, &token)?;
    Ok(token)
}
