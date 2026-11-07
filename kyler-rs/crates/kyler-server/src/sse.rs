use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};

use std::{convert::Infallible, time::Duration};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};

pub async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let mut cursor = match headers.get("last-event-id") {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| ApiError::bad("Last-Event-ID must be an unsigned integer."))?,
        None => 0,
    };
    let entry = state.runs.entry(&id).await?;
    entry.after(cursor)?;
    let permit = entry.subscribers.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "subscriber_limit",
            "This run has too many event subscribers.",
        )
    })?;
    let mut updates = entry.changed.subscribe();
    let mut stopping = state.runs.stopping.subscribe();
    let stream = async_stream::stream! {
        let _permit=permit;
        loop {
            if *stopping.borrow() {break;}
            updates.borrow_and_update();
            let (batch,terminal)=match entry.after(cursor) {
                Ok(view)=>view,
                Err(error)=> {yield Ok::<Event,Infallible>(Event::default().event("stream_error").data(serde_json::to_string(&error.body).expect("serializable API error")));break;}
            };
            let had_events=!batch.is_empty();
            for item in batch {
                cursor=item.seq;
                yield Ok::<Event,Infallible>(Event::default().id(item.seq.to_string()).event(item.payload.name()).data(serde_json::to_string(item.as_ref()).expect("serializable SSE event")));
            }
            if terminal {break;}
            if had_events {continue;}
            tokio::select! { _=updates.changed()=>{}, _=stopping.changed()=>break }
        }
    };
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}
