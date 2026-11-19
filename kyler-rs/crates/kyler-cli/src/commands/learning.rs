//! Explicit local review authority; this command never calls a model.
use crate::{
    CliError, Result,
    args::{DraftReviewCommand, LearningCommand, MemoryReviewCommand, MemoryReviewStatus},
    output,
};

use std::path::Path;

use kyler_core::{
    memory::{MemoryStatus, MemoryStore},
    skills::SkillManager,
};

pub async fn execute(config: Option<&Path>, command: &LearningCommand, json: bool) -> Result<()> {
    let result: anyhow::Result<serde_json::Value> = async {
        match command {
            LearningCommand::Memory {
                store,
                scope,
                command,
            } => {
                let store = MemoryStore::new(store.clone());
                let scope = scope.clone();
                match command {
                    MemoryReviewCommand::List => Ok(serde_json::to_value(
                        tokio::task::spawn_blocking(move || store.list(&scope)).await??,
                    )?),
                    MemoryReviewCommand::Review {
                        key,
                        revision,
                        status,
                    } => {
                        let key = key.clone();
                        let revision = *revision;
                        let status = match status {
                            MemoryReviewStatus::Active => MemoryStatus::Active,
                            MemoryReviewStatus::Rejected => MemoryStatus::Rejected,
                            MemoryReviewStatus::Invalid => MemoryStatus::Invalid,
                        };
                        Ok(serde_json::to_value(
                            tokio::task::spawn_blocking(move || {
                                store.review(&scope, &key, revision, status)
                            })
                            .await??,
                        )?)
                    }
                }
            }
            LearningCommand::Drafts { command } => {
                let settings = kyler_config::loader::load_for_setup(config)?.skills;
                let manager = SkillManager::new(settings)?;
                match command {
                    DraftReviewCommand::List => Ok(serde_json::to_value(manager.drafts().await?)?),
                    DraftReviewCommand::Show { name } => {
                        Ok(serde_json::to_value(manager.draft_details(name).await?)?)
                    }
                    DraftReviewCommand::Publish {
                        name,
                        fingerprint,
                        expected_installed,
                    } => Ok(serde_json::to_value(
                        manager
                            .publish_draft(name, fingerprint, expected_installed.clone())
                            .await?,
                    )?),
                }
            }
        }
    }
    .await;
    let value = result.map_err(|error| CliError::new(2, error.to_string()))?;
    if json {
        output::json(&value)
    } else {
        output::text(
            &(serde_json::to_string_pretty(&value)
                .map_err(|error| CliError::new(3, error.to_string()))?
                + "\n"),
        )
    }
}
