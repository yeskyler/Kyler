use std::collections::HashMap;

use kyler_config::{AppConfig, SecretString, loader};

const VALID: &str = r#"
[llm]
base_url = "http://localhost:1234/v1"
model = "local-model"
api_key = "file-key"
"#;

#[test]
fn defaults_validation_and_unknown_fields() {
    let config = loader::parse(VALID).unwrap();
    config.validate().unwrap();
    assert_eq!(config.agent.max_steps, 50);
    assert!(!config.session.enabled && !config.subagent.enabled);
    assert!(loader::parse(&format!("{VALID}\n[agent]\nmax_step = 3")).is_err());
    assert!(loader::parse(&format!("{VALID}\n[embed]\nmodel = 'obsolete'")).is_err());
    for invalid in [
        "[agent]\nmax_steps=0",
        "[context]\nwindow_ratio=0.9",
        "[subagent]\npersist_sessions=true",
        "[logging]\nfilter='info['",
    ] {
        assert!(
            loader::parse(&format!("{VALID}\n{invalid}"))
                .unwrap()
                .validate()
                .is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn environment_overrides_config_and_ignores_dotenv_without_mutating_process() {
    let directory = tempfile::tempdir().unwrap();
    let config_dir = directory.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();
    let path = config_dir.join("default.toml");
    std::fs::write(&path, VALID).unwrap();
    std::fs::write(
        directory.path().join(".env"),
        "KYLER_LLM_API_KEY=dotenv-key\nKYLER_LLM_MODEL=dotenv-model\n",
    )
    .unwrap();
    let before = std::env::var_os("KYLER_LLM_API_KEY");
    let environment = HashMap::from([
        ("OPENAI_API_KEY".into(), "process-key".into()),
        ("KYLER_MAX_STEPS".into(), "7".into()),
        ("KYLER_SESSION_DIR".into(), "runtime-data".into()),
    ]);
    let config = loader::load_from_with_env(&path, &environment).unwrap();
    assert_eq!(config.llm.api_key.expose(), "process-key");
    assert_eq!(config.llm.model, "local-model");
    assert_eq!(config.agent.max_steps, 7);
    assert_eq!(
        config.session.directory,
        directory
            .path()
            .canonicalize()
            .unwrap()
            .join("runtime-data")
    );
    assert_eq!(std::env::var_os("KYLER_LLM_API_KEY"), before);
    let fallback = loader::load_from_with_env(&path, &HashMap::new()).unwrap();
    assert_eq!(fallback.llm.api_key.expose(), "file-key");
    assert_eq!(fallback.llm.model, "local-model");
}

#[test]
fn credential_debug_and_parse_errors_do_not_echo_secrets() {
    let mut config = loader::parse(VALID).unwrap();
    config.llm.api_key = SecretString::new("PRIVATE_API_KEY");
    let debug = format!("{config:?}");
    assert!(!debug.contains("PRIVATE_API_KEY"));
    let error = loader::parse("[llm]\napi_key={ PRIVATE_PARSE_SECRET='value' }").unwrap_err();
    assert!(!format!("{error:#}").contains("PRIVATE_PARSE_SECRET"));
    let mut config = AppConfig::default();
    config.llm.base_url = "https://username:PRIVATE_URL_PASSWORD@example.com/v1".into();
    assert!(!format!("{:#}", config.validate().unwrap_err()).contains("PRIVATE_URL_PASSWORD"));
}

#[test]
fn server_remote_binding_and_browser_origins_are_explicit() {
    use kyler_config::ServerSettings;

    let mut server = ServerSettings {
        bind: "0.0.0.0:3000".parse().unwrap(),
        ..Default::default()
    };
    assert!(server.validate().is_err());
    server.access_token = SecretString::new("access-token");
    server.validate().unwrap();
    for origin in [
        "*",
        "https://example.com/path",
        "https://name:password@example.com",
    ] {
        server.allowed_origins = vec![origin.into()];
        assert!(server.validate().is_err());
    }
    server.allowed_origins = vec!["https://example.com".into()];
    server.validate().unwrap();
}

#[test]
fn client_load_requires_no_model_configuration_and_obeys_connection_overrides() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("kyler.toml");
    std::fs::write(&path, "[client]\nserver_url='http://file:3000'\n").unwrap();
    std::fs::write(directory.path().join(".env"),"KYLER_CLIENT_SERVER_URL=http://dotenv:3000\nKYLER_SERVER_TOKEN=dotenv-token\nKYLER_SERVER_BIND=invalid\n").unwrap();
    let environment = HashMap::from([
        (
            "KYLER_CLIENT_SERVER_URL".into(),
            "http://environment:3000".into(),
        ),
        ("KYLER_CLIENT_TOKEN".into(), "process-token".into()),
        ("KYLER_MAX_STEPS".into(), "invalid".into()),
    ]);
    assert!(loader::load_from_with_env(&path, &environment).is_err());
    let client = loader::load_client_with_env(Some(&path), None, &environment).unwrap();
    assert_eq!(client.server_url, "http://environment:3000");
    assert_eq!(client.access_token.expose(), "process-token");
    let client =
        loader::load_client_with_env(Some(&path), Some("http://flag:3000"), &environment).unwrap();
    assert_eq!(client.server_url, "http://flag:3000");
    for invalid in [
        "file:///tmp/socket",
        "https://user:secret@example.com",
        "https://example.com?token=secret",
    ] {
        assert!(loader::load_client_with_env(Some(&path), Some(invalid), &environment).is_err());
    }
}

#[test]
fn launch_overrides_apply_before_validation_and_do_not_disclose_keys() {
    use kyler_config::loader::{LlmOverrides, load_with_llm_overrides_and_env};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("kyler.toml");
    std::fs::write(&path, "[llm]\nmodel=''\nbase_url=''\napi_key=''\n").unwrap();
    let environment = HashMap::from([
        ("KYLER_LLM_MODEL".into(), "environment-model".into()),
        ("KYLER_LLM_BASE_URL".into(), "invalid-url".into()),
    ]);
    assert!(loader::load_from_with_env(&path, &environment).is_err());
    let overrides = LlmOverrides {
        model: Some("launch-model".into()),
        base_url: Some("http://localhost:1234/v1".into()),
        api_key: Some(SecretString::new("PRIVATE_LAUNCH_KEY")),
    };
    let config = load_with_llm_overrides_and_env(Some(&path), &overrides, &environment).unwrap();
    assert_eq!(config.llm.model, "launch-model");
    assert_eq!(config.llm.base_url, "http://localhost:1234/v1");
    assert_eq!(config.llm.api_key.expose(), "PRIVATE_LAUNCH_KEY");
    assert!(!format!("{overrides:?} {config:?}").contains("PRIVATE_LAUNCH_KEY"));
}

#[test]
fn default_config_uses_toml_and_ignores_user_dotenv() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().canonicalize().unwrap();
    let base = home.join(".kyler");
    std::fs::create_dir(&base).unwrap();
    std::fs::write(
        base.join("config.toml"),
        format!("{VALID}\n[client]\nserver_url='http://user-config:3000'\n"),
    )
    .unwrap();
    std::fs::write(
        base.join(".env"),
        "KYLER_LLM_MODEL=user-dotenv-model\nKYLER_CLIENT_TOKEN=user-dotenv-token\nBROKEN='unterminated\n",
    )
    .unwrap();
    let environment = HashMap::from([("HOME".into(), home.to_string_lossy().into_owned())]);
    let config =
        loader::load_with_llm_overrides_and_env(None, &Default::default(), &environment).unwrap();
    assert_eq!(config.llm.model, "local-model");
    assert_eq!(config.session.directory, base.join("sessions"));
    let client = loader::load_client_with_env(None, None, &environment).unwrap();
    assert_eq!(client.server_url, "http://user-config:3000");
    assert!(client.access_token.is_empty());
    std::fs::write(base.join("config.toml"), "[invalid]\nvalue=true\n").unwrap();
    assert!(loader::load_client_with_env(None, None, &environment).is_err());
}

#[test]
fn missing_user_config_uses_user_directory_defaults_without_creating_files() {
    use loader::{LlmOverrides, load_with_llm_overrides_and_env};

    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().canonicalize().unwrap();
    let environment = HashMap::from([("HOME".into(), home.to_string_lossy().into_owned())]);
    let overrides = LlmOverrides {
        model: Some("model".into()),
        base_url: Some("http://localhost:1234/v1".into()),
        api_key: Some(SecretString::new("test")),
    };
    let config = load_with_llm_overrides_and_env(None, &overrides, &environment).unwrap();
    assert_eq!(config.session.directory, home.join(".kyler/sessions"));
    assert!(!home.join(".kyler").exists());
    assert_eq!(
        loader::load_client_with_env(None, None, &environment)
            .unwrap()
            .server_url,
        "http://127.0.0.1:30001"
    );
    assert!(loader::load_client_with_env(None, None, &HashMap::new()).is_err());
}

#[test]
fn config_selection_obeys_explicit_then_environment_then_user_default() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().canonicalize().unwrap();
    let base = home.join(".kyler");
    std::fs::create_dir(&base).unwrap();
    let explicit = home.join("explicit.toml");
    let selected = home.join("selected.toml");
    for (path, host) in [
        (base.join("config.toml"), "default"),
        (explicit.clone(), "explicit"),
        (selected.clone(), "environment"),
    ] {
        std::fs::write(path, format!("[client]\nserver_url='http://{host}:3000'\n")).unwrap();
    }
    let mut environment = HashMap::from([
        ("HOME".into(), home.to_string_lossy().into_owned()),
        (
            "KYLER_CONFIG".into(),
            selected.to_string_lossy().into_owned(),
        ),
    ]);
    assert_eq!(
        loader::load_client_with_env(Some(&explicit), None, &environment)
            .unwrap()
            .server_url,
        "http://explicit:3000"
    );
    assert_eq!(
        loader::load_client_with_env(None, None, &environment)
            .unwrap()
            .server_url,
        "http://environment:3000"
    );
    environment.insert(
        "KYLER_CONFIG".into(),
        home.join("missing.toml").to_string_lossy().into_owned(),
    );
    assert!(loader::load_client_with_env(None, None, &environment).is_err());
    environment.remove("KYLER_CONFIG");
    assert_eq!(
        loader::load_client_with_env(None, None, &environment)
            .unwrap()
            .server_url,
        "http://default:3000"
    );
}

