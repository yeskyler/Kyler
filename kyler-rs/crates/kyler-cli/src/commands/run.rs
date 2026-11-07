use crate::{CliError, Result, output::Output};

use std::time::Duration;

use tokio::time::Instant;

use kyler_client::{Client, ClientEvent};
use kyler_protocol::api::run::{CreateRunRequest, RunStatus};

pub async fn session(client: &Client, id: Option<String>) -> Result<String> {
    let session = match id {
        Some(id) => client.session(&id).await?,
        None => {
            client
                .create_session_in(&std::env::current_dir().map_err(CliError::io)?)
                .await?
        }
    };
    eprintln!("Session: {}", session.session_id);
    Ok(session.session_id)
}
pub async fn start(
    client: &Client,
    session: &str,
    input: String,
    key: Option<String>,
) -> Result<String> {
    let key = key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    eprintln!("Request: {key}");
    let accepted = client
        .start_run(
            session,
            &CreateRunRequest {
                attachment_ids: Vec::new(),
                resume: false,
                images: Vec::new(),
                input,
                client_request_id: Some(key),
            },
        )
        .await?;
    eprintln!("Run: {}", accepted.run_id);
    Ok(accepted.run_id)
}
pub async fn watch(
    client: &Client,
    id: &str,
    after: u64,
    json: bool,
    final_only: bool,
) -> Result<()> {
    let mut stream = client.watch(id, after).await?;
    let mut output = Output::new(json, final_only, after > 0);
    let mut deadline = None;
    loop {
        let event = tokio::select! {
            result=stream.next()=>match result {Ok(event)=>event,Err(error)=>{
                output.newline()?;
                return Err(if deadline.is_some() {CliError::new(130,format!("Cancellation could not be confirmed for run {id}: {error}"))} else {error.into()});
            }},
            signal=tokio::signal::ctrl_c()=>{
                signal.map_err(|e|CliError::new(3,e.to_string()))?;
                output.newline()?;
                if deadline.is_some() {return Err(CliError::new(130,format!("Stopped waiting for run {id}; cancellation is not confirmed.")));}
                let until=Instant::now()+Duration::from_secs(client.settings().cancel_timeout_seconds);
                eprintln!("Cancelling run {id}… (Ctrl+C again to stop waiting)");
                let cancelled=tokio::select! {
                    result=client.cancel(id)=>Some(result),
                    _=tokio::signal::ctrl_c()=>None,
                    _=tokio::time::sleep_until(until)=>None,
                };
                match cancelled {
                    Some(Ok(_))=>deadline=Some(until),
                    Some(Err(error))=>return Err(CliError::new(130,format!("Cancellation request failed for run {id}: {error}. The run may still be active."))),
                    None=>return Err(CliError::new(130,format!("Cancellation is not confirmed for run {id}."))),
                }
                continue;
            },
            _=async {if let Some(until)=deadline {tokio::time::sleep_until(until).await} else {std::future::pending::<()>().await}}=>{
                output.newline()?;
                return Err(CliError::new(130,format!("Timed out waiting for cancellation of run {id}; check its status.")));
            }
        };
        let Some(event) = event else {
            return Err(CliError::new(
                3,
                format!("Run {id} ended without a final result."),
            ));
        };
        output.event(&event)?;
        if let ClientEvent::Result(result) = event {
            return match result.status {
                RunStatus::Completed => Ok(()),
                RunStatus::Cancelled => Err(CliError::new(130, format!("Run {id} cancelled."))),
                RunStatus::Failed => Err(CliError::new(
                    1,
                    result
                        .error
                        .map(|e| format!("{}: {}", e.code, e.message))
                        .unwrap_or_else(|| format!("Run {id} failed.")),
                )),
                _ => Err(CliError::new(3, "Received a nonterminal result.")),
            };
        }
    }
}
