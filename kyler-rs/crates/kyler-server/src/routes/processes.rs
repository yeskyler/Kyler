use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, Query, State},
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
};

use kyler_protocol::api::processes::ProcessInfo;

#[derive(serde::Deserialize, Default)]
pub struct Cursor {
    #[serde(default)]
    pub cursor: u64,
}
pub async fn list(
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> ApiResult<Json<Vec<ProcessInfo>>> {
    valid_id(&session)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    Ok(Json(state.runs.processes.list(&session)))
}
pub async fn get(
    State(state): State<AppState>,
    Path((session, id)): Path<(String, String)>,
    Query(query): Query<Cursor>,
) -> ApiResult<Json<ProcessInfo>> {
    valid_id(&session)?;
    valid_id(&id)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    Ok(Json(
        state
            .runs
            .processes
            .read(&session, &id, query.cursor)
            .map_err(|_| ApiError::missing())?,
    ))
}
pub async fn stop(
    State(state): State<AppState>,
    Path((session, id)): Path<(String, String)>,
) -> ApiResult<axum::http::StatusCode> {
    valid_id(&session)?;
    valid_id(&id)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    state
        .runs
        .processes
        .stop(&session, &id)
        .map_err(|_| ApiError::missing())?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn events(
    State(state): State<AppState>,
    Path((session, id)): Path<(String, String)>,
    Query(query): Query<Cursor>,
    headers: axum::http::HeaderMap,
) -> ApiResult<axum::response::Response> {
    valid_id(&session)?;
    valid_id(&id)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    let mut receiver = state.runs.processes.subscribe();
    let manager = state.runs.processes.clone();
    let cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(query.cursor)
        .max(query.cursor);
    let initial = manager
        .read(&session, &id, cursor)
        .map_err(|_| ApiError::missing())?;
    Ok(Sse::new(async_stream::stream!{
        let mut cursor=cursor;
        for chunk in initial.output{cursor=cursor.max(chunk.cursor);yield Ok::<_,std::convert::Infallible>(Event::default().event("process_output").id(chunk.cursor.to_string()).json_data(chunk).unwrap());}
        if initial.status!="running"{yield Ok(Event::default().event("process_status").data(initial.status));return;}
        loop{let event=tokio::select!{
            event=receiver.recv()=>event,
            _=tokio::time::sleep(std::time::Duration::from_secs(1))=>{
                if let Ok(info)=manager.read(&session,&id,cursor){if info.status!="running"{yield Ok(Event::default().event("process_status").data(info.status));return;}}else{return;}continue;
            }
        };match event{
            Ok(chunk) if chunk.session_id==session && chunk.process_id==id && chunk.cursor>cursor=>{
                cursor=chunk.cursor;let terminal=chunk.stream=="status";
                yield Ok(Event::default().event("process_output").id(cursor.to_string()).json_data(chunk).unwrap());if terminal{return;}
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{yield Ok(Event::default().event("gap").data("Output expired; query the process with its last cursor."));return;}
            Err(_)=>return,
            _=>{}
        }}
    }).keep_alive(KeepAlive::default()).into_response())
}
