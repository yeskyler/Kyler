mod support;

use std::time::Duration;

use reqwest::Client;
use serde_json::{Value, json};

use kyler_gateway::security::Security;
use kyler_protocol::api::gateway::{CreateToken, Principal, Role, Scope};
use support::Harness;

async fn credential(h: &Harness, role: Role, sessions: Vec<String>) -> (String, Client) {
    let security = Security::open(h.store.directory()).unwrap();
    let issued = security
        .issue(
            &Principal::local(),
            CreateToken {
                name: "api-fixture".into(),
                role,
                scope: Scope {
                    sessions,
                    ..Default::default()
                },
                expires_at: None,
            },
        )
        .await
        .unwrap();
    let client = Client::builder()
        .default_headers(
            [(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", issued.secret).parse().unwrap(),
            )]
            .into_iter()
            .collect(),
        )
        .build()
        .unwrap();
    (issued.secret, client)
}

#[tokio::test]
async fn legacy_and_anonymous_access_are_removed_and_denials_are_audited() {
    let mut h = Harness::new(Default::default()).await;
    for secret in [None, Some("old-shared-token")] {
        let request = Client::new().get(format!("{}/v1/info?token=DO_NOT_RECORD", h.url));
        let request = if let Some(secret) = secret {
            request.bearer_auth(secret)
        } else {
            request
        };
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 401);
        assert!(response.headers().contains_key("x-request-id"));
    }
    let records: Value = h
        .client
        .get(format!("{}/v1/gateway/audit", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        records["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["status"] == 401)
    );
    let rendered = records.to_string();
    assert!(!rendered.contains("DO_NOT_RECORD") && !rendered.contains("old-shared-token"));
    assert!(!rendered.contains(h.settings.access_token.expose()));
    h.close().await;
}

#[tokio::test]
async fn legacy_roles_cannot_authenticate_or_be_issued_or_rotated() {
    let mut h = Harness::new(Default::default()).await;
    for (role, name) in [
        (Role::Admin, "admin"),
        (Role::Member, "member"),
        (Role::Viewer, "viewer"),
    ] {
        let (secret, legacy) = credential(&h, role, vec![]).await;
        let id = Security::token_id(&secret).unwrap();
        assert_eq!(
            legacy
                .get(format!("{}/v1/gateway/identity", h.url))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            h.client
                .post(format!("{}/v1/gateway/tokens", h.url))
                .json(&json!({"name":"legacy","role":name}))
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
        assert_eq!(
            h.client
                .post(format!("{}/v1/gateway/tokens/{id}/rotate", h.url))
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    let listed: Vec<Value> = h
        .client
        .get(format!("{}/v1/gateway/tokens", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!listed.is_empty());
    assert!(listed.iter().all(|token| token["role"] == "owner"));
    h.close().await;
}

#[tokio::test]
async fn revoked_tokens_close_sse_without_cancelling_admitted_work() {
    let mut h = Harness::new(Default::default()).await;
    let session = h.session().await;
    let (secret, owner) = credential(&h, Role::Owner, vec![]).await;
    let accepted: Value = owner
        .post(format!("{}/v1/sessions/{session}/runs", h.url))
        .json(&json!({"input":"wait-before-reply","client_request_id":"same-client-key"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let run = accepted["run_id"].as_str().unwrap();
    let mut stream = owner
        .get(format!("{}/v1/runs/{run}/events", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(stream.chunk().await.unwrap().is_some());
    let security = Security::open(h.store.directory()).unwrap();
    let id = Security::token_id(&secret).unwrap();
    security.revoke(&Principal::local(), id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while stream.chunk().await.unwrap().is_some() {}
    })
    .await
    .unwrap();
    assert_eq!(
        owner
            .get(format!("{}/v1/runs/{run}", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert!(!h.info(run).await.status.is_terminal());
    h.client
        .post(format!("{}/v1/runs/{run}/cancel", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    h.close().await;
}

#[tokio::test]
async fn rotation_returns_a_secret_once_and_preserves_owner_identity() {
    let mut h = Harness::new(Default::default()).await;
    let session = h.session().await;
    let request = json!({"name":"operator"});
    let created: Value = h
        .client
        .post(format!("{}/v1/gateway/tokens", h.url))
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["info"]["role"], "owner");
    let id = created["info"]["id"].as_str().unwrap();
    let old = created["token"].as_str().unwrap();
    let rotated: Value = h
        .client
        .post(format!("{}/v1/gateway/tokens/{id}/rotate", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        created["info"]["principal_id"],
        rotated["info"]["principal_id"]
    );
    let client = Client::new();
    assert_eq!(
        client
            .get(format!("{}/v1/sessions/{session}", h.url))
            .bearer_auth(old)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .get(format!("{}/v1/sessions/{session}", h.url))
            .bearer_auth(rotated["token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let listing = h
        .client
        .get(format!("{}/v1/gateway/tokens", h.url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!listing.contains(old) && !listing.contains(rotated["token"].as_str().unwrap()));
    h.restart().await;
    assert_eq!(
        client
            .get(format!("{}/v1/sessions/{session}", h.url))
            .bearer_auth(old)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    h.close().await;
}

#[tokio::test]
async fn admission_keys_are_namespaced_by_identity_and_survive_rotation() {
    let mut h = Harness::new(Default::default()).await;
    let session = h.session().await;
    let (secret, owner) = credential(&h, Role::Owner, vec![]).await;
    let (_, other) = credential(&h, Role::Owner, vec![]).await;
    let url = format!("{}/v1/sessions/{session}/runs", h.url);
    let request = json!({"input":"wait-before-reply","client_request_id":"stable"});
    let first: Value = owner
        .post(&url)
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        other
            .post(&url)
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let security = Security::open(h.store.directory()).unwrap();
    let replacement = security
        .rotate(&Principal::local(), Security::token_id(&secret).unwrap())
        .await
        .unwrap();
    let client = Client::new();
    let second: Value = client
        .post(&url)
        .bearer_auth(&replacement.secret)
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["run_id"], second["run_id"]);
    client
        .post(format!(
            "{}/v1/runs/{}/cancel",
            h.url,
            first["run_id"].as_str().unwrap()
        ))
        .bearer_auth(&replacement.secret)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    h.close().await;
}
