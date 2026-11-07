use std::collections::HashMap;

use kyler_config::{OutputTokenParameter, loader};

const CONFIG: &str = r#"
[models]
default = "fast"
[models.providers.fixture]
base_url = "http://127.0.0.1:1234/v1"
api_key_env = "FIXTURE_MODEL_KEY"
[models.profiles.fast]
provider = "fixture"
model = "fast-model"
context_tokens = 64000
output_tokens = 2048
"#;
#[test]
fn profiles_resolve_credentials_without_requiring_legacy_llm_settings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, CONFIG).unwrap();
    std::fs::write(
        directory.path().join(".env"),
        "FIXTURE_MODEL_KEY=DOTENV_SECRET",
    )
    .unwrap();
    let config = loader::load_from_with_env(
        &path,
        &HashMap::from([("FIXTURE_MODEL_KEY".into(), "PROCESS_SECRET".into())]),
    )
    .unwrap();
    assert!(config.llm.model.is_empty());
    assert_eq!(
        config.models.providers["fixture"].api_key.expose(),
        "PROCESS_SECRET"
    );
    assert_eq!(
        config.models.profiles["fast"].output_token_parameter,
        OutputTokenParameter::MaxCompletionTokens
    );
    assert!(!format!("{config:?}").contains("PROCESS_SECRET"));
    let overrides = loader::LlmOverrides {
        model: Some("override-model".into()),
        ..Default::default()
    };
    let config = loader::load_with_llm_overrides_and_env(
        Some(&path),
        &overrides,
        &HashMap::from([("FIXTURE_MODEL_KEY".into(), "PROCESS_SECRET".into())]),
    )
    .unwrap();
    assert_eq!(config.models.profiles["fast"].model, "override-model");
    assert!(
        loader::load_from_with_env(&path, &HashMap::new()).is_err(),
        "A .env file must not supply a provider credential"
    );
}
#[test]
fn invalid_profiles_are_rejected_and_legacy_configuration_still_works() {
    let config = loader::parse(
        r#"[llm]
base_url = "http://localhost:1234/v1"
model = "legacy"
api_key = "fixture"
"#,
    )
    .unwrap();
    config.validate().unwrap();
    let models = config.effective_models();
    assert_eq!(
        models.profiles["default"].output_token_parameter,
        OutputTokenParameter::MaxTokens
    );
    let valid = CONFIG.replace(
        "api_key_env = \"FIXTURE_MODEL_KEY\"",
        "api_key = \"fixture\"",
    );
    loader::parse(&valid).unwrap().validate().unwrap();
    for invalid in [
        valid.replace("provider = \"fixture\"", "provider = \"missing\""),
        valid.replace("default = \"fast\"", "default = \"missing\""),
    ] {
        assert!(loader::parse(&invalid).unwrap().validate().is_err());
    }
}
