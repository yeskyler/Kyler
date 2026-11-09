use std::{
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::post,
};
use futures::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    sync::{oneshot, watch},
    task::JoinHandle,
};

use kyler_config::ServerSettings;
use kyler_core::{Agent, SessionStore, Tool, ToolBox, ToolContext};
use kyler_protocol::api::{
    event::ServerEvent,
    run::{RunAccepted, RunInfo},
    session::SessionSummary,
};
use kyler_server::Server;

#[derive(Clone)]
struct Model {
    calls: Arc<AtomicUsize>,
    release: watch::Sender<bool>,
    requests: Arc<Mutex<Vec<Value>>>,
}
fn chunk(delta: Value, reason: Value) -> String {
    json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"test","choices":[{"index":0,"delta":delta,"finish_reason":reason}]}).to_string()
}
async fn completion(State(model): State<Model>, Json(request): Json<Value>) -> Response {
    model.calls.fetch_add(1, Ordering::SeqCst);
    model.requests.lock().unwrap().push(request.clone());
    let input = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .and_then(|m| {
            m["content"]
                .as_str()
                .or_else(|| m["content"][0]["text"].as_str())
        })
        .unwrap_or_default()
        .to_owned();
    if input == "reject-image" {
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"code":20041,"message":"The model is not a VLM (Vision Language Model). PRIVATE_PROVIDER_VALUE data:image/png;base64,PRIVATE_IMAGE","data":null}))).into_response();
    }
    if request["stream"] != true {
        return Json(json!({"id":"summary", "object":"chat.completion", "created":0, "model":"test",
            "choices":[{"index":0,"message":{"role":"assistant","content":"Retained conversation facts."},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}})).into_response();
    }
    let has_result = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool");
    Sse::new(async_stream::stream! {
        if input == "wait-before-reply" {
            let mut release = model.release.subscribe();
            release.wait_for(|v| *v).await.unwrap();
        }
        if let Some(output) = input.strip_prefix("deliver:") {
            yield Ok::<_, Infallible>(Event::default().data(chunk(json!({"content":output}),Value::Null)));
            yield Ok::<_, Infallible>(Event::default().data(chunk(json!({}),json!("stop"))));
        } else if input=="question" && !has_result {
            let args = json!({"questions":[{"id":"format","question":"Choose an output format","type":"single_select","options":[{"id":"pdf","label":"PDF"},{"id":"pptx","label":"PowerPoint"}]}]});
            yield Ok::<_, Infallible>(Event::default().data(chunk(json!({"tool_calls":[{"index":0,"id":"question-call","function":{"name":"ask_question","arguments":args.to_string()}}]}),json!("tool_calls"))));
        } else if input=="history-tool" && !has_result {
            yield Ok::<_, Infallible>(Event::default().data(chunk(json!({"content":"Inspecting the project before calling a tool."}),Value::Null)));
            yield Ok::<_, Infallible>(Event::default().data(chunk(json!({"tool_calls":[{"index":0,"id":"history-call","function":{"name":"echo","arguments":"{\"api_key\":\"DO_NOT_EXPOSE\",\"path\":\"/project\"}"}}]}),json!("tool_calls"))));
        } else if input=="tool" && !has_result {
            yield Ok::<_, Infallible>(Event::default().data(chunk(json!({"tool_calls":[{"index":0,"id":"echo-call","function":{"name":"echo","arguments":"{\"private\":\"PRIVATE_ARGUMENT\"}"}}]}),json!("tool_calls"))));
        } else {
            yield Ok::<_, Infallible>(Event::default().data(chunk(json!({"content":"Hel"}),Value::Null)));
            if input=="slow" {let mut release=model.release.subscribe();let _=release.wait_for(|v|*v).await;}
            if input=="long" {
                for _ in 0..100 {yield Ok::<_, Infallible>(Event::default().data(chunk(json!({"content":"abcdefghij"}),Value::Null)));tokio::task::yield_now().await;}
            }
            if input!="broken" {
                yield Ok::<_, Infallible>(Event::default().data(chunk(json!({"content":"lo"}),Value::Null)));
                yield Ok::<_, Infallible>(Event::default().data(chunk(json!({}),json!("stop"))));
                yield Ok::<_, Infallible>(Event::default().data(json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"test","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}).to_string()));
            }
        }
        yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
    }).into_response()
}
struct Echo;
#[async_trait::async_trait]
impl Tool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "fixture"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object"})
    }
    async fn execute(&self, _: &str, _: &mut ToolContext<'_>) -> anyhow::Result<String> {
        Ok("PRIVATE_TOOL_RESULT".into())
    }
}