#[test]
fn mcp_config_validates_transport_and_resolves_only_named_environment_references() {
    use kyler_config::loader;

    let mut config = loader::parse(
        r#"
[mcp.servers.docs]
transport = "streamable_http"
url = "https://example.com/mcp"
bearer_token_env = "DOCS_TOKEN"
activity = "search"
[mcp.servers.local]
transport = "stdio"
command = "/usr/bin/example"
[mcp.servers.local.env]
API_KEY = "LOCAL_TOKEN"
"#,
    )
    .unwrap();
    config.mcp.validate().unwrap();
    config.mcp.resolve_environment(&HashMap::from([
        ("DOCS_TOKEN".into(), "private-http-key".into()),
        ("LOCAL_TOKEN".into(), "private-process-key".into()),
        ("OTHER_TOKEN".into(), "unrelated-secret".into()),
    ]));
    assert_eq!(
        config.mcp.servers["docs"]
            .resolved_token
            .as_ref()
            .unwrap()
            .expose(),
        "private-http-key"
    );
    assert_eq!(config.mcp.servers["local"].resolved_env.len(), 1);
    let debug = format!("{:?}", config.mcp);
    assert!(
        !debug.contains("private-http-key")
            && !debug.contains("private-process-key")
            && !debug.contains("unrelated-secret")
    );
    config.mcp.servers.get_mut("local").unwrap().command = "relative".into();
    assert!(config.mcp.validate().is_err());
}

