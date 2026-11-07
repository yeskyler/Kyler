use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use axum::{Json, Router, response::IntoResponse, routing::post};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpListener;

use kyler_config::{AppConfig, SecretString};
use kyler_core::{AgentStreamEvent, Session, runtime::UserTurn};
use kyler_protocol::api::mcp::McpStatus;
use kyler_server::Server;

use super::Scenario;
use crate::{
    config::EvalConfig,
    runtime::{Runtime, TimingData, Timings},
};

pub struct MockServer {
    pub url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub async fn mock_server() -> Result<MockServer> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/v1", listener.local_addr()?);
    let router=Router::new().route("/v1/chat/completions",post(|Json(request):Json<serde_json::Value>|async move {
        if request["stream"] != true {
            return Json(json!({"id":"eval","object":"chat.completion","created":0,"model":"eval-mock","choices":[{"index":0,"message":{"role":"assistant","content":"Synthetic prior context retained."},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}})).into_response();
        }
        let chunk=json!({"id":"eval","object":"chat.completion.chunk","created":0,"model":"eval-mock","choices":[{"index":0,"delta":{"role":"assistant","content":"FINAL ANSWER: 42"},"finish_reason":null}]});
        let end=json!({"id":"eval","object":"chat.completion.chunk","created":0,"model":"eval-mock","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}});
        ([("content-type","text/event-stream")],format!("data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n")).into_response()
    }));
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(MockServer { url, task })
}
pub fn mock_config(url: &str) -> AppConfig {
    let mut config = AppConfig::default();
    config.llm.base_url = url.into();
    config.llm.model = "eval-mock".into();
    config.llm.api_key = SecretString::new("local-mock-no-secret");
    config.skills.enabled = false;
    config
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measurement {
    pub configuration: serde_json::Value,
    pub configuration_fingerprint: String,
    pub phases_ms: BTreeMap<String, f64>,
    pub timings: TimingData,
    pub first_text_ms: Option<f64>,
    pub turn_ms: Option<f64>,
    pub peak_rss_bytes: Option<u64>,
    pub cpu_seconds: Option<f64>,
    pub ready_scope: String,
    #[serde(default)]
    pub warmup_turn_ms: Option<f64>,
    #[serde(default)]
    pub restore_ms: Option<f64>,
    #[serde(default)]
    pub capabilities_evaluated: bool,
    #[serde(default)]
    pub degraded: Vec<String>,
}
fn emit(value: serde_json::Value) -> Result<()> {
    let event = value["event"].as_str().context("Missing probe event")?;
    kyler_protocol::startup::emit(event, value.clone());
    Ok(())
}
pub async fn run(
    settings: &super::Settings,
    private: &Path,
    app_config: Option<&Path>,
    eval_config: Option<&Path>,
    worker: &Path,
    public_configuration: Option<&Path>,
) -> Result<()> {
    let started = Instant::now();
    let app = crate::runtime::load_app(app_config)?;
    let mut eval = EvalConfig::load(eval_config)?;
    let configuration = if let Some(path) = public_configuration {
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        crate::runtime::public_configuration(&app, &eval)?
    };
    let configuration_fingerprint = crate::report::fingerprint(&configuration)?;
    let config_ms = started.elapsed().as_secs_f64() * 1000.0;
    kyler_protocol::startup::mark("configuration_ready");
    eval.run.results_directory = private.canonicalize()?;
    let runtime = Runtime::build(&app, &eval, private, worker).await?;
    kyler_protocol::startup::mark("agent_ready");
    kyler_protocol::startup::mark("runtime_ready");
    let mut phases = runtime.phases_ms.clone();
    phases.insert("configuration".into(), config_ms);
    let mut session = Session::new();
    session.environment = runtime.agent.workspace_environment().cloned();
    let mut first_text = None;
    let mut turn_ms = None;
    let mut warmup_turn_ms = None;
    let mut restore_ms = None;
    let mut degraded = Vec::new();
    let mut capabilities_evaluated = false;
    let operation = async {
        let mut http = None;
        if settings.scenario == Scenario::Server {
            let begin = Instant::now();
            let server = Server::with_agent(
                app.server.clone(),
                Arc::new(runtime.agent.clone()),
                runtime.store.clone(),
            )
            .await?;
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(server.serve(listener, async {
                let _ = stopped.await;
            }));
            http = Some((stop, task));
            super::service::health(&address.to_string()).await?;
            kyler_protocol::startup::mark("service_ready");
            phases.insert(
                "server_state_and_http_ready".into(),
                begin.elapsed().as_secs_f64() * 1000.0,
            );
        }
        if matches!(
            settings.scenario,
            Scenario::Runtime | Scenario::Server | Scenario::FirstTurn
        ) {
            let begin = Instant::now();
            if let Some(mcp) = runtime.agent.mcp_manager() {
                mcp.snapshot(&session.session_id, runtime.agent.execution_environment())
                    .await?;
            }
            phases.insert(
                "mcp_connect_and_discovery".into(),
                begin.elapsed().as_secs_f64() * 1000.0,
            );
            let begin = Instant::now();
            if let Some(skills) = runtime.agent.skill_manager() {
                skills.snapshot(Some(runtime.workspace.path())).await?;
            }
            phases.insert("skills_scan".into(), begin.elapsed().as_secs_f64() * 1000.0);
            degraded = capability_status(&runtime.agent, &session.session_id, settings).await?;
            capabilities_evaluated = true;
            kyler_protocol::startup::mark("capabilities_ready");
        }
        emit(json!({"event":"ready"}))?;
        if settings.scenario == Scenario::SessionRestore {
            session = history_fixture(&runtime, settings.restore_turns).await?;
            if !settings.restore_snapshot {
                std::fs::remove_file(
                    runtime
                        .store
                        .directory()
                        .join(&session.session_id)
                        .join("snapshot.json"),
                )?;
            }
            kyler_protocol::startup::mark("restore_started");
            let begin = Instant::now();
            session = runtime.store.load(&session.session_id).await?;
            restore_ms = Some(begin.elapsed().as_secs_f64() * 1000.0);
            kyler_protocol::startup::mark("restore_ready");
        }
        if settings.scenario == Scenario::SessionWarm {
            let begin = Instant::now();
            let (next, _, _) = turn(&runtime.agent, session, false).await?;
            session = next;
            warmup_turn_ms = Some(begin.elapsed().as_secs_f64() * 1000.0);
            kyler_protocol::startup::mark("warmup_turn_complete");
        }
        let timing = if matches!(
            settings.scenario,
            Scenario::SessionCold
                | Scenario::SessionWarm
                | Scenario::SessionRestore
                | Scenario::FirstTurn
        ) {
            let id = session.session_id.clone();
            kyler_protocol::startup::mark("turn_submitted");
            let begin = Instant::now();
            let (_, text, timings) = turn(&runtime.agent, session, true).await?;
            first_text = text;
            turn_ms = Some(begin.elapsed().as_secs_f64() * 1000.0);
            kyler_protocol::startup::mark("turn_complete");
            degraded = capability_status(&runtime.agent, &id, settings).await?;
            capabilities_evaluated = true;
            timings
        } else {
            TimingData::default()
        };
        if let Some((stop, task)) = http {
            let _ = stop.send(());
            task.await??;
        }
        Ok::<_, anyhow::Error>(timing)
    }
    .await;
    runtime.shutdown().await;
    let timing = operation?;
    let resources = kyler_protocol::startup::resources();
    emit(
        json!({"event":"result","measurement":Measurement{configuration,configuration_fingerprint,phases_ms:phases,timings:timing,first_text_ms:first_text,turn_ms,peak_rss_bytes:resources["peak_rss_bytes"].as_u64(),cpu_seconds:resources["cpu_seconds"].as_f64(),ready_scope:if matches!(settings.scenario,Scenario::SessionCold|Scenario::SessionWarm|Scenario::SessionRestore){"Runtime ready before submission; capability readiness is observed during the measured turn".into()}else{"Legacy embedding probe: capabilities are prewarmed before readiness".into()},warmup_turn_ms,restore_ms,capabilities_evaluated,degraded}}),
    )
}
async fn turn(
    agent: &kyler_core::Agent,
    session: Session,
    measured: bool,
) -> Result<(Session, Option<f64>, TimingData)> {
    let started = Instant::now();
    let timing = if measured {
        Timings::startup()
    } else {
        Timings::new()
    };
    let mut stream = agent
        .clone()
        .with_observer(timing.clone())
        .run_turn_in_session(
            session,
            UserTurn {
                text: "Reply with exactly: ready".into(),
                ..Default::default()
            },
        )
        .await?;
    let mut first = None;
    let consume = async {
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::TextDelta { text, .. } = event
                && !text.is_empty()
                && first.is_none()
            {
                first = Some(started.elapsed().as_secs_f64() * 1000.0);
                if measured {
                    kyler_protocol::startup::mark("first_text");
                }
            }
        }
    };
    if tokio::time::timeout(Duration::from_secs(90), consume)
        .await
        .is_err()
    {
        stream.cancel();
    }
    let outcome =
        tokio::time::timeout(Duration::from_secs(30), stream.finish_with_session()).await??;
    outcome.result?;
    ensure!(first.is_some(), "Model returned no visible text");
    Ok((outcome.session, first, timing.snapshot()))
}
async fn capability_status(
    agent: &kyler_core::Agent,
    session: &str,
    settings: &super::Settings,
) -> Result<Vec<String>> {
    let mut degraded = Vec::new();
    let states = if let Some(mcp) = agent.mcp_manager() {
        mcp.list(session, agent.execution_environment()).await?
    } else {
        Vec::new()
    };
    for id in &settings.required_mcp {
        ensure!(
            states.iter().any(|s| &s.id == id),
            "Required MCP connector is not configured: {id}"
        );
    }
    for state in states {
        if state.status != McpStatus::Connected {
            ensure!(
                !settings.required_mcp.contains(&state.id)
                    && settings.profile != super::Profile::Mcp,
                "Required MCP connector failed readiness: {}",
                state.id
            );
            degraded.push(format!("MCP connector {} is not connected", state.id));
        }
    }
    Ok(degraded)
}
async fn history_fixture(runtime: &Runtime, turns: usize) -> Result<Session> {
    use kyler_core::context::{ContentItem, Event, ExecutionStatus, Role};
    let mut session = Session::new();
    session.environment = runtime.agent.workspace_environment().cloned();
    for index in 0..turns {
        session.begin_execution(&format!(
            "Synthetic history turn {index}: {}",
            "fixture context ".repeat(16)
        ))?;
        session.add_event(Event::new(
            session.current_execution()?.execution_id.clone(),
            session.author.clone(),
            vec![ContentItem::Message {
                role: Role::Assistant,
                content: format!("Synthetic response {index}"),
            }],
        ));
        session.finish(
            ExecutionStatus::Completed,
            Some(format!("Synthetic response {index}")),
            None,
        )?;
    }
    runtime.store.save(&mut session).await?;
    Ok(session)
}
