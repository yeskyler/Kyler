use super::operation::ToolOperation;
use crate::tools::{ToolSource, command::RunCommandArgs};

use serde_json::Value;

use kyler_protocol::api::permissions::{ApprovalStatus, PermissionMode};

pub fn review(mode: PermissionMode, operation: ToolOperation, arguments: &Value) -> ApprovalStatus {
    if matches!(operation, ToolOperation::Read | ToolOperation::State) {
        return ApprovalStatus::Allowed;
    }
    if mode == PermissionMode::ReadOnly {
        return ApprovalStatus::Denied;
    }
    if mode == PermissionMode::FullAccess {
        return ApprovalStatus::Allowed;
    }
    if mode == PermissionMode::AutoReview {
        if operation == ToolOperation::Network {
            return ApprovalStatus::Allowed;
        }
        if operation == ToolOperation::Command
            && arguments
                .get("working_directory")
                .is_none_or(Value::is_null)
            && arguments.get("environment").is_none_or(|value| {
                value.is_null() || value.as_object().is_some_and(|values| values.is_empty())
            })
            && matches!(
                arguments["program"].as_str(),
                Some("/usr/bin/pwd" | "/bin/pwd" | "/usr/bin/whoami")
            )
            && arguments
                .get("args")
                .is_none_or(|a| a.as_array().is_some_and(|a| a.is_empty()))
        {
            return ApprovalStatus::Allowed;
        }
    }
    ApprovalStatus::Pending
}

/// Automatic writes require the real builtin worker and a host-authorized workspace.
pub(crate) fn review_in_environment(
    mode: PermissionMode,
    operation: ToolOperation,
    arguments: &Value,
    tool: &dyn crate::tools::Tool,
    environment: Option<&crate::environment::ExecutionEnvironment>,
) -> ApprovalStatus {
    let status = review(mode, operation, arguments);
    if mode != PermissionMode::AutoReview || status != ApprovalStatus::Pending {
        return status;
    }
    let Some(environment) = environment else {
        return status;
    };
    if !tool.execution_mode().sandboxed() || tool.source() != ToolSource::Builtin {
        return status;
    }
    let allowed = match (tool.name(), operation) {
        ("apply_patch", ToolOperation::Write) => arguments["input"]
            .as_str()
            .and_then(|input| {
                crate::tools::apply_patch::automatic_patch_targets(
                    &environment.session_environment().workspace_root,
                    input,
                )
                .ok()
                .flatten()
            })
            .is_some_and(|targets| {
                !targets.is_empty()
                    && targets.iter().all(|path| {
                        ordinary_writable_path(
                            environment,
                            path,
                            &environment.session_environment().workspace_root,
                        )
                    })
            }),
        ("run_command", ToolOperation::Command) => ordinary_command(environment, arguments),
        _ => false,
    };
    if allowed {
        ApprovalStatus::Allowed
    } else {
        status
    }
}

fn sensitive_path(path: &std::path::Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        matches!(
            name.as_str(),
            ".git"
                | ".hg"
                | ".svn"
                | ".github"
                | ".gitlab"
                | ".kyler"
                | ".ssh"
                | ".aws"
                | ".gnupg"
                | ".config"
                | ".secrets"
                | ".envrc"
                | ".direnv"
                | ".zed"
                | ".vscode"
                | ".idea"
                | ".codex"
                | ".agents"
                | ".claude"
                | ".cursor"
                | "agents.md"
                | "claude.md"
                | ".npmrc"
                | ".pypirc"
                | ".netrc"
                | ".zshrc"
                | ".zshenv"
                | ".zprofile"
                | ".bashrc"
                | ".bash_profile"
                | ".profile"
                | "credentials"
                | "secrets"
                | "id_rsa"
                | "id_ed25519"
        ) || name == ".env"
            || name.starts_with(".env.")
            || name.starts_with("credentials.")
            || name.starts_with("secrets.")
            || [".pem", ".key", ".p12", ".pfx", ".keystore", ".kdbx"]
                .iter()
                .any(|extension| name.ends_with(extension))
    })
}

fn ordinary_writable_path(
    environment: &crate::environment::ExecutionEnvironment,
    path: &std::path::Path,
    base: &std::path::Path,
) -> bool {
    let workspace = &environment.session_environment().workspace_root;
    let logical = if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    };
    let Ok(relative) = logical.strip_prefix(workspace) else {
        return false;
    };
    if sensitive_path(relative)
        || path
            .as_os_str()
            .to_string_lossy()
            .chars()
            .any(char::is_control)
    {
        return false;
    }
    let Ok(resolved) = environment.policy().filesystem.check_write(base, path) else {
        return false;
    };
    let Ok(relative) = resolved.strip_prefix(workspace) else {
        return false;
    };
    if sensitive_path(relative) {
        return false;
    }
    if let Ok(metadata) = std::fs::metadata(&resolved) {
        if !metadata.is_file() && !metadata.is_dir() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            if metadata.is_file() && metadata.nlink() > 1 {
                return false;
            }
        }
    }
    true
}

