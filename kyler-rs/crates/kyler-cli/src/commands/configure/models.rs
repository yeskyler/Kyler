//! Interactive model discovery and capability-aware setup; never sends inference requests.
use super::super::ui;
use super::input::{ModelChoice, Prompts};
use crate::{CliError, Result};

use std::time::Duration;

use kyler_config::{AppConfig, ModelProfile, OutputTokenParameter, ProviderSettings};
use kyler_core::{context::ContextPolicy, models::ModelRegistry};
use kyler_provider::{Adapter, AdapterKind, discovery::DiscoveredModel};

pub(super) type Discovery = (String, ProviderSettings, Vec<DiscoveredModel>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Connection,
    Provider,
    Url,
    Key,
    Model,
    Manual,
    Context,
}
/// All edits remain in this draft until the final review accepts them.
pub(super) struct Wizard {
    baseline: AppConfig,
    values: toml::Value,
    names: Vec<String>,
    name: String,
    provider: ProviderSettings,
    selected: String,
    profile: String,
    window: Option<Option<usize>>,
    step: Step,
    directory_for: Option<(String, ProviderSettings)>,
    cached: Vec<DiscoveredModel>,
    models: Vec<DiscoveredModel>,
    discovered: Option<Vec<DiscoveredModel>>,
    force_fetch: bool,
    make_default: bool,
}
impl Wizard {
    pub fn new(config: &AppConfig, values: &toml::Value) -> Self {
        let names = config.models.providers.keys().cloned().collect::<Vec<_>>();
        let name = config
            .models
            .profiles
            .get(&config.models.default)
            .map(|p| p.provider.clone())
            .or_else(|| names.first().cloned())
            .unwrap_or_else(|| "default".into());
        let provider = if names.is_empty() {
            ProviderSettings {
                adapter: config.llm.adapter,
                protocol: config.llm.protocol,
                base_url: config.llm.base_url.clone(),
                api_key: config.llm.api_key.clone(),
                api_key_env: None,
                discover_models: config.llm.discover_models,
                enabled_models: None,
                default_model: None,
            }
        } else {
            config.models.providers[&name].clone()
        };
        let selected = if names.is_empty() {
            config.llm.model.clone()
        } else {
            config
                .models
                .profiles
                .get(&config.models.default)
                .map(|p| p.model.clone())
                .unwrap_or_default()
        };
        Self {
            baseline: config.clone(),
            values: values.clone(),
            step: if !names.is_empty() || config.effective_models().validate().is_ok() {
                Step::Connection
            } else {
                Step::Provider
            },
            names,
            name,
            provider,
            selected,
            profile: config.models.default.clone(),
            window: None,
            directory_for: None,
            cached: Vec::new(),
            models: Vec::new(),
            discovered: None,
            force_fetch: false,
            make_default: true,
        }
    }
    pub fn manage_connections(&mut self) {
        self.step = Step::Connection;
    }
    fn begin_new_connection(&mut self, name: String) {
        if self.baseline.models.providers.is_empty()
            && self.baseline.effective_models().validate().is_ok()
        {
            self.baseline.models = self.baseline.effective_models();
        }
        self.names = self.baseline.models.providers.keys().cloned().collect();
        self.name = name;
        self.provider = ProviderSettings {
            enabled_models: Some(Vec::new()),
            ..Default::default()
        };
        self.selected.clear();
        self.profile = format!("setup_{}", uuid::Uuid::new_v4().simple());
        self.window = None;
        self.directory_for = None;
        self.cached.clear();
        self.models.clear();
        self.discovered = None;
        self.make_default = self.baseline.models.profiles.is_empty();
    }
    pub fn edit_model(&mut self) {
        self.step = Step::Model;
    }
    fn previous(&mut self) -> Result<()> {
        self.step = match self.step {
            Step::Connection => return Err(super::input::cancelled()),
            Step::Provider
                if !self.names.is_empty()
                    || self.baseline.effective_models().validate().is_ok() =>
            {
                Step::Connection
            }
            Step::Provider => return Err(super::input::cancelled()),
            Step::Url => Step::Provider,
            Step::Key if self.provider.adapter.editable_url() => Step::Url,
            Step::Key => Step::Provider,
            Step::Model => Step::Key,
            Step::Manual | Step::Context => Step::Model,
        };
        Ok(())
    }
    async fn load_directory(&mut self, prompts: &mut Prompts) -> Result<()> {
        self.cached = ModelRegistry::cached_model_ids(&self.baseline, &self.name, &self.provider);
        self.models = self.cached.clone();
        self.discovered = None;
        let force_fetch = std::mem::take(&mut self.force_fetch);
        if self.provider.discover_models || force_fetch {
            let adapter = Adapter::resolve(self.provider.adapter, &self.provider.base_url);
            let response = prompts
                .loading(
                    "Fetching model directory…",
                    tokio::time::timeout(
                        Duration::from_secs(8),
                        kyler_provider::discovery::list_models(
                            &self.provider.base_url,
                            self.provider.api_key.expose(),
                            adapter,
                        ),
                    ),
                )
                .await?;
            match response {
                Ok(Ok(entries)) => {
                    self.models = entries
                        .into_iter()
                        .filter(|m| adapter.selectable_model(&m.id))
                        .collect();
                    self.discovered = Some(self.models.clone());
                    ui::field("Models", &format!("{} available", self.models.len()));
                }
                Ok(Err(error)) => ui::warning(&format!(
                    "Model directory unavailable: {error}. Choose retry, cached models, or manual entry."
                )),
                Err(_) => ui::warning(
                    "Model directory timed out. Choose retry, cached models, or manual entry.",
                ),
            }
        }
        if self.discovered.is_none() && !self.cached.is_empty() {
            ui::field("Models", &format!("{} cached", self.cached.len()));
        }
        self.directory_for = Some((self.name.clone(), self.provider.clone()));
        Ok(())
    }
    fn directory_matches(&self) -> bool {
        self.directory_for.as_ref().is_some_and(|(name, p)| {
            name == &self.name
                && p.adapter == self.provider.adapter
                && p.base_url == self.provider.base_url
                && p.api_key.expose() == self.provider.api_key.expose()
        })
    }
    fn choose(&mut self, id: String) {
        if id != self.selected {
            self.window = None;
        }
        self.selected = id;
        if !self.names.is_empty() {
            self.profile = self
                .baseline
                .models
                .profiles
                .get(&self.baseline.models.default)
                .filter(|p| p.provider == self.name && p.model == self.selected)
                .map(|_| self.baseline.models.default.clone())
                .or_else(|| {
                    self.baseline
                        .models
                        .profiles
                        .iter()
                        .find(|(_, p)| p.provider == self.name && p.model == self.selected)
                        .map(|(n, _)| n.clone())
                })
                .unwrap_or_else(|| format!("setup_{}", uuid::Uuid::new_v4().simple()));
        }
    }
    pub async fn run(
        &mut self,
        prompts: &mut Prompts,
        config: &mut AppConfig,
        values: &mut toml::Value,
        advanced: bool,
    ) -> Result<Vec<Discovery>> {
        loop {
            let result: Result<bool> = match self.step {
                Step::Connection => {
                    let names = if self.names.is_empty() {
                        vec!["default".into()]
                    } else {
                        self.names.clone()
                    };
                    let settings = self.baseline.effective_models();
                    let mut choices = names
                        .iter()
                        .map(|name| {
                            let provider = &settings.providers[name];
                            format!(
                                "{} ({name})",
                                Adapter::resolve(provider.adapter, &provider.base_url)
                                    .0
                                    .preset()
                                    .name
                            )
                        })
                        .collect::<Vec<_>>();
                    choices.push("Add provider…".into());
                    let result = prompts.select(
                        "Provider connection",
                        choices,
                        names
                            .iter()
                            .position(|name| name == &self.name)
                            .unwrap_or(0),
                    );
                    match result {
                        Ok(index) if index == names.len() => match prompts.provider_name(&names) {
                            Ok(name) => {
                                self.begin_new_connection(name);
                                self.step = Step::Provider;
                                Ok(false)
                            }
                            Err(error) => Err(error),
                        },
                        Ok(index) => {
                            let name = &names[index];
                            if name != &self.name {
                                self.provider = settings.providers[name].clone();
                                self.selected = self
                                    .provider
                                    .default_model
                                    .clone()
                                    .or_else(|| {
                                        settings
                                            .profiles
                                            .values()
                                            .find(|p| p.provider == *name)
                                            .map(|p| p.model.clone())
                                    })
                                    .unwrap_or_default();
                                self.window = None;
                                self.name = name.clone();
                                self.directory_for = None;
                            }
                            self.step = Step::Provider;
                            Ok(false)
                        }
                        Err(error) => Err(error),
                    }
                }
                Step::Provider => prompts
                    .provider("Provider", wizard_adapter(&self.provider))
                    .map(|kind| {
                        let previous = wizard_adapter(&self.provider);
                        let previous_url = self.provider.base_url.clone();
                        if previous != kind || !kind.editable_url() {
                            self.provider.base_url = default_url(kind, &self.provider.base_url);
                        }
                        if previous != kind || previous_url != self.provider.base_url {
                            self.provider.protocol = Default::default();
                            self.provider.api_key = Default::default();
                            self.provider.api_key_env = None;
                            self.selected.clear();
                            self.window = None;
                        }
                        self.provider.adapter = kind;
                        self.step = if kind.editable_url() {
                            Step::Url
                        } else {
                            Step::Key
                        };
                        false
                    }),
                Step::Url => prompts
                    .api_url(
                        match self.provider.adapter {
                            AdapterKind::AzureOpenai => "Azure OpenAI v1 resource URL",
                            AdapterKind::Bedrock => "Bedrock regional base URL",
                            AdapterKind::VertexAi => "Vertex AI project and location URL",
                            _ => "API URL",
                        },
                        &self.provider.base_url,
                    )
                    .map(|url| {
                        if self.provider.base_url != url {
                            self.provider.api_key = Default::default();
                            self.provider.api_key_env = None;
                            self.selected.clear();
                            self.window = None;
                        }
                        self.provider.base_url = url;
                        self.step = Step::Key;
                        false
                    }),
                Step::Key => prompts
                    .key(
                        if self.provider.adapter == AdapterKind::VertexAi {
                            "Google Cloud access token"
                        } else {
                            "API key"
                        },
                        &self.provider.api_key,
                        self.provider.adapter.is_local(),
                    )
                    .map(|key| {
                        if key.expose() != self.provider.api_key.expose() {
                            self.provider.api_key_env = None;
                        }
                        self.provider.api_key = key;
                        self.step = Step::Model;
                        false
                    }),
                Step::Model => {
                    if !self.directory_matches() {
                        if let Err(e) = self.load_directory(prompts).await {
                            return Err(e);
                        }
                    }
                    match prompts.model_choice(
                        &self.selected,
                        &self.models,
                        !self.cached.is_empty(),
                    ) {
                        Ok(ModelChoice::Back) => {
                            Err(CliError::new(super::input::BACK, "Return to credentials"))
                        }
                        Ok(ModelChoice::Selected(id)) => {
                            self.choose(id);
                            if advanced {
                                self.step = Step::Context;
                            }
                            Ok(!advanced)
                        }
                        Ok(ModelChoice::Manual) => {
                            self.step = Step::Manual;
                            Ok(false)
                        }
                        Ok(ModelChoice::Retry) => {
                            self.directory_for = None;
                            self.force_fetch = true;
                            Ok(false)
                        }
                        Ok(ModelChoice::Cached) => {
                            self.models = self.cached.clone();
                            Ok(false)
                        }
                        Err(e) => Err(e),
                    }
                }
                Step::Manual => prompts.manual_model(&self.selected).map(|id| {
                    self.choose(id);
                    self.step = if advanced { Step::Context } else { Step::Model };
                    !advanced
                }),
                Step::Context => {
                    let (draft, _) = self.project();
                    let (policy, automatic) = window_policy(&draft);
                    prompts
                        .context_window(
                            "Context window (tokens or auto)",
                            (!automatic).then_some(policy.context_tokens),
                            &policy,
                        )
                        .map(|window| {
                            self.window = Some(window);
                            true
                        })
                }
            };
            match result {
                Err(e) if e.code == super::input::BACK => self.previous()?,
                Err(e) => return Err(e),
                Ok(true) => {
                    let replacing_default = self
                        .baseline
                        .models
                        .profiles
                        .get(&self.baseline.models.default)
                        .is_some_and(|profile| profile.provider == self.name)
                        && self
                            .baseline
                            .models
                            .providers
                            .get(&self.name)
                            .is_some_and(|old| {
                                old.adapter != self.provider.adapter
                                    || old.base_url != self.provider.base_url
                            });
                    if replacing_default {
                        self.make_default = true;
                    } else if !self.names.is_empty() && !self.baseline.models.default.is_empty() {
                        self.make_default = prompts.select(
                            "Default provider and model",
                            vec![
                                format!("Use {} / {} as default", self.name, self.selected),
                                "Keep current default".into(),
                            ],
                            if self.make_default { 0 } else { 1 },
                        )? == 0;
                    }
                    let (draft, raw) = self.project();
                    *config = draft;
                    *values = raw;
                    self.step = if advanced { Step::Context } else { Step::Model };
                    return Ok(self
                        .discovered
                        .clone()
                        .map(|models| vec![(self.name.clone(), self.provider.clone(), models)])
                        .unwrap_or_default());
                }
                Ok(false) => {}
            }
        }
    }
    fn project(&self) -> (AppConfig, toml::Value) {
        let mut config = self.baseline.clone();
        let mut values = self.values.clone();
        if self.names.is_empty() {
            let changed = config.llm.adapter != self.provider.adapter
                || config.llm.base_url != self.provider.base_url
                || config.llm.model != self.selected;
            if changed {
                config.llm.capabilities = Default::default();
                config.llm.reasoning_effort = None;
                config.llm.output_token_parameter = OutputTokenParameter::Auto;
                clear_model_fields(values.get_mut("llm"));
            }
            config.llm.model = self.selected.clone();
            config.llm.base_url = self.provider.base_url.clone();
            config.llm.api_key = self.provider.api_key.clone();
            config.llm.adapter = self.provider.adapter;
            config.llm.protocol = self.provider.protocol;
            if let Some(window) = self.window {
                config.context.auto_model_window = window.is_none();
                if let Some(n) = window {
                    config.context.context_tokens = n;
                }
            }
        } else {
            let changed = self
                .baseline
                .models
                .providers
                .get(&self.name)
                .is_none_or(|old| {
                    old.adapter != self.provider.adapter || old.base_url != self.provider.base_url
                });
            let profile = config
                .models
                .profiles
                .entry(self.profile.clone())
                .or_insert_with(|| ModelProfile {
                    provider: self.name.clone(),
                    model: self.selected.clone(),
                    auto_context_window: true,
                    context_tokens: config.context.context_tokens,
                    output_tokens: config.context.output_tokens,
                    reasoning_effort: None,
                    capabilities: Default::default(),
                    output_token_parameter: OutputTokenParameter::Auto,
                });
            if changed {
                profile.capabilities = Default::default();
                profile.reasoning_effort = None;
                profile.output_token_parameter = OutputTokenParameter::Auto;
                clear_model_fields(
                    values
                        .get_mut("models")
                        .and_then(|v| v.get_mut("profiles"))
                        .and_then(|v| v.get_mut(&self.profile)),
                );
            }
            if let Some(window) = self.window {
                profile.auto_context_window = window.is_none();
                if let Some(n) = window {
                    profile.context_tokens = n;
                }
            }
            if self.make_default || config.models.default.is_empty() {
                config.models.default = self.profile.clone();
            }
            if changed {
                config
                    .models
                    .profiles
                    .retain(|name, profile| profile.provider != self.name || name == &self.profile);
            }
            let mut provider = self.provider.clone();
            if changed {
                provider.enabled_models = Some(vec![self.selected.clone()]);
            }
            update_connection_defaults(&mut config, &self.name, &self.selected, &provider);
        }
        (config, values)
    }
}
fn update_connection_defaults(
    config: &mut AppConfig,
    name: &str,
    selected: &str,
    provider: &ProviderSettings,
) {
    let mut provider = provider.clone();
    provider.default_model = Some(selected.into());
    if let Some(enabled) = &mut provider.enabled_models {
        if !enabled.iter().any(|model| model == selected) {
            enabled.push(selected.into());
        }
    }
    config.models.providers.insert(name.into(), provider);
}

