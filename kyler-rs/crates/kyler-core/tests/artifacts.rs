use std::sync::Arc;

use kyler_core::{
    ExecutionEnvironment, SandboxPolicy, artifacts::ArtifactStore, sandbox::direct::DirectSandbox,
};
use kyler_protocol::api::artifacts::MAX_ARTIFACT_BYTES;
use uuid::Uuid;

#[test]
fn final_links_create_immutable_session_snapshots_and_survive_reload() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(workspace.join("output")).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let source = workspace.join("output/\u{62a5}\u{544a} final.pptx");
    std::fs::write(&source, b"original-presentation").unwrap();
    let environment = ExecutionEnvironment::new(
        &workspace,
        &workspace,
        SandboxPolicy::workspace_write("artifacts", &workspace),
        std::env::current_exe().unwrap(),
        Arc::new(DirectSandbox),
    )
    .unwrap();
    let store = ArtifactStore::new(root.path().join("artifacts"));
    let session = Uuid::new_v4().to_string();
    let execution = Uuid::new_v4().to_string();
    let output = "Done: [Report](./output/%E6%8A%A5%E5%91%8A%20final.pptx)\n[Again](./output/%E6%8A%A5%E5%91%8A%20final.pptx)\n`[code](./ignored.txt)`\n```md\n[fake](./fake.txt)\n```\n[Website](https://example.com/)";
    store
        .capture(&session, &execution, output, Some(&environment))
        .unwrap();
    let files = store.list(&session).unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0].available);
    assert_eq!(files[0].name, "\u{62a5}\u{544a} final.pptx");
    assert_eq!(files[0].execution_id, execution);
    std::fs::write(&source, b"changed").unwrap();
    store
        .capture(&session, &execution, output, Some(&environment))
        .unwrap();
    assert_eq!(store.list(&session).unwrap().len(), 1);
    let reloaded = ArtifactStore::new(root.path().join("artifacts"));
    assert_eq!(
        reloaded.download(&session, &files[0].id).unwrap().1,
        b"original-presentation"
    );
    assert!(
        reloaded
            .download(&Uuid::new_v4().to_string(), &files[0].id)
            .is_err()
    );
    reloaded.delete_session(&session).unwrap();
    assert!(reloaded.download(&session, &files[0].id).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"changed");
}

#[test]
fn denied_missing_symlinked_and_oversized_files_never_become_downloads() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    std::fs::write(workspace.join("secret.txt"), b"private").unwrap();
    let mut policy = SandboxPolicy::workspace_write("artifacts", &workspace);
    policy
        .filesystem
        .denied_paths
        .push(workspace.join("secret.txt"));
    let file = std::fs::File::create(workspace.join("large.bin")).unwrap();
    file.set_len(MAX_ARTIFACT_BYTES as u64 + 1).unwrap();
    let environment = ExecutionEnvironment::new(
        &workspace,
        &workspace,
        policy,
        std::env::current_exe().unwrap(),
        Arc::new(DirectSandbox),
    )
    .unwrap();
    let mut output = "[secret](./secret.txt) [missing](./missing.pdf) [outside](../outside.txt) [encoded](%2E%2E/outside.txt) [large](./large.bin)".to_owned();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(workspace.join("secret.txt"), workspace.join("linked.txt"))
            .unwrap();
        output.push_str(" [symlink](./linked.txt)");
    }
    let store = ArtifactStore::new(root.path().join("artifacts"));
    let session = Uuid::new_v4().to_string();
    store
        .capture(
            &session,
            &Uuid::new_v4().to_string(),
            &output,
            Some(&environment),
        )
        .unwrap();
    let files = store.list(&session).unwrap();
    assert!(files.len() >= 5);
    for file in files {
        assert!(!file.available);
        assert!(file.error.is_some());
        assert!(store.download(&session, &file.id).is_err());
    }
}

#[test]
fn reference_links_and_images_are_discovered_but_unlinked_files_are_not() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    for name in ["report.pdf", "image.png", "unlinked.txt"] {
        std::fs::write(workspace.join(name), b"fixture").unwrap();
    }
    let env = ExecutionEnvironment::new(
        &workspace,
        &workspace,
        SandboxPolicy::workspace_write("references", &workspace),
        std::env::current_exe().unwrap(),
        Arc::new(DirectSandbox),
    )
    .unwrap();
    let store = ArtifactStore::new(root.path().join("artifacts"));
    let session = Uuid::new_v4().to_string();
    store
        .capture(
            &session,
            &Uuid::new_v4().to_string(),
            "[Report][deck]\n\n[deck]: ./report.pdf\n\n![Image](./image.png)",
            Some(&env),
        )
        .unwrap();
    let files = store.list(&session).unwrap();
    assert_eq!(files.len(), 2);
    assert!(
        files
            .iter()
            .all(|file| file.available && file.name != "unlinked.txt")
    );
}

#[test]
fn html_keeps_local_dependencies_without_crawling_or_crossing_session_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(workspace.join("report")).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    std::fs::write(workspace.join("report/index.html"), "<html><head><link rel=\"stylesheet\" href=\"style.css\"></head><body><script src=\"app.js\"></script><img src=\"image.png\"><a href=\"secret.txt\">not an asset</a></body></html>").unwrap();
    std::fs::write(
        workspace.join("report/style.css"),
        "body{background:url(image.png)}",
    )
    .unwrap();
    std::fs::write(
        workspace.join("report/app.js"),
        "document.body.dataset.ready='yes';",
    )
    .unwrap();
    std::fs::write(workspace.join("report/image.png"), b"image bytes").unwrap();
    std::fs::write(workspace.join("report/secret.txt"), b"not included").unwrap();
    let environment = ExecutionEnvironment::new(
        &workspace,
        &workspace,
        SandboxPolicy::workspace_write("preview", &workspace),
        std::env::current_exe().unwrap(),
        Arc::new(DirectSandbox),
    )
    .unwrap();
    let store = ArtifactStore::new(root.path().join("artifacts"));
    let session = Uuid::new_v4().to_string();
    let execution = Uuid::new_v4().to_string();
    store
        .capture(
            &session,
            &execution,
            "[Report](report/index.html)",
            Some(&environment),
        )
        .unwrap();
    let files = store.list(&session).unwrap();
    assert_eq!(files.len(), 4);
    assert_eq!(files.iter().filter(|file| !file.resource).count(), 1);
    assert!(files.iter().all(|file| file.available));
    let script = files.iter().find(|file| file.name == "app.js").unwrap();
    std::fs::write(workspace.join("report/app.js"), "changed").unwrap();
    assert_eq!(
        store.download(&session, &script.id).unwrap().1,
        b"document.body.dataset.ready='yes';"
    );
    assert!(store.list(&Uuid::new_v4().to_string()).unwrap().is_empty());
}
