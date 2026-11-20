//! End-to-end Responses tool continuation without model calls or API credentials.
use super::*;
use axum::{
    Json, Router, body::Body, extract::State, http::header, response::Response, routing::post,
};

#[derive(Clone)]
struct Fixture {
    requests: Arc<Mutex<Vec<Value>>>,
}
fn first() -> Value {
    json!({"id":"resp_one","status":"completed","usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15},"output":[
        {"type":"reasoning","id":"rs_one","summary":[],"encrypted_content":"OPAQUE_REASONING"},
        {"type":"message","id":"msg_one","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Using a tool.","annotations":[]}]},
        {"type":"function_call","id":"fc_one","call_id":"call_one","name":"remember","arguments":"{}","status":"completed"}
    ]})
}
async fn respond(State(state): State<Fixture>, Json(request): Json<Value>) -> Response<Body> {
    let mut requests = state.requests.lock().await;
    let index = requests.len();
    requests.push(request.clone());
    let response = if index == 0 {
        first()
    } else {
        json!({"id":"resp_two","status":"completed","usage":{"input_tokens":20,"output_tokens":5,"total_tokens":25},"output":[{"type":"message","id":"msg_two","role":"assistant","status":"completed","content":[{"type":"output_text","text":"remembered","annotations":[]}]}]})
    };
    if request["stream"] == true {
        let mut events = Vec::new();
        if index == 0 {
            events.push(json!({"type":"response.reasoning_summary_text.delta","delta":"Checking the tool."}));
            events.push(json!({"type":"response.output_text.delta","delta":"Using a tool."}));
            events.push(json!({"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","id":"fc_one","call_id":"call_one","name":"remember","arguments":""}}));
            for part in ["{", "}"] {
                events.push(json!({"type":"response.function_call_arguments.delta","output_index":2,"delta":part}));
            }
            events.push(json!({"type":"response.output_item.done","output_index":2,"item":response["output"][2]}));
        } else {
            events.push(json!({"type":"response.output_text.delta","delta":"remembered"}));
        }
        events.push(json!({"type":"response.completed","response":response}));
        let wire = events
            .into_iter()
            .map(|e| format!("data: {e}\n\n"))
            .collect::<String>();
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(wire))
            .unwrap()
    } else {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(response.to_string()))
            .unwrap()
    }
}
#[tokio::test]
async fn responses_streaming_and_complete_preserve_opaque_items_and_tool_call_identity() {
    for streaming in [false, true] {
        let state = Fixture {
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let router = Router::new()
            .route("/v1/responses", post(respond))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let service = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut config = kyler_config::AppConfig::default();
        config.llm.base_url = url;
        config.llm.model = "gpt-5.6-sol".into();
        config.llm.api_key = kyler_config::SecretString::new("fixture-only");
        config.llm.adapter = kyler_provider::AdapterKind::Openai;
        config.llm.reasoning_effort = Some(kyler_protocol::api::models::ThinkingLevel::High);
        config.skills.enabled = false;
        config.subagent.enabled = false;
        config.session.enabled = false;
        let mut tools = ToolBox::new();
        tools.insert("remember".into(), Arc::new(Remember));
        let agent = Agent::from_config_with_tools(&config, Arc::new(tools))
            .await
            .unwrap()
            .bind_model(None)
            .unwrap();
        let result = if streaming {
            let mut source = agent.run_stream("remember 42").await.unwrap();
            while let Some(event) = source.next().await {
                assert!(!format!("{event:?}").contains("OPAQUE_REASONING"));
            }
            source.finish().await.unwrap()
        } else {
            agent.run("remember 42").await.unwrap()
        };
        assert_eq!(result.output, "remembered");
        assert_eq!(result.session.state["fact"], 42);
        assert_eq!(result.session.usage.total_tokens, 40);
        let requests = state.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["store"], false);
        assert_eq!(requests[0]["reasoning"]["effort"], "high");
        assert_eq!(requests[0]["tools"][0]["type"], "function");
        assert!(requests[0].get("messages").is_none());
        let input = requests[1]["input"].as_array().unwrap();
        let reasoning = input.iter().position(|i| i["type"] == "reasoning").unwrap();
        assert_eq!(input[reasoning]["encrypted_content"], "OPAQUE_REASONING");
        assert_eq!(input[reasoning + 1]["type"], "message");
        assert_eq!(input[reasoning + 2]["call_id"], "call_one");
        assert_eq!(input[reasoning + 3]["type"], "function_call_output");
        assert_eq!(input[reasoning + 3]["call_id"], "call_one");
        assert_eq!(
            input
                .iter()
                .filter(|i| i["role"] == "assistant" || i["type"] == "message")
                .count(),
            1
        );
        service.abort();
    }
}

#[tokio::test]
async fn responses_never_execute_calls_from_incomplete_or_mismatched_streams() {
    for terminal in ["missing", "incomplete", "mismatch"] {
        let mut events = vec![
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_one","name":"remember","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{}"}),
        ];
        if terminal == "incomplete" {
            events.push(json!({"type":"response.incomplete","response":{"status":"incomplete"}}));
        } else if terminal == "mismatch" {
            let mut response = first();
            response["output"][2]["arguments"] = json!("{\"different\":true}");
            events.push(json!({"type":"response.completed","response":response}));
        }
        let wire = events
            .into_iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        let router = Router::new().route(
            "/v1/responses",
            post(move || {
                let wire = wire.clone();
                async move {
                    Response::builder()
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(wire))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = kyler_config::AppConfig::default();
        config.llm.base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        config.llm.model = "gpt-5.6-sol".into();
        config.llm.adapter = kyler_provider::AdapterKind::Openai;
        config.llm.api_key = kyler_config::SecretString::new("fixture-only");
        config.skills.enabled = false;
        config.subagent.enabled = false;
        config.session.enabled = false;
        let service = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut tools = ToolBox::new();
        tools.insert("remember".into(), Arc::new(Remember));
        let agent = Agent::from_config_with_tools(&config, Arc::new(tools))
            .await
            .unwrap()
            .bind_model(None)
            .unwrap();
        let mut source = agent.run_stream("remember 42").await.unwrap();
        while source.next().await.is_some() {}
        let outcome = source.finish_with_session().await.unwrap();
        assert!(outcome.result.is_err(), "{terminal}");
        assert!(outcome.session.state.get("fact").is_none(), "{terminal}");
        service.abort();
    }
}
