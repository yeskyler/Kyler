//! Task snapshots and explicit user controls; no new command execution is accepted here.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use kyler_core::{context::ContentItem, sandbox::ProcessInput};
use kyler_protocol::api::{
    processes::ProcessInfo,
    tasks::{TaskAction, TaskInfo, TaskKind},
};
use std::collections::{HashMap, HashSet};

fn command(info: ProcessInfo, session: &str) -> TaskInfo {
    let status = if info.status == "completed"
        && info
            .metadata
            .as_ref()
            .is_some_and(|metadata| metadata.exit_code != Some(0))
    {
        "failed".into()
    } else {
        info.status
    };
    TaskInfo {
        id: info.id,
        kind: TaskKind::Command,
        session_id: session.into(),
        name: info
            .metadata
            .as_ref()
            .map(|metadata| metadata.command.clone())
            .unwrap_or_else(|| "Command".into()),
        status,
        started_at: info.metadata.as_ref().map(|metadata| metadata.started_at),
        finished_at: info
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.finished_at),
        working_directory: info
            .metadata
            .as_ref()
            .map(|metadata| metadata.working_directory.clone()),
        exit_code: info.metadata.and_then(|metadata| metadata.exit_code),
        total_tokens: None,
        output: None,
        output_truncated: false,
        error: info.error,
        children: Vec::new(),
    }
}
async fn snapshot(state: &AppState, session: &str) -> ApiResult<Vec<TaskInfo>> {
    valid_id(session)?;
    let saved = state
        .runs
        .store
        .load(session)
        .await
        .map_err(ApiError::storage)?;
    let mut agents = saved.subagent_tasks.clone();
    let mut live = HashSet::new();
    if let Some(manager) = state.runs.agent.subagent_manager() {
        agents.extend(manager.list_session(session).into_iter().map(|task| {
            live.insert(task.task_id.clone());
            (task.task_id.clone(), task)
        }));
    }
    let goals = saved
        .events
        .iter()
        .flat_map(|event| &event.content)
        .filter_map(|item| match item {
            ContentItem::ToolCall {
                tool_call_id,
                name,
                arguments,
            } if name == "delegate_task" => arguments["task"]
                .as_str()
                .map(|goal| (tool_call_id.clone(), goal.to_owned())),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let mut tasks = state
        .runs
        .processes
        .list(session)
        .into_iter()
        .map(|info| command(info, session))
        .collect::<Vec<_>>();
    for task in agents
        .into_values()
        .filter(|task| task.parent_session_id == session)
    {
        let status = format!("{:?}", task.status).to_ascii_lowercase();
        let unconfirmed =
            !live.contains(&task.task_id) && matches!(status.as_str(), "queued" | "running");
        let output_truncated = task.output.as_ref().is_some_and(|text| text.len() > 32000);
        tasks.push(TaskInfo {
            id: task.task_id,
            kind: TaskKind::Agent,
            session_id: session.into(),
            name: goals
                .get(&task.parent_tool_call_id)
                .map(|text| {
                    text.split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(160)
                        .collect()
                })
                .unwrap_or_else(|| "Delegated task".into()),
            status: if unconfirmed {
                "interrupted".into()
            } else {
                status
            },
            started_at: task.started_at.or(Some(task.created_at)),
            finished_at: task.finished_at,
            working_directory: saved
                .environment
                .as_ref()
                .map(|environment| environment.working_directory.to_string_lossy().into_owned()),
            exit_code: None,
            total_tokens: Some(u64::from(task.usage.total_tokens)),
            output: task
                .output
                .map(|text| crate::runs::events::prefix(&text, 32000)),
            output_truncated,
            error: if unconfirmed {
                Some("Task is no longer running on this server; its outcome is unknown.".into())
            } else {
                task.error
                    .map(|error| crate::runs::events::prefix(&error, 1000))
            },
            children: state
                .runs
                .processes
                .list(&task.session_id)
                .into_iter()
                .map(|info| command(info, &task.session_id))
                .collect(),
        });
    }
    tasks.sort_by(|a, b| {
        b.active()
            .cmp(&a.active())
            .then_with(|| b.started_at.cmp(&a.started_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(tasks)
}
fn find(tasks: Vec<TaskInfo>, kind: TaskKind, id: &str) -> Option<TaskInfo> {
    tasks.into_iter().find_map(|task| {
        if task.id == id && task.kind == kind {
            Some(task)
        } else {
            find(task.children, kind, id)
        }
    })
}
fn kind(value: &str) -> ApiResult<TaskKind> {
    match value {
        "command" => Ok(TaskKind::Command),
        "agent" => Ok(TaskKind::Agent),
        _ => Err(ApiError::missing()),
    }
}
pub async fn list(
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> ApiResult<Json<Vec<TaskInfo>>> {
    let mut tasks = snapshot(&state, &session).await?;
    for task in &mut tasks {
        task.output = None;
    }
    Ok(Json(tasks))
}
pub async fn get(
    State(state): State<AppState>,
    Path((session, category, id)): Path<(String, String, String)>,
) -> ApiResult<Json<TaskInfo>> {
    valid_id(&id)?;
    let mut task = find(snapshot(&state, &session).await?, kind(&category)?, &id)
        .ok_or_else(ApiError::missing)?;
    if task.kind == TaskKind::Command {
        let info = state
            .runs
            .processes
            .read(&task.session_id, &id, 0)
            .map_err(|_| ApiError::missing())?;
        let output = info
            .output
            .iter()
            .filter(|chunk| chunk.stream != "status")
            .map(|chunk| chunk.text.as_str())
            .collect::<String>();
        let mut start = output.len().saturating_sub(16000);
        while !output.is_char_boundary(start) {
            start += 1;
        }
        task.output_truncated = start > 0 || info.oldest_cursor > 1;
        task.output = Some(output[start..].into());
    }
    Ok(Json(task))
}
pub async fn act(
    State(state): State<AppState>,
    Path((session, category, id)): Path<(String, String, String)>,
    Json(action): Json<TaskAction>,
) -> ApiResult<Json<TaskInfo>> {
    valid_id(&id)?;
    let task = find(snapshot(&state, &session).await?, kind(&category)?, &id)
        .ok_or_else(ApiError::missing)?;
    match (task.kind, action) {
        (TaskKind::Command, TaskAction::Stop) => state
            .runs
            .processes
            .stop(&task.session_id, &id)
            .map_err(ApiError::storage)?,
        (TaskKind::Agent, TaskAction::Stop) => state
            .runs
            .agent
            .subagent_manager()
            .ok_or_else(ApiError::missing)?
            .request_cancel(&session, &id)
            .map_err(|_| ApiError::missing())?,
        (TaskKind::Command, TaskAction::Input { data, close }) => {
            if data.len() > 4096 {
                return Err(ApiError::bad("Input exceeds 4 KiB."));
            }
            state
                .runs
                .processes
                .write(&task.session_id, &id, ProcessInput { data, close })
                .map_err(|_| ApiError::bad("Process input is unavailable."))?;
        }
        _ => return Err(ApiError::bad("Only commands accept input.")),
    }
    get(State(state), Path((session, category, id))).await
}
