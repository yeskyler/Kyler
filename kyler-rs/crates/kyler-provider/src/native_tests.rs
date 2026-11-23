use axum::{
    Json, Router,
    http::{HeaderMap, header},
    routing::post,
};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::{
    WireProtocol,
    transport::{self, ModelSession},
};

async fn fixture(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (host, task)
}

fn sse(events: &[Value]) -> ([(axum::http::HeaderName, &'static str); 1], String) {
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect(),
    )
}

fn initial() -> Value {
    json!({"model":"fixture","messages":[{"role":"user","content":"Use a tool"}],"tools":[{
        "type":"function","function":{"name":"read","description":"Read","parameters":{"type":"object"}}
    }]})
}

fn follow_up(id: &str, arguments: &str) -> Value {
    json!({"model":"fixture","messages":[
        {"role":"user","content":"Use a tool"},
        {"role":"assistant","tool_calls":[{"id":id,"type":"function","function":{"name":"read","arguments":arguments}}]},
        {"role":"tool","tool_call_id":id,"content":"Found it"}
    ]})
}

#[tokio::test]
async fn anthropic_stream_assembles_arguments_and_returns_the_matching_result() {
    let (host, task) = fixture(Router::new().route("/v1/messages", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
        assert_eq!(headers["x-api-key"], "fixture");
        assert!(!headers.contains_key("authorization"));
        if body["stream"] == true {
            assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
            return axum::response::IntoResponse::into_response(sse(&[
                json!({"type":"message_start","message":{"usage":{"input_tokens":5}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_fixture","name":"read","input":{}}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"key\":"}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"value\"}"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
                json!({"type":"message_stop"}),
            ]));
        }
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "toolu_fixture");
        assert_eq!(body["messages"][2]["content"][0]["content"], "Found it");
        axum::response::IntoResponse::into_response(Json(json!({"id":"message","content":[{"type":"text","text":"Done"}],"stop_reason":"end_turn","usage":{"input_tokens":6,"output_tokens":1}})))
    }))).await;
    let model = ModelSession::new(
        transport::client(&format!("{host}/v1"), "fixture"),
        WireProtocol::AnthropicMessages,
    );
    let mut stream = transport::stream(&model, initial()).await.unwrap();
    let mut id = String::new();
    let mut arguments = String::new();
    while let Some(chunk) = stream.next().await {
        for choice in chunk.unwrap().choices {
            for call in choice.delta.standard.tool_calls.unwrap_or_default() {
                id.push_str(call.id.as_deref().unwrap_or_default());
                arguments.push_str(
                    call.function
                        .and_then(|f| f.arguments)
                        .as_deref()
                        .unwrap_or_default(),
                );
            }
        }
    }
    assert_eq!(id, "toolu_fixture");
    assert_eq!(
        serde_json::from_str::<Value>(&arguments).unwrap(),
        json!({"key":"value"})
    );
    let reply = transport::complete(&model, follow_up(&id, &arguments))
        .await
        .unwrap();
    assert_eq!(reply["choices"][0]["message"]["content"], "Done");
    task.abort();
}

#[tokio::test]
async fn gemini_stream_replays_the_original_signed_call_and_result() {
    let (host, task) = fixture(Router::new()
        .route("/v1beta/models/fixture:streamGenerateContent", post(|headers:HeaderMap, Json(body):Json<Value>| async move {
            assert_eq!(headers["x-goog-api-key"], "fixture");
            assert!(!headers.contains_key("authorization"));
            assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "read");
            sse(&[
                json!({"candidates":[{"content":{"role":"model","parts":[{"thoughtSignature":"opaque","functionCall":{"name":"read","id":"native","args":{}}}]}}]}),
                json!({"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":2,"totalTokenCount":7}}),
            ])
        }))
        .route("/v1beta/models/fixture:generateContent", post(|Json(body):Json<Value>| async move {
            assert_eq!(body["contents"][1]["parts"][0]["thoughtSignature"], "opaque");
            assert_eq!(body["contents"][2]["parts"][0]["functionResponse"]["id"], "native");
            assert_eq!(body["contents"][2]["parts"][0]["functionResponse"]["response"]["output"], "Found it");
            Json(json!({"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"Done"}]}}]}))
        }))).await;
    let model = ModelSession::new(
        transport::client(&format!("{host}/v1beta"), "fixture"),
        WireProtocol::GeminiGenerateContent,
    );
    let mut stream = transport::stream(&model, initial()).await.unwrap();
    let mut id = None;
    while let Some(chunk) = stream.next().await {
        for choice in chunk.unwrap().choices {
            for call in choice.delta.standard.tool_calls.unwrap_or_default() {
                id = call.id;
            }
        }
    }
    let reply = transport::complete(&model, follow_up(&id.unwrap(), "{}"))
        .await
        .unwrap();
    assert_eq!(reply["choices"][0]["message"]["content"], "Done");
    task.abort();
}
