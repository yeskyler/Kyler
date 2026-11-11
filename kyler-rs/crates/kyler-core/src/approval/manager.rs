//! Conservative automatic review and cancellable, exact-call human approvals.
use super::store;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{
    broadcast, oneshot,
    watch::{Receiver, Sender},
};

use kyler_protocol::api::permissions::{
    ApprovalDecision, ApprovalRecord, ApprovalResponse, ApprovalStatus, PermissionSnapshot,
};

#[derive(Clone)]
pub struct ApprovalContext {
    pub manager: Arc<ApprovalManager>,
    pub policy: PermissionSnapshot,
    pub session_id: String,
    pub run_id: String,
}
impl ApprovalContext {
    /// One source of truth for tool discovery, presentation and authorization.
    pub fn current_policy(&self) -> PermissionSnapshot {
        self.manager
            .policy_changes(&self.session_id, &self.policy)
            .borrow()
            .clone()
    }
}

struct Pending {
    record: ApprovalRecord,
    sender: Option<oneshot::Sender<ApprovalStatus>>,
    grant: String,
}
#[derive(Default)]
struct State {
    records: HashMap<String, Pending>,
    grants: HashSet<(String, String)>,
    cancelled_runs: HashSet<String>,
    policies: HashMap<String, Sender<PermissionSnapshot>>,
}
pub struct ApprovalManager {
    state: Mutex<State>,
    events: broadcast::Sender<ApprovalRecord>,
    timeout: Duration,
    directory: Option<PathBuf>,
}
impl ApprovalManager {
    pub fn new(timeout: Duration) -> Self {
        Self {
            state: Mutex::new(State::default()),
            events: broadcast::channel(256).0,
            timeout,
            directory: None,
        }
    }
    pub fn persistent(timeout: Duration, directory: PathBuf) -> Result<Self> {
        let records = store::load(&directory)?;
        let mut manager = Self::new(timeout);
        manager.directory = Some(directory);
        for mut record in records {
            if record.status == ApprovalStatus::Pending {
                record.status = ApprovalStatus::Cancelled;
                record.reason = "Service restarted; approval invalidated without replay.".into();
                manager.persist(&record)?;
            }
            manager
                .state
                .get_mut()
                .expect("new approval state")
                .records
                .insert(
                    record.id.clone(),
                    Pending {
                        record,
                        sender: None,
                        grant: String::new(),
                    },
                );
        }
        Ok(manager)
    }
    fn persist(&self, record: &ApprovalRecord) -> Result<()> {
        if let Some(directory) = &self.directory {
            store::save(directory, record)?;
        }
        Ok(())
    }
    pub fn policy_changes(
        &self,
        session: &str,
        initial: &PermissionSnapshot,
    ) -> Receiver<PermissionSnapshot> {
        self.state
            .lock()
            .expect("approval state poisoned")
            .policies
            .entry(session.into())
            .or_insert_with(|| tokio::sync::watch::channel(initial.clone()).0)
            .subscribe()
    }
    pub fn update_policy(&self, session: &str, policy: PermissionSnapshot) {
        let mut state = self.state.lock().expect("approval state poisoned");
        if state.policies.get(session).is_some_and(|sender| {
            let current = sender.borrow();
            current.revision == policy.revision
                && current.mode == policy.mode
                && current.approval == policy.approval
        }) {
            return;
        }
        state.grants.retain(|(id, _)| id != session);
        state
            .policies
            .entry(session.into())
            .or_insert_with(|| tokio::sync::watch::channel(policy.clone()).0)
            .send_replace(policy);
    }
    pub fn publish(&self, record: ApprovalRecord) {
        let _ = self.events.send(record);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<ApprovalRecord> {
        self.events.subscribe()
    }
    pub fn get(&self, id: &str) -> Option<ApprovalRecord> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .records
            .get(id)
            .map(|pending| pending.record.clone())
    }
    pub fn list(&self, session: &str) -> Vec<ApprovalRecord> {
        let mut records: Vec<_> = self
            .state
            .lock()
            .expect("approval state poisoned")
            .records
            .values()
            .filter(|p| p.record.session_id == session)
            .map(|p| p.record.clone())
            .collect();
        records.sort_by_key(|r| (r.created_at, r.id.clone()));
        records
    }
    pub fn waiting_for_session(&self, session: &str) -> bool {
        self.state
            .lock()
            .expect("approval state poisoned")
            .records
            .values()
            .any(|p| {
                p.record.origin_session_id == session && p.record.status == ApprovalStatus::Pending
            })
    }
    pub fn waiting(&self, run: &str) -> bool {
        self.state
            .lock()
            .expect("approval state poisoned")
            .records
            .values()
            .any(|p| p.record.run_id == run && p.record.status == ApprovalStatus::Pending)
    }
    pub fn register(
        self: &Arc<Self>,
        mut record: ApprovalRecord,
        identity: &str,
        arguments: &Value,
    ) -> Result<ApprovalTicket> {
        let grant = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(
                identity,
                arguments,
                &record.policy.revision,
                &record.working_directory
            ))?)
        );
        record.digest = format!(
            "{:x}",
            Sha256::digest(format!(
                "{}:{}:{}:{}:{grant}",
                record.id, record.run_id, record.execution_id, record.tool_call_id
            ))
        );
        let mut state = self.state.lock().expect("approval state poisoned");
        ensure!(
            !state.cancelled_runs.contains(&record.run_id),
            "Task was cancelled."
        );
        ensure!(
            state
                .records
                .values()
                .filter(|p| p.record.status == ApprovalStatus::Pending)
                .count()
                < 256,
            "approval queue is full"
        );
        if state
            .grants
            .contains(&(record.session_id.clone(), grant.clone()))
        {
            record.status = ApprovalStatus::Allowed;
            record.reason = "Exact operation previously allowed for this session.".into();
        }
        let (sender, receiver) = oneshot::channel();
        record.expires_at = record.created_at + self.timeout.as_millis() as i64;
        self.persist(&record)?;
        state.records.insert(
            record.id.clone(),
            Pending {
                record: record.clone(),
                sender: Some(sender),
                grant,
            },
        );
        // Bound completed metadata; pending operations are never evicted.
        if state.records.len() > 1024 {
            let mut finished: Vec<_> = state
                .records
                .values()
                .filter(|p| p.record.status != ApprovalStatus::Pending && p.record.id != record.id)
                .map(|p| (p.record.created_at, p.record.id.clone()))
                .collect();
            finished.sort();
            for (_, id) in finished.into_iter().take(state.records.len() - 1024) {
                state.records.remove(&id);
                if let Some(directory) = &self.directory {
                    let _ = std::fs::remove_file(directory.join(format!("{id}.json")));
                }
            }
        }
        drop(state);
        Ok(ApprovalTicket {
            manager: self.clone(),
            record,
            receiver: Some(receiver),
        })
    }
    pub fn decide(&self, id: &str, response: &ApprovalResponse) -> Result<ApprovalRecord> {
        let mut state = self.state.lock().expect("approval state poisoned");
        ensure!(
            response.decision != ApprovalDecision::AllowSession || state.grants.len() < 4096,
            "Session approval grant limit reached"
        );
        let pending = state
            .records
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("approval is unavailable"))?;
        ensure!(
            pending.record.status == ApprovalStatus::Pending
                && pending.record.digest == response.digest
                && chrono::Utc::now().timestamp_millis() < pending.record.expires_at,
            "approval is stale or already resolved"
        );
        let status = if response.decision == ApprovalDecision::Deny {
            ApprovalStatus::Denied
        } else {
            ApprovalStatus::Allowed
        };
        let mut updated = pending.record.clone();
        updated.status = status;
        updated.reason = "User decision.".into();
        self.persist(&updated)?;
        pending.record = updated;
        let grant = (pending.record.session_id.clone(), pending.grant.clone());
        let record = pending.record.clone();
        if let Some(sender) = pending.sender.take() {
            let _ = sender.send(status);
        }
        if response.decision == ApprovalDecision::AllowSession {
            state.grants.insert(grant);
        }
        drop(state);
        let _ = self.events.send(record.clone());
        Ok(record)
    }
    fn resolve(&self, id: &str, status: ApprovalStatus, reason: &str) {
        let mut state = self.state.lock().expect("approval state poisoned");
        if let Some(pending) = state.records.get_mut(id) {
            if pending.record.status == ApprovalStatus::Pending {
                pending.record.status = status;
                pending.record.reason = reason.into();
                if self.persist(&pending.record).is_err() {
                    tracing::warn!(
                        "Unable to persist approval cancellation; pending requests will be invalidated on restart"
                    );
                }
                if let Some(sender) = pending.sender.take() {
                    let _ = sender.send(status);
                }
                let _ = self.events.send(pending.record.clone());
            }
        }
    }
    pub fn cancel_run(&self, run: &str) {
        self.state
            .lock()
            .expect("approval state poisoned")
            .cancelled_runs
            .insert(run.into());
        let ids: Vec<_> = self
            .state
            .lock()
            .expect("approval state poisoned")
            .records
            .values()
            .filter(|p| p.record.run_id == run)
            .map(|p| p.record.id.clone())
            .collect();
        for id in ids {
            self.resolve(&id, ApprovalStatus::Cancelled, "Task ended or cancelled.");
        }
    }
    pub fn forget_run(&self, run: &str) {
        let mut state = self.state.lock().expect("approval state poisoned");
        state.cancelled_runs.remove(run);
        let removed: Vec<_> = state
            .records
            .values()
            .filter(|p| p.record.run_id == run)
            .map(|p| p.record.id.clone())
            .collect();
        state.records.retain(|_, p| p.record.run_id != run);
        if let Some(directory) = &self.directory {
            for id in removed {
                let _ = std::fs::remove_file(directory.join(format!("{id}.json")));
            }
        }
    }
    pub fn forget_session(&self, session: &str) {
        let mut state = self.state.lock().expect("approval state poisoned");
        state.grants.retain(|(s, _)| s != session);
        state.policies.remove(session);
        let removed: Vec<_> = state
            .records
            .values()
            .filter(|p| p.record.session_id == session)
            .map(|p| p.record.id.clone())
            .collect();
        state.records.retain(|_, p| p.record.session_id != session);
        if let Some(directory) = &self.directory {
            for id in removed {
                let _ = std::fs::remove_file(directory.join(format!("{id}.json")));
            }
        }
    }
}
pub struct ApprovalTicket {
    manager: Arc<ApprovalManager>,
    pub record: ApprovalRecord,
    receiver: Option<oneshot::Receiver<ApprovalStatus>>,
}
impl ApprovalTicket {
    pub async fn wait(mut self) -> ApprovalRecord {
        let _ = self.manager.events.send(self.record.clone());
        if self.record.status == ApprovalStatus::Pending {
            if tokio::time::timeout(
                self.manager.timeout,
                self.receiver.take().expect("approval receiver exists"),
            )
            .await
            .is_err()
            {
                self.manager.resolve(
                    &self.record.id,
                    ApprovalStatus::Expired,
                    "Approval timed out.",
                );
            }
        }
        if let Some(pending) = self
            .manager
            .state
            .lock()
            .expect("approval state poisoned")
            .records
            .get(&self.record.id)
        {
            self.record = pending.record.clone();
        }
        self.record.clone()
    }
}
impl Drop for ApprovalTicket {
    fn drop(&mut self) {
        self.manager.resolve(
            &self.record.id,
            ApprovalStatus::Cancelled,
            "Approval wait cancelled.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{super::store, ApprovalManager};
    use crate::approval::{ToolOperation, review};

    use std::{sync::Arc, time::Duration};

    use serde_json::Value;

    use kyler_protocol::api::permissions::{
        ApprovalDecision, ApprovalRecord, ApprovalResponse, ApprovalStatus, PermissionMode,
        PermissionSnapshot,
    };

    fn record(session: &str) -> ApprovalRecord {
        ApprovalRecord {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: "run".into(),
            session_id: session.into(),
            origin_session_id: "child".into(),
            execution_id: "execution".into(),
            tool_call_id: "call".into(),
            tool: "fixture".into(),
            operation: "Write".into(),
            preview: "exact operation".into(),
            requires_user: true,
            working_directory: Some("/workspace".into()),
            policy: PermissionSnapshot {
                approval: None,
                mode: PermissionMode::WorkspaceAccess,
                revision: "one".into(),
            },
            digest: String::new(),
            status: ApprovalStatus::Pending,
            reason: "Review".into(),
            created_at: chrono::Utc::now().timestamp_millis(),
            expires_at: 0,
        }
    }
    #[tokio::test]
    async fn approvals_are_exact_single_use_and_scoped_to_session_and_policy() {
        let manager = Arc::new(ApprovalManager::new(Duration::from_secs(1)));
        let ticket = manager
            .register(
                record("parent"),
                "tool-v1",
                &serde_json::json!({"file":"a"}),
            )
            .unwrap();
        let request = ticket.record.clone();
        assert!(manager.waiting("run") && manager.waiting_for_session("child"));
        assert!(
            manager
                .decide(
                    &request.id,
                    &ApprovalResponse {
                        decision: ApprovalDecision::AllowOnce,
                        digest: "wrong".into()
                    }
                )
                .is_err()
        );
        let response = ApprovalResponse {
            decision: ApprovalDecision::AllowSession,
            digest: request.digest.clone(),
        };
        manager.decide(&request.id, &response).unwrap();
        assert!(manager.decide(&request.id, &response).is_err());
        assert_eq!(ticket.wait().await.status, ApprovalStatus::Allowed);
        let repeated = manager
            .register(
                record("parent"),
                "tool-v1",
                &serde_json::json!({"file":"a"}),
            )
            .unwrap();
        assert_eq!(repeated.record.status, ApprovalStatus::Allowed);
        assert_eq!(repeated.wait().await.status, ApprovalStatus::Allowed);
        for (session, identity, args, revision) in [
            ("other", "tool-v1", "a", "one"),
            ("parent", "tool-v2", "a", "one"),
            ("parent", "tool-v1", "b", "one"),
            ("parent", "tool-v1", "a", "two"),
        ] {
            let mut request = record(session);
            request.policy.revision = revision.into();
            let ticket = manager
                .register(request, identity, &serde_json::json!({"file":args}))
                .unwrap();
            assert_eq!(ticket.record.status, ApprovalStatus::Pending);
            drop(ticket);
        }
    }
    #[tokio::test]
    async fn timeout_cancellation_and_dropped_wait_never_allow_execution() {
        let manager = Arc::new(ApprovalManager::new(Duration::from_millis(5)));
        let ticket = manager
            .register(record("session"), "tool", &Value::Null)
            .unwrap();
        assert_eq!(ticket.wait().await.status, ApprovalStatus::Expired);
        let ticket = manager
            .register(record("session"), "tool", &Value::Null)
            .unwrap();
        manager.cancel_run("run");
        assert_eq!(ticket.wait().await.status, ApprovalStatus::Cancelled);
        assert!(
            manager
                .register(record("session"), "tool", &Value::Null)
                .is_err()
        );
        manager.forget_run("run");
        let ticket = manager
            .register(record("session"), "tool", &Value::Null)
            .unwrap();
        let id = ticket.record.id.clone();
        drop(ticket);
        assert!(
            manager
                .list("session")
                .iter()
                .any(|r| r.id == id && r.status == ApprovalStatus::Cancelled)
        );
    }
    #[test]
    fn automatic_review_never_guesses_shell_commands_or_unknown_tools_are_read_only() {
        assert_eq!(
            review(
                PermissionMode::AutoReview,
                ToolOperation::Read,
                &Value::Null
            ),
            ApprovalStatus::Allowed
        );
        assert_eq!(
            review(
                PermissionMode::AutoReview,
                ToolOperation::Command,
                &serde_json::json!({"program":"sh","args":["-c","pwd; rm -rf project"]})
            ),
            ApprovalStatus::Pending
        );
        assert_eq!(
            review(
                PermissionMode::AutoReview,
                ToolOperation::External,
                &Value::Null
            ),
            ApprovalStatus::Pending
        );
        assert_eq!(
            review(PermissionMode::ReadOnly, ToolOperation::Write, &Value::Null),
            ApprovalStatus::Denied
        );
        assert_eq!(
            review(
                PermissionMode::AutoReview,
                ToolOperation::Command,
                &serde_json::json!({"program":"/usr/bin/pwd","args":[]})
            ),
            ApprovalStatus::Allowed
        );
    }
    #[tokio::test]
    async fn persisted_pending_requests_are_invalidated_and_grants_do_not_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let pending = record("session");
        store::save(directory.path(), &pending).unwrap();
        let manager = Arc::new(
            ApprovalManager::persistent(Duration::from_secs(1), directory.path().to_owned())
                .unwrap(),
        );
        assert_eq!(manager.list("session")[0].status, ApprovalStatus::Cancelled);
        assert!(
            manager
                .decide(
                    &pending.id,
                    &ApprovalResponse {
                        decision: ApprovalDecision::AllowOnce,
                        digest: pending.digest
                    }
                )
                .is_err()
        );
        let ticket = manager
            .register(record("session"), "tool", &Value::Null)
            .unwrap();
        let approved = ticket.record.clone();
        manager
            .decide(
                &approved.id,
                &ApprovalResponse {
                    decision: ApprovalDecision::AllowSession,
                    digest: approved.digest.clone(),
                },
            )
            .unwrap();
        assert_eq!(ticket.wait().await.status, ApprovalStatus::Allowed);
        drop(manager);
        let manager = Arc::new(
            ApprovalManager::persistent(Duration::from_secs(1), directory.path().to_owned())
                .unwrap(),
        );
        let ticket = manager
            .register(record("session"), "tool", &Value::Null)
            .unwrap();
        assert_eq!(ticket.record.status, ApprovalStatus::Pending);
        let id = ticket.record.id.clone();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(directory.path().join(format!("{id}.json")))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(directory.path()).unwrap();
        assert!(
            manager
                .decide(
                    &id,
                    &ApprovalResponse {
                        decision: ApprovalDecision::AllowOnce,
                        digest: ticket.record.digest.clone()
                    }
                )
                .is_err()
        );
        assert!(manager.waiting("run"));
        drop(ticket);
    }
}
