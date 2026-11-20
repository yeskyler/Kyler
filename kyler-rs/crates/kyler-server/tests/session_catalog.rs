mod support;

use serde_json::{Value, json};
use std::{sync::atomic::Ordering, time::Duration};

use kyler_config::{SecretString, ServerSettings};
use support::Harness;

#[tokio::test]
async fn fresh_clients_restore_preferences_and_observe_runs_without_admitting_again() {
    let mut h = Harness::new(ServerSettings {
        access_token: SecretString::new("catalog-test"),
        ..Default::default()
    })
    .await;
    let session = h.session().await;
    assert_eq!(
        reqwest::get(format!("{}/v1/preferences", h.url))
            .await
            .unwrap()
            .status(),
        401
    );
    let initial: Value = h
        .client
        .get(format!("{}/v1/preferences", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(initial["revision"], 0);
    let saved: Value = h
        .client
        .put(format!("{}/v1/preferences", h.url))
        .json(&json!({"last_session_id":session,"sidebar_collapsed":true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["last_session_id"], session);
    let run = h.run(&session, "wait-before-reply", "once").await;
    let fresh = reqwest::Client::new();
    let active: Value = fresh
        .get(format!("{}/v1/sessions/{session}/active-run", h.url))
        .bearer_auth(h.settings.access_token.expose())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(active["run"]["run_id"], run.run_id);
    assert_eq!(active["input"], "wait-before-reply");
    let preferences: Value = fresh
        .get(format!("{}/v1/preferences", h.url))
        .bearer_auth(h.settings.access_token.expose())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(preferences["sidebar_collapsed"].as_bool().unwrap());
    let patched: Value = fresh
        .put(format!("{}/v1/preferences", h.url))
        .bearer_auth(h.settings.access_token.expose())
        .json(&json!({"sidebar_collapsed":false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(patched["last_session_id"], session);
    h.release.send_replace(true);
    let result = h.terminal(&run.run_id).await;
    assert!(result.status.is_terminal());
    assert_eq!(h.calls.load(Ordering::SeqCst), 1);
    let active: Value = fresh
        .get(format!("{}/v1/sessions/{session}/active-run", h.url))
        .bearer_auth(h.settings.access_token.expose())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(active.is_null());
    assert!(h.store.catalog().path.is_file());
    let listed: Value = h
        .client
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["sessions"][0]["session_id"], session);
    assert_eq!(listed["sessions"][0]["title"], "wait-before-reply");
    // A browser preference cannot refer to an unknown session.
    assert_eq!(
        fresh
            .put(format!("{}/v1/preferences", h.url))
            .bearer_auth(h.settings.access_token.expose())
            .json(&json!({"last_session_id":uuid::Uuid::new_v4().to_string()}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    tokio::time::timeout(Duration::from_secs(15), h.restart())
        .await
        .unwrap();
    let preferences: Value = h
        .client
        .get(format!("{}/v1/preferences", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preferences["last_session_id"], session);
    assert_eq!(preferences["sidebar_collapsed"], false);
    h.client
        .delete(format!("{}/v1/sessions/{session}", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let preferences: Value = h
        .client
        .get(format!("{}/v1/preferences", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(preferences["last_session_id"].is_null());
    h.close().await;
}
