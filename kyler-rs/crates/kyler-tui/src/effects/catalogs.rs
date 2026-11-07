use super::{Effects, Event, Sender};

use std::time::Duration;

use kyler_client::Client;
use kyler_protocol::api::{mcp::McpAction, skills::SkillAction};

impl Effects {
    pub fn models(
        &mut self,
        client: Client,
        session: String,
        generation: u64,
        send: Sender,
        reload: bool,
        selection: Option<Option<String>>,
    ) {
        self.spawn(async move {
            let result = async {
                let catalog = client.models(reload).await?;
                let model = match selection {
                    Some(profile) => client.select_model(&session, profile).await?,
                    None => client.session_model(&session).await?,
                };
                Ok::<_, kyler_client::Error>((catalog, model))
            }
            .await;
            let event = match result {
                Ok((catalog, model)) => Event::Models(catalog, model),
                Err(e) => Event::ModelsError(e.to_string()),
            };
            let _ = send.send((generation, event)).await;
        });
    }
    pub fn skills(
        &mut self,
        client: Client,
        session: String,
        generation: u64,
        send: Sender,
        reload: bool,
    ) {
        self.spawn(async move {
            let event = match client.skills(&session, reload).await {
                Ok(skills) => Event::Skills(skills),
                Err(error) => Event::SkillsError(error.to_string()),
            };
            let _ = send.send((generation, event)).await;
        });
    }
    pub fn skill_details(
        &mut self,
        client: Client,
        session: String,
        skill: String,
        generation: u64,
        send: Sender,
    ) {
        self.spawn(async move {
            let event = match client.skill_details(&session, &skill).await {
                Ok(details) => Event::SkillDetails(details),
                Err(error) => Event::SkillsError(error.to_string()),
            };
            let _ = send.send((generation, event)).await;
        });
    }
    pub fn skill_action(
        &mut self,
        client: Client,
        session: String,
        skill: String,
        action: SkillAction,
        fingerprint: String,
        generation: u64,
        send: Sender,
    ) {
        self.spawn(async move {
            let result = async {
                client
                    .skill_action(&session, &skill, action, fingerprint)
                    .await?;
                client.skills(&session, false).await
            }
            .await;
            let event = match result {
                Ok(skills) => Event::Skills(skills),
                Err(error) => Event::SkillsError(error.to_string()),
            };
            let _ = send.send((generation, event)).await;
        });
    }
    pub fn watch_mcp(&mut self, client: Client, session: String, generation: u64, send: Sender) {
        if let Some(previous) = self.mcp.take() {
            previous.abort();
        }
        self.mcp = Some(tokio::spawn(async move {
            loop {
                let event = match client.mcp_servers(&session).await {
                    Ok(servers) => Event::Mcp(servers),
                    Err(_) => Event::McpError(
                        "MCP status unavailable; verify the server version and connection.".into(),
                    ),
                };
                if send.send((generation, event)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }));
    }
    pub fn mcp_action(
        &mut self,
        client: Client,
        session: String,
        server: String,
        action: McpAction,
        generation: u64,
        send: Sender,
    ) {
        self.spawn(async move {
            if client.mcp_action(&session, &server, action).await.is_err() {
                let _ = send
                    .send((
                        generation,
                        Event::McpError(
                            "MCP action failed. Check server status and try again.".into(),
                        ),
                    ))
                    .await;
            }
        });
    }
}
