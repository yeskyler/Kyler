use std::{collections::BTreeMap, path::Path, sync::Arc};

use tokio::net::TcpListener;

use kyler_config::{AppConfig, SecretString, SkillSettings};
use kyler_core::{Agent, skills::SkillManager};
use kyler_protocol::api::skills::{SaveSkillRequest, SkillAction, SkillStatus};

fn settings(root: &Path) -> SkillSettings {
    SkillSettings {
        user_directory: root.join("user"),
        state_file: root.join("state.json"),
        ..Default::default()
    }
}
fn package(root: &Path, name: &str, body: &str) {
    let directory = root.join(name);
    std::fs::create_dir_all(directory.join("references")).unwrap();
    std::fs::write(directory.join("SKILL.md"),format!("---\nname: {name}\ndescription: >\n  Help with {name} tasks.\nmetadata:\n  author: fixture\n---\n{body}\n")).unwrap();
    std::fs::write(
        directory.join("references/guide.md"),
        "Reference instructions",
    )
    .unwrap();
}
#[tokio::test]
async fn project_approval_is_scoped_fingerprinted_and_preserved_across_reload() {
    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path());
    let workspace = root.path().join("project");
    let other = root.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    package(&settings.user_directory, "review", "User instructions");
    package(
        &workspace.join(".kyler/skills"),
        "review",
        "Project instructions",
    );
    let manager = Arc::new(SkillManager::new(settings.clone()).unwrap());
    let infos = manager.list(Some(&workspace), false).await.unwrap();
    assert_eq!(infos.len(), 4);
    assert!(infos.iter().any(|i| i.status == SkillStatus::Shadowed));
    let project = infos
        .iter()
        .find(|i| i.status == SkillStatus::NeedsApproval)
        .unwrap();
    assert!(
        manager
            .snapshot(Some(&workspace))
            .await
            .unwrap()
            .load("review")
            .is_err()
    );
    let details = manager
        .details(Some(&workspace), &project.id)
        .await
        .unwrap();
    assert_eq!(details.instructions, "Project instructions");
    manager
        .action(
            Some(&workspace),
            &project.id,
            SkillAction::Enable,
            &project.fingerprint,
        )
        .await
        .unwrap();
    let snapshot = manager.snapshot(Some(&workspace)).await.unwrap();
    assert_eq!(
        snapshot.load("review").unwrap().instructions,
        "Project instructions"
    );
    assert_eq!(
        snapshot.resource("review", "references/guide.md").unwrap()["content"],
        "Reference instructions"
    );
    for path in [
        "../state.json",
        "/etc/passwd",
        "references/../../state.json",
    ] {
        assert!(snapshot.resource("review", path).is_err());
    }
    assert_eq!(
        manager
            .snapshot(Some(&other))
            .await
            .unwrap()
            .load("review")
            .unwrap()
            .instructions,
        "User instructions"
    );
    let restored = Arc::new(SkillManager::new(settings).unwrap());
    assert_eq!(
        restored
            .snapshot(Some(&workspace))
            .await
            .unwrap()
            .load("review")
            .unwrap()
            .instructions,
        "Project instructions"
    );
    std::fs::write(
        workspace.join(".kyler/skills/review/references/guide.md"),
        "Changed resource",
    )
    .unwrap();
    assert!(snapshot.load("review").is_err());
    assert!(snapshot.resource("review", "references/guide.md").is_err());
    assert!(
        manager
            .action(
                Some(&workspace),
                &project.id,
                SkillAction::Enable,
                &project.fingerprint
            )
            .await
            .is_err()
    );
    let refreshed = manager.list(Some(&workspace), true).await.unwrap();
    assert!(refreshed.iter().any(|i|i.status==SkillStatus::NeedsApproval&&i.fingerprint!=project.fingerprint));
}
#[tokio::test]
async fn user_skills_can_be_disabled_and_untrusted_packages_cannot_escape_roots() {
    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path());
    package(&settings.user_directory, "review", "User instructions");
    let manager = Arc::new(SkillManager::new(settings.clone()).unwrap());
    let infos = manager.list(None, false).await.unwrap();
    let review = infos.iter().find(|s| s.name == "review").unwrap();
    let snapshot = manager.snapshot(None).await.unwrap();
    assert!(snapshot.catalog_text().contains("Help with review"));
    assert!(!snapshot.catalog_text().contains("User instructions"));
    manager
        .action(None, &review.id, SkillAction::Disable, &review.fingerprint)
        .await
        .unwrap();
    assert!(snapshot.load("review").is_err());
    assert!(
        !manager
            .snapshot(None)
            .await
            .unwrap()
            .available
            .contains_key("review")
    );
    #[cfg(unix)]
    {
        let directory = settings.user_directory.join("escape");
        std::fs::create_dir_all(&directory).unwrap();
        std::os::unix::fs::symlink(
            settings.user_directory.join("review/SKILL.md"),
            directory.join("SKILL.md"),
        )
        .unwrap();
        assert!(
            manager
                .list(None, true)
                .await
                .unwrap()
                .iter()
                .any(|s| s.status == SkillStatus::Invalid)
        );
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            std::fs::metadata(&settings.state_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        kyler_core::skills::references("@review $review @connector:server @file.rs Task"),
        vec!["review"]
    );
}
#[tokio::test]
async fn explicit_skill_is_loaded_as_tool_content_before_the_first_model_request() {
    use std::future::IntoFuture;

    use axum::{Json, Router, routing::post};
    use serde_json::{Value, json};

    async fn model(Json(request): Json<Value>) -> Json<Value> {
        let messages = request["messages"].as_array().unwrap();
        assert!(messages.iter().any(|m| m["role"] == "system"
            && m["content"].as_str().unwrap().contains("Available skills")));
        assert!(
            messages
                .iter()
                .filter(|m| m["role"] == "system")
                .all(|m| !m["content"].as_str().unwrap().contains("UNIQUE_SKILL_BODY"))
        );
        assert!(
            messages.iter().any(|m| m["role"] == "tool"
                && m["content"].as_str().unwrap().contains("UNIQUE_SKILL_BODY"))
        );
        Json(
            json!({"id":"test","object":"chat.completion","created":0,"model":"test","choices":[{"index":0,"message":{"role":"assistant","content":"Done"},"finish_reason":"stop"}]}),
        )
    }
    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path());
    package(&settings.user_directory, "review", "UNIQUE_SKILL_BODY");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().route("/v1/chat/completions", post(model)),
        )
        .into_future(),
    );
    let mut config = AppConfig::default();
    config.skills = settings;
    config.llm.base_url = base;
    config.llm.model = "test".into();
    config.llm.api_key = SecretString::new("fixture");
    let agent = Agent::from_config_with_tools(&config, Arc::new(Default::default()))
        .await
        .unwrap();
    let result = agent.run("@review Inspect the changes").await.unwrap();
    assert_eq!(result.output, "Done");
    assert!(result.session.events.iter().any(|event|event.content.iter().any(|item|matches!(item,kyler_core::context::ContentItem::ToolResult {name,content,..} if name=="load_skill"&&content.contains("UNIQUE_SKILL_BODY")))));
    agent.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn builtins_are_embedded_enabled_and_follow_override_precedence() {
    use kyler_protocol::api::skills::SkillSource;

    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path());
    let manager = Arc::new(SkillManager::new(settings.clone()).unwrap());
    let builtin = manager.list(None, false).await.unwrap();
    assert_eq!(builtin.len(), 2);
    assert!(
        builtin
            .iter()
            .all(|p| p.source == SkillSource::Builtin && p.status == SkillStatus::Enabled)
    );
    assert!(!settings.user_directory.exists());
    let snapshot = manager.snapshot(None).await.unwrap();
    assert!(
        snapshot
            .load("skill-creator")
            .unwrap()
            .instructions
            .contains("save_skill")
    );
    assert!(!snapshot.catalog_text().contains("## Authoring"));
    assert!(
        snapshot.resource("skill-creator", "SKILL.md").unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("name: skill-creator")
    );
    let creator = builtin.iter().find(|p| p.name == "skill-creator").unwrap();
    manager
        .action(
            None,
            &creator.id,
            SkillAction::Disable,
            &creator.fingerprint,
        )
        .await
        .unwrap();
    assert!(snapshot.load("skill-creator").is_err());
    let workspace = root.path().join("project");
    package(&settings.user_directory, "skill-creator", "User override");
    package(
        &workspace.join(".kyler/skills"),
        "skill-creator",
        "Project override",
    );
    let infos = manager.list(Some(&workspace), true).await.unwrap();
    assert_eq!(
        infos
            .iter()
            .filter(|p| p.name == "skill-creator" && p.status == SkillStatus::Shadowed)
            .count(),
        2
    );
    assert!(
        manager
            .snapshot(Some(&workspace))
            .await
            .unwrap()
            .load("skill-creator")
            .is_err()
    );
    let project = infos
        .iter()
        .find(|p| p.source == SkillSource::Project)
        .unwrap();
    manager
        .action(
            Some(&workspace),
            &project.id,
            SkillAction::Enable,
            &project.fingerprint,
        )
        .await
        .unwrap();
    assert_eq!(
        manager
            .snapshot(Some(&workspace))
            .await
            .unwrap()
            .load("skill-creator")
            .unwrap()
            .instructions,
        "Project override"
    );
    std::fs::write(
        workspace.join(".kyler/skills/skill-creator/SKILL.md"),
        "invalid",
    )
    .unwrap();
    assert!(
        !manager
            .snapshot(Some(&workspace))
            .await
            .unwrap()
            .available
            .contains_key("skill-creator")
    );
}

