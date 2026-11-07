use super::*;
use crate::sandbox::{
    NetworkAccess, Sandbox, SandboxMode, SandboxPolicy, SandboxRequest, WorkerResponse,
};

use std::path::Path;

use serde_json::json;

use kyler_protocol::review::ReviewLocation;

pub(crate) struct Worker;
#[async_trait::async_trait]
impl Sandbox for Worker {
    async fn run(&self, request: SandboxRequest) -> Result<WorkerResponse> {
        assert_eq!(request.policy.mode, SandboxMode::ReadOnly);
        assert!(matches!(request.policy.network, NetworkAccess::Deny));
        assert!(!request.worker_request.allow_commands);
        assert_eq!(request.worker_request.tool, "review_git");
        Ok(crate::sandbox::worker::dispatch(request.worker_request).await)
    }
}
pub(crate) fn git(root: &Path, args: &[&str]) -> String {
    let result = std::process::Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().into()
}
pub(crate) fn fixture() -> (tempfile::TempDir, Arc<ExecutionEnvironment>) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "review@example.invalid"]);
    git(&root, &["config", "user.name", "Review fixture"]);
    std::fs::write(root.join("code.rs"), "pub fn value() -> u32 { 1 }\n").unwrap();
    std::fs::write(root.join("README.md"), "Project context\n").unwrap();
    git(&root, &["add", "code.rs", "README.md"]);
    git(&root, &["commit", "-qm", "base"]);
    git(&root, &["branch", "-M", "main"]);
    let policy = SandboxPolicy::workspace_write("review-fixture", &root);
    let environment = Arc::new(
        ExecutionEnvironment::new(
            &root,
            &root,
            policy,
            std::env::current_exe().unwrap(),
            Arc::new(Worker),
        )
        .unwrap(),
    );
    (directory, environment)
}
fn request(target: ReviewTarget) -> ReviewRequest {
    ReviewRequest {
        target,
        paths: Vec::new(),
        include_untracked: true,
    }
}
pub(crate) fn report() -> serde_json::Value {
    json!({"assessment":"no_findings","summary":"No actionable defect found.","findings":[],"coverage":{"status":"complete","limitations":[]}})
}

