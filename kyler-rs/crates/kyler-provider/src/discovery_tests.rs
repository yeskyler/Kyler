use super::{Adapter, AdapterKind, discovery::list_models};

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde_json::json;

#[tokio::test]
async fn discovery_filters_ids_and_sends_provider_specific_query() {
    let request = Arc::new(Mutex::new(None));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let app=Router::new().route("/v1/models",get(|State(request):State<Arc<Mutex<Option<String>>>>,headers:HeaderMap, uri:axum::http::Uri|async move {
        assert_eq!(headers["authorization"],"Bearer FIXTURE_SECRET");
        *request.lock().unwrap()=Some(uri.to_string());
        Json(json!({"data":[{"id":"model"},{"id":"model"},{"id":""},{"id":"bad\nmodel"},{}]}))
    })).with_state(request.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let models = list_models(&base, "FIXTURE_SECRET", Adapter(AdapterKind::Siliconflow))
        .await
        .unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "model");
    assert_eq!(
        request.lock().unwrap().as_deref(),
        Some("/v1/models?sub_type=chat")
    );
    server.abort();
}

#[tokio::test]
async fn discovery_never_exposes_error_bodies_or_follows_redirects() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async {
                (
                    StatusCode::FOUND,
                    [("location", "/secret-target")],
                    "PRIVATE_PROVIDER_BODY",
                )
            }),
        )
        .route(
            "/secret-target",
            get(|| async {
                panic!("redirect must not be followed");
                #[allow(unreachable_code)]
                ""
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let error = list_models(&base, "FIXTURE_SECRET", Adapter(AdapterKind::Generic))
        .await
        .unwrap_err();
    let text = format!("{error:#}");
    assert!(text.contains("302"));
    assert!(!text.contains("PRIVATE"));
    assert!(!text.contains("FIXTURE_SECRET"));
    server.abort();
}

#[tokio::test]
async fn gemini_discovers_native_models_and_retrieves_the_next_page() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1beta", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/v1beta/models",
        get(|headers: HeaderMap, uri: axum::http::Uri| async move {
            assert_eq!(headers["x-goog-api-key"], "fixture");
            assert!(!headers.contains_key("authorization"));
            if uri.query() == Some("pageToken=next") {
                Json(json!({"models":[{"name":"models/other","supportedGenerationMethods":["generateContent"]}]}))
            } else {
                assert!(uri.query().is_none());
                Json(json!({"nextPageToken":"next","models":[
                    {"name":"models/embedding","supportedGenerationMethods":["embedContent"]},
                    {"name":"models/chat","supportedGenerationMethods":["generateContent"],"inputTokenLimit":131072,"outputTokenLimit":4096}
                ]}))
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let models = list_models(&base, "fixture", Adapter(AdapterKind::Gemini))
        .await
        .unwrap();
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["chat", "other"]
    );
    assert_eq!(models[0].capabilities.max_context_tokens, Some(131072));
    assert_eq!(models[0].capabilities.max_output_tokens, Some(4096));
    server.abort();
}

#[tokio::test]
async fn local_discovery_does_not_require_an_authorization_header() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/v1/models",
        get(|headers: HeaderMap| async move {
            assert!(!headers.contains_key("authorization"));
            Json(json!({"data":[{"id":"local-chat"}]}))
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let models = list_models(&base, "", Adapter(AdapterKind::Ollama))
        .await
        .unwrap();
    assert_eq!(models[0].id, "local-chat");
    server.abort();
}
