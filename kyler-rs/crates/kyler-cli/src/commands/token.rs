//! Explicit local host administration; never sends a credential to a model.
use crate::{
    CliError, Result,
    args::{GatewayCommand, TokenCommand},
    output,
};

use std::path::Path;

use kyler_config::{AppConfig, SecretString};
use kyler_gateway::security::{IssuedToken, Security};
use kyler_protocol::api::gateway::{CreateToken, Principal, Role, Scope};

fn failure(error: anyhow::Error) -> CliError {
    CliError::new(2, error.to_string())
}
async fn open(explicit: Option<&Path>) -> Result<Security> {
    let config = kyler_config::loader::load_for_setup(explicit).map_err(failure)?;
    tokio::task::spawn_blocking(move || Security::open(&config.session.directory))
        .await
        .map_err(|_| CliError::new(3, "Gateway storage unavailable"))?
        .map_err(failure)
}
fn issued(token: IssuedToken, json: bool) -> Result<()> {
    if json {
        output::json(&serde_json::json!({"token":token.secret,"info":token.info}))
    } else {
        output::text(&format!(
            "ID:{}\nIdentity:{}\nRole:{}\nToken:{}\nSave this token now; its secret cannot be retrieved again.\n",
            token.info.id,
            token.info.principal_id,
            serde_json::to_value(token.info.role)
                .unwrap()
                .as_str()
                .unwrap(),
            token.secret
        ))
    }
}
pub async fn execute(explicit: Option<&Path>, command: &TokenCommand, json: bool) -> Result<()> {
    let security = open(explicit).await?;
    let actor = Principal::local();
    match command {
        TokenCommand::Create {
            name,
            expires_in_seconds,
            no_expiry,
        } => {
            let seconds = expires_in_seconds.unwrap_or(86400);
            if !*no_expiry && !(1..=31536000).contains(&seconds) {
                return Err(CliError::new(
                    2,
                    "Expiry must be between 1 second and 365 days",
                ));
            }
            let expires_at = (!*no_expiry)
                .then(|| chrono::Utc::now().timestamp_millis() + seconds as i64 * 1000);
            issued(
                security
                    .issue(
                        &actor,
                        CreateToken {
                            name: name.clone(),
                            role: Role::Owner,
                            scope: Scope::default(),
                            expires_at,
                        },
                    )
                    .await
                    .map_err(failure)?,
                json,
            )
        }
        TokenCommand::List => {
            let mut records = security.store.list().await.map_err(failure)?;
            records.retain(|record| record.role == Role::Owner);
            if json {
                output::json(&records)
            } else {
                for record in records {
                    let status = if record.revoked_at.is_some() {
                        "revoked"
                    } else if record
                        .expires_at
                        .is_some_and(|expiry| expiry <= chrono::Utc::now().timestamp_millis())
                    {
                        "expired"
                    } else {
                        "active"
                    };
                    output::text(&format!(
                        "{}\t{}\t{:?}\t{}\n",
                        record.id, record.name, record.role, status
                    ))?;
                }
                Ok(())
            }
        }
        TokenCommand::Revoke { id } => {
            if !security.revoke(&actor, id).await.map_err(failure)? {
                return Err(CliError::new(2, "Token not found"));
            }
            if json {
                output::json(&serde_json::json!({"id":id,"revoked":true}))
            } else {
                output::text("Token revoked.\n")
            }
        }
        TokenCommand::Rotate { id } => {
            issued(security.rotate(&actor, id).await.map_err(failure)?, json)
        }
    }
}
pub async fn audit(explicit: Option<&Path>, command: &GatewayCommand) -> Result<()> {
    let security = open(explicit).await?;
    let GatewayCommand::Audit { before, limit } = command;
    let records = security
        .store
        .audit(*before, *limit)
        .await
        .map_err(failure)?;
    output::json(
        &serde_json::json!({"next_before":records.last().map(|record|record.id),"records":records}),
    )
}
/// Setup explicitly provisions a gateway credential, never imports an old shared secret.
pub(super) async fn configure(config: &mut AppConfig) -> Result<()> {
    let root = config.session.directory.clone();
    let security = tokio::task::spawn_blocking(move || Security::open(&root))
        .await
        .map_err(|_| CliError::new(3, "Gateway storage unavailable"))?
        .map_err(failure)?;
    let valid = security
        .authenticate(config.server.access_token.expose())
        .await
        .map_err(failure)?
        .is_some();
    if !valid && std::env::var_os("KYLER_GATEWAY_TOKEN").is_some() {
        return Err(CliError::new(
            2,
            "Unset KYLER_GATEWAY_TOKEN or provide a gateway owner token issued for this service",
        ));
    }
    if !valid {
        let token = security
            .issue(
                &Principal::local(),
                CreateToken {
                    name: "local-service".into(),
                    role: Role::Owner,
                    scope: Scope::default(),
                    expires_at: None,
                },
            )
            .await
            .map_err(failure)?;
        config.server.access_token = SecretString::new(token.secret);
    }
    if kyler_config::web::is_local_backend(config.server.bind, &config.client.server_url) {
        config.client.access_token = config.server.access_token.clone();
    }
    Ok(())
}
