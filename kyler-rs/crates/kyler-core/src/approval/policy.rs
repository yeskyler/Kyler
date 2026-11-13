//! Execution rules precede approval policy; neither can override sandbox validation.
use super::{operation::ToolOperation, review::review_in_environment};
use crate::{
    environment::ExecutionEnvironment,
    sandbox::SandboxMode,
    tools::{Tool, ToolSource},
};

use serde_json::Value;

use kyler_protocol::{
    api::permissions::{ApprovalStatus, PermissionMode},
    approval::{ApprovalPolicy, ApprovalRequestKind, ApprovalReviewer, ApprovalSettings, ToolRule},
};

pub(crate) fn decide(
    settings: &ApprovalSettings,
    operation: ToolOperation,
    arguments: &Value,
    tool: &dyn Tool,
    environment: Option<&ExecutionEnvironment>,
    requested: bool,
) -> ApprovalStatus {
    if settings.validation_error().is_some() {
        return ApprovalStatus::Denied;
    }
    let legacy = match tool.name() {
        "exec_command" | "shell" => "run_command",
        "write_stdin" => match operation {
            ToolOperation::Read => "read_process",
            ToolOperation::State => "stop_process",
            _ => "write_process",
        },
        _ => tool.name(),
    };
    let rule = settings
        .rules
        .get(tool.name())
        .or_else(|| settings.rules.get(legacy));
    match rule {
        Some(ToolRule::Deny) => return ApprovalStatus::Denied,
        Some(ToolRule::Allow) if !requested => return ApprovalStatus::Allowed,
        _ => {}
    }
    let ask = requested || rule == Some(&ToolRule::Ask);
    if ask {
        return if settings.rejects(ApprovalRequestKind::ToolRule) {
            ApprovalStatus::Denied
        } else {
            ApprovalStatus::Pending
        };
    }
    if settings.policy == ApprovalPolicy::Never {
        return ApprovalStatus::Allowed;
    }
    // On-request trusts known worker operations inside the effective sandbox.
    // Host mutations and opaque remote operations still need an explicit decision.
    if settings.policy == ApprovalPolicy::OnRequest
        && settings.reviewer == ApprovalReviewer::User
        && tool.source() == ToolSource::Builtin
        && tool.execution_mode().sandboxed()
        && environment.is_some_and(|environment| {
            environment.policy().mode != SandboxMode::ReadOnly
                && match operation {
                    ToolOperation::Write => tool.name() == "apply_patch",
                    ToolOperation::Command => environment.policy().process.allow_commands,
                    _ => false,
                }
        })
    {
        return ApprovalStatus::Allowed;
    }
    let mut status = if matches!(operation, ToolOperation::Read | ToolOperation::State)
        || (operation == ToolOperation::Command && safe_read_command(arguments))
    {
        ApprovalStatus::Allowed
    } else if settings.policy != ApprovalPolicy::Untrusted
        && settings.reviewer == ApprovalReviewer::AutoReview
    {
        review_in_environment(
            PermissionMode::AutoReview,
            operation,
            arguments,
            tool,
            environment,
        )
    } else {
        ApprovalStatus::Pending
    };
    if status == ApprovalStatus::Pending && settings.rejects(ApprovalRequestKind::ToolRule) {
        status = ApprovalStatus::Denied;
    }
    status
}

fn safe_read_command(args: &Value) -> bool {
    if args["command"].is_array() {
        return serde_json::from_value::<crate::tools::shell::ShellArgs>(args.clone())
            .ok()
            .and_then(|args| args.into_command().ok())
            .and_then(|args| serde_json::to_value(args).ok())
            .is_some_and(|args| safe_read_command(&args));
    }

    if args
        .get("working_directory")
        .is_some_and(|value| !value.is_null())
        || args.get("environment").is_some_and(|value| {
            !value.is_null() && value.as_object().is_none_or(|values| !values.is_empty())
        })
    {
        return false;
    }
    matches!(
        args["program"].as_str(),
        Some(
            "/bin/pwd"
                | "/usr/bin/pwd"
                | "/usr/bin/whoami"
                | "/bin/ls"
                | "/bin/cat"
                | "/usr/bin/stat"
                | "/usr/bin/head"
                | "/usr/bin/tail"
        )
    ) && args.get("args").is_none_or(|args| {
        args.as_array()
            .is_some_and(|args| args.iter().all(Value::is_string))
    })
}

/// Metadata belongs to Kyler, never to a tool's domain arguments or a remote connector.
pub(crate) fn arguments(value: &Value) -> anyhow::Result<(Value, bool)> {
    anyhow::ensure!(value.is_object(), "tool arguments must be a JSON object");
    let mut value = value.clone();
    let metadata = value
        .as_object_mut()
        .and_then(|object| object.remove("__kyler_approval"));
    let requested = if let Some(metadata) = metadata {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Intent {
            request: bool,
            #[serde(default)]
            reason: String,
        }
        let intent: Intent = serde_json::from_value(metadata)?;
        anyhow::ensure!(
            intent.reason.len() <= 1024,
            "approval reason exceeds 1024 bytes"
        );
        intent.request
    } else {
        false
    };
    Ok((value, requested))
}

