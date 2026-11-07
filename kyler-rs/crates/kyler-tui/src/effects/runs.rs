use super::{Effects, Event, Sender};
use crate::state::Submission;

use kyler_client::{Client, ClientEvent, ErrorKind};
use kyler_protocol::api::{ApiErrorBody, run::CreateRunRequest};

impl Effects {
    pub fn start(
        &mut self,
        client: Client,
        session: String,
        submission: Submission,
        generation: u64,
        send: Sender,
    ) {
        self.spawn_stream(async move {
            let mut attachment_ids=Vec::new();
            for attachment in &submission.attachments {
                let local=attachment.clone();
                let bytes=match tokio::task::spawn_blocking(move||local.read_bytes()).await {
                    Ok(Ok(bytes))=>bytes,
                    _=>{ let _=send.send((generation,Event::Error("Cannot read attachment; retry or start a new session.".into()))).await;return; }
                };
                match client.upload_attachment(&session,&attachment.name,bytes).await {
                    Ok(info)=>attachment_ids.push(info.id),
                    Err(error)=>{let _=send.send((generation,Event::Error(error.to_string()))).await;return;}
                }
            }

            match client
                .start_run(
                    &session,
                    &CreateRunRequest {
                        attachment_ids,
                        resume: submission.resume,
                        images: Vec::new(),
                        input: if submission.resume {String::new()} else {submission.input},
                        client_request_id: Some(submission.key),
                    },
                )
                .await
            {
                Ok(run) => {
                    if send
                        .send((generation, Event::Accepted(run.run_id.clone())))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    watch(client, session, run.run_id, 0, generation, send).await;
                }
                Err(error) if error.kind==ErrorKind::Api && error.status.is_some_and(|status|status<500 && status!=408 && status!=429) => {
                    let _=send.send((generation,Event::Rejected(ApiErrorBody{code:error.code,message:error.message}))).await;
                }
                Err(error) => {
                    let _ = send
                        .send((
                            generation,
                            Event::Error(format!(
                                "Submission unconfirmed: {error}. F5 retries with the same request ID; no automatic resubmission."
                            )),
                        ))
                        .await;
                }
            }
        });
    }
    pub fn watch(
        &mut self,
        client: Client,
        session: String,
        run: String,
        cursor: u64,
        generation: u64,
        send: Sender,
    ) {
        self.spawn_stream(watch(client, session, run, cursor, generation, send));
    }
    pub fn cancel(&mut self, client: Client, run: String, generation: u64, send: Sender) {
        self.spawn(async move {
            let event = match client.cancel(&run).await {
                Ok(_) => Event::Cancelled,
                Err(error) => Event::CancelError(format!(
                    "Cancellation unconfirmed: {error}; the task may still be running."
                )),
            };
            let _ = send.send((generation, event)).await;
        });
    }
}

pub(super) async fn watch(
    client: Client,
    session: String,
    run: String,
    cursor: u64,
    generation: u64,
    send: Sender,
) {
    let result = async {
        let mut stream = client.watch(&run, cursor).await?;
        while let Some(event) = stream.next().await? {
            let finished = matches!(event, ClientEvent::Result(_));
            if send.send((generation, Event::Stream(event))).await.is_err() {
                return Ok(());
            }
            if finished {
                match client.session_usage(&session).await {
                    Ok(usage) => {
                        let _ = send.send((generation, Event::Usage(usage))).await;
                    }
                    Err(error) => {
                        let _ = send
                            .send((
                                generation,
                                Event::CancelError(format!(
                                    "Usage refresh failed: {error}; showing the last confirmed value."
                                )),
                            ))
                            .await;
                    }
                }
                return Ok(());
            }
        }
        Err(kyler_client::Error::protocol(
            "Stream ended without a result.",
        ))
    }
    .await;
    if let Err(error) = result {
        let _ = send
            .send((
                generation,
                Event::Error(format!(
                    "Stream interrupted: {error}. Run: {run}; F5 to reconnect."
                )),
            ))
            .await;
    }
}