pub(super) fn fill_auto_thinking(config: &mut AppConfig, discovered: &[Discovery]) {
    let settings = config.effective_models();
    for (name, profile) in &settings.profiles {
        if profile.reasoning_effort.is_some() {
            continue;
        }
        let provider = &settings.providers[&profile.provider];
        let metadata = discovered
            .iter()
            .find(|(id, settings, _)| {
                id == &profile.provider && settings.base_url == provider.base_url
            })
            .and_then(|(_, _, entries)| entries.iter().find(|entry| entry.id == profile.model))
            .map(|entry| &entry.capabilities);
        let capabilities = Adapter::resolve(provider.adapter, &provider.base_url)
            .resolve_capabilities(&profile.model, &profile.capabilities, metadata, None)
            .unwrap_or_default();
        if capabilities
            .reasoning
            .as_ref()
            .is_none_or(|caps| caps.levels.is_empty())
        {
            let auto = Some(kyler_protocol::api::models::ThinkingLevel::Auto);
            if config.models.profiles.is_empty() {
                config.llm.reasoning_effort = auto;
            } else if let Some(profile) = config.models.profiles.get_mut(name) {
                profile.reasoning_effort = auto;
            }
        }
    }
}

fn window_policy(config: &AppConfig) -> (ContextPolicy, bool) {
    let mut policy = ContextPolicy::from(&config.context);
    if let Some(profile) = config.models.profiles.get(&config.models.default) {
        policy.context_tokens = profile.context_tokens;
        policy.output_tokens = profile.output_tokens;
        (policy, profile.auto_context_window)
    } else {
        (policy, config.context.auto_model_window)
    }
}
pub(super) fn configuration_details(
    config: &AppConfig,
    advanced: bool,
    access: Option<&str>,
    discovered: &[Discovery],
) -> Result<String> {
    let settings = config.effective_models();
    let profile = settings
        .profiles
        .get(&settings.default)
        .ok_or_else(|| CliError::new(2, "Select a model before saving"))?;
    let provider = &settings.providers[&profile.provider];
    let cached = ModelRegistry::cached_model_ids(config, &profile.provider, provider);
    let entries = discovered
        .iter()
        .find(|(name, settings, _)| {
            name == &profile.provider
                && settings.base_url == provider.base_url
                && settings.adapter == provider.adapter
        })
        .map(|(_, _, entries)| entries.as_slice())
        .unwrap_or(&cached);
    let metadata = entries
        .iter()
        .find(|entry| entry.id == profile.model)
        .map(|entry| &entry.capabilities);
    let mut details = vec![format!(
        "Provider  {}",
        Adapter::resolve(provider.adapter, &provider.base_url)
            .0
            .preset()
            .name
    )];
    details.push(summary(
        Adapter::resolve(provider.adapter, &provider.base_url),
        &profile.model,
        &profile.capabilities,
        profile.reasoning_effort,
        &window_policy(config).0,
        window_policy(config).1,
        advanced,
        metadata,
    )?);
    if let Some(access) = access {
        details.push(format!("Access    {access}"));
    }
    Ok(details.join("\n"))
}