fn save_request(name: &str, body: &str) -> SaveSkillRequest {
    SaveSkillRequest {
        name: name.into(),
        scope: Default::default(),
        expected_fingerprint: None,
        files: BTreeMap::from([
            (
                "SKILL.md".into(),
                format!("---\nname: {name}\ndescription: Help with {name} tasks\n---\n{body}"),
            ),
            ("references/guide.md".into(), "Guide".into()),
        ]),
    }
}

#[tokio::test]
async fn saving_refreshes_other_workspaces_and_updates_require_current_fingerprint() {
    use kyler_protocol::api::skills::SkillScope;

    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path());
    let workspace = root.path().join("project");
    std::fs::create_dir(&workspace).unwrap();
    let manager = Arc::new(SkillManager::new(settings.clone()).unwrap());
    manager.list(Some(&workspace), false).await.unwrap();
    let mut request = save_request("new-skill", "Initial instructions");
    let info = manager.save(None, request.clone()).await.unwrap();
    assert_eq!(info.status, SkillStatus::Enabled);
    assert!(
        manager
            .list(Some(&workspace), false)
            .await
            .unwrap()
            .iter()
            .any(|p| p.name == "new-skill")
    );
    let old = manager.snapshot(None).await.unwrap();
    assert!(manager.save(None, request.clone()).await.is_err());
    request
        .files
        .insert("references/guide.md".into(), "Updated guide".into());
    request.expected_fingerprint = Some("stale".into());
    assert!(manager.save(None, request.clone()).await.is_err());
    assert_eq!(
        old.resource("new-skill", "references/guide.md").unwrap()["content"],
        "Guide"
    );
    request.expected_fingerprint = Some(info.fingerprint);
    let updated = manager.save(None, request).await.unwrap();
    assert!(old.load("new-skill").is_err());
    assert_eq!(
        manager
            .snapshot(None)
            .await
            .unwrap()
            .resource("new-skill", "references/guide.md")
            .unwrap()["content"],
        "Updated guide"
    );
    let mut project = save_request("project-skill", "Project instructions");
    project.scope = SkillScope::Project;
    let project_info = manager
        .save(Some(&workspace), project.clone())
        .await
        .unwrap();
    assert_eq!(project_info.status, SkillStatus::NeedsApproval);
    manager
        .action(
            Some(&workspace),
            &project_info.id,
            SkillAction::Enable,
            &project_info.fingerprint,
        )
        .await
        .unwrap();
    project.expected_fingerprint = Some(project_info.fingerprint);
    project
        .files
        .insert("references/guide.md".into(), "Changed again".into());
    assert_eq!(
        manager
            .save(Some(&workspace), project)
            .await
            .unwrap()
            .status,
        SkillStatus::NeedsApproval
    );
    let restored = Arc::new(SkillManager::new(settings).unwrap());
    assert_eq!(
        restored
            .snapshot(None)
            .await
            .unwrap()
            .load("new-skill")
            .unwrap()
            .info
            .fingerprint,
        updated.fingerprint
    );
}

