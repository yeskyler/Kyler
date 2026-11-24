use std::{fs, path::Path, sync::Arc};

use kyler_config::SkillSettings;
use kyler_protocol::api::skills::{SkillAction, SkillReference, SkillSource, SkillStatus};
use kyler_skills::{SkillManager, validate_references};

fn settings(root: &Path) -> SkillSettings {
    SkillSettings {
        user_directory: root.join("user"),
        state_file: root.join("state.json"),
        ..Default::default()
    }
}
fn package(root: &Path, name: &str, body: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.join("references")).unwrap();
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Help with {name}\n---\n{body}"),
    )
    .unwrap();
    fs::write(path.join("references/guide.md"), "Original reference").unwrap();
}
#[tokio::test]
async fn unchanged_catalog_reuses_validated_packages_and_external_changes_invalidate_them() {
    let temp = tempfile::tempdir().unwrap();
    let settings = settings(temp.path());
    package(&settings.user_directory, "alpha", "Original instructions");
    let manager = Arc::new(SkillManager::new(settings.clone()).unwrap());
    let first = manager.snapshot(None).await.unwrap();
    let second = manager.snapshot(None).await.unwrap();
    assert!(Arc::ptr_eq(
        &first.available["alpha"],
        &second.available["alpha"]
    ));
    let reference = SkillReference {
        id: first.available["alpha"].info.id.clone(),
        fingerprint: first.available["alpha"].info.fingerprint.clone(),
    };
    assert_eq!(
        first.load_reference(&reference).unwrap().instructions,
        "Original instructions"
    );
    fs::write(
        settings.user_directory.join("alpha/references/guide.md"),
        "Changed reference",
    )
    .unwrap();
    let changed = manager.snapshot(None).await.unwrap();
    assert_ne!(first.version, changed.version);
    assert!(!Arc::ptr_eq(
        &first.available["alpha"],
        &changed.available["alpha"]
    ));
    assert!(first.load_reference(&reference).is_err());
    assert!(changed.load_reference(&reference).is_err());
}
#[tokio::test]
async fn project_overrides_require_exact_review_and_never_fall_back_to_user_packages() {
    let temp = tempfile::tempdir().unwrap();
    let settings = settings(temp.path());
    let workspace = temp.path().join("project");
    package(&settings.user_directory, "same", "User instructions");
    package(
        &workspace.join(&settings.project_directory),
        "same",
        "Project instructions",
    );
    let manager = Arc::new(SkillManager::new(settings.clone()).unwrap());
    assert!(
        !manager
            .snapshot(Some(&workspace))
            .await
            .unwrap()
            .available
            .contains_key("same")
    );
    let info = manager
        .list(Some(&workspace), false)
        .await
        .unwrap()
        .into_iter()
        .find(|info| info.name == "same" && info.source == SkillSource::Project)
        .unwrap();
    assert_eq!(info.status, SkillStatus::NeedsApproval);
    manager
        .action(
            Some(&workspace),
            &info.id,
            SkillAction::Enable,
            &info.fingerprint,
        )
        .await
        .unwrap();
    let snapshot = manager.snapshot(Some(&workspace)).await.unwrap();
    assert_eq!(
        snapshot.load("same").unwrap().instructions,
        "Project instructions"
    );
    fs::write(
        workspace
            .join(&settings.project_directory)
            .join("same/SKILL.md"),
        "---\nname: same\ndescription: Changed\n---\nChanged instructions",
    )
    .unwrap();
    assert!(snapshot.load("same").is_err());
    assert!(
        !manager
            .snapshot(Some(&workspace))
            .await
            .unwrap()
            .available
            .contains_key("same")
    );
    assert!(
        manager
            .action(
                Some(&workspace),
                &info.id,
                SkillAction::Enable,
                &info.fingerprint
            )
            .await
            .is_err()
    );
    fs::write(
        workspace
            .join(&settings.project_directory)
            .join("same/SKILL.md"),
        "invalid metadata",
    )
    .unwrap();
    assert!(
        !manager
            .snapshot(Some(&workspace))
            .await
            .unwrap()
            .available
            .contains_key("same")
    );
}
#[tokio::test]
async fn grants_are_workspace_scoped_and_disable_revokes_old_snapshots() {
    let temp = tempfile::tempdir().unwrap();
    let settings = settings(temp.path());
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    package(
        &a.join(&settings.project_directory),
        "project",
        "Same instructions",
    );
    package(
        &b.join(&settings.project_directory),
        "project",
        "Same instructions",
    );
    let manager = Arc::new(SkillManager::new(settings).unwrap());
    let info = manager
        .list(Some(&a), false)
        .await
        .unwrap()
        .into_iter()
        .find(|info| info.name == "project")
        .unwrap();
    manager
        .action(Some(&a), &info.id, SkillAction::Enable, &info.fingerprint)
        .await
        .unwrap();
    let first = manager.snapshot(Some(&a)).await.unwrap();
    assert!(first.load("project").is_ok());
    assert!(
        manager
            .snapshot(Some(&b))
            .await
            .unwrap()
            .load("project")
            .is_err()
    );
    manager
        .action(Some(&a), &info.id, SkillAction::Disable, &info.fingerprint)
        .await
        .unwrap();
    assert!(first.load("project").is_err());
    assert!(
        first
            .directory()
            .unwrap()
            .skills
            .iter()
            .all(|info| info.name != "project")
    );
}
#[tokio::test]
async fn binary_resources_are_read_only_and_cannot_escape_the_package() {
    let temp = tempfile::tempdir().unwrap();
    let settings = settings(temp.path());
    package(
        &settings.user_directory,
        "binary",
        "Never execute scripts while loading",
    );
    fs::write(
        settings.user_directory.join("binary/references/data.bin"),
        [0, 255, 1, 2],
    )
    .unwrap();
    let manager = Arc::new(SkillManager::new(settings).unwrap());
    let snapshot = manager.snapshot(None).await.unwrap();
    assert_eq!(
        snapshot
            .resource_bytes("binary", "references/data.bin")
            .unwrap()
            .bytes,
        [0, 255, 1, 2]
    );
    assert!(snapshot.resource_bytes("binary", "../outside").is_err());
    #[cfg(unix)]
    {
        let root = &snapshot.available["binary"].root;
        fs::remove_file(root.join("references/data.bin")).unwrap();
        std::os::unix::fs::symlink(
            temp.path().join("outside"),
            root.join("references/data.bin"),
        )
        .unwrap();
        assert!(
            snapshot
                .resource_bytes("binary", "references/data.bin")
                .is_err()
        );
    }
}
#[test]
fn structured_references_are_bounded_and_cannot_be_paths_or_conflicting_versions() {
    let reference = SkillReference {
        id: "a".repeat(64),
        fingerprint: "b".repeat(64),
    };
    validate_references(std::slice::from_ref(&reference)).unwrap();
    assert!(validate_references(&vec![reference.clone(); 17]).is_err());
    assert!(
        validate_references(&[SkillReference {
            id: "/tmp/skill".into(),
            fingerprint: "b".repeat(64)
        }])
        .is_err()
    );
    let other = SkillReference {
        fingerprint: "c".repeat(64),
        ..reference.clone()
    };
    assert!(validate_references(&[reference, other]).is_err());
}

