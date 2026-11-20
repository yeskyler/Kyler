//! Interactive model discovery and capability-aware setup; never sends inference requests.
use super::input::{ModelChoice, Prompts};
use crate::{CliError, Result};

use std::time::Duration;

use kyler_config::{AppConfig, ModelProfile, OutputTokenParameter, ProviderSettings};
use kyler_core::{context::ContextPolicy, models::ModelRegistry};
use kyler_provider::{Adapter, AdapterKind, discovery::DiscoveredModel};

pub(super) type Discovery = (String, ProviderSettings, Vec<DiscoveredModel>);

pub(super) async fn configure(
    prompts: &mut Prompts,
    config: &mut AppConfig,
    values: &mut toml::Value,
    advanced: bool,
) -> Result<Vec<Discovery>> {
    let legacy = config.models.providers.is_empty();
    let name = if legacy {
        "default".into()
    } else {
        let names = config.models.providers.keys().cloned().collect::<Vec<_>>();
        let current = config
            .models
            .profiles
            .get(&config.models.default)
            .map(|p| p.provider.as_str())
            .unwrap_or(&names[0]);
        if names.len() == 1 {
            names[0].clone()
        } else {
            prompts.connection(&names, current)?
        }
    };
    let mut provider = if legacy {
        ProviderSettings {
            adapter: config.llm.adapter,
            base_url: config.llm.base_url.clone(),
            api_key: config.llm.api_key.clone(),
            api_key_env: None,
            discover_models: config.llm.discover_models,
        }
    } else {
        config.models.providers[&name].clone()
    };
    let previous = provider.clone();
    provider.adapter = prompts.provider("Provider", provider.adapter)?;
    let suggested = default_url(provider.adapter, &provider.base_url);
    provider.base_url = prompts.api_url("API URL", &suggested)?;
    provider.api_key = prompts.key("API key", &provider.api_key, false)?;
    if provider.api_key.expose() != previous.api_key.expose() {
        provider.api_key_env = None;
    }
    let previous_model = if legacy {
        config.llm.model.clone()
    } else {
        config
            .models
            .profiles
            .get(&config.models.default)
            .filter(|p| p.provider == name)
            .map(|p| p.model.clone())
            .unwrap_or_default()
    };
    let adapter = Adapter::resolve(provider.adapter, &provider.base_url);
    let cached = ModelRegistry::cached_model_ids(config, &name, &provider);
    let mut models = cached.clone();
    let mut discovered = None;
    let mut fetch = provider.discover_models;
    let selected = loop {
        if fetch {
            eprintln!("Fetching model directory (no generation request)...");
            match tokio::time::timeout(
                Duration::from_secs(8),
                kyler_provider::discovery::list_models(
                    &provider.base_url,
                    provider.api_key.expose(),
                    adapter,
                ),
            )
            .await
            {
                Ok(Ok(entries)) => {
                    models = entries
                        .into_iter()
                        .filter(|m| adapter.selectable_model(&m.id))
                        .collect();
                    discovered = Some(models.clone());
                    eprintln!("Available chat models: {}", models.len());
                }
                Ok(Err(error)) => {
                    eprintln!(
                        "Model directory unavailable: {error}. Use cached models or enter an ID manually."
                    );
                    models = cached.clone();
                }
                Err(_) => {
                    eprintln!(
                        "Model directory timed out. Use cached models or enter an ID manually."
                    );
                    models = cached.clone();
                }
            }
        }
        if !cached.is_empty() && discovered.is_none() {
            eprintln!("Using {} cached model IDs.", cached.len());
        }
        match prompts.model_choice(&previous_model, &models, !cached.is_empty())? {
            ModelChoice::Selected(model) => break model,
            ModelChoice::Retry => fetch = true,
            ModelChoice::Cached => {
                models = cached.clone();
                fetch = false;
            }
        }
    };
    let connection_changed =
        provider.adapter != previous.adapter || provider.base_url != previous.base_url;
    if legacy {
        let changed = connection_changed || selected != previous_model;
        if changed {
            config.llm.capabilities = Default::default();
            config.llm.reasoning_effort = None;
            config.llm.output_token_parameter = OutputTokenParameter::Auto;
            clear_model_fields(values.get_mut("llm"));
        }
        config.llm.model = selected.clone();
        config.llm.base_url = provider.base_url.clone();
        config.llm.api_key = provider.api_key.clone();
        config.llm.adapter = provider.adapter;
        if advanced {
            let chosen = prompts.context_window(
                "Context window (tokens or auto)",
                (!config.context.auto_model_window).then_some(config.context.context_tokens),
                &ContextPolicy::from(&config.context),
            )?;
            config.context.auto_model_window = chosen.is_none();
            if let Some(tokens) = chosen {
                config.context.context_tokens = tokens;
            }
        }
        summary(
            adapter,
            &selected,
            &config.llm.capabilities,
            config.llm.reasoning_effort,
            &ContextPolicy::from(&config.context),
            config.context.auto_model_window,
        )?;
    } else {
        let selected_profile = config
            .models
            .profiles
            .get(&config.models.default)
            .filter(|p| p.provider == name && p.model == selected)
            .map(|_| config.models.default.clone())
            .or_else(|| {
                config
                    .models
                    .profiles
                    .iter()
                    .find(|(_, p)| p.provider == name && p.model == selected)
                    .map(|(n, _)| n.clone())
            })
            .unwrap_or_else(|| format!("setup_{}", uuid::Uuid::new_v4().simple()));
        let profile = config
            .models
            .profiles
            .entry(selected_profile.clone())
            .or_insert_with(|| ModelProfile {
                provider: name.clone(),
                model: selected.clone(),
                auto_context_window: true,
                context_tokens: config.context.context_tokens,
                output_tokens: config.context.output_tokens,
                reasoning_effort: None,
                capabilities: Default::default(),
                output_token_parameter: OutputTokenParameter::Auto,
            });
        if connection_changed {
            profile.capabilities = Default::default();
            profile.reasoning_effort = None;
            profile.output_token_parameter = OutputTokenParameter::Auto;
            clear_model_fields(
                values
                    .get_mut("models")
                    .and_then(|v| v.get_mut("profiles"))
                    .and_then(|v| v.get_mut(&selected_profile)),
            );
        }
        let mut policy = ContextPolicy::from(&config.context);
        policy.context_tokens = profile.context_tokens;
        policy.output_tokens = profile.output_tokens;
        if advanced {
            let chosen = prompts.context_window(
                "Context window (tokens or auto)",
                (!profile.auto_context_window).then_some(profile.context_tokens),
                &policy,
            )?;
            profile.auto_context_window = chosen.is_none();
            if let Some(tokens) = chosen {
                profile.context_tokens = tokens;
                policy.context_tokens = tokens;
            }
        }
        summary(
            adapter,
            &selected,
            &profile.capabilities,
            profile.reasoning_effort,
            &policy,
            profile.auto_context_window,
        )?;
        config.models.default = selected_profile;
        config
            .models
            .providers
            .insert(name.clone(), provider.clone());
    }
    Ok(discovered
        .map(|models| vec![(name, provider, models)])
        .unwrap_or_default())
}
fn default_url(kind: AdapterKind, current: &str) -> String {
    let old = Adapter::resolve(AdapterKind::Auto, current).0;
    if current.is_empty()
        || (old != AdapterKind::Generic
            && old != kind
            && kind != AdapterKind::Auto
            && kind != AdapterKind::Generic)
    {
        match kind {
            AdapterKind::Openai => "https://api.openai.com/v1".into(),
            AdapterKind::Siliconflow => "https://api.siliconflow.cn/v1".into(),
            _ => current.into(),
        }
    } else {
        current.into()
    }
}
fn clear_model_fields(value: Option<&mut toml::Value>) {
    if let Some(table) = value.and_then(toml::Value::as_table_mut) {
        for key in ["capabilities", "reasoning_effort", "output_token_parameter"] {
            table.remove(key);
        }
    }
}
fn summary(
    adapter: Adapter,
    model: &str,
    overrides: &kyler_protocol::api::models::ModelCapabilities,
    effort: Option<kyler_protocol::api::models::ThinkingLevel>,
    policy: &ContextPolicy,
    automatic: bool,
) -> Result<()> {
    let inferred = adapter.capabilities(model, effort);
    let capacity = overrides.max_context_tokens.or(inferred.max_context_tokens);
    let mut effective = policy.clone();
    if automatic {
        effective.context_tokens = capacity.unwrap_or(policy.context_tokens);
    }
    effective.validate().map_err(|e| {
        CliError::new(
            2,
            format!("Invalid context budget: {e}. Use configure --advanced to adjust it."),
        )
    })?;
    eprintln!("Selected model: {model}");
    match capacity {
        Some(tokens) => eprintln!("Model context capacity: {tokens} tokens (capability metadata)."),
        None => eprintln!("Model context capacity: unknown."),
    }
    eprintln!(
        "Context budget: {} tokens ({}).",
        effective.context_tokens,
        if !automatic {
            "explicit override; preserved"
        } else if capacity.is_some() {
            "automatic"
        } else {
            "fallback budget; not a verified model capacity"
        }
    );
    let reasoning = overrides.reasoning.as_ref().or(inferred.reasoning.as_ref());
    if let Some(r) = reasoning.filter(|r| !r.levels.is_empty()) {
        eprintln!(
            "Thinking levels: {}. Default: {}.",
            r.levels
                .iter()
                .map(|l| l.reasoning_effort())
                .collect::<Vec<_>>()
                .join(", "),
            r.default.reasoning_effort()
        );
    } else {
        eprintln!("Thinking level selection: unavailable or not yet known.");
    }
    Ok(())
}