#[tokio::test]
async fn invalid_skill_writes_do_not_escape_or_replace_existing_packages() {
    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path());
    let manager = SkillManager::new(settings.clone()).unwrap();
    for path in [
        "../escape",
        "/escape",
        "references/../../escape",
        "references\\escape",
    ] {
        let mut request = save_request("invalid", "Instructions");
        request.files.insert(path.into(), "Unwanted content".into());
        assert!(manager.save(None, request).await.is_err());
    }
    assert!(!settings.user_directory.join("invalid").exists());
    let mut request = save_request("mismatch", "Instructions");
    request.files.insert(
        "SKILL.md".into(),
        "---\nname: other\ndescription: Wrong name\n---\nInstructions".into(),
    );
    assert!(manager.save(None, request).await.is_err());
    assert!(!settings.user_directory.join("mismatch").exists());
    package(
        &settings.user_directory,
        "existing",
        "Preserved instructions",
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            settings.user_directory.join("existing"),
            settings.user_directory.join("linked"),
        )
        .unwrap();
        assert!(
            manager
                .save(None, save_request("linked", "Replacement"))
                .await
                .is_err()
        );
        let other = root.path().join("elsewhere");
        std::fs::create_dir(&other).unwrap();
        std::os::unix::fs::symlink(&other, settings.user_directory.join("resources")).unwrap();
        assert!(
            manager
                .save(None, save_request("resources", "Replacement"))
                .await
                .is_err()
        );
        assert!(!other.join("SKILL.md").exists());
    }
    assert!(
        std::fs::read_to_string(settings.user_directory.join("existing/SKILL.md"))
            .unwrap()
            .contains("Preserved instructions")
    );
}