pub(crate) fn advertise(parameters: &mut Value) -> anyhow::Result<()> {
    if let Some(properties) = parameters
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        anyhow::ensure!(
            !properties.contains_key("__kyler_approval"),
            "tool uses a reserved approval parameter"
        );
        properties.insert("__kyler_approval".into(), serde_json::json!({"type":"object","description":"Request approval before this call when needed, including a one-call exception to the selected access preset. This never grants access outside the host-authorized boundary.","properties":{"request":{"type":"boolean"},"reason":{"type":"string"}},"required":["request"],"additionalProperties":false}));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{super::ToolOperation, arguments, decide};
    use crate::tools::{apply_patch::ApplyPatchTool, shell::RunCommandTool};

    use serde_json::json;

    use kyler_protocol::{
        api::permissions::ApprovalStatus,
        approval::{ApprovalPolicy, ApprovalSettings, ToolRule},
    };

    #[test]
    fn renamed_process_tools_preserve_legacy_deny_rules() {
        use crate::tools::exec_command::{ExecCommandTool, WriteStdinTool};
        let mut settings = ApprovalSettings::default();
        settings.policy = ApprovalPolicy::Never;
        settings.rules.insert("run_command".into(), ToolRule::Deny);
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Command,
                &serde_json::json!({"command":"echo x"}),
                &ExecCommandTool,
                None,
                false
            ),
            ApprovalStatus::Denied
        );
        settings
            .rules
            .insert("write_process".into(), ToolRule::Deny);
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Command,
                &serde_json::json!({"session_id":"p","chars":"x"}),
                &WriteStdinTool,
                None,
                false
            ),
            ApprovalStatus::Denied
        );
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Read,
                &serde_json::json!({"session_id":"p"}),
                &WriteStdinTool,
                None,
                false
            ),
            ApprovalStatus::Allowed
        );
        settings.rules.insert("read_process".into(), ToolRule::Deny);
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Read,
                &serde_json::json!({"session_id":"p"}),
                &WriteStdinTool,
                None,
                false
            ),
            ApprovalStatus::Denied
        );
    }

    #[test]
    fn policies_and_tool_rules_never_confuse_prompting_with_access() {
        let mut settings = ApprovalSettings::default();
        settings.policy = ApprovalPolicy::Untrusted;
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Command,
                &json!({"program":"/bin/ls","args":["-la"]}),
                &RunCommandTool,
                None,
                false
            ),
            ApprovalStatus::Allowed
        );
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Command,
                &json!({"program":"/bin/sh","args":["-c","ls"]}),
                &RunCommandTool,
                None,
                false
            ),
            ApprovalStatus::Pending
        );
        settings.policy = ApprovalPolicy::Never;
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Write,
                &json!({}),
                &ApplyPatchTool,
                None,
                false
            ),
            ApprovalStatus::Allowed
        );
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Write,
                &json!({}),
                &ApplyPatchTool,
                None,
                true
            ),
            ApprovalStatus::Denied
        );
        settings.rules.insert("apply_patch".into(), ToolRule::Ask);
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Write,
                &json!({}),
                &ApplyPatchTool,
                None,
                false
            ),
            ApprovalStatus::Denied
        );
        settings.policy = ApprovalPolicy::OnRequest;
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Write,
                &json!({}),
                &ApplyPatchTool,
                None,
                false
            ),
            ApprovalStatus::Pending
        );
        settings.rules.insert("apply_patch".into(), ToolRule::Deny);
        assert_eq!(
            decide(
                &settings,
                ToolOperation::Read,
                &json!({}),
                &ApplyPatchTool,
                None,
                false
            ),
            ApprovalStatus::Denied
        );
        settings.policy = ApprovalPolicy::Reject;
        settings.rules.clear();
        assert_eq!(
            decide(
                &settings,
                ToolOperation::External,
                &json!({}),
                &ApplyPatchTool,
                None,
                false
            ),
            ApprovalStatus::Denied
        );
        settings.reject.tool_rules = false;
        assert_eq!(
            decide(
                &settings,
                ToolOperation::External,
                &json!({}),
                &ApplyPatchTool,
                None,
                false
            ),
            ApprovalStatus::Pending
        );
        settings.policy = ApprovalPolicy::OnFailure;
        assert!(settings.validation_error().is_some());
    }
    #[test]
    fn custom_command_context_does_not_use_the_safe_read_shortcut() {
        let mut settings = ApprovalSettings::default();
        settings.policy = ApprovalPolicy::Untrusted;
        for args in [
            json!({"program":"/bin/pwd", "working_directory":"other-project"}),
            json!({"program":"/bin/ls", "environment":{"PATH":"/project/bin:/bin"}}),
        ] {
            assert_eq!(
                decide(
                    &settings,
                    ToolOperation::Command,
                    &args,
                    &RunCommandTool,
                    None,
                    false
                ),
                ApprovalStatus::Pending
            );
        }
    }
    #[test]
    fn approval_annotations_are_validated_and_removed_from_domain_arguments() {
        let (args, requested) = arguments(&json!({"program":"/bin/ls","__kyler_approval":{"request":true,"reason":"Review this call"}})).unwrap();
        assert!(requested);
        assert_eq!(args, json!({"program":"/bin/ls"}));
        assert!(arguments(&json!({"__kyler_approval":{"request":"yes"}})).is_err());
    }
}
