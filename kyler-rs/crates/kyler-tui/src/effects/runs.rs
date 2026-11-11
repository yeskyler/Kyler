use super::{Effects, Event, Sender};
use crate::agent::{AgentEvent, AgentHandle};
use crate::state::Submission;

use kyler_protocol::api::ApiErrorBody;

impl Effects {
    pub fn start(
        &mut self,
        client: AgentHandle,
        session: String,
        submission: Submission,
        generation: u64,
        send: Sender,
    ) {
        self.spawn_stream(async move {
            let mut attachments = Vec::new();
            for attachment in &submission.attachments {
                let local = attachment.clone();
                let bytes = match tokio::task::spawn_blocking(move || local.read_bytes()).await {
                    Ok(Ok(bytes)) => bytes,
                    _ => {
                        let _ = send
                            .send((
                                generation,
                                Event::Error(
                                    "Cannot read attachment; retry or start a new session.".into(),
                                ),
                            ))
                            .await;
                        return;
                    }
                };
                match client
                    .upload_attachment(&session, &attachment.name, bytes)
                    .await
                {
                    Ok(info) => attachments.push(info),
                    Err(error) => {
                        let _ = send
                            .send((generation, Event::Error(error.to_string())))
                            .await;
                        return;
                    }
                }
            }

            match client
                .start_turn(
                    &session,
                    if submission.resume {
                        String::new()
                    } else {
                        submission.input
                    },
                    attachments,
                    submission.resume,
                )
                .await
            {
                Ok(run) => {
                    if send
                        .send((generation, Event::Accepted(run.clone())))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    watch(client, session, run, generation, send).await;
                }
                Err(error) => {
                    let _ = send
                        .send((
                            generation,
                            Event::Rejected(ApiErrorBody {
                                code: "local_submission_failed".into(),
                                message: error.to_string(),
                            }),
                        ))
                        .await;
                }
            }
        });
    }
    pub fn cancel(&mut self, client: AgentHandle, run: String, generation: u64, send: Sender) {
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
    client: AgentHandle,
    session: String,
    run: String,
    generation: u64,
    send: Sender,
) {
    let result = async {
        let mut stream = client.watch(&run).await?;
        while let Some(event) = stream.next().await? {
            let finished = matches!(event, AgentEvent::Result(_));
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
        Err(anyhow::anyhow!(
            "Stream ended without a result.",
        ))
    }
    .await;
    if let Err(error) = result {
        let _ = send
            .send((
                generation,
                Event::Error(format!(
                    "Local agent event loop ended: {error}. Start a new session to continue."
                )),
            ))
            .await;
    }
}
