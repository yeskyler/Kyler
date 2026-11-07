//! Bounded, session-owned background commands using the existing sandbox worker.
use super::{output::publish, store::persist};
use crate::{
    environment::ExecutionEnvironment,
    sandbox::{ProcessChunk, ProcessInput},
    tools::command::RunCommandArgs,
};

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc::SyncSender},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use tokio::{
    sync::{broadcast, mpsc},
    task::AbortHandle,
};

use kyler_protocol::api::processes::{ProcessInfo, ProcessMetadata, ProcessOutput};

pub(super) struct Entry {
    pub(super) session: String,
    pub(super) info: ProcessInfo,
    pub(super) input: SyncSender<ProcessInput>,
    pub(super) abort: Option<AbortHandle>,
}
pub(super) struct Inner {
    pub(super) root: Option<PathBuf>,
    pub(super) entries: Mutex<HashMap<String, Entry>>,
    pub(super) events: broadcast::Sender<ProcessOutput>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        for entry in self.entries.get_mut().unwrap().values() {
            if let Some(task) = &entry.abort {
                task.abort();
            }
        }
    }
}
#[derive(Clone)]
pub struct ProcessManager {
    pub(super) inner: Arc<Inner>,
}
impl Default for ProcessManager {
    fn default() -> Self {
        Self {
            inner: Arc::new(Inner {
                root: None,
                entries: Mutex::new(HashMap::new()),
                events: broadcast::channel(256).0,
            }),
        }
    }
}
impl ProcessManager {
    /// Start once, then yield control without restarting or cancelling the command.
    pub async fn run_command(
        &self,
        session: &str,
        environment: ExecutionEnvironment,
        command: RunCommandArgs,
    ) -> Result<Value> {
        command.validate()?;
        ensure!(
            environment.policy().process.allow_commands,
            "Sandbox blocks commands"
        );
        command.directory(
            &environment.session_environment().working_directory,
            &environment.policy().filesystem,
        )?;
        if let Some(timeout) = command.timeout_ms {
            ensure!(
                timeout <= environment.policy().process.background_timeout_ms,
                "requested command timeout exceeds host background limit"
            );
        }
        let wait = command.yield_time_ms;
        let mut events = self.subscribe();
        let id = self.start(session, environment, serde_json::to_value(&command)?)?;
        let mut pending = PendingCommand {
            manager: self.clone(),
            session: session.into(),
            id: id.clone(),
            armed: true,
        };
        let _ = tokio::time::timeout(Duration::from_millis(wait.into()), async {
            loop {
                if self.read(session, &id, 0)?.status != "running" {
                    return Ok::<_, anyhow::Error>(());
                }
                let _ = events.recv().await;
            }
        })
        .await;
        let info = self.read(session, &id, 0)?;
        pending.armed = false;
        if let Some(error) = info.error {
            anyhow::bail!("command failed: {error}");
        }
        let mut result = if let Some(result) = info.result {
            serde_json::from_str::<Value>(&result).context("Invalid command result")?
        } else {
            let output = |stream: &str| {
                info.output
                    .iter()
                    .filter(|chunk| chunk.stream == stream)
                    .map(|chunk| chunk.text.as_str())
                    .collect::<String>()
            };
            serde_json::json!({"stdout":output("stdout"), "stderr":output("stderr")})
        };
        result["process_id"] = serde_json::json!(id);
        result["status"] = serde_json::json!(info.status);
        result["cursor"] = serde_json::json!(info.cursor);
        result["oldest_cursor"] = serde_json::json!(info.oldest_cursor);
        Ok(result)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<ProcessOutput> {
        self.inner.events.subscribe()
    }
    pub fn start(
        &self,
        session: &str,
        environment: ExecutionEnvironment,
        args: Value,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut command = args["program"]
            .as_str()
            .unwrap_or("Command")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if let Some(action) = args["args"][0].as_str().filter(|value| {
            matches!(
                *value,
                "test" | "build" | "check" | "install" | "run" | "dev" | "serve" | "start"
            )
        }) {
            command.push(' ');
            command.push_str(action);
        }
        let metadata = ProcessMetadata {
            command: command.chars().take(160).collect(),
            working_directory: serde_json::from_value::<RunCommandArgs>(args.clone())?
                .directory(
                    &environment.session_environment().working_directory,
                    &environment.policy().filesystem,
                )?
                .to_string_lossy()
                .into_owned(),
            started_at: chrono::Utc::now().timestamp_millis(),
            finished_at: None,
            exit_code: None,
        };
        let (input, receive) = std::sync::mpsc::sync_channel(32);
        let mut entries = self.inner.entries.lock().unwrap();
        ensure!(
            entries
                .values()
                .filter(|entry| entry.session == session && entry.info.status == "running")
                .count()
                < 4,
            "At most four active processes are allowed per session"
        );
        if entries.len() >= 128 {
            let expired = entries
                .iter()
                .find(|(_, entry)| entry.info.status != "running")
                .map(|(id, _)| id.clone());
            if let Some(id) = expired {
                entries.remove(&id);
                if let Some(root) = &self.inner.root {
                    let _ = std::fs::remove_file(root.join(format!("{id}.json")));
                }
            } else {
                anyhow::bail!("Process capacity reached");
            }
        }
        entries.insert(
            id.clone(),
            Entry {
                session: session.into(),
                input,
                abort: None,
                info: ProcessInfo {
                    metadata: Some(metadata),
                    id: id.clone(),
                    status: "running".into(),
                    cursor: 0,
                    oldest_cursor: 0,
                    output: Vec::new(),
                    result: None,
                    error: None,
                },
            },
        );
        if let Err(error) = persist(&self.inner, &entries[&id]) {
            entries.remove(&id);
            return Err(error);
        }
        let manager = Arc::downgrade(&self.inner);
        let key = id.clone();
        let owner = session.to_owned();
        let arguments = serde_json::to_string(&args)?;
        let task = tokio::spawn(async move {
            let (progress, mut output) = mpsc::unbounded_channel::<ProcessChunk>();
            let run =
                environment.run_tool_io("run_command", &arguments, Some(progress), Some(receive));
            tokio::pin!(run);
            let result = loop {
                tokio::select! {
                    result=&mut run=>break result,
                    Some(chunk)=output.recv()=>{if let Some(inner)=manager.upgrade(){publish(&inner,&key,&owner,chunk); }else{return;}}
                }
            };
            while let Ok(chunk) = output.try_recv() {
                if let Some(inner) = manager.upgrade() {
                    publish(&inner, &key, &owner, chunk);
                }
            }
            if let Some(inner) = manager.upgrade() {
                if let Some(entry) = inner.entries.lock().unwrap().get_mut(&key) {
                    match result {
                        Ok(result) => {
                            if let Some(metadata) = &mut entry.info.metadata {
                                metadata.exit_code = serde_json::from_str::<Value>(&result)
                                    .ok()
                                    .and_then(|value| value["exit_code"].as_i64())
                                    .and_then(|code| i32::try_from(code).ok());
                            }
                            entry.info.status = "completed".into();
                            entry.info.result = Some(bounded_result(&result));
                        }
                        Err(error) => {
                            entry.info.status = "failed".into();
                            entry.info.error = Some(format!("{error:#}"));
                        }
                    }
                    if let Some(metadata) = &mut entry.info.metadata {
                        metadata.finished_at = Some(chrono::Utc::now().timestamp_millis());
                    }
                    if let Err(error) = persist(&inner, entry) {
                        entry.info.error =
                            Some(format!("Process state could not be persisted: {error}"));
                    }
                }
                let status = inner
                    .entries
                    .lock()
                    .unwrap()
                    .get(&key)
                    .map(|entry| entry.info.status.clone())
                    .unwrap_or_default();
                publish(
                    &inner,
                    &key,
                    &owner,
                    ProcessChunk {
                        stream: "status".into(),
                        text: status,
                    },
                );
            }
        });
        entries.get_mut(&id).unwrap().abort = Some(task.abort_handle());
        Ok(id)
    }
    pub fn read(&self, session: &str, id: &str, cursor: u64) -> Result<ProcessInfo> {
        let entries = self.inner.entries.lock().unwrap();
        let entry = entries
            .get(id)
            .filter(|entry| entry.session == session)
            .context("Process is unavailable in this session")?;
        let mut info = entry.info.clone();
        info.output.retain(|chunk| chunk.cursor > cursor);
        Ok(info)
    }
    pub fn list(&self, session: &str) -> Vec<ProcessInfo> {
        self.inner
            .entries
            .lock()
            .unwrap()
            .values()
            .filter(|entry| entry.session == session)
            .map(|entry| {
                let mut info = entry.info.clone();
                info.output.clear();
                info.result = None;
                info
            })
            .collect()
    }
    pub fn write(&self, session: &str, id: &str, input: ProcessInput) -> Result<()> {
        ensure!(input.data.len() <= 8192, "Process input exceeds 8 KiB");
        let entries = self.inner.entries.lock().unwrap();
        let entry = entries
            .get(id)
            .filter(|entry| entry.session == session)
            .context("Process is unavailable")?;
        ensure!(
            entry.info.status == "running",
            "Process is no longer running"
        );
        entry
            .input
            .try_send(input)
            .context("Process input queue is full or closed")?;
        Ok(())
    }
    pub fn stop(&self, session: &str, id: &str) -> Result<()> {
        let mut entries = self.inner.entries.lock().unwrap();
        let entry = entries
            .get_mut(id)
            .filter(|entry| entry.session == session)
            .context("Process is unavailable")?;
        if entry.info.status == "running" {
            if let Some(task) = &entry.abort {
                task.abort();
            }
            entry.info.status = "cancelled".into();
            if let Some(metadata) = &mut entry.info.metadata {
                metadata.finished_at = Some(chrono::Utc::now().timestamp_millis());
            }
            let _ = persist(&self.inner, entry);
        }
        Ok(())
    }
    pub fn stop_session(&self, session: &str) {
        let ids = self
            .list(session)
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.stop(session, &id);
        }
    }
    pub fn forget_session(&self, session: &str) {
        self.stop_session(session);
        let mut entries = self.inner.entries.lock().unwrap();
        let ids = entries
            .iter()
            .filter(|(_, entry)| entry.session == session)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            entries.remove(&id);
            if let Some(root) = &self.inner.root {
                let _ = std::fs::remove_file(root.join(format!("{id}.json")));
            }
        }
    }
    pub fn shutdown(&self) {
        let mut entries = self.inner.entries.lock().unwrap();
        for entry in entries.values_mut() {
            if entry.info.status == "running" {
                if let Some(task) = &entry.abort {
                    task.abort();
                }
                entry.info.status = "cancelled".into();
                if let Some(metadata) = &mut entry.info.metadata {
                    metadata.finished_at = Some(chrono::Utc::now().timestamp_millis());
                }
                let _ = persist(&self.inner, entry);
            }
        }
    }
}
// Cancellation while waiting must stop the command; a returned process ID transfers ownership.
struct PendingCommand {
    manager: ProcessManager,
    session: String,
    id: String,
    armed: bool,
}
impl Drop for PendingCommand {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.manager.stop(&self.session, &self.id);
        }
    }
}
fn bounded_result(result: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(result) else {
        return result.into();
    };
    for field in ["stdout", "stderr"] {
        if let Some(text) = value[field]
            .as_str()
            .filter(|text| text.len() > 6000)
            .map(str::to_owned)
        {
            value[field] = Value::String(crate::tools::output::text_prefix(&text, 6000).into());
            value[format!("{field}_truncated")] = Value::Bool(true);
        }
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::ProcessManager;
    use crate::sandbox::ProcessInput;

    use kyler_protocol::api::processes::ProcessInfo;

    #[test]
    fn persisted_live_processes_become_interrupted_and_are_not_restarted() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let info = ProcessInfo {
            metadata: None,
            id: id.clone(),
            status: "running".into(),
            cursor: 0,
            oldest_cursor: 0,
            output: Vec::new(),
            result: None,
            error: None,
        };
        std::fs::write(
            root.path().join(format!("{id}.json")),
            serde_json::to_vec(&("owner", info)).unwrap(),
        )
        .unwrap();
        let manager = ProcessManager::persistent(root.path().into()).unwrap();
        let info = manager.read("owner", &id, 0).unwrap();
        assert_eq!(info.status, "interrupted");
        assert!(info.error.unwrap().contains("unknown"));
        assert!(manager.read("other", &id, 0).is_err());
        assert!(
            manager
                .write(
                    "owner",
                    &id,
                    ProcessInput {
                        data: "no".into(),
                        close: false
                    }
                )
                .is_err()
        );
        manager.forget_session("owner");
        assert!(!root.path().join(format!("{id}.json")).exists());
    }
}