#[test]
fn skill_configuration_uses_user_paths_and_rejects_project_traversal() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(
        &config,
        "[llm]\napi_key='fixture'\nmodel='test'\nbase_url='https://example.com/v1'\n",
    )
    .unwrap();
    let environment = HashMap::from([("HOME".into(), root.path().to_string_lossy().into_owned())]);
    let mut loaded = kyler_config::loader::load_from_with_env(&config, &environment).unwrap();
    assert_eq!(
        loaded.skills.user_directory,
        root.path().join(".kyler/skills")
    );
    assert_eq!(
        loaded.skills.state_file,
        root.path().join(".kyler/skills-state.json")
    );
    loaded.skills.project_directory = "../skills".into();
    assert!(loaded.skills.validate().is_err());
    assert!(!loaded.skills.state_file.exists());
}

#[test]
fn enabled_mcp_credentials_fail_early_and_disabled_integrations_need_no_secret() {
    let mut config = kyler_config::loader::parse(
        r#"
[mcp.servers.secured]
transport = "streamable_http"
url = "https://example.com/mcp"
bearer_token_env = "EXAMPLE_API_KEY"
"#,
    )
    .unwrap();
    let message = config.mcp.validate_credentials().unwrap_err().to_string();
    assert!(message.contains("EXAMPLE_API_KEY"));
    config.mcp.servers.get_mut("secured").unwrap().enabled = false;
    config.mcp.validate_credentials().unwrap();
    config.mcp.servers.get_mut("secured").unwrap().enabled = true;
    config.mcp.resolve_environment(&HashMap::from([(
        "EXAMPLE_API_KEY".into(),
        "captured-test-key".into(),
    )]));
    config.mcp.validate_credentials().unwrap();
}

