//! State reconstruction only: never replays external side effects.
use super::{
    reader::RolloutReader,
    record::{
        RolloutPayload::{
            self, CheckpointCommitted, ContextCompacted, ContextUpdated, EnvironmentUpdated,
            EventRecorded, ExecutionArchived, ExecutionFinished, ExecutionStarted,
            ExecutionUpdated, SessionCreated, StateUpdated, SubagentRemoved, SubagentUpdated,
            UsageUpdated,
        },
        RolloutRecord,
    },
};
use crate::{context::ExecutionStatus, session::Session};

use std::collections::HashMap;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

fn same(a: &impl Serialize, b: &impl Serialize) -> Result<bool> {
    Ok(serde_json::to_value(a)? == serde_json::to_value(b)?)
}

pub(crate) fn changes(previous: Option<&Session>, next: &Session) -> Result<Vec<RolloutPayload>> {
    next.validate()?;
    let mut records = Vec::new();
    let Some(old) = previous else {
        let mut baseline = Session::new();
        baseline.author = next.author.clone();
        baseline.session_id = next.session_id.clone();
        baseline.schema_version = next.schema_version;
        baseline.created_at = next.created_at;
        baseline.updated_at = next.updated_at;
        baseline.revision = next.revision;
        records.push(SessionCreated {
            session: Box::new(baseline.clone()),
        });
        records.extend(changes(Some(&baseline), next)?);
        return Ok(records);
    };
    ensure!(
        old.author == next.author
            && old.session_id == next.session_id
            && old.schema_version == next.schema_version
            && old.created_at == next.created_at,
        "immutable session metadata changed"
    );
    ensure!(
        next.events.len() >= old.events.len()
            && same(&old.events, &next.events[..old.events.len()].to_vec())?,
        "committed events cannot be rewritten or removed"
    );
    ensure!(
        next.executions.len() >= old.executions.len()
            && same(
                &old.executions,
                &next.executions[..old.executions.len()].to_vec()
            )?,
        "archived executions cannot be rewritten or removed"
    );
    for event in &next.events[old.events.len()..] {
        records.push(EventRecorded {
            event: event.clone(),
        });
    }
    for execution in &next.executions[old.executions.len()..] {
        records.push(ExecutionArchived {
            execution: execution.clone(),
        });
    }
    if !same(&old.execution, &next.execution)? {
        if let Some(execution) = &next.execution {
            let payload = if execution.status != ExecutionStatus::Running {
                ExecutionFinished {
                    execution: execution.clone(),
                }
            } else if old.execution.as_ref().map(|e| &e.execution_id)
                != Some(&execution.execution_id)
            {
                ExecutionStarted {
                    execution: execution.clone(),
                }
            } else {
                ExecutionUpdated {
                    execution: execution.clone(),
                }
            };
            records.push(payload);
        } else {
            ensure!(
                old.execution.as_ref().is_none_or(|e| next
                    .executions
                    .iter()
                    .any(|a| a.execution_id == e.execution_id)),
                "current execution cannot be discarded without archiving"
            );
        }
    }
    if !same(&old.memory, &next.memory)? {
        records.push(
            if next.memory.compression_count > old.memory.compression_count {
                ContextCompacted {
                    memory: next.memory.clone(),
                }
            } else {
                ContextUpdated {
                    memory: next.memory.clone(),
                }
            },
        );
    }
    let set = next
        .state
        .iter()
        .filter(|(key, value)| old.state.get(*key) != Some(*value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();
    let removed = old
        .state
        .keys()
        .filter(|key| !next.state.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    if !set.is_empty() || !removed.is_empty() {
        records.push(StateUpdated { set, removed });
    }
    if !same(&old.usage, &next.usage)? || !same(&old.subagent_usage, &next.subagent_usage)? {
        records.push(UsageUpdated {
            usage: next.usage,
            subagent_usage: next.subagent_usage,
        });
    }
    if !same(&old.environment, &next.environment)? || !same(&old.parent, &next.parent)? {
        records.push(EnvironmentUpdated {
            environment: next.environment.clone(),
            parent: next.parent.clone(),
        });
    }
    for (id, task) in &next.subagent_tasks {
        if !same(&old.subagent_tasks.get(id), &Some(task))? {
            records.push(SubagentUpdated { task: task.clone() });
        }
    }
    for id in old
        .subagent_tasks
        .keys()
        .filter(|id| !next.subagent_tasks.contains_key(*id))
    {
        records.push(SubagentRemoved {
            task_id: id.clone(),
        });
    }
    Ok(records)
}

pub(crate) fn apply(session: &mut Option<Session>, record: RolloutRecord) -> Result<()> {
    if let SessionCreated { session: initial } = record.payload {
        ensure!(session.is_none(), "duplicate session_created");
        ensure!(
            initial.session_id == record.session_id,
            "initial session ID mismatch"
        );
        initial.validate()?;
        *session = Some(*initial);
        return Ok(());
    }
    let session = session.as_mut().context("rollout has no session_created")?;
    ensure!(
        session.session_id == record.session_id,
        "rollout session ID mismatch"
    );
    match record.payload {
        EventRecorded { event } => {
            ensure!(
                !session.events.iter().any(|e| e.id == event.id),
                "duplicate event ID"
            );
            session.events.push(event);
        }
        ExecutionStarted { execution }
        | ExecutionUpdated { execution }
        | ExecutionFinished { execution } => session.execution = Some(execution),
        ExecutionArchived { execution } => {
            ensure!(
                !session
                    .executions
                    .iter()
                    .any(|e| e.execution_id == execution.execution_id),
                "duplicate archived execution"
            );
            if session
                .execution
                .as_ref()
                .is_some_and(|e| e.execution_id == execution.execution_id)
            {
                session.execution = None;
            }
            session.executions.push(execution);
        }
        ContextCompacted { memory } | ContextUpdated { memory } => session.memory = memory,
        StateUpdated { set, removed } => {
            for key in removed {
                session.state.remove(&key);
            }
            session.state.extend(set);
        }
        UsageUpdated {
            usage,
            subagent_usage,
        } => {
            session.usage = usage;
            session.subagent_usage = subagent_usage;
        }
        EnvironmentUpdated {
            environment,
            parent,
        } => {
            session.environment = environment;
            session.parent = parent;
        }
        SubagentUpdated { task } => {
            session.subagent_tasks.insert(task.task_id.clone(), task);
        }
        SubagentRemoved { task_id } => {
            session.subagent_tasks.remove(&task_id);
        }
        CheckpointCommitted {
            revision,
            updated_at,
        } => {
            ensure!(
                session.revision.checked_add(1) == Some(revision),
                "checkpoint revision gap"
            );
            session.revision = revision;
            session.updated_at = updated_at;
            session.validate()?;
        }
        SessionCreated { .. } => unreachable!(),
    }
    Ok(())
}

pub async fn replay(reader: &mut RolloutReader) -> Result<Option<Session>> {
    let mut session = None;
    while let Some(record) = reader.next().await? {
        apply(&mut session, record)?;
    }
    Ok(session)
}

pub(crate) fn replay_from(
    reader: &mut RolloutReader,
    mut session: Option<Session>,
) -> Result<Option<Session>> {
    while let Some(record) = reader.read_next()? {
        apply(&mut session, record)?;
    }
    Ok(session)
}
