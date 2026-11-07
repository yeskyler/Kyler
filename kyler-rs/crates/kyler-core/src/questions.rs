//! Durable, cancellable user questions. Replies never grant execution privileges.
use anyhow::{Context, Result, ensure};
use kyler_protocol::api::questions::*;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::{broadcast, watch};

/// Host scheduling hook. Waiting releases capacity; continuing acquires it again.
#[async_trait::async_trait]
pub trait QuestionWaiter: Send + Sync {
    async fn suspend(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
}
struct Entry {
    record: QuestionRecord,
    changed: watch::Sender<QuestionRecord>,
}
pub struct QuestionManager {
    entries: Mutex<HashMap<String, Entry>>,
    events: broadcast::Sender<QuestionRecord>,
    directory: PathBuf,
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
}
pub fn validate(args: &AskQuestionArgs) -> Result<()> {
    ensure!(
        (1..=3).contains(&args.questions.len()),
        "Ask one to three questions at a time"
    );
    let mut ids = HashSet::new();
    for question in &args.questions {
        ensure!(
            identifier(&question.id) && ids.insert(&question.id),
            "Question IDs must be unique identifiers"
        );
        ensure!(
            !question.question.trim().is_empty() && question.question.len() <= 2000,
            "Question text must contain 1 to 2000 bytes"
        );
        ensure!(
            question.options.len() <= 8,
            "A question supports at most eight options"
        );
        match question.kind {
            QuestionType::Text => ensure!(
                question.options.is_empty(),
                "Text questions cannot contain options"
            ),
            _ => ensure!(
                !question.options.is_empty(),
                "Selection questions require options"
            ),
        }
        let mut options = HashSet::new();
        for option in &question.options {
            ensure!(
                identifier(&option.id) && options.insert(&option.id),
                "Option IDs must be unique identifiers"
            );
            ensure!(
                !option.label.trim().is_empty() && option.label.len() <= 300,
                "Option labels must contain 1 to 300 bytes"
            );
        }
    }
    Ok(())
}
fn validate_answers(
    record: &QuestionRecord,
    answers: &BTreeMap<String, QuestionAnswer>,
) -> Result<()> {
    ensure!(
        answers.len() == record.questions.len(),
        "Answer every question"
    );
    for question in &record.questions {
        let answer = answers
            .get(&question.id)
            .context("Missing question answer")?;
        ensure!(
            answer.text.len() <= 4000,
            "Custom answers must not exceed 4000 bytes"
        );
        let mut seen = HashSet::new();
        for id in &answer.selected {
            ensure!(
                seen.insert(id) && question.options.iter().any(|option| &option.id == id),
                "Invalid or duplicate answer option"
            );
        }
        match question.kind {
            QuestionType::SingleSelect => ensure!(
                answer.selected.len() <= 1
                    && (answer.selected.is_empty() || answer.text.trim().is_empty()),
                "Choose one option or provide a custom answer"
            ),
            QuestionType::Text => ensure!(
                answer.selected.is_empty(),
                "Text answers cannot select options"
            ),
            QuestionType::MultiSelect => {}
        }
        ensure!(
            !answer.selected.is_empty() || !answer.text.trim().is_empty(),
            "An answer cannot be empty"
        );
    }
    Ok(())
}
impl QuestionManager {
    pub fn persistent(directory: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let manager = Self {
            entries: Mutex::new(HashMap::new()),
            events: broadcast::channel(256).0,
            directory,
        };
        for file in std::fs::read_dir(&manager.directory)? {
            let path = file?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let mut record: QuestionRecord = serde_json::from_slice(&std::fs::read(path)?)?;
            if record.status == QuestionStatus::Pending {
                record.status = QuestionStatus::Interrupted;
                record.resolved_at = Some(chrono::Utc::now().timestamp_millis());
                manager.persist(&record)?;
            }
            let changed = watch::channel(record.clone()).0;
            manager
                .entries
                .lock()
                .unwrap()
                .insert(record.id.clone(), Entry { record, changed });
        }
        Ok(manager)
    }
    fn persist(&self, record: &QuestionRecord) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(&mut file, record)?;
        file.as_file().sync_all()?;
        file.persist(self.directory.join(format!("{}.json", record.id)))
            .map_err(|error| error.error)?;
        Ok(())
    }
    pub fn forget_session(&self, session: &str) -> Result<()> {
        let mut entries = self.entries.lock().unwrap();
        let ids = entries
            .values()
            .filter(|entry| entry.record.session_id == session)
            .map(|entry| entry.record.id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            match std::fs::remove_file(self.directory.join(format!("{id}.json"))) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            entries.remove(&id);
        }
        Ok(())
    }
    pub fn subscribe(&self) -> broadcast::Receiver<QuestionRecord> {
        self.events.subscribe()
    }
    pub fn list(&self, session: &str) -> Vec<QuestionRecord> {
        let mut records = self
            .entries
            .lock()
            .unwrap()
            .values()
            .filter(|entry| entry.record.session_id == session)
            .map(|entry| entry.record.clone())
            .collect::<Vec<_>>();
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        records
    }
    pub fn waiting(&self, session: &str) -> bool {
        self.entries.lock().unwrap().values().any(|entry| {
            entry.record.session_id == session && entry.record.status == QuestionStatus::Pending
        })
    }
    pub fn respond(
        &self,
        session: &str,
        id: &str,
        response: &QuestionResponse,
    ) -> Result<QuestionRecord> {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries
            .get_mut(id)
            .filter(|entry| entry.record.session_id == session)
            .context("Question not found")?;
        let (status, answers) = match response {
            QuestionResponse::Cancel => (QuestionStatus::Dismissed, BTreeMap::new()),
            QuestionResponse::Submit { answers } => {
                validate_answers(&entry.record, answers)?;
                (QuestionStatus::Answered, answers.clone())
            }
        };
        if entry.record.status != QuestionStatus::Pending {
            ensure!(
                entry.record.status == status && entry.record.answers == answers,
                "Question is already resolved"
            );
            return Ok(entry.record.clone());
        }
        let mut record = entry.record.clone();
        record.status = status;
        record.answers = answers;
        record.resolved_at = Some(chrono::Utc::now().timestamp_millis());
        self.persist(&record)?;
        entry.record = record.clone();
        entry.changed.send_replace(record.clone());
        let _ = self.events.send(record.clone());
        Ok(record)
    }
    pub async fn ask(
        self: &Arc<Self>,
        session: &str,
        execution: &str,
        call: &str,
        args: AskQuestionArgs,
        waiter: Option<&dyn QuestionWaiter>,
    ) -> Result<QuestionRecord> {
        validate(&args)?;
        let (id, mut changed) = {
            let mut entries = self.entries.lock().unwrap();
            ensure!(
                !entries
                    .values()
                    .any(|entry| entry.record.session_id == session
                        && entry.record.status == QuestionStatus::Pending),
                "This session already has a pending question"
            );
            ensure!(
                !entries
                    .values()
                    .any(|entry| entry.record.session_id == session
                        && entry.record.execution_id == execution
                        && entry.record.tool_call_id == call),
                "This question call has already been handled"
            );
            let record = QuestionRecord {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: session.into(),
                execution_id: execution.into(),
                tool_call_id: call.into(),
                questions: args.questions,
                status: QuestionStatus::Pending,
                answers: BTreeMap::new(),
                created_at: chrono::Utc::now().timestamp_millis(),
                resolved_at: None,
            };
            self.persist(&record)?;
            let (sender, receiver) = watch::channel(record.clone());
            entries.insert(
                record.id.clone(),
                Entry {
                    record: record.clone(),
                    changed: sender,
                },
            );
            let _ = self.events.send(record.clone());
            (record.id, receiver)
        };
        let mut guard = PendingQuestion {
            manager: self.clone(),
            id,
            armed: true,
        };
        if let Some(waiter) = waiter {
            waiter.suspend().await?;
        }
        let record = changed
            .wait_for(|record| record.status != QuestionStatus::Pending)
            .await?
            .clone();
        if let Some(waiter) = waiter {
            waiter.resume().await?;
        }
        guard.armed = false;
        Ok(record)
    }
    fn cancel(&self, id: &str) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries
            .get_mut(id)
            .filter(|entry| entry.record.status == QuestionStatus::Pending)
        {
            let mut record = entry.record.clone();
            record.status = QuestionStatus::Cancelled;
            record.resolved_at = Some(chrono::Utc::now().timestamp_millis());
            if self.persist(&record).is_err() {
                tracing::error!("Could not persist question cancellation");
            }
            entry.record = record.clone();
            entry.changed.send_replace(record.clone());
            let _ = self.events.send(record);
        }
    }
}
struct PendingQuestion {
    manager: Arc<QuestionManager>,
    id: String,
    armed: bool,
}
impl Drop for PendingQuestion {
    fn drop(&mut self) {
        if self.armed {
            self.manager.cancel(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args() -> AskQuestionArgs {
        AskQuestionArgs {
            questions: vec![Question {
                id: "format".into(),
                question: "Choose output format".into(),
                kind: QuestionType::SingleSelect,
                options: vec![QuestionOption {
                    id: "pdf".into(),
                    label: "PDF".into(),
                    recommended: true,
                }],
            }],
        }
    }
    async fn pending(manager: &QuestionManager) -> QuestionRecord {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(record) = manager
                    .list("session")
                    .into_iter()
                    .find(|record| record.status == QuestionStatus::Pending)
                {
                    return record;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn answers_are_validated_scoped_durable_and_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let manager = Arc::new(QuestionManager::persistent(root.path().into()).unwrap());
        let task = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .ask("session", "execution", "call", args(), None)
                    .await
                    .unwrap()
            })
        };
        let record = pending(&manager).await;
        let invalid = QuestionResponse::Submit {
            answers: BTreeMap::from([(
                "format".into(),
                QuestionAnswer {
                    selected: vec!["not-an-option".into()],
                    text: String::new(),
                },
            )]),
        };
        assert!(manager.respond("session", &record.id, &invalid).is_err());
        assert!(
            manager
                .respond("other", &record.id, &QuestionResponse::Cancel)
                .is_err()
        );
        let response = QuestionResponse::Submit {
            answers: BTreeMap::from([(
                "format".into(),
                QuestionAnswer {
                    selected: vec![],
                    text: "PowerPoint".into(),
                },
            )]),
        };
        let answer = manager.respond("session", &record.id, &response).unwrap();
        assert_eq!(answer.status, QuestionStatus::Answered);
        assert_eq!(
            manager.respond("session", &record.id, &response).unwrap(),
            answer
        );
        assert!(
            manager
                .respond("session", &record.id, &QuestionResponse::Cancel)
                .is_err()
        );
        assert_eq!(task.await.unwrap(), answer);
        drop(manager);
        let manager = QuestionManager::persistent(root.path().into()).unwrap();
        assert_eq!(manager.list("session"), vec![answer]);
        manager.forget_session("session").unwrap();
        assert!(manager.list("session").is_empty());
    }
    #[tokio::test]
    async fn dropped_waiters_cancel_and_crash_snapshots_become_interrupted() {
        let root = tempfile::tempdir().unwrap();
        let crashed = tempfile::tempdir().unwrap();
        let manager = Arc::new(QuestionManager::persistent(root.path().into()).unwrap());
        let task = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .ask("session", "execution", "call", args(), None)
                    .await
            })
        };
        let record = pending(&manager).await;
        std::fs::copy(
            root.path().join(format!("{}.json", record.id)),
            crashed.path().join(format!("{}.json", record.id)),
        )
        .unwrap();
        assert!(
            manager
                .ask("session", "other", "call-2", args(), None)
                .await
                .is_err()
        );
        task.abort();
        let _ = task.await;
        assert_eq!(manager.list("session")[0].status, QuestionStatus::Cancelled);
        let recovered = QuestionManager::persistent(crashed.path().into()).unwrap();
        assert_eq!(
            recovered.list("session")[0].status,
            QuestionStatus::Interrupted
        );
        assert!(
            recovered
                .respond("session", &record.id, &QuestionResponse::Cancel)
                .is_err()
        );
    }
    #[test]
    fn question_and_answer_limits_reject_ambiguous_payloads() {
        let mut request = args();
        request.questions.push(request.questions[0].clone());
        assert!(validate(&request).is_err());
        let mut request = args();
        request.questions[0].options.clear();
        assert!(validate(&request).is_err());
        request.questions[0].kind = QuestionType::Text;
        assert!(validate(&request).is_ok());
    }
}