fn wizard_adapter(provider: &ProviderSettings) -> AdapterKind {
    let inferred = Adapter::resolve(AdapterKind::Auto, &provider.base_url).0;
    if provider.adapter == AdapterKind::Generic
        || (matches!(
            provider.adapter,
            AdapterKind::Openai | AdapterKind::Siliconflow
        ) && inferred == AdapterKind::Generic)
    {
        AdapterKind::Generic
    } else if provider.adapter == AdapterKind::Auto {
        inferred
    } else {
        provider.adapter
    }
}
fn default_url(kind: AdapterKind, current: &str) -> String {
    kind.preset().base_url.unwrap_or(current).into()
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
    advanced: bool,
    metadata: Option<&kyler_protocol::api::models::ModelCapabilities>,
) -> Result<String> {
    let resolved = adapter
        .resolve_capabilities(model, overrides, metadata, effort)
        .map_err(|error| CliError::new(2, error.to_string()))?;
    let capacity = resolved.max_context_tokens;
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
    let reasoning = resolved.reasoning.as_ref();
    let level = effort
        .or_else(|| reasoning.map(|r| r.default))
        .unwrap_or(kyler_protocol::api::models::ThinkingLevel::Auto);
    let mut details = vec![
        format!("Model     {model}"),
        format!(
            "Thinking  {}",
            match (reasoning, level) {
                (_, kyler_protocol::api::models::ThinkingLevel::Auto) => "Auto",
                (None, _) => "Auto",
                (_, kyler_protocol::api::models::ThinkingLevel::Off) => "Off",
                (_, kyler_protocol::api::models::ThinkingLevel::Low) => "Low",
                (_, kyler_protocol::api::models::ThinkingLevel::Medium) => "Medium",
                (_, kyler_protocol::api::models::ThinkingLevel::High) => "High",
                (_, kyler_protocol::api::models::ThinkingLevel::Xhigh) => "Extra high",
                (_, kyler_protocol::api::models::ThinkingLevel::Max) => "Max",
                (_, kyler_protocol::api::models::ThinkingLevel::Ultra) => "Ultra",
            }
        ),
    ];
    if advanced {
        details.push(match capacity {
            Some(tokens) => format!("Capacity  {tokens} tokens"),
            None => "Capacity  Unknown".into(),
        });
        details.push(format!(
            "Context   {} tokens ({})",
            effective.context_tokens,
            if !automatic {
                "explicit override; preserved"
            } else if capacity.is_some() {
                "automatic"
            } else {
                "fallback budget; not a verified model capacity"
            }
        ));
        if let Some(reasoning) = reasoning.filter(|r| !r.levels.is_empty()) {
            details.push(format!(
                "Levels    {}",
                reasoning
                    .levels
                    .iter()
                    .map(|level| level.reasoning_effort())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(details.join("\n"))
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
    fn editing_a_draft_model_does_not_accumulate_profiles_or_publish_secrets() {
        let config = kyler_config::loader::parse(
            r#"[models]
default="original"
[models.providers.local]
base_url="http://localhost:1234/v1"
api_key="fixture-private"
[models.profiles.original]
provider="local"
model="original"
context_tokens=64000
output_tokens=8192
"#,
        )
        .unwrap();
        let raw = toml::from_str("[models]\ndefault=\"original\"\n").unwrap();
        let mut wizard = Wizard::new(&config, &raw);
        wizard.choose("draft-one".into());
        assert_eq!(wizard.project().0.models.profiles.len(), 2);
        wizard.choose("draft-two".into());
        let (projected, _) = wizard.project();
        assert_eq!(projected.models.profiles.len(), 2);
        assert!(
            !projected
                .models
                .profiles
                .values()
                .any(|p| p.model == "draft-one")
        );
        assert_eq!(config.models.profiles.len(), 1);
        assert_eq!(config.models.default, "original");
    }
    #[test]
    fn adding_connection_preserves_legacy_provider_and_default() {
        let mut config = AppConfig::default();
        config.llm.base_url = "http://localhost:1234/v1".into();
        config.llm.model = "existing-model".into();
        config.llm.api_key = kyler_config::SecretString::new("EXISTING_TEST_KEY");
        let original = config.effective_models();
        let mut wizard = Wizard::new(&config, &toml::Value::Table(Default::default()));
        wizard.begin_new_connection("work".into());
        wizard.provider.base_url = "http://localhost:5678/v1".into();
        wizard.provider.api_key = kyler_config::SecretString::new("NEW_TEST_KEY");
        wizard.choose("new-model".into());
        let (projected, _) = wizard.project();
        assert_eq!(projected.models.default, original.default);
        assert_eq!(projected.models.providers.len(), 2);
        assert_eq!(
            projected.models.providers["default"].base_url,
            original.providers["default"].base_url
        );
        assert_eq!(
            projected.models.providers["work"].default_model.as_deref(),
            Some("new-model")
        );
        projected.models.validate().unwrap();
        wizard.make_default = true;
        let (projected, _) = wizard.project();
        assert_eq!(
            projected.models.profiles[&projected.models.default].provider,
            "work"
        );
        projected.models.validate().unwrap();
    }

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
    fn custom_connections_default_to_custom_and_back_navigation_skips_fixed_urls() {
        let config = AppConfig::default();
        let mut wizard = Wizard::new(&config, &toml::Value::Table(Default::default()));
        wizard.provider.adapter = AdapterKind::Openai;
        wizard.provider.base_url = "http://localhost:1234/v1".into();
        assert_eq!(wizard_adapter(&wizard.provider), AdapterKind::Generic);
        wizard.step = Step::Key;
        wizard.previous().unwrap();
        assert!(matches!(wizard.step, Step::Provider));
        wizard.provider.adapter = AdapterKind::Generic;
        wizard.step = Step::Key;
        wizard.previous().unwrap();
        assert!(matches!(wizard.step, Step::Url));
        assert_eq!(
            default_url(AdapterKind::Generic, "http://localhost:1234/v1"),
            "http://localhost:1234/v1"
        );
    }
    #[test]
    fn provider_selection_uses_fixed_official_urls_and_preserves_custom_urls() {
        assert_eq!(
            default_url(AdapterKind::Openai, "https://api.siliconflow.cn/v1"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            default_url(AdapterKind::Siliconflow, "http://localhost:1234/v1"),
            "https://api.siliconflow.cn/v1"
        );
    }
}

#[cfg(test)]
mod auto_fallback_tests {
    use super::*;
    use kyler_protocol::api::models::ThinkingLevel;

    #[test]
    fn unknown_models_fill_auto_for_legacy_and_named_profiles() {
        let mut config = AppConfig::default();
        config.llm.base_url = "http://localhost:1234/v1".into();
        config.llm.model = "unknown-model".into();
        fill_auto_thinking(&mut config, &[]);
        assert_eq!(config.llm.reasoning_effort, Some(ThinkingLevel::Auto));
        config.llm.reasoning_effort = None;
        config.models = config.effective_models();
        fill_auto_thinking(&mut config, &[]);
        assert_eq!(
            config.models.profiles["default"].reasoning_effort,
            Some(ThinkingLevel::Auto)
        );
    }

    #[test]
    fn manual_levels_are_kept_without_capability_discovery() {
        let mut config = AppConfig::default();
        config.llm.model = "unknown-model".into();
        config.llm.reasoning_effort = Some(ThinkingLevel::Ultra);
        fill_auto_thinking(&mut config, &[]);
        assert_eq!(config.llm.reasoning_effort, Some(ThinkingLevel::Ultra));
    }
}
