use std::sync::Arc;

use serde_json::json;

use kyler_core::{
    ExecutionEnvironment, SandboxPolicy, SessionEnvironment,
    artifacts::ArtifactStore,
    sandbox::{WorkerRequest, direct::DirectSandbox, worker::dispatch},
};

fn environment(
    base: &std::path::Path,
    cwd: &std::path::Path,
    policy: SandboxPolicy,
) -> ExecutionEnvironment {
    ExecutionEnvironment::new(
        base,
        cwd,
        policy,
        std::env::current_exe().unwrap(),
        Arc::new(DirectSandbox),
    )
    .unwrap()
}
async fn tool(
    env: &ExecutionEnvironment,
    name: &str,
    args: serde_json::Value,
) -> kyler_core::sandbox::WorkerResponse {
    dispatch(WorkerRequest {
        tool: name.into(),
        arguments: args,
        environment: env.session_environment().clone(),
        filesystem: env.policy().filesystem.clone(),
        allow_commands: false,
        max_output_bytes: 65536,
        stream_output: false,
    })
    .await
}
#[tokio::test]
async fn all_relative_tools_use_cwd_and_explicit_targets_still_require_write_permission() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let cwd = base.join("backend");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::write(base.join("same.txt"), "project root").unwrap();
    std::fs::write(cwd.join("same.txt"), "current directory").unwrap();
    let env = environment(
        &base,
        &cwd,
        SandboxPolicy::workspace_write("fixture", &base),
    );
    let result = tool(&env, "read_file", json!({"file_path":"same.txt"})).await;
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(result.output.unwrap().contains("current directory"));
    let patch = "*** Begin Patch\n*** Add File: result.txt\n+from cwd\n*** End Patch";
    let result = tool(&env, "apply_patch", json!({"input":patch})).await;
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(cwd.join("result.txt").exists());
    assert!(!base.join("result.txt").exists());
    // cwd is not the write boundary: the host policy explicitly authorizes this target.
    let outside_cwd = base.join("authorized.txt");
    let result=tool(&env,"apply_patch",json!({"input":format!("*** Begin Patch\n*** Add File: {}\n+authorized\n*** End Patch",outside_cwd.display())})).await;
    assert!(result.error.is_none(), "{:?}", result.error);
    let denied = root
        .path()
        .parent()
        .unwrap()
        .join(format!("denied-{}.txt", uuid::Uuid::new_v4()));
    let result=tool(&env,"apply_patch",json!({"input":format!("*** Begin Patch\n*** Add File: {}\n+denied\n*** End Patch",denied.display())})).await;
    assert!(result.error.is_some());
    assert!(!denied.exists());
    let readonly = environment(&base, &cwd, SandboxPolicy::read_only("readonly", &base));
    assert!(
        tool(
            &readonly,
            "apply_patch",
            json!({"input":"*** Begin Patch\n*** Add File: forbidden.txt\n+no\n*** End Patch"})
        )
        .await
        .error
        .is_some()
    );
    assert!(!cwd.join("forbidden.txt").exists());
}
#[test]
fn saved_environments_use_only_cwd_and_reject_removed_fields() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let cwd = base.join("child");
    std::fs::create_dir(&cwd).unwrap();
    let env = environment(
        &base,
        &cwd,
        SandboxPolicy::workspace_write("fixture", &base),
    );
    let encoded = serde_json::to_value(env.session_environment()).unwrap();
    assert_eq!(encoded["cwd"], json!(cwd));
    assert_eq!(
        serde_json::from_value::<SessionEnvironment>(encoded.clone()).unwrap(),
        *env.session_environment()
    );
    for key in [
        "workspace_root",
        "working_directory",
        "project_root",
        "sandbox_policy_id",
    ] {
        assert!(encoded.get(key).is_none());
        let mut removed = encoded.clone();
        removed[key] = json!(base);
        assert!(serde_json::from_value::<SessionEnvironment>(removed).is_err());
    }
}

#[test]
fn artifacts_use_cwd_and_preserve_authorized_external_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let cwd = base.join("child");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::write(cwd.join("report.txt"), b"cwd report").unwrap();
    std::fs::write(base.join("report.txt"), b"external report").unwrap();
    let env = environment(
        &base,
        &cwd,
        SandboxPolicy::workspace_write("fixture", &base),
    );
    let store = ArtifactStore::new(base.join("snapshots"));
    let session = uuid::Uuid::new_v4().to_string();
    let execution = uuid::Uuid::new_v4().to_string();
    store
        .capture(
            &session,
            &execution,
            &format!(
                "[Local](./report.txt) [External]({})",
                base.join("report.txt").display()
            ),
            Some(&env),
        )
        .unwrap();
    let artifacts = store.list(&session).unwrap();
    assert_eq!(artifacts.len(), 2);
    assert!(artifacts.iter().all(|item| item.available));
    std::fs::remove_file(cwd.join("report.txt")).unwrap();
    std::fs::remove_file(base.join("report.txt")).unwrap();
    let outputs = artifacts
        .iter()
        .map(|item| store.download(&session, &item.id).unwrap().1)
        .collect::<Vec<_>>();
    assert!(outputs.contains(&b"cwd report".to_vec()));
    assert!(outputs.contains(&b"external report".to_vec()));
    assert!(
        store
            .download(&uuid::Uuid::new_v4().to_string(), &artifacts[0].id)
            .is_err()
    );
}
