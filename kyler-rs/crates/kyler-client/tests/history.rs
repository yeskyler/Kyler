use axum::{Json, Router, http::StatusCode, routing::get};
use kyler_client::Client;
use kyler_config::ClientSettings;
use kyler_protocol::api::history::HistoryContent;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::net::TcpListener;

#[tokio::test]
async fn legacy_history_fallback_preserves_text_but_never_bypasses_authorization() {
    for status in [StatusCode::NOT_FOUND, StatusCode::FORBIDDEN] {
        let reads = Arc::new(AtomicUsize::new(0));
        let observed = reads.clone();
        let router = Router::new()
            .route("/v1/sessions/{id}/history", get(move || async move { status }))
            .route("/v1/sessions/{id}/messages", get(move || {
                let reads = observed.clone();
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"messages":[{"id":"event","execution_id":"execution","timestamp":123,"role":"assistant","content":"Legacy text"}],"next_cursor":null}))
                }
            }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::new(ClientSettings {
            server_url: format!("http://{}", listener.local_addr().unwrap()),
            ..Default::default()
        })
        .unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let result = client
            .history("00000000-0000-4000-8000-000000000001", 0, 50)
            .await;
        if status == StatusCode::NOT_FOUND {
            let page = result.unwrap();
            assert_eq!(page.events[0].timestamp_ms, 123000);
            assert!(
                matches!(&page.events[0].content[0], HistoryContent::Message { text, .. } if text == "Legacy text")
            );
            assert!(page.executions.is_empty());
            assert_eq!(reads.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(result.unwrap_err().status, Some(403));
            assert_eq!(reads.load(Ordering::SeqCst), 0);
        }
        task.abort();
    }
}