fn trusted_program(
    environment: &crate::environment::ExecutionEnvironment,
    program: &str,
    name: &str,
) -> bool {
    let path = std::path::Path::new(program);
    let candidate = if program == name {
        std::env::split_paths(&environment.policy().executable_search_path)
            .map(|directory| {
                let directory = if directory.is_absolute() {
                    directory
                } else {
                    environment
                        .session_environment()
                        .working_directory
                        .join(directory)
                };
                directory.join(name)
            })
            .find(|candidate| candidate.is_file())
    } else if path.is_absolute() {
        Some(path.to_owned())
    } else {
        None
    };
    let Some(candidate) = candidate.and_then(|path| path.canonicalize().ok()) else {
        return false;
    };
    [format!("/bin/{name}"), format!("/usr/bin/{name}")]
        .iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .any(|trusted| trusted == candidate)
}

fn ordinary_command(
    environment: &crate::environment::ExecutionEnvironment,
    arguments: &Value,
) -> bool {
    if !environment.policy().process.allow_commands {
        return false;
    }
    let Ok(command) = serde_json::from_value::<RunCommandArgs>(arguments.clone()) else {
        return false;
    };
    if !command.environment.is_empty() || command.working_directory.is_some() {
        return false;
    }
    let Some(name) = ["mkdir", "touch"]
        .into_iter()
        .find(|name| trusted_program(environment, &command.program, name))
    else {
        return false;
    };
    if command.args.is_empty() || command.args.len() > 64 {
        return false;
    }
    let mut options = true;
    let mut paths = 0;
    for argument in &command.args {
        if options && argument == "--" {
            options = false;
            continue;
        }
        if options && name == "mkdir" && matches!(argument.as_str(), "-p" | "--parents") {
            continue;
        }
        if options && argument.starts_with('-') {
            return false;
        }
        let path = std::path::Path::new(argument);
        if !ordinary_writable_path(
            environment,
            path,
            &environment.session_environment().working_directory,
        ) {
            return false;
        }
        let target = if path.is_absolute() {
            path.to_owned()
        } else {
            environment
                .session_environment()
                .working_directory
                .join(path)
        };
        if name == "touch" && target.is_dir() {
            return false;
        }
        paths += 1;
    }
    paths > 0 && paths <= 32
}

#[cfg(test)]
mod tests {
    use super::{super::ToolOperation, ordinary_writable_path, review_in_environment};
    use crate::{
        ExecutionEnvironment,
        sandbox::{Sandbox, SandboxPolicy, SandboxRequest, WorkerResponse},
        tools::{Tool, apply_patch::ApplyPatchTool, command::RunCommandTool},
    };

    use std::{path::Path, sync::Arc};

    use serde_json::{Value, json};

    use kyler_protocol::api::permissions::{ApprovalStatus, PermissionMode};

