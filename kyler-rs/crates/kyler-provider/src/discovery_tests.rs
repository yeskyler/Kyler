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
