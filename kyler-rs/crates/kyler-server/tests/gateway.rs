mod support;

use std::{sync::atomic::Ordering, time::Duration};

use reqwest::Client;
use serde_json::{Value, json};

use kyler_gateway::security::Security;
use kyler_protocol::api::gateway::{CreateToken, Principal, Role, Scope};
use support::Harness;

struct NoCommands;
#[async_trait::async_trait]
impl kyler_core::sandbox::Sandbox for NoCommands {
    async fn run(
        &self,
        _: kyler_core::sandbox::SandboxRequest,
    ) -> anyhow::Result<kyler_core::sandbox::WorkerResponse> {
        anyhow::bail!("No commands are used by this fixture")
    }
}

#[tokio::test]
async fn workspace_grants_do_not_share_session_ownership_or_allow_other_directories() {
    let mut h = Harness::new(Default::default()).await;
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let other = root.path().join("other");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&other).unwrap();
    let mut policy = kyler_core::SandboxPolicy::workspace_write("gateway-project", &project);
    policy.filesystem.readwrite_paths.push(other.clone());
    let environment = kyler_core::ExecutionEnvironment::new(
        &project,
        &project,
        policy,
        std::env::current_exe().unwrap(),
        std::sync::Arc::new(NoCommands),
    )
    .unwrap();
    h.agent = std::sync::Arc::new(
        h.agent
            .as_ref()
            .clone()
            .with_environment(std::sync::Arc::new(environment)),
    );
    h.restart().await;
    let security = Security::open(h.store.directory()).unwrap();
    let mut clients = Vec::new();
    for _ in 0..2 {
        let issued = security
            .issue(
                &Principal::local(),
                CreateToken {
                    name: "project-worker".into(),
                    role: Role::Member,
                    scope: Scope {
                        workspaces: vec![project.to_str().unwrap().into()],
                        ..Default::default()
                    },
                    expires_at: None,
                },
            )
            .await
            .unwrap();
        clients.push(
            Client::builder()
                .default_headers(
                    [(
                        reqwest::header::AUTHORIZATION,
                        format!("Bearer {}", issued.secret).parse().unwrap(),
                    )]
                    .into_iter()
                    .collect(),
                )
                .build()
                .unwrap(),
        );
    }
    let created: Value = clients[0]
        .post(format!("{}/v1/sessions", h.url))
        .json(&json!({"cwd":project,"workspace_mode":"project"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["session_id"].as_str().unwrap();
    assert_eq!(
        clients[0]
            .get(format!("{}/v1/sessions/{id}/history", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        clients[1]
            .get(format!("{}/v1/sessions/{id}/history", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        clients[0]
            .post(format!("{}/v1/sessions", h.url))
            .json(&json!({"cwd":other,"workspace_mode":"project"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let private: Value = clients[0]
        .post(format!("{}/v1/sessions", h.url))
        .json(&json!({"cwd":project,"workspace_mode":"isolated"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        clients[0]
            .get(format!(
                "{}/v1/sessions/{}/history",
                h.url,
                private["session_id"].as_str().unwrap()
            ))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    h.close().await;
}

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
async fn resource_authorization_covers_listing_history_tools_and_sensitive_operations() {
    let mut h = Harness::new(Default::default()).await;
    let shared = h.session().await;
    let other = h.session().await;
    let (_, viewer) = credential(&h, Role::Viewer, vec![shared.clone()]).await;
    let (_, member) = credential(&h, Role::Member, vec![shared.clone()]).await;
    let (_, stranger) = credential(&h, Role::Member, vec![]).await;
    let page: Value = viewer
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(page["sessions"][0]["session_id"], shared);
    for suffix in [
        "",
        "/history",
        "/attachments",
        "/workspace-files",
        "/tool-calls/call",
        "/usage",
        "/rollout",
    ] {
        assert_eq!(
            viewer
                .get(format!("{}/v1/sessions/{other}{suffix}", h.url))
                .send()
                .await
                .unwrap()
                .status(),
            404,
            "{suffix}"
        );
    }
    assert_eq!(
        stranger
            .get(format!("{}/v1/sessions/{shared}", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        viewer
            .post(format!("{}/v1/sessions/{shared}/runs", h.url))
            .json(&json!({"input":"forbidden"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    for path in ["gateway/tokens", "plugins/connectors", "models/settings"] {
        assert_eq!(
            member
                .get(format!("{}/v1/{path}", h.url))
                .send()
                .await
                .unwrap()
                .status(),
            403,
            "{path}"
        );
    }
    assert_eq!(
        member
            .put(format!("{}/v1/sessions/{shared}/permissions", h.url))
            .json(&json!({"mode":"full_access","confirm_full_access":true}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(h.calls.load(Ordering::SeqCst), 0);
    h.close().await;
}

#[tokio::test]
async fn revoked_tokens_close_sse_without_cancelling_admitted_work() {
    let mut h = Harness::new(Default::default()).await;
    let session = h.session().await;
    let (secret, member) = credential(&h, Role::Member, vec![session.clone()]).await;
    let (_, other) = credential(&h, Role::Member, vec![session.clone()]).await;
    let accepted: Value = member
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
    assert_eq!(
        other
            .post(format!("{}/v1/runs/{run}/cancel", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut stream = member
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
        member
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
async fn rotation_returns_a_secret_once_and_preserves_resource_grants() {
    let mut h = Harness::new(Default::default()).await;
    let session = h.session().await;
    let request =
        json!({"name":"reader","role":"viewer","scope":{"sessions":[session]},"expires_at":null});
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
    let (secret, member) = credential(&h, Role::Member, vec![session.clone()]).await;
    let (_, other) = credential(&h, Role::Member, vec![session.clone()]).await;
    let url = format!("{}/v1/sessions/{session}/runs", h.url);
    let request = json!({"input":"wait-before-reply","client_request_id":"stable"});
    let first: Value = member
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
