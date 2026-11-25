use super::Channel;
use crate::runs::RunManager;

use anyhow::{Result, ensure};

use kyler_channel::{Adapter, Job, OutgoingFile, Reply};
use kyler_protocol::api::{attachments::MAX_ATTACHMENTS, gateway::Principal, run::RunInfo};

impl Channel {
    pub(super) async fn prepare_attachments(
        &self,
        manager: &RunManager,
        job: &Job,
    ) -> Result<Vec<String>> {
        ensure!(
            job.attachments.len() <= MAX_ATTACHMENTS,
            "Too many channel attachments"
        );
        if job.attachments.is_empty() {
            return Ok(Vec::new());
        }
        ensure!(
            matches!(self.bot, Adapter::QqBot(_)),
            "Attachments are unsupported on this channel"
        );
        // Admission will recheck the same principal/session before launching the model.
        let principal = super::principal(manager, &job.binding)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Channel credential is inactive"))?;
        let session = manager.store.load(&job.binding.session).await?;
        let workspace = session.environment.as_ref().map(|e| e.cwd.as_path());
        ensure!(
            manager
                .security
                .session_allowed(&principal, &job.binding.session, workspace)
                .await?,
            "Channel session is inaccessible"
        );
        let mut ids = job.attachment_ids.clone();
        ensure!(
            ids.len() <= job.attachments.len(),
            "Invalid attachment checkpoint"
        );
        for file in &job.attachments[ids.len()..] {
            let bytes = kyler_channel::download_qq_attachment(file).await?;
            let store = manager.attachments.clone();
            let session = job.binding.session.clone();
            let name = attachment_name(file, &bytes);
            let info =
                tokio::task::spawn_blocking(move || store.put(&session, &name, &bytes)).await??;
            ids.push(info.id);
            self.store.save_attachment_ids(job.id, ids.clone()).await?;
        }
        Ok(ids)
    }

    pub(super) async fn registered_files(
        &self,
        manager: &RunManager,
        info: &RunInfo,
    ) -> Result<Vec<OutgoingFile>> {
        let Some(execution_id) = info.execution_id.as_deref() else {
            return Ok(Vec::new());
        };
        let session = manager.store.load(&info.session_id).await?;
        let registered = session
            .execution
            .iter()
            .chain(session.executions.iter())
            .find(|e| e.execution_id == execution_id)
            .map(|e| e.deliverables.clone())
            .unwrap_or_default();
        let store = manager.artifacts.clone();
        let session_id = info.session_id.clone();
        let artifacts = tokio::task::spawn_blocking(move || store.list(&session_id)).await??;
        Ok(artifacts
            .into_iter()
            .filter(|file| {
                !file.resource
                    && file.execution_id == execution_id
                    && registered.iter().any(|r| r.path == file.reference)
            })
            .map(|file| OutgoingFile {
                session: info.session_id.clone(),
                artifact: file.id,
            })
            .collect())
    }

    pub(super) async fn deliver_file(
        &self,
        manager: &RunManager,
        reply: &Reply,
    ) -> Result<(), Option<u16>> {
        let Adapter::QqBot(bot) = &self.bot else {
            return Err(Some(400));
        };
        let file = reply.file.as_ref().ok_or(Some(400))?;
        let token = reply.token.as_ref().ok_or(Some(403))?;
        let record = manager
            .security
            .store
            .token(token.clone())
            .await
            .map_err(|_| None)?
            .ok_or(Some(403))?;
        let principal = Principal {
            id: record.info.principal_id,
            token_id: Some(record.info.id),
            role: record.info.role,
            scope: record.info.scope,
        };
        let session = manager
            .store
            .load(&file.session)
            .await
            .map_err(|_| Some(404))?;
        let workspace = session.environment.as_ref().map(|e| e.cwd.as_path());
        if !manager
            .security
            .session_allowed(&principal, &file.session, workspace)
            .await
            .map_err(|_| None)?
        {
            return Err(Some(403));
        }
        if !reply
            .receipt
            .as_ref()
            .is_some_and(|r| r.expires > chrono::Utc::now().timestamp())
        {
            return Err(Some(403));
        }
        let media = match &reply.media {
            Some(media) => media.clone(),
            None => {
                let store = manager.artifacts.clone();
                let file = file.clone();
                let (info, bytes) = tokio::task::spawn_blocking(move || {
                    store.download(&file.session, &file.artifact)
                })
                .await
                .map_err(|_| None)?
                .map_err(|_| Some(404))?;
                let media = bot
                    .upload_file(&reply.chat, &info.name, &info.mime_type, &bytes)
                    .await?;
                self.store
                    .save_media(reply.id, media.file_info.clone(), media.expires_at)
                    .await
                    .map_err(|_| None)?;
                media.file_info
            }
        };
        bot.send_file(&reply.chat, &media, reply.receipt.as_ref())
            .await
    }
}