pub(super) fn write_window(
    table: &mut toml::Table,
    automatic: bool,
    tokens: usize,
    flag: &str,
) -> Result<()> {
    if automatic {
        if tokens == kyler_config::ContextSettings::default().context_tokens {
            table.remove("context_tokens");
        } else {
            table.insert(
                "context_tokens".into(),
                i64::try_from(tokens)
                    .map_err(|_| CliError::new(2, "Context window is too large"))?
                    .into(),
            );
        }
        table.insert(flag.into(), true.into());
    } else {
        table.insert(
            "context_tokens".into(),
            i64::try_from(tokens)
                .map_err(|_| CliError::new(2, "Context window is too large"))?
                .into(),
        );
        if table.contains_key(flag) {
            table.insert(flag.into(), false.into());
        }
    }
    Ok(())
}
pub(super) fn save_cache(config: &AppConfig, discovered: Vec<Discovery>) {
    for (name, provider, models) in discovered {
        if ModelRegistry::cache_model_ids(config, &name, &provider, models).is_err() {
            eprintln!(
                "Model directory cache could not be saved; configuration was saved successfully."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_serialization_distinguishes_fallback_from_manual_override() {
        let mut settings = toml::Table::new();
        settings.insert("context_tokens".into(), 64000.into());
        write_window(&mut settings, false, 64000, "auto_model_window").unwrap();
        assert_eq!(settings["context_tokens"].as_integer(), Some(64000));
        assert!(!settings.contains_key("auto_model_window"));
        write_window(&mut settings, true, 128000, "auto_model_window").unwrap();
        assert!(!settings.contains_key("context_tokens"));
        assert_eq!(settings["auto_model_window"].as_bool(), Some(true));
        write_window(&mut settings, true, 64000, "auto_model_window").unwrap();
        assert_eq!(settings["context_tokens"].as_integer(), Some(64000));
        write_window(&mut settings, false, 32000, "auto_model_window").unwrap();
        assert_eq!(settings["context_tokens"].as_integer(), Some(32000));
        assert_eq!(settings["auto_model_window"].as_bool(), Some(false));
    }
    #[test]
    fn provider_selection_suggests_official_urls_without_replacing_custom_proxies() {
        assert_eq!(
            default_url(AdapterKind::Openai, "https://api.siliconflow.cn/v1"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            default_url(AdapterKind::Siliconflow, "http://localhost:1234/v1"),
            "http://localhost:1234/v1"
        );
    }
}
