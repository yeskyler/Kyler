//! User skill package management over the same API used by web uploads.
use crate::{CliError, Result, args::SkillCommand, output};

use kyler_client::Client;
use kyler_protocol::api::skills::{SkillInfo, SkillSource};

pub async fn execute(client: &Client, command: SkillCommand, json: bool) -> Result<()> {
    match command {
        SkillCommand::Install { path } => {
            let archive =
                tokio::task::spawn_blocking(move || kyler_core::skills::package_directory(&path))
                    .await
                    .map_err(|_| CliError::new(2, "Could not read skill directory."))?
                    .map_err(|error| CliError::new(2, format!("Cannot package skill: {error}")))?;
            let info = client.install_user_skill(archive).await?;
            if json {
                output::json(&info)
            } else {
                output::text(&format!(
                    "Installed {}\n  {}\n  Invoke: @{}\n",
                    info.name, info.path, info.name
                ))
            }
        }
        SkillCommand::List => {
            let skills = client.user_skills().await?;
            if json {
                return output::json(&skills);
            }
            if skills.is_empty() {
                return output::text("No skills installed.\n");
            }
            for skill in skills {
                output::text(&format!(
                    "{}  {:?}  {:?}\n  {}\n",
                    skill.name, skill.source, skill.status, skill.description
                ))?;
            }
            Ok(())
        }
        SkillCommand::Show { name } => {
            let skill = find(client, &name, false).await?;
            let details = client.user_skill_details(&skill.id).await?;
            if json {
                output::json(&details)
            } else {
                output::text(&format!(
                    "{}\n{}\nSource: {:?}\nStatus: {:?}\nPath: {}\nResources:\n{}\n\n{}\n",
                    details.info.name,
                    details.info.description,
                    details.info.source,
                    details.info.status,
                    details.info.path,
                    details
                        .info
                        .resources
                        .iter()
                        .map(|path| format!("  {path}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    details.instructions
                ))
            }
        }
        SkillCommand::Remove { name } => {
            let skill = find(client, &name, true).await?;
            client
                .remove_user_skill(&skill.id, skill.fingerprint)
                .await?;
            if json {
                output::json(&serde_json::json!({"removed":skill.name}))
            } else {
                output::text(&format!("Removed {}\n", skill.name))
            }
        }
    }
}
async fn find(client: &Client, name: &str, user_only: bool) -> Result<SkillInfo> {
    let skills = client.user_skills().await?;
    skills
        .iter()
        .find(|skill| (skill.name == name || skill.id == name) && skill.source == SkillSource::User)
        .or_else(|| {
            (!user_only)
                .then(|| {
                    skills
                        .iter()
                        .find(|skill| skill.name == name || skill.id == name)
                })
                .flatten()
        })
        .cloned()
        .ok_or_else(|| {
            CliError::new(
                2,
                if user_only {
                    "User skill not found; system skills cannot be removed."
                } else {
                    "Skill not found."
                },
            )
        })
}