#[test]
fn delegate_schema_is_backward_compatible_and_review_targets_are_explicit() {
    let general: crate::subagent::DelegateTaskArgs =
        serde_json::from_value(json!({"task":"Explain a file"})).unwrap();
    assert_eq!(general.agent, crate::subagent::AgentKind::General);
    general.validate().unwrap();
    for value in [
        json!({"agent":"review","task":"Review"}),
        json!({"task":"Review","review":{"target":{"type":"uncommitted"}}}),
        json!({"agent":"review","task":"Review","review":{"target":{"type":"commit","commit":"--help"}}}),
    ] {
        assert!(
            serde_json::from_value::<crate::subagent::DelegateTaskArgs>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    for path in [
        "../escape",
        "/absolute",
        ".git/config",
        "code/../../escape",
        "",
        "a\\b",
    ] {
        assert!(validate_path(path).is_err(), "{path}");
    }
    validate_path("src/code.rs").unwrap();
    let mut root = request(ReviewTarget::Uncommitted);
    root.paths = vec![".".into()];
    validate_request(&root).unwrap();
}
#[test]
fn reports_are_strict_and_malformed_text_never_becomes_a_clean_review() {
    let valid = report().to_string();
    assert!(parse_report(&valid).is_ok());
    assert!(parse_report(&format!("```json\n{valid}\n```")).is_ok());
    assert!(parse_report(&format!("Explanation before {valid}")).is_err());
    let mut value = report();
    value["assessment"] = "issues_found".into();
    assert!(parse_report(&value.to_string()).is_err());
    value = report();
    value["extra"] = "invented".into();
    assert!(parse_report(&value.to_string()).is_err());
}
#[tokio::test]
async fn uncommitted_capture_is_read_only_and_ignores_repository_command_configuration() {
    let (directory, environment) = fixture();
    let root = directory.path();
    let marker = root.join("MUST_NOT_EXIST");
    git(
        root,
        &[
            "config",
            "filter.evil.clean",
            &format!("touch {}", marker.display()),
        ],
    );
    git(
        root,
        &[
            "config",
            "diff.evil.command",
            &format!("touch {}", marker.display()),
        ],
    );
    git(
        root,
        &[
            "config",
            "core.fsmonitor",
            &format!("touch {}", marker.display()),
        ],
    );
    std::fs::write(root.join(".gitattributes"), "*.rs filter=evil diff=evil\n").unwrap();
    std::fs::write(root.join("code.rs"), "pub fn value() -> u32 { 0 }\n").unwrap();
    std::fs::write(root.join("new.rs"), "pub fn added() {}\n").unwrap();
    let repository = Repository::prepare(
        Arc::new(environment.read_only()),
        request(ReviewTarget::Uncommitted),
    )
    .await
    .unwrap();
    assert!(
        repository.captured.files.contains(&"code.rs".into())
            && repository.captured.untracked.contains(&"new.rs".into())
    );
    assert!(
        repository
            .captured
            .patch
            .contains("+pub fn value() -> u32 { 0 }")
    );
    assert!(!marker.exists());
    assert!(!repository.stale().await);
    std::fs::write(root.join("code.rs"), "pub fn value() -> u32 { 2 }\n").unwrap();
    assert!(repository.stale().await);
    assert!(!marker.exists());
}
#[tokio::test]
async fn immutable_commit_and_branch_targets_read_the_correct_version_including_root_commit() {
    let (directory, environment) = fixture();
    let root = directory.path();
    let initial = git(root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("code.rs"), "pub fn value() -> u32 { 2 }\n").unwrap();
    git(root, &["add", "code.rs"]);
    git(root, &["commit", "-qm", "change"]);
    let latest = git(root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("code.rs"), "WORKTREE_ONLY\n").unwrap();
    for target in [
        ReviewTarget::Commit {
            commit: latest.clone(),
        },
        ReviewTarget::BaseBranch {
            branch: initial.clone(),
        },
    ] {
        let repository = Repository::prepare(Arc::new(environment.read_only()), request(target))
            .await
            .unwrap();
        assert!(
            repository
                .read("code.rs", true)
                .await
                .unwrap()
                .content
                .contains("{ 2 }")
        );
        assert!(!repository.captured.patch.contains("WORKTREE_ONLY"));
        assert_eq!(repository.captured.base.as_deref(), Some(initial.as_str()));
    }
    let repository = Repository::prepare(
        Arc::new(environment.read_only()),
        request(ReviewTarget::Commit { commit: initial }),
    )
    .await
    .unwrap();
    assert!(repository.captured.base.is_none());
    assert!(
        repository
            .captured
            .patch
            .contains("+pub fn value() -> u32 { 1 }")
    );
    assert!(repository.captured.all_files.contains(&"code.rs".into()));
}
#[tokio::test]
async fn inspected_context_changes_and_symlink_escapes_are_not_hidden() {
    let (directory, environment) = fixture();
    let root = directory.path();
    std::fs::write(root.join("code.rs"), "pub fn value() -> u32 { 0 }\n").unwrap();
    let mut target = request(ReviewTarget::Uncommitted);
    target.paths = vec!["code.rs".into()];
    let repository = Repository::prepare(Arc::new(environment.read_only()), target)
        .await
        .unwrap();
    repository.read("README.md", true).await.unwrap();
    std::fs::write(root.join("README.md"), "Changed context\n").unwrap();
    assert!(repository.stale().await);
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "PRIVATE").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path().join("secret"), root.join("link")).unwrap();
        assert!(repository.read("link", true).await.is_err());
    }
    assert!(
        repository
            .validate_location(&ReviewLocation {
                path: "README.md".into(),
                start_line: 1,
                end_line: 1
            })
            .await
            .is_err()
    );
    repository
        .validate_location(&ReviewLocation {
            path: "code.rs".into(),
            start_line: 1,
            end_line: 1,
        })
        .await
        .unwrap();
}