#[tokio::test]
async fn local_install_preserves_binary_resources_and_never_executes_scripts() {
    use kyler_protocol::api::skills::InstallSkillRequest;

    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path());
    let source = root.path().join("source");
    package(&source, "imported", "Installed instructions");
    std::fs::create_dir(source.join("imported/assets")).unwrap();
    std::fs::write(source.join("imported/assets/binary.dat"), [0, 255, 128]).unwrap();
    let marker = root.path().join("should-not-exist");
    std::fs::write(
        source.join("imported/hook.sh"),
        format!("touch {}", marker.display()),
    )
    .unwrap();
    let manager = Arc::new(SkillManager::new(settings.clone()).unwrap());
    let request = InstallSkillRequest {
        path: source.join("imported"),
        scope: Default::default(),
        expected_fingerprint: None,
    };
    let info = manager.install(None, request.clone()).await.unwrap();
    assert_eq!(info.status, SkillStatus::Enabled);
    assert_eq!(
        std::fs::read(settings.user_directory.join("imported/assets/binary.dat")).unwrap(),
        [0, 255, 128]
    );
    assert!(!marker.exists());
    assert!(manager.install(None, request.clone()).await.is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&marker, source.join("imported/link")).unwrap();
        let mut update = request;
        update.expected_fingerprint = Some(info.fingerprint);
        assert!(manager.install(None, update).await.is_err());
    }
}

#[tokio::test]
async fn creator_can_save_and_load_a_new_skill_during_one_run() {
    use std::{
        future::IntoFuture,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use axum::{Json, Router, extract::State, routing::post};
    use serde_json::{Value, json};

    async fn model(
        State(step): State<Arc<AtomicUsize>>,
        Json(request): Json<Value>,
    ) -> Json<Value> {
        let index = step.fetch_add(1, Ordering::SeqCst);
        let messages = request["messages"].as_array().unwrap();
        let message = match index {
            0 => {
                assert!(messages.iter().any(|m| m["role"] == "tool"
                    && m["content"].as_str().unwrap().contains("# Skill Creator")));
                assert!(
                    request["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|t| t["function"]["name"] == "save_skill")
                );
                json!({"role":"assistant","tool_calls":[{"id":"save","type":"function","function":{"name":"save_skill","arguments":serde_json::to_string(&save_request("created-in-run", "UNIQUE_NEW_INSTRUCTIONS")).unwrap()}}]})
            }
            1 => {
                assert!(messages.iter().any(|m| m["role"] == "system"
                    && m["content"].as_str().unwrap().contains("created-in-run")));
                assert!(messages.iter().any(|m| m["role"] == "tool"
                    && m["content"].as_str().unwrap().contains("created-in-run")));
                json!({"role":"assistant","tool_calls":[{"id":"load","type":"function","function":{"name":"load_skill","arguments":"{\"name\":\"created-in-run\"}"}}]})
            }
            _ => {
                assert!(messages.iter().any(|m| {
                    m["role"] == "tool"
                        && m["content"]
                            .as_str()
                            .unwrap()
                            .contains("UNIQUE_NEW_INSTRUCTIONS")
                }));
                json!({"role":"assistant","content":"Created and loaded"})
            }
        };
        Json(
            json!({"id":"fixture","object":"chat.completion","created":0,"model":"test","choices":[{"index":0,"message":message,"finish_reason":if index < 2 {"tool_calls"} else {"stop"}}]}),
        )
    }
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let steps = Arc::new(AtomicUsize::new(0));
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(model))
                .with_state(steps.clone()),
        )
        .into_future(),
    );
    let mut config = AppConfig::default();
    config.skills = settings(root.path());
    config.llm.base_url = base;
    config.llm.model = "test".into();
    config.llm.api_key = SecretString::new("fixture");
    let agent = Agent::from_config_with_tools(&config, Arc::new(Default::default()))
        .await
        .unwrap();
    let result = agent
        .run("$skill-creator Create a reusable skill")
        .await
        .unwrap();
    assert_eq!(result.output, "Created and loaded");
    assert_eq!(steps.load(Ordering::SeqCst), 3);
    assert!(
        config
            .skills
            .user_directory
            .join("created-in-run/SKILL.md")
            .is_file()
    );
    agent.shutdown().await;
    server.abort();
}

