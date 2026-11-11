use super::{
    events::{Entry, prefix},
    manager::{RunManager, public_error},
    persistence,
};
use crate::error::ApiResult;

use kyler_protocol::api::{
    ApiErrorBody,
    event::EventPayload,
    run::{RunInfo, RunStatus},
};

impl RunManager {
    pub(crate) async fn finish(
        &self,
        entry: &Entry,
        outcome: Option<RunInfo>,
        error: Option<ApiErrorBody>,
    ) {
        let mut info = entry.info();
        info.finished_at = Some(chrono::Utc::now().timestamp_millis());
        if let Some(error) = error {
            info.status = RunStatus::Failed;
            info.error = Some(error);
        } else if let Some(outcome) = outcome {
            info.status = outcome.status;
            info.output = outcome.output;
            info.usage = outcome.usage;
            info.error = outcome.error;
            info.execution_id = outcome.execution_id;
        } else {
            info.status = RunStatus::Failed;
            info.error = Some(public_error("execution_failed", "The execution failed."));
        }
        if info
            .output
            .as_ref()
            .is_some_and(|text| !text.trim().is_empty())
            && self.publish_session(&info.session_id).await.is_err()
        {
            info.status = RunStatus::Failed;
            info.error = Some(public_error(
                "storage_error",
                "The conversation could not be saved.",
            ));
        }
        info.last_event_seq += 1;
        if persistence::save(self.root.clone(), info.clone())
            .await
            .is_err()
        {
            info.status = RunStatus::Failed;
            info.output = None;
            info.error = Some(public_error(
                "storage_error",
                "The final run record could not be persisted.",
            ));
            tracing::error!(run_id=%info.run_id,"final run manifest write failed");
        }
        let payload = match info.status {
            RunStatus::Completed => {
                let output = info.output.as_deref().unwrap_or_default();
                let preview = prefix(output, self.settings.event_buffer_max_bytes / 8);
                EventPayload::RunCompleted {
                    output_truncated: preview.len() != output.len(),
                    output: preview,
                    usage: info.usage.clone().unwrap_or_default(),
                }
            }
            RunStatus::Cancelled => EventPayload::RunCancelled,
            _ => EventPayload::RunFailed {
                error: info
                    .error
                    .clone()
                    .unwrap_or_else(|| public_error("execution_failed", "The execution failed.")),
            },
        };
        entry.complete(info, payload);
    }
    pub async fn cancel(&self, id: &str) -> ApiResult<RunInfo> {
        let entry = self.entry(id).await?;
        {
            let mut data = entry.inner.lock().unwrap_or_else(|e| e.into_inner());
            if !data.info.status.is_terminal() {
                data.info.status = RunStatus::Cancelling;
                entry.cancel.send_replace(true);
            }
        }
        Ok(entry.info())
    }
}

#[cfg(test)]
use kyler_core::thread::projection::tool_summary;

#[cfg(test)]
mod presentation_tests {
    use super::tool_summary;

    #[test]
    fn display_summaries_are_bounded_and_do_not_copy_arbitrary_arguments() {
        use kyler_protocol::activity::ToolSource;

        let args = serde_json::json!({"program":"cargo","args":["secret"],"api_key":"secret"});
        assert_eq!(
            tool_summary("run_command", &args, &ToolSource::Builtin).as_deref(),
            Some("cargo")
        );
        assert!(tool_summary("remote", &args, &ToolSource::Builtin).is_none());
        let source = ToolSource::Mcp {
            server_id: "remote".into(),
            remote_name: "read_file".into(),
        };
        assert!(
            tool_summary(
                "read_file",
                &serde_json::json!({"file_path":"secret"}),
                &source
            )
            .is_none()
        );
        let summary = tool_summary(
            "read_file",
            &serde_json::json!({"file_path":"a".repeat(500)}),
            &ToolSource::Builtin,
        )
        .unwrap();
        assert!(summary.len() <= 160);
    }
}