#[tokio::test]
async fn drafts_are_invisible_until_exact_fingerprint_publication_and_preserve_live_packages() {
    use kyler_protocol::api::skills::{SaveSkillRequest, SkillScope};
    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path());
    package(&settings.user_directory, "workflow", "Installed original");
    let manager = Arc::new(SkillManager::new(settings).unwrap());
    let installed = manager.snapshot(None).await.unwrap().available["workflow"]
        .info
        .fingerprint
        .clone();
    let request = |text: &str, expected| SaveSkillRequest {
        name: "workflow".into(),
        scope: SkillScope::User,
        files: [(
            "SKILL.md".into(),
            format!("---\nname: workflow\ndescription: A tested workflow\n---\n{text}"),
        )]
        .into(),
        expected_fingerprint: expected,
    };
    let draft = manager
        .save_draft(request("Draft instructions", None))
        .await
        .unwrap();
    assert_eq!(
        manager.snapshot(None).await.unwrap().available["workflow"].instructions,
        "Installed original"
    );
    assert_eq!(manager.drafts().await.unwrap().len(), 1);
    assert!(
        manager
            .publish_draft("workflow", "stale", Some(installed.clone()))
            .await
            .is_err()
    );
    assert!(
        manager
            .publish_draft("workflow", &draft.fingerprint, None)
            .await
            .is_err()
    );
    let next = manager
        .save_draft(request("Revised draft", Some(draft.fingerprint.clone())))
        .await
        .unwrap();
    assert!(
        manager
            .publish_draft("workflow", &draft.fingerprint, Some(installed.clone()))
            .await
            .is_err()
    );
    manager
        .publish_draft("workflow", &next.fingerprint, Some(installed))
        .await
        .unwrap();
    assert_eq!(
        manager.snapshot(None).await.unwrap().available["workflow"].instructions,
        "Revised draft"
    );
}