    struct Unused;
    #[async_trait::async_trait]
    impl Sandbox for Unused {
        async fn run(&self, _: SandboxRequest) -> anyhow::Result<WorkerResponse> {
            unreachable!("review must not execute tools")
        }
    }
    fn environment() -> (tempfile::TempDir, ExecutionEnvironment) {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join("nested")).unwrap();
        std::fs::write(workspace.join("existing.txt"), "old\n").unwrap();
        let environment = ExecutionEnvironment::new(
            &workspace,
            workspace.join("nested"),
            SandboxPolicy::workspace_write("review", &workspace),
            std::env::current_exe().unwrap(),
            Arc::new(Unused),
        )
        .unwrap();
        (root, environment)
    }
    fn assess(
        environment: &ExecutionEnvironment,
        tool: &dyn Tool,
        arguments: Value,
    ) -> ApprovalStatus {
        review_in_environment(
            PermissionMode::AutoReview,
            tool.approval_requirement(&arguments).unwrap(),
            &arguments,
            tool,
            Some(environment),
        )
    }
    #[test]
    fn automatically_allows_direct_workspace_directory_and_file_creation() {
        let (_root, environment) = environment();
        for arguments in [
            json!({"program":"mkdir","args":["-p","assets/images","assets/icons"]}),
            json!({"program":"touch","args":["notes.txt"]}),
        ] {
            assert_eq!(
                assess(&environment, &RunCommandTool, arguments),
                ApprovalStatus::Allowed
            );
        }
        assert!(
            !environment
                .session_environment()
                .working_directory
                .join("assets")
                .exists()
        );
    }
    #[test]
    fn automatically_allows_bounded_new_files_and_normal_edits() {
        let (_root, environment) = environment();
        for input in [
            "*** Begin Patch\n*** Add File: src/new.rs\n+fn main() {}\n*** End Patch",
            "*** Begin Patch\n*** Update File: existing.txt\n@@\n-old\n+new\n*** End Patch",
        ] {
            assert_eq!(
                assess(&environment, &ApplyPatchTool, json!({"input":input})),
                ApprovalStatus::Allowed
            );
        }
        assert_eq!(
            std::fs::read_to_string(
                environment
                    .session_environment()
                    .workspace_root
                    .join("existing.txt")
            )
            .unwrap(),
            "old\n"
        );
    }
    #[test]
    fn asks_for_destructive_sensitive_large_and_unknown_operations() {
        let (_root, environment) = environment();
        for input in [
            "*** Begin Patch\n*** Delete File: existing.txt\n*** End Patch",
            "*** Begin Patch\n*** Add File: existing.txt\n+replacement\n*** End Patch",
            "*** Begin Patch\n*** Update File: existing.txt\n*** Move to: moved.txt\n@@\n-old\n+new\n*** End Patch",
        ] {
            assert_eq!(
                assess(&environment, &ApplyPatchTool, json!({"input":input})),
                ApprovalStatus::Pending
            );
        }
        for path in [
            ".env",
            ".envrc",
            ".zed/settings.json",
            ".git/config",
            ".github/workflows/build.yml",
            "AGENTS.md",
            "secrets.json",
            "credentials.json",
            "id_rsa",
            "private.key",
            ".zshrc",
        ] {
            assert_eq!(
                assess(
                    &environment,
                    &ApplyPatchTool,
                    json!({"input":format!("*** Begin Patch\n*** Add File: {path}\n+data\n*** End Patch")})
                ),
                ApprovalStatus::Pending,
                "{path}"
            );
        }
        for arguments in [
            json!({"program":"/bin/sh","args":["-c","mkdir -p assets"]}),
            json!({"program":"mkdir","args":["-m","777","assets"]}),
            json!({"program":"rm","args":["existing.txt"]}),
            json!({"program":"mkdir","args":["../traversal"]}),
            json!({"program":"mkdir","args":["/outside-review-workspace"]}),
            json!({"program":"mkdir","args":[".git"]}),
        ] {
            assert_eq!(
                assess(&environment, &RunCommandTool, arguments),
                ApprovalStatus::Pending
            );
        }
        let large = format!(
            "*** Begin Patch\n*** Add File: big.txt\n+{}\n*** End Patch",
            "a".repeat(128 * 1024)
        );
        assert_eq!(
            assess(&environment, &ApplyPatchTool, json!({"input":large})),
            ApprovalStatus::Pending
        );
        let many = format!(
            "*** Begin Patch\n{}*** End Patch",
            (0..21)
                .map(|i| format!("*** Add File: file-{i}\n+x\n"))
                .collect::<String>()
        );
        assert_eq!(
            assess(&environment, &ApplyPatchTool, json!({"input":many})),
            ApprovalStatus::Pending
        );
    }
    #[cfg(unix)]
    #[test]
    fn links_shadowed_programs_and_sandbox_denials_do_not_get_auto_approval() {
        let (_root, environment) = environment();
        let workspace = &environment.session_environment().workspace_root;
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.join("external")).unwrap();
        std::fs::write(workspace.join(".env"), "placeholder").unwrap();
        std::os::unix::fs::symlink(workspace.join(".env"), workspace.join("alias.txt")).unwrap();
        std::fs::hard_link(
            workspace.join("existing.txt"),
            workspace.join("hardlink.txt"),
        )
        .unwrap();
        for path in ["external/new.txt", "alias.txt", "hardlink.txt"] {
            assert!(
                !ordinary_writable_path(&environment, Path::new(path), workspace),
                "{path}"
            );
        }
        std::fs::create_dir(workspace.join("bin")).unwrap();
        std::fs::write(workspace.join("bin/mkdir"), "not a system executable").unwrap();
        let mut policy = environment.policy().clone();
        policy.executable_search_path =
            format!("{}:/bin:/usr/bin", workspace.join("bin").display());
        let shadowed = ExecutionEnvironment::new(
            workspace,
            workspace,
            policy,
            std::env::current_exe().unwrap(),
            Arc::new(Unused),
        )
        .unwrap();
        assert_eq!(
            assess(
                &shadowed,
                &RunCommandTool,
                json!({"program":"mkdir","args":["new"]})
            ),
            ApprovalStatus::Pending
        );
        let protected = environment
            .with_protected_paths(&[workspace.join("private")])
            .unwrap();
        assert_eq!(
            assess(
                &protected,
                &ApplyPatchTool,
                json!({"input":"*** Begin Patch\n*** Add File: private/new.txt\n+x\n*** End Patch"})
            ),
            ApprovalStatus::Pending
        );
    }
    #[test]
    fn other_modes_and_missing_environments_keep_their_existing_boundaries() {
        let (_root, environment) = environment();
        let args = json!({"program":"mkdir","args":["assets"]});
        assert_eq!(
            review_in_environment(
                PermissionMode::ReadOnly,
                ToolOperation::Command,
                &args,
                &RunCommandTool,
                Some(&environment)
            ),
            ApprovalStatus::Denied
        );
        assert_eq!(
            review_in_environment(
                PermissionMode::WorkspaceAccess,
                ToolOperation::Command,
                &args,
                &RunCommandTool,
                Some(&environment)
            ),
            ApprovalStatus::Pending
        );
        assert_eq!(
            review_in_environment(
                PermissionMode::AutoReview,
                ToolOperation::Command,
                &args,
                &RunCommandTool,
                None
            ),
            ApprovalStatus::Pending
        );
    }
}