fn attachment_name(file: &kyler_channel::IncomingAttachment, bytes: &[u8]) -> String {
    // CDN image names may be absent or have no extension; the store verifies bytes.
    let raw = file.name.rsplit(['/', '\\']).next().unwrap_or("attachment");
    let mut name = String::new();
    for ch in raw.chars().filter(|c| !c.is_control()) {
        if name.len() + ch.len_utf8() > 180 {
            break;
        }
        name.push(ch);
    }
    let name = if name.is_empty() || name == "." || name == ".." {
        "attachment".into()
    } else {
        name
    };
    if std::path::Path::new(&name).extension().is_none() {
        let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "png"
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            "jpg"
        } else if bytes.starts_with(b"%PDF-") {
            "pdf"
        } else {
            return name;
        };
        format!("{name}.{extension}")
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn only_registered_current_execution_artifacts_are_queued() {
        use kyler_core::{
            Agent, ExecutionEnvironment, SandboxPolicy, Session, SessionStore, ToolBox,
            artifacts::Deliverable, sandbox::direct::DirectSandbox,
        };
        use kyler_protocol::api::run::RunStatus;
        use std::sync::Arc;
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let environment = Arc::new(
            ExecutionEnvironment::new(
                &workspace,
                &workspace,
                SandboxPolicy::workspace_write("fixture", &workspace),
                std::env::current_exe().unwrap(),
                Arc::new(DirectSandbox),
            )
            .unwrap(),
        );
        let agent = Agent::new(
            "http://127.0.0.1:9/v1".into(),
            "TEST_ONLY_KEY".into(),
            "fixture",
            None::<String>,
            Arc::new(ToolBox::default()),
        )
        .with_environment(environment.clone());
        let sessions = Arc::new(
            SessionStore::new(root.path().join("sessions"))
                .await
                .unwrap(),
        );
        let server =
            crate::Server::with_agent(Default::default(), Arc::new(agent), sessions.clone())
                .await
                .unwrap();
        let mut session = Session::new();
        session.begin_execution("fixture").unwrap();
        let execution = session.execution.as_mut().unwrap();
        execution.deliverables = vec![Deliverable {
            path: "report.txt".into(),
            name: "report.txt".into(),
            primary: true,
        }];
        let execution_id = execution.execution_id.clone();
        std::fs::write(workspace.join("report.txt"), "final report").unwrap();
        std::fs::write(workspace.join("intermediate.txt"), "not a deliverable").unwrap();
        let artifacts = &server.state.runs.artifacts;
        artifacts
            .capture(
                &session.session_id,
                &execution_id,
                "[temporary](intermediate.txt)",
                Some(&environment),
            )
            .unwrap();
        artifacts
            .capture_registered(
                &session.session_id,
                &execution_id,
                "",
                Some(&environment),
                &execution.deliverables,
            )
            .unwrap();
        sessions.save(&mut session).await.unwrap();
        let mut values = std::collections::BTreeMap::new();
        values.insert("KYLER_QQBOT_APP_ID".into(), "123456".into());
        values.insert("KYLER_QQBOT_APP_SECRET".into(), "TEST_ONLY_SECRET".into());
        let channel = Channel {
            bot: Adapter::from_settings(
                kyler_channel::CHANNELS
                    .iter()
                    .find(|c| c.id == "qqbot")
                    .unwrap(),
                &kyler_channel::Settings(&values),
            )
            .unwrap(),
            store: kyler_channel::ChannelStore::open(
                &root.path().join("channels.sqlite"),
                "qqbot:fixture".into(),
            )
            .unwrap(),
            web_url: "http://localhost:30002/".parse().unwrap(),
        };
        let mut info = RunInfo {
            run_id: uuid::Uuid::new_v4().to_string(),
            session_id: session.session_id.clone(),
            execution_id: Some(execution_id),
            status: RunStatus::Completed,
            created_at: 0,
            finished_at: Some(1),
            last_event_seq: 0,
            output: Some("done".into()),
            usage: None,
            error: None,
            permissions: None,
            model: None,
        };
        let files = channel
            .registered_files(&server.state.runs, &info)
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
        let (metadata, bytes) = artifacts
            .download(&files[0].session, &files[0].artifact)
            .unwrap();
        assert_eq!(metadata.name, "report.txt");
        assert_eq!(bytes, b"final report");
        info.execution_id = Some(uuid::Uuid::new_v4().to_string());
        assert!(
            channel
                .registered_files(&server.state.runs, &info)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cdn_names_stay_local_bounded_and_images_gain_extensions() {
        let file = kyler_channel::IncomingAttachment {
            name: "../../image".into(),
            url: String::new(),
            content_type: "image/png".into(),
            size: None,
        };
        assert_eq!(
            attachment_name(&file, b"\x89PNG\r\n\x1a\nfixture"),
            "image.png"
        );
        let file = kyler_channel::IncomingAttachment {
            name: "图".repeat(180),
            ..file
        };
        assert!(attachment_name(&file, b"fixture").len() <= 180);
    }
}