#[test]
fn obsolete_search_configuration_is_ignored_without_hiding_unknown_settings() {
    let config = loader::parse(&format!(
        "{VALID}\n[tools.tavily]\nenabled=true\napi_key='obsolete-secret'\n"
    ))
    .unwrap();
    config.validate().unwrap();
    assert!(!format!("{config:?}").contains("obsolete-secret"));
    assert!(loader::parse(&format!("{VALID}\n[tools.other]\nenabled=true")).is_err());
}

#[test]
fn local_agent_ignores_service_and_client_transport_settings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("local.toml");
    std::fs::write(&path, format!("{VALID}\n[client]\nserver_url='not-a-url'\n[server]\nbind='0.0.0.0:30001'\n[web]\nbind='0.0.0.0:30002'\n")).unwrap();
    let environment = HashMap::from([
        ("KYLER_SERVER_BIND".into(), "invalid-address".into()),
        ("KYLER_WEB_BIND".into(), "invalid-address".into()),
        ("KYLER_CLIENT_SERVER_URL".into(), "invalid-url".into()),
    ]);
    let config = loader::load_local_with_env(Some(&path), &environment).unwrap();
    assert_eq!(config.llm.model, "local-model");
    assert!(config.server.bind.ip().is_loopback());
    assert!(config.web.bind.ip().is_loopback());
    assert!(loader::load_from_with_env(&path, &environment).is_err());
}

#[test]
fn thinking_flags_parse_for_legacy_and_named_models_without_forcing_other_providers() {
    let config = loader::parse(VALID).unwrap();
    assert_eq!(
        config.effective_models().profiles["default"].enable_thinking,
        None
    );
    for enabled in [true, false] {
        let config = loader::parse(&format!("{VALID}\nenable_thinking={enabled}\n")).unwrap();
        assert_eq!(
            config.effective_models().profiles["default"].enable_thinking,
            Some(enabled)
        );
    }
    let config = loader::parse(
        r#"
[models]
default="fast"
[models.providers.fixture]
base_url="http://localhost:1234/v1"
api_key="fixture"
[models.profiles.fast]
provider="fixture"
model="fixture"
context_tokens=64000
output_tokens=2048
enable_thinking=false
"#,
    )
    .unwrap();
    config.validate().unwrap();
    assert_eq!(
        config.effective_models().profiles["fast"].enable_thinking,
        Some(false)
    );
}