#[test]
fn format_repair_cannot_upgrade_missing_evidence_to_a_clean_or_invented_report() {
    let mut repaired = parse_report(&report().to_string()).unwrap();
    validate_repair("No structured evidence.", &mut repaired).unwrap();
    assert_eq!(repaired.assessment, ReviewAssessment::Inconclusive);
    assert_eq!(repaired.coverage.status, ReviewCoverageStatus::Partial);
    let invented = json!({"assessment":"issues_found","summary":"Invented finding.","findings":[{"priority":"P1","title":"Invented title","explanation":"Invented evidence.","location":{"path":"code.rs","start_line":1,"end_line":1},"confidence":1.0}],"coverage":{"status":"complete","limitations":[]}});
    let mut invented = parse_report(&invented.to_string()).unwrap();
    assert!(validate_repair("No structured evidence.", &mut invented).is_err());
}
#[tokio::test]
async fn scoped_subdirectory_reviews_and_deleted_files_use_correct_relative_paths() {
    let (directory, environment) = fixture();
    let root = directory.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
    git(root, &["add", "src/lib.rs"]);
    git(root, &["commit", "-qm", "nested"]);
    std::fs::remove_file(root.join("src/lib.rs")).unwrap();
    // Bind a workspace below the Git root without expanding its authorized read ceiling.
    let workspace = root.join("src").canonicalize().unwrap();
    let nested = Arc::new(
        ExecutionEnvironment::new(
            &workspace,
            &workspace,
            environment.policy().clone(),
            std::env::current_exe().unwrap(),
            Arc::new(Worker),
        )
        .unwrap()
        .read_only(),
    );
    let repository = Repository::prepare(nested, request(ReviewTarget::Uncommitted))
        .await
        .unwrap();
    assert_eq!(repository.captured.files, vec!["lib.rs"]);
    let file = repository.read("lib.rs", true).await.unwrap();
    assert!(file.deleted && file.content.contains("pub fn lib"));
    repository
        .validate_location(&ReviewLocation {
            path: "lib.rs".into(),
            start_line: 1,
            end_line: 1,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn large_or_binary_files_are_omitted_explicitly_and_untracked_reads_are_accounted_for() {
    let (root, environment) = fixture();
    std::fs::write(root.path().join("large.bin"), vec![0; 600000]).unwrap();
    std::fs::write(root.path().join("new.rs"), "first line\nsecond line\n").unwrap();
    let repository = Repository::prepare(
        Arc::new(environment.read_only()),
        request(ReviewTarget::Uncommitted),
    )
    .await
    .unwrap();
    assert!(
        repository
            .captured
            .omitted_files
            .contains(&"large.bin".into())
    );
    assert!(repository.stale().await);
    assert!(repository.unread_untracked().contains(&"new.rs".into()));
    repository.note_visible("new.rs", 1, 1, 2);
    assert!(repository.unread_untracked().contains(&"new.rs".into()));
    repository.note_visible("new.rs", 2, 2, 2);
    assert!(!repository.unread_untracked().contains(&"new.rs".into()));
}

#[tokio::test]
async fn narrowing_changed_files_keeps_root_project_instructions_available() {
    let (root, environment) = fixture();
    std::fs::write(root.path().join("code.rs"), "pub fn value() -> u32 { 0 }\n").unwrap();
    std::fs::write(root.path().join("AGENTS.md"), "Preserve the public API.\n").unwrap();
    std::fs::write(
        root.path().join("CLAUDE.md"),
        "Follow project acceptance criteria.\n",
    )
    .unwrap();
    let mut target = request(ReviewTarget::Uncommitted);
    target.paths = vec!["code.rs".into()];
    let repository = Repository::prepare(Arc::new(environment.read_only()), target)
        .await
        .unwrap();
    assert_eq!(repository.captured.files, vec!["code.rs"]);
    let instructions = repository.instructions().await;
    assert_eq!(instructions.len(), 2);
    assert!(
        instructions[0]["content"]
            .as_str()
            .unwrap()
            .contains("public API")
    );
    assert!(!repository.stale().await);
    std::fs::write(
        root.path().join("AGENTS.md"),
        "The project API contract changed.\n",
    )
    .unwrap();
    assert!(repository.stale().await);
}