#[test]
fn skill_writes_follow_existing_approval_modes() {
    use kyler_core::{
        approval::review,
        tools::{Tool, ToolExecutionMode, save_skill::SaveSkillTool},
    };
    use kyler_protocol::api::permissions::{ApprovalStatus, PermissionMode};

    let root = tempfile::tempdir().unwrap();
    let tool = SaveSkillTool {
        manager: Arc::new(SkillManager::new(settings(root.path())).unwrap()),
        workspace: None,
    };
    let args = serde_json::to_value(save_request("reviewed", "Instructions")).unwrap();
    let operation = tool.approval_requirement(&args).unwrap();
    assert_eq!(tool.execution_mode(), ToolExecutionMode::Host);
    assert_eq!(
        review(PermissionMode::ReadOnly, operation, &args),
        ApprovalStatus::Denied
    );
    assert_eq!(
        review(PermissionMode::AutoReview, operation, &args),
        ApprovalStatus::Pending
    );
    assert_eq!(
        review(PermissionMode::FullAccess, operation, &args),
        ApprovalStatus::Allowed
    );
}

#[tokio::test]
async fn uploaded_packages_preserve_binary_resources_reject_conflicts_and_remove_only_user_skills()
{
    use kyler_core::skills::package_directory;
    use kyler_protocol::api::skills::SkillSource;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    package(&source, "skill-creator", "User override");
    std::fs::write(
        source.join("skill-creator/references/image.bin"),
        [0, 255, 17, 128],
    )
    .unwrap();
    let archive = package_directory(&source.join("skill-creator")).unwrap();
    let manager = SkillManager::new(settings(root.path())).unwrap();
    let info = manager.install_archive(archive.clone()).await.unwrap();
    assert_eq!(info.source, SkillSource::User);
    assert_eq!(info.status, SkillStatus::Enabled);
    assert_eq!(
        std::fs::read(Path::new(&info.path).join("references/image.bin")).unwrap(),
        [0, 255, 17, 128]
    );
    assert!(manager.install_archive(archive).await.is_err());
    assert!(
        manager
            .remove_user(&info.id, "stale-fingerprint")
            .await
            .is_err()
    );
    let builtin = manager
        .list(None, true)
        .await
        .unwrap()
        .into_iter()
        .find(|skill| skill.name == "skill-installer")
        .unwrap();
    assert!(
        manager
            .remove_user(&builtin.id, &builtin.fingerprint)
            .await
            .is_err()
    );
    manager
        .remove_user(&info.id, &info.fingerprint)
        .await
        .unwrap();
    let restored = manager
        .list(None, true)
        .await
        .unwrap()
        .into_iter()
        .find(|skill| skill.name == "skill-creator")
        .unwrap();
    assert_eq!(restored.source, SkillSource::Builtin);
    assert_eq!(restored.status, SkillStatus::Enabled);
    assert!(!Path::new(&info.path).exists());
}
