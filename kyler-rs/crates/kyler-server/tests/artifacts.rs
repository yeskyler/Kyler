mod support;
use support::Harness;

use std::sync::Arc;

use kyler_config::{SecretString, ServerSettings};
use kyler_core::{ExecutionEnvironment, SandboxPolicy, sandbox::direct::DirectSandbox};
use kyler_protocol::api::{artifacts::ArtifactInfo, run::RunStatus, workspaces::WorkspaceFiles};

#[tokio::test]
async fn final_response_files_are_retained_authorized_replayed_and_deleted_with_the_session() {
    let mut h = Harness::new(ServerSettings {
        access_token: SecretString::new("artifact-test-token"),
        ..Default::default()
    })
    .await;
    h.close().await;
    let workspace = h.root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    std::fs::write(
        workspace.join("\u{62a5}\u{544a}.pptx"),
        b"immutable-presentation",
    )
    .unwrap();
    h.agent = Arc::new(
        (*h.agent).clone().with_environment(Arc::new(
            ExecutionEnvironment::new(
                &workspace,
                &workspace,
                SandboxPolicy::workspace_write("artifacts", &workspace),
                std::env::current_exe().unwrap(),
                Arc::new(DirectSandbox),
            )
            .unwrap(),
        )),
    );
    h.restart().await;
    let session = h.session().await;
    let private = h
        .store
        .load(&session)
        .await
        .unwrap()
        .environment
        .unwrap()
        .workspace_root;
    std::fs::write(
        private.join("\u{62a5}\u{544a}.pptx"),
        b"immutable-presentation",
    )
    .unwrap();
    let answer = "Here is [the presentation](./%E6%8A%A5%E5%91%8A.pptx). [Missing](./missing.zip)";
    let run = h
        .run(&session, &format!("deliver:{answer}"), "deliver")
        .await;
    let result = h.terminal(&run.run_id).await;
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.output.as_deref(), Some(answer));
    let url = format!("{}/v1/sessions/{session}/artifacts", h.url);
    assert_eq!(reqwest::get(&url).await.unwrap().status(), 401);
    let items: Vec<ArtifactInfo> = h
        .client
        .get(&url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(items.len(), 2);
    let file = items.iter().find(|file| file.available).unwrap();
    let missing = items.iter().find(|file| !file.available).unwrap();
    assert_eq!(file.execution_id, result.execution_id.unwrap());
    assert_eq!(
        h.client
            .get(format!("{url}/{}/content", missing.id))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    std::fs::remove_file(workspace.join("\u{62a5}\u{544a}.pptx")).unwrap();
    let downloaded = h
        .client
        .get(format!("{url}/{}/content", file.id))
        .send()
        .await
        .unwrap();
    assert!(downloaded.status().is_success());
    assert!(
        downloaded.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("filename*=UTF-8''")
    );
    assert_eq!(downloaded.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        downloaded.bytes().await.unwrap().as_ref(),
        b"immutable-presentation"
    );
    let other = h.session().await;
    assert_eq!(
        h.client
            .get(format!(
                "{}/v1/sessions/{other}/artifacts/{}/content",
                h.url, file.id
            ))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let uploaded: serde_json::Value = h
        .client
        .post(format!(
            "{}/v1/sessions/{session}/attachments?filename=notes.txt",
            h.url
        ))
        .body("workspace notes")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    for owner in [&session, &other] {
        let shared: WorkspaceFiles = h
            .client
            .get(format!("{}/v1/sessions/{owner}/workspace-files", h.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        if owner == &other {
            assert!(shared.outputs.is_empty());
            assert!(shared.inputs.is_empty());
            continue;
        }
        assert_eq!(shared.outputs.len(), 2);
        assert!(
            shared
                .outputs
                .iter()
                .any(|item| item.id == file.id && item.session_id == session)
        );
        assert_eq!(shared.inputs.len(), 1);
        assert_eq!(
            shared.inputs[0].attachment.id,
            uploaded["id"].as_str().unwrap()
        );
        assert_eq!(shared.inputs[0].session_id, session);
        let bytes = h
            .client
            .get(format!(
                "{}/v1/sessions/{}/attachments/{}/content",
                h.url, shared.inputs[0].session_id, shared.inputs[0].attachment.id
            ))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), b"workspace notes");
    }
    let mut isolated = kyler_core::session::Session::new();
    isolated.environment = Some(kyler_core::environment::SessionEnvironment {
        project_root: None,
        workspace_root: h.root.path().join("other-workspace"),
        working_directory: h.root.path().join("other-workspace"),
        sandbox_policy_id: "isolated".into(),
    });
    h.store.save(&mut isolated).await.unwrap();
    let separate: WorkspaceFiles = h
        .client
        .get(format!(
            "{}/v1/sessions/{}/workspace-files",
            h.url, isolated.session_id
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(separate.inputs.is_empty() && separate.outputs.is_empty());
    h.restart().await;
    assert_eq!(
        h.client
            .get(format!(
                "{}/v1/sessions/{session}/artifacts/{}/content",
                h.url, file.id
            ))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        b"immutable-presentation"
    );
    assert_eq!(
        h.client
            .delete(format!("{}/v1/sessions/{session}", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert!(
        !h.store
            .directory()
            .join("_artifacts")
            .join(&session)
            .exists()
    );
    h.close().await;
}
