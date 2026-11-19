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
