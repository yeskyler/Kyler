use super::SkillManager;
use crate::sandbox::FilesystemPolicy;

use std::{collections::BTreeMap, fs};

use kyler_protocol::api::skills::{InstallSkillRequest, SaveSkillRequest, SkillScope};

#[tokio::test]
async fn installation_obeys_source_and_destination_descendant_restrictions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let workspace = root.join("workspace");
    let source = workspace.join("source");
    fs::create_dir_all(source.join("references")).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: imported\ndescription: Test imported package\n---\nInstructions",
    )
    .unwrap();
    fs::write(source.join("references/private.txt"), "Private reference").unwrap();
    let settings = kyler_config::SkillSettings {
        user_directory: root.join("user"),
        state_file: root.join("state.json"),
        ..Default::default()
    };
    let manager = SkillManager::new(settings).unwrap();
    let mut policy = FilesystemPolicy {
        readwrite_paths: vec![workspace.clone()],
        denied_paths: vec![source.join("references/private.txt")],
        ..Default::default()
    };
    let request = InstallSkillRequest {
        path: source.clone(),
        scope: SkillScope::Project,
        expected_fingerprint: None,
    };
    assert!(
        manager
            .install_checked(Some(&workspace), request.clone(), Some(policy.clone()))
            .await
            .is_err()
    );
    let destination = workspace.join(".kyler/skills/imported");
    assert!(!destination.exists());
    policy.denied_paths.clear();
    policy
        .readonly_paths
        .push(destination.join("protected.txt"));
    assert!(
        manager
            .install_checked(Some(&workspace), request.clone(), Some(policy.clone()))
            .await
            .is_err()
    );
    assert!(
        manager
            .check_destination_policy(Some(&workspace), SkillScope::Project, "imported", &policy)
            .is_err()
    );
    assert!(!destination.exists());
    policy.readonly_paths.clear();
    assert!(
        manager
            .install_checked(Some(&workspace), request, Some(policy))
            .await
            .is_ok()
    );
    assert!(destination.join("SKILL.md").is_file());
}

#[tokio::test]
async fn rejected_updates_leave_existing_package_and_resources_intact() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let settings = kyler_config::SkillSettings {
        user_directory: root.join("user"),
        state_file: root.join("state.json"),
        ..Default::default()
    };
    let manager = SkillManager::new(settings.clone()).unwrap();
    let mut request = SaveSkillRequest {
        name: "existing".into(),
        scope: SkillScope::User,
        expected_fingerprint: None,
        files: BTreeMap::from([
            (
                "SKILL.md".into(),
                "---\nname: existing\ndescription: Existing package\n---\nPreserved instructions"
                    .into(),
            ),
            ("references/guide.md".into(), "Preserved resource".into()),
        ]),
    };
    let info = manager.save(None, request.clone()).await.unwrap();
    request.expected_fingerprint = Some(info.fingerprint);
    request
        .files
        .insert("SKILL.md".into(), "invalid metadata".into());
    assert!(manager.save(None, request).await.is_err());
    assert!(
        fs::read_to_string(settings.user_directory.join("existing/SKILL.md"))
            .unwrap()
            .contains("Preserved instructions")
    );
    assert_eq!(
        fs::read_to_string(settings.user_directory.join("existing/references/guide.md")).unwrap(),
        "Preserved resource"
    );
}

#[tokio::test]
async fn directories_use_real_model_tokens_and_prioritize_explicit_skills() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let settings = kyler_config::SkillSettings {
        user_directory: root.join("user"),
        state_file: root.join("state.json"),
        catalog_token_budget: 256,
        ..Default::default()
    };
    for index in 0..20 {
        let name = format!("skill-{index:02}");
        let dir = settings.user_directory.join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"),format!("---\nname: {name}\ndescription: Useful guidance\n---\nBODY_MUST_NOT_APPEAR_IN_DIRECTORY")).unwrap();
    }
    let manager = std::sync::Arc::new(SkillManager::new(settings).unwrap());
    let snapshot = manager.snapshot(None).await.unwrap();
    for tokenizer in [
        kyler_protocol::api::models::Tokenizer::O200kBase,
        kyler_protocol::api::models::Tokenizer::Cl100kBase,
    ] {
        let view = snapshot.catalog_view(tokenizer, &["skill-19".into()]);
        assert!(view.tokens <= 256);
        assert_eq!(view.tokens, super::render::tokens(&view.text, tokenizer));
        assert!(view.omitted > 0);
        assert!(view.text.contains("list_skills"));
        assert!(view.text.contains("\"name\":\"skill-19\""));
        assert!(!view.text.contains("BODY_MUST_NOT_APPEAR"));
    }
}

#[tokio::test]
async fn skill_pages_and_compressed_cursors_never_skip_entries_or_expose_instructions() {
    use crate::{
        context::ExecutionContext,
        tools::{Tool, ToolContext, ToolOutputBudget, list_skills::ListSkillsTool},
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let settings = kyler_config::SkillSettings {
        user_directory: root.join("user"),
        state_file: root.join("state.json"),
        ..Default::default()
    };
    for index in 0..14 {
        let name = format!("page-{index:02}");
        let dir = settings.user_directory.join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: {}\n---\nPRIVATE_SKILL_BODY",
                "Detailed metadata ".repeat(30)
            ),
        )
        .unwrap();
    }
    let manager = std::sync::Arc::new(SkillManager::new(settings).unwrap());
    let snapshot = std::sync::Arc::new(manager.snapshot(None).await.unwrap());
    let tool = ListSkillsTool(snapshot.clone());
    let mut execution = ExecutionContext::new();
    let mut state = std::collections::HashMap::new();
    let mut context = ToolContext {
        processes: None,
        execution: &mut execution,
        state: &mut state,
        session_id: "test",
        tool_call_id: "page",
        subagents: None,
        environment: None,
    };
    let mut offset = 0;
    let mut names = Vec::new();
    loop {
        let output=tool.execute(&serde_json::json!({"offset":offset,"limit":20,"expected_version":snapshot.version}).to_string(),&mut context).await.unwrap();
        let compressed = tool
            .compress_output(&output, &ToolOutputBudget { max_bytes: 2400 })
            .unwrap();
        assert!(compressed.content.len() <= 2400);
        assert!(!compressed.content.contains("PRIVATE_SKILL_BODY"));
        let page: serde_json::Value = serde_json::from_str(&compressed.content).unwrap();
        for item in page["skills"].as_array().unwrap() {
            names.push(item["name"].as_str().unwrap().to_owned());
            assert!(item.get("path").is_none());
        }
        let Some(next) = page["next_offset"].as_u64() else {
            break;
        };
        assert!(next as usize > offset);
        offset = next as usize;
    }
    assert_eq!(names.len(), snapshot.available.len());
    let unique = names.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), names.len());
    let info = &snapshot.available["page-00"].info;
    manager
        .action(
            None,
            &info.id,
            kyler_protocol::api::skills::SkillAction::Disable,
            &info.fingerprint,
        )
        .await
        .unwrap();
    assert!(
        tool.execute(
            &serde_json::json!({"expected_version":snapshot.version}).to_string(),
            &mut context
        )
        .await
        .is_err()
    );
}
