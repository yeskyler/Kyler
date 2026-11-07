//! Reproducible profiles with temporary configuration and private service storage.
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use serde_json::json;
use tokio::net::TcpListener;

use kyler_config::AppConfig;

use super::{ModelMode, Profile, Settings};
use crate::config::EvalConfig;

pub const ISOLATION_OVERRIDES: &[&str] = &[
    "KYLER_SERVER_BIND",
    "KYLER_SERVER_TOKEN",
    "KYLER_SESSION_DIR",
    "KYLER_SESSION_ENABLED",
];
pub const MODEL_OVERRIDES: &[&str] = &[
    "KYLER_LLM_MODEL",
    "KYLER_LLM_BASE_URL",
    "KYLER_LLM_API_KEY",
    "OPENAI_API_KEY",
];

pub struct Prepared {
    pub config: AppConfig,
    pub eval: EvalConfig,
    pub config_path: PathBuf,
    pub eval_path: PathBuf,
    pub public_path: PathBuf,
    pub workspace: PathBuf,
    pub configuration: serde_json::Value,
    _directory: tempfile::TempDir,
    _mcp: Option<McpFixture>,
}

fn set(value: &mut toml::Value, section: &str, key: &str, item: impl Into<toml::Value>) {
    let root = value.as_table_mut().expect("TOML root must be a table");
    let table = root
        .entry(section.to_owned())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    if !table.is_table() {
        *table = toml::Value::Table(Default::default());
    }
    table
        .as_table_mut()
        .unwrap()
        .insert(key.to_owned(), item.into());
}
fn clear(value: &mut toml::Value, section: &str) {
    value
        .as_table_mut()
        .expect("TOML root must be a table")
        .insert(section.to_owned(), toml::Value::Table(Default::default()));
}
fn resolved(path: &Path, base: &Path) -> Result<PathBuf> {
    if let Ok(suffix) = path.strip_prefix("~") {
        return Ok(PathBuf::from(std::env::var_os("HOME").context("HOME missing")?).join(suffix));
    }
    Ok(if path.is_absolute() {
        path.into()
    } else {
        base.join(path)
    })
}
pub async fn prepare(
    settings: &Settings,
    app_config: Option<&Path>,
    eval_config: Option<&Path>,
    mock_url: Option<&str>,
) -> Result<Prepared> {
    let directory = tempfile::Builder::new()
        .prefix("kyler-startup-private-")
        .tempdir()?;
    let root = directory.path().canonicalize()?;
    let selected = app_config
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("KYLER_CONFIG").map(PathBuf::from))
        .or_else(|| {
            kyler_config::loader::default_config_path()
                .ok()
                .filter(|p| p.is_file())
        });
    let (mut raw, source_base) = if let Some(path) = selected {
        let path = path
            .canonicalize()
            .context("Application configuration not found")?;
        ensure!(
            std::fs::metadata(&path)?.len() <= 1024 * 1024,
            "Application configuration exceeds 1 MiB"
        );
        let parent = path.parent().unwrap();
        let base = if parent.file_name().is_some_and(|n| n == "config") {
            parent.parent().unwrap_or(parent)
        } else {
            parent
        };
        (
            std::fs::read_to_string(&path)?
                .parse::<toml::Value>()
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Invalid application configuration TOML; validate the source file"
                    )
                })?,
            base.to_path_buf(),
        )
    } else {
        (toml::Value::Table(Default::default()), root.clone())
    };
    let source_skills = resolved(
        Path::new(
            raw.get("skills")
                .and_then(|v| v.get("user_directory"))
                .and_then(toml::Value::as_str)
                .unwrap_or("~/.kyler/skills"),
        ),
        &source_base,
    )?;
    let source_state = resolved(
        Path::new(
            raw.get("skills")
                .and_then(|v| v.get("state_file"))
                .and_then(toml::Value::as_str)
                .unwrap_or("~/.kyler/skills-state.json"),
        ),
        &source_base,
    )?;
    let mut eval = EvalConfig::load(eval_config)?;
    match settings.profile {
        Profile::Minimal => {
            set(&mut raw, "skills", "enabled", false);
            clear(&mut raw, "mcp");
            set(&mut raw, "subagent", "enabled", false);
            eval.agent = Default::default();
        }
        Profile::Default => {
            eval.agent.load_user_skills = raw
                .get("skills")
                .and_then(|v| v.get("enabled"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(true);
            eval.agent.enable_mcp = true;
            eval.agent.enable_subagents = raw
                .get("subagent")
                .and_then(|v| v.get("enabled"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
        }
        Profile::Skills => {
            set(&mut raw, "skills", "enabled", true);
            clear(&mut raw, "mcp");
            set(&mut raw, "subagent", "enabled", false);
            eval.agent = Default::default();
            eval.agent.load_user_skills = true;
        }
        Profile::Mcp => {
            set(&mut raw, "sandbox", "network_access", true);
            set(&mut raw, "skills", "enabled", false);
            clear(&mut raw, "mcp");
            set(&mut raw, "subagent", "enabled", false);
            eval.agent = Default::default();
            eval.agent.enable_mcp = true;
        }
    }
    set(
        &mut raw,
        "subagent",
        "persist_sessions",
        eval.agent.enable_subagents,
    );
    let skills = root.join("skills");
    if settings.profile == Profile::Skills {
        for index in 0..settings.skill_count {
            let package = skills.join(format!("bench-skill-{index}"));
            std::fs::create_dir_all(&package)?;
            let header = format!(
                "---\nname: bench-skill-{index}\ndescription: Synthetic startup benchmark skill {index}.\n---\n"
            );
            std::fs::write(
                package.join("SKILL.md"),
                format!(
                    "{header}{}\n",
                    "x".repeat(
                        settings
                            .skill_instruction_bytes
                            .saturating_sub(header.len() + 1)
                    )
                ),
            )?;
        }
    } else if eval.agent.load_user_skills {
        crate::runtime::skill_tree(&source_skills, Some(&skills))?;
        if source_state.is_file() {
            std::fs::copy(source_state, root.join("skills-state.json"))?;
        }
    }
    set(
        &mut raw,
        "skills",
        "user_directory",
        skills.to_string_lossy().into_owned(),
    );
    if settings.profile == Profile::Skills {
        set(&mut raw, "skills", "max_instruction_bytes", 32768);
    }
    set(
        &mut raw,
        "skills",
        "state_file",
        root.join("skills-state.json")
            .to_string_lossy()
            .into_owned(),
    );
    let mcp = if settings.profile == Profile::Mcp {
        Some(McpFixture::start(settings.mcp_delay_ms).await?)
    } else {
        None
    };
    if let Some(mcp) = &mcp {
        let mut servers = toml::map::Map::new();
        for index in 0..settings.mcp_count {
            let mut server = toml::map::Map::new();
            server.insert("transport".into(), "streamable_http".into());
            server.insert("url".into(), mcp.url.clone().into());
            server.insert("auto_connect".into(), true.into());
            servers.insert(format!("bench_{index}"), toml::Value::Table(server));
        }
        set(&mut raw, "mcp", "servers", toml::Value::Table(servers));
    }
    if settings.model_mode == ModelMode::Mock {
        clear(&mut raw, "models");
        set(&mut raw, "llm", "model", "eval-mock");
        set(
            &mut raw,
            "llm",
            "base_url",
            mock_url.context("Mock endpoint missing")?,
        );
        set(&mut raw, "llm", "api_key", "local-mock-no-secret");
    }
    let workspace = root.join("workspace");
    std::fs::create_dir(&workspace)?;
    set(
        &mut raw,
        "workspace",
        "default_directory",
        workspace.to_string_lossy().into_owned(),
    );
    set(&mut raw, "session", "enabled", true);
    set(
        &mut raw,
        "session",
        "directory",
        root.join("sessions").to_string_lossy().into_owned(),
    );
    set(&mut raw, "server", "bind", "127.0.0.1:0");
    set(&mut raw, "server", "access_token", "");
    set(&mut raw, "logging", "ansi", false);
    // Benchmark profiles never inherit extra filesystem grants from a daily-use service.
    set(&mut raw, "sandbox", "mode", "workspace-write");
    set(
        &mut raw,
        "sandbox",
        "additional_read_roots",
        toml::Value::Array(vec![]),
    );
    set(
        &mut raw,
        "sandbox",
        "additional_write_roots",
        toml::Value::Array(vec![]),
    );
    set(
        &mut raw,
        "sandbox",
        "protected_paths",
        toml::Value::Array(vec![]),
    );
    let config_path = root.join("config.toml");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(toml::to_string(&raw)?.as_bytes())?;
    file.sync_all()?;
    let mut environment: HashMap<String, String> = std::env::vars().collect();
    for key in ISOLATION_OVERRIDES {
        environment.remove(*key);
    }
    if settings.model_mode == ModelMode::Mock {
        for key in MODEL_OVERRIDES {
            environment.remove(*key);
        }
    }
    let config = kyler_config::loader::load_from_with_env(&config_path, &environment)?;
    let mut configuration = crate::runtime::public_configuration(&config, &eval)?;
    if settings.model_mode == ModelMode::Mock {
        if let Some(providers) = configuration["providers"].as_object_mut() {
            for value in providers.values_mut() {
                *value = json!("local-mock");
            }
        }
    }
    if settings.profile == Profile::Mcp {
        configuration["connector_definition_fingerprint"]=crate::report::fingerprint(&json!({"count":settings.mcp_count,"delay_ms":settings.mcp_delay_ms,"implementation":"json-rpc-fixture-v1"}))?.into();
    }
    configuration["startup_profile"] = json!({"profile":settings.profile,"skill_count":settings.skill_count,"skill_instruction_bytes":settings.skill_instruction_bytes,"mcp_count":settings.mcp_count,"mcp_delay_ms":settings.mcp_delay_ms});
    let eval_path = root.join("eval.toml");
    std::fs::write(&eval_path, toml::to_string(&eval)?)?;
    let public_path = root.join("public.json");
    std::fs::write(&public_path, serde_json::to_vec(&configuration)?)?;
    Ok(Prepared {
        config,
        eval,
        config_path,
        eval_path,
        public_path,
        workspace,
        configuration,
        _directory: directory,
        _mcp: mcp,
    })
}

struct McpFixture {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for McpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl McpFixture {
    async fn start(delay_ms: u64) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/mcp", listener.local_addr()?);
        let router=Router::new().route("/mcp",post(move|Json(request):Json<serde_json::Value>|async move {
            if request.get("id").is_none(){return StatusCode::ACCEPTED.into_response();}
            let result=match request["method"].as_str(){
                Some("initialize")=>{tokio::time::sleep(Duration::from_millis(delay_ms)).await;json!({"protocolVersion":request["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"startup-fixture","version":"1"}})},
                Some("tools/list")=>json!({"tools":[{"name":"bench_echo","description":"Synthetic startup fixture tool","inputSchema":{"type":"object","properties":{}}}]}),
                Some("tools/call")=>json!({"content":[{"type":"text","text":"fixture"}]}),
                _=>json!({}),
            };
            Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result})).into_response()
        }));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self { url, task })
    }
}