#[tokio::test]
async fn draft_paths_cannot_escape_or_execute_resources() {
    use kyler_protocol::api::skills::{SaveSkillRequest, SkillScope};
    let root = tempfile::tempdir().unwrap();
    let manager = Arc::new(SkillManager::new(settings(root.path())).unwrap());
    let request = |name: &str, path: &str| SaveSkillRequest {
        name: name.into(),
        scope: SkillScope::User,
        files: [
            (
                "SKILL.md".into(),
                format!("---\nname: {name}\ndescription: A safe draft\n---\nReview first"),
            ),
            (path.into(), "This script must never run".into()),
        ]
        .into(),
        expected_fingerprint: None,
    };
    assert!(
        manager
            .save_draft(request("../outside", "reference.txt"))
            .await
            .is_err()
    );
    assert!(
        manager
            .save_draft(request("safe", "../outside"))
            .await
            .is_err()
    );
    let draft = manager
        .save_draft(request("safe", "scripts/example.sh"))
        .await
        .unwrap();
    assert!(
        !manager
            .snapshot(None)
            .await
            .unwrap()
            .available
            .contains_key("safe")
    );
    assert_eq!(
        manager
            .draft_details("safe")
            .await
            .unwrap()
            .info
            .fingerprint,
        draft.fingerprint
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            root.path(),
            manager.settings.user_directory.join(".drafts/linked"),
        )
        .unwrap();
        assert!(manager.draft_details("linked").await.is_err());
    }
}

#[tokio::test]
async fn review_agent_builtin_is_discoverable_resource_scoped_and_fingerprint_bound() {
    let temp = tempfile::tempdir().unwrap();
    let settings = settings(temp.path());
    let manager = Arc::new(SkillManager::new(settings.clone()).unwrap());
    let snapshot = manager.snapshot(None).await.unwrap();
    let info = snapshot.available["review-agent"].info.clone();
    assert_eq!(info.label(), "Review Agent");
    assert_eq!(info.path, "builtin://review-agent");
    assert_eq!(info.source, SkillSource::Builtin);
    assert_eq!(info.status, SkillStatus::Enabled);
    assert!(!settings.user_directory.exists());
    let reference = SkillReference {
        id: info.id.clone(),
        fingerprint: info.fingerprint.clone(),
    };
    let loaded = snapshot.load_reference(&reference).unwrap();
    assert_eq!(loaded.info.name, "review-agent");
    for resource in &info.resources {
        assert!(
            !snapshot
                .resource_bytes("review-agent", resource)
                .unwrap()
                .bytes
                .is_empty()
        );
    }
    assert!(
        snapshot
            .resource_bytes("review-agent", "../SKILL.md")
            .is_err()
    );
    manager
        .action(None, &info.id, SkillAction::Disable, &info.fingerprint)
        .await
        .unwrap();
    let disabled = manager.snapshot(None).await.unwrap();
    assert!(!disabled.available.contains_key("review-agent"));
    manager
        .action(None, &info.id, SkillAction::Enable, "wrong-fingerprint")
        .await
        .unwrap_err();
    manager
        .action(None, &info.id, SkillAction::Enable, &info.fingerprint)
        .await
        .unwrap();
    assert!(
        manager
            .snapshot(None)
            .await
            .unwrap()
            .load_reference(&reference)
            .is_ok()
    );
}

#[tokio::test]
async fn skill_display_names_are_optional_and_do_not_change_invocation_identity() {
    let temp = tempfile::tempdir().unwrap();
    let settings = settings(temp.path());
    package(&settings.user_directory, "custom", "Custom instructions");
    fs::write(settings.user_directory.join("custom/SKILL.md"), "---\nname: custom\ndescription: Review this project\nmetadata:\n  display-name: Project Reviewer\n---\nCustom instructions").unwrap();
    let manager = Arc::new(SkillManager::new(settings).unwrap());
    let snapshot = manager.snapshot(None).await.unwrap();
    let info = &snapshot.available["custom"].info;
    assert_eq!(info.label(), "Project Reviewer");
    assert_eq!(info.name, "custom");
    assert!(snapshot.load("Project Reviewer").is_err());
    let mut serialized = serde_json::to_value(info).unwrap();
    serialized.as_object_mut().unwrap().remove("display_name");
    let legacy: kyler_protocol::api::skills::SkillInfo =
        serde_json::from_value(serialized).unwrap();
    assert_eq!(legacy.label(), "custom");
}