pub struct Harness {
    pub root: tempfile::TempDir,
    pub store: Arc<SessionStore>,
    pub agent: Arc<Agent>,
    pub settings: ServerSettings,
    pub client: Client,
    pub url: String,
    pub calls: Arc<AtomicUsize>,
    pub release: watch::Sender<bool>,
    pub requests: Arc<Mutex<Vec<Value>>>,
    shutdown: Option<oneshot::Sender<()>>,
    http: Option<JoinHandle<anyhow::Result<()>>>,
    model: JoinHandle<()>,
}
impl Harness {
    pub async fn new(settings: ServerSettings) -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(root.path()).await.unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let release = watch::channel(false).0;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let model_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/v1/chat/completions", post(completion))
            .with_state(Model {
                calls: calls.clone(),
                release: release.clone(),
                requests: requests.clone(),
            });
        let model = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let agent = Arc::new(Agent::new(
            model_url,
            "fixture-key".into(),
            "test",
            None::<String>,
            Arc::new(ToolBox::from([(
                "echo".into(),
                Arc::new(Echo) as Arc<dyn Tool>,
            )])),
        ));
        let mut headers = reqwest::header::HeaderMap::new();
        if !settings.access_token.is_empty() {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", settings.access_token.expose())
                    .parse()
                    .unwrap(),
            );
        }
        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let mut harness = Self {
            root,
            store,
            agent,
            settings,
            client,
            url: String::new(),
            calls,
            requests,
            release,
            shutdown: None,
            http: None,
            model,
        };
        harness.start_http().await;
        harness
    }
    async fn start_http(&mut self) {
        let server = Server::with_agent(
            self.settings.clone(),
            self.agent.clone(),
            self.store.clone(),
        )
        .await
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.url = format!("http://{}", listener.local_addr().unwrap());
        let (send, recv) = oneshot::channel();
        self.shutdown = Some(send);
        self.http = Some(tokio::spawn(server.serve(listener, async {
            let _ = recv.await;
        })));
    }
    pub async fn close(&mut self) {
        if let Some(send) = self.shutdown.take() {
            let _ = send.send(());
        }
        if let Some(task) = self.http.take() {
            tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }
    pub async fn restart(&mut self) {
        self.close().await;
        self.start_http().await;
    }
    pub async fn session(&self) -> String {
        let response = self
            .client
            .post(format!("{}/v1/sessions", self.url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        response.json::<SessionSummary>().await.unwrap().session_id
    }
    pub async fn run(&self, session: &str, input: &str, key: &str) -> RunAccepted {
        let response = self
            .client
            .post(format!("{}/v1/sessions/{session}/runs", self.url))
            .json(&json!({"input":input,"client_request_id":key}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            202,
            "{}",
            response.text().await.unwrap_or_default()
        );
        response.json().await.unwrap()
    }
    pub async fn info(&self, id: &str) -> RunInfo {
        self.client
            .get(format!("{}/v1/runs/{id}", self.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    pub async fn terminal(&self, id: &str) -> RunInfo {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let info = self.info(id).await;
                if info.status.is_terminal() {
                    return info;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    pub async fn first_delta_then_disconnect(&self, id: &str) -> u64 {
        let response = self
            .client
            .get(format!("{}/v1/runs/{id}/events", self.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let mut stream = response.bytes_stream();
        let mut text = String::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(bytes) = stream.next().await {
                text.push_str(&String::from_utf8_lossy(&bytes.unwrap()));
                for event in parse(&text) {
                    if matches!(
                        event.payload,
                        kyler_protocol::api::event::EventPayload::TextDelta { .. }
                    ) {
                        return event.seq;
                    }
                }
            }
            panic!("no delta before EOF");
        })
        .await
        .unwrap()
    }
    pub async fn events(&self, id: &str, after: u64) -> Vec<ServerEvent> {
        let response = self
            .client
            .get(format!("{}/v1/runs/{id}/events", self.url))
            .header("last-event-id", after)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            200,
            "{}",
            response.text().await.unwrap_or_default()
        );
        parse(&response.text().await.unwrap())
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(task) = &self.http {
            task.abort();
        }
        self.model.abort();
    }
}
pub fn parse(text: &str) -> Vec<ServerEvent> {
    text.split("\n\n")
        .filter_map(|frame| {
            frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .and_then(|data| serde_json::from_str(data).ok())
        })
        .collect()
}
