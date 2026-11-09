mod catalogs;
mod permissions;
mod runs;

use crate::attachments::Paste;

use std::path::PathBuf;

use tokio::{sync::mpsc, task::JoinHandle};

use kyler_client::{Client, ClientEvent};
use kyler_protocol::api::{
    ApiErrorBody, ServerInfo, SessionUsage,
    history::{HistoryEvent, HistoryExecution},
    mcp::McpServerInfo,
    models::{ModelCatalog, SessionModel},
    permissions::{ApprovalRecord, PermissionState},
    skills::{SkillDetails, SkillInfo},
    tools::ToolDetails,
};

pub enum Event {
    Questions(Vec<kyler_protocol::api::questions::QuestionRecord>),
    Question(kyler_protocol::api::questions::QuestionRecord),
    QuestionError(String, String),
    ToolDetails(ToolDetails),
    Rejected(ApiErrorBody),
    ImagePaste(u64, Result<Paste, String>),
    Permissions(u64, PermissionState),
    PermissionsChanged(u64, PermissionState),
    PermissionsError(u64, String),
    Approvals(Vec<ApprovalRecord>),
    Approval(ApprovalRecord),
    ApprovalError(String),
    Models(ModelCatalog, SessionModel),
    ModelsError(String),
    Loaded {
        session: String,
        info: ServerInfo,
        usage: SessionUsage,
        events: Vec<HistoryEvent>,
        executions: Vec<HistoryExecution>,
        truncated: bool,
        active: bool,
    },
    Mcp(Vec<McpServerInfo>),
    McpError(String),
    Skills(Vec<SkillInfo>),
    SkillDetails(SkillDetails),
    SkillsError(String),
    Accepted(String),
    Stream(ClientEvent),
    Usage(SessionUsage),
    Cancelled,
    CancelError(String),
    Error(String),
}
pub type Sender = mpsc::Sender<(u64, Event)>;
#[derive(Default)]
pub struct Effects {
    tasks: Vec<JoinHandle<()>>,
    stream: Option<JoinHandle<()>>,
    mcp: Option<JoinHandle<()>>,
    permissions: Option<JoinHandle<()>>,
}
impl Drop for Effects {
    fn drop(&mut self) {
        if let Some(task) = &self.permissions {
            task.abort();
        }
        if let Some(task) = &self.mcp {
            task.abort();
        }
        if let Some(stream) = &self.stream {
            stream.abort();
        }
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Effects {
    pub fn tool_details(
        &mut self,
        client: Client,
        session: String,
        call: String,
        generation: u64,
        send: Sender,
    ) {
        self.spawn(async move {
            let event = match client.tool_details(&session, &call).await {
                Ok(details) => Event::ToolDetails(details),
                Err(error) => Event::CancelError(error.to_string()),
            };
            let _ = send.send((generation, event)).await;
        });
    }
    pub fn paste_image(
        &mut self,
        path: Option<PathBuf>,
        generation: u64,
        epoch: u64,
        send: Sender,
    ) {
        self.spawn(async move {
            let result = tokio::task::spawn_blocking(move || match path {
                Some(path) => crate::attachments::from_path(&path).map(Paste::Image),
                None => crate::attachments::clipboard(),
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| format!("{e:#}")));
            let _ = send
                .send((generation, Event::ImagePaste(epoch, result)))
                .await;
        });
    }

    fn spawn(&mut self, future: impl Future<Output = ()> + Send + 'static) {
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio::spawn(future));
    }
    fn spawn_stream(&mut self, future: impl Future<Output = ()> + Send + 'static) {
        if let Some(previous) = self.stream.take() {
            previous.abort();
        }
        self.stream = Some(tokio::spawn(future));
    }

    pub fn load(&mut self, client: Client, session: Option<String>, generation: u64, send: Sender) {
        self.spawn(async move {
            let result = async {
                let mut info = client.info().await?;
                let session = match session {
                    Some(id) => client.session(&id).await?,
                    None => {
                        let cwd = std::env::current_dir().map_err(|_| {
                            kyler_client::Error::invalid("Cannot determine the current workspace.")
                        })?;
                        client.create_project_session_in(&cwd).await?
                    }
                };
                info.workspace_root = session.workspace_root.clone();
                info.working_directory = session.working_directory.clone();
                let usage = client.session_usage(&session.session_id).await?;
                let mut events = Vec::new();
                let mut executions = std::collections::HashMap::new();
                let mut cursor = 0;
                let mut truncated = false;
                loop {
                    let page = client.history(&session.session_id, cursor, 500).await?;
                    events.extend(page.events);
                    for execution in page.executions { executions.insert(execution.execution_id.clone(), execution); }
                    if events.len() > 2000 {
                        events.drain(..events.len() - 2000);
                        truncated = true;
                    }
                    if let Some(next) = page.next_cursor {
                        if next <= cursor {
                            return Err(kyler_client::Error::protocol(
                                "Invalid history pagination cursor.",
                            ));
                        }
                        cursor = next;
                    } else {
                        break;
                    }
                }
                Ok::<_, kyler_client::Error>(Event::Loaded {
                    session: session.session_id,
                    info,
                    usage,
                    events,
                    executions: executions.into_values().collect(),
                    truncated,
                    active: session.status.is_some_and(|s| !s.is_terminal()),
                })
            }
            .await;
            let event = match result {
                Ok(event) => event,
                Err(error) => Event::Error(format!(
                    "Unable to load server info or session: {error}. Restart the latest server and press F5 to retry."
                )),
            };
            let _ = send.send((generation, event)).await;
        });
    }
}
