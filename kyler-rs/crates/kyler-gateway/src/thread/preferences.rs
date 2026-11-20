use super::ThreadHandle;

use std::{io::Write, path::Path};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use kyler_config::AppConfig;
use kyler_protocol::{
    api::{
        models::{ModelCatalog, SessionModel},
        permissions::*,
    },
    approval::{ApprovalPolicy, ApprovalReviewer},
    sandbox::SandboxMode,
};

#[derive(Serialize, Deserialize)]
struct Preferences {
    permissions: PermissionSnapshot,
    profile: Option<String>,
}
pub(super) fn load(
    path: &Path,
    config: &AppConfig,
) -> Result<(PermissionSnapshot, Option<String>)> {
    let mut saved = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Preferences>(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Preferences {
            permissions: PermissionSnapshot {
                mode: PermissionMode::WorkspaceAccess,
                approval: Some(config.approval.clone()),
                revision: "initial".into(),
            },
            profile: None,
        },
        Err(error) => return Err(error.into()),
    };
    if config.sandbox.mode == SandboxMode::ReadOnly {
        saved.permissions.mode = PermissionMode::ReadOnly;
    }
    if saved.permissions.mode == PermissionMode::FullAccess
        && !matches!(
            config.sandbox.mode,
            SandboxMode::WorkspaceWrite | SandboxMode::DangerFullAccess
        )
    {
        saved.permissions.mode = PermissionMode::WorkspaceAccess;
        saved.permissions.approval = Some(config.approval.clone());
    }
    Ok((saved.permissions, saved.profile))
}
impl ThreadHandle {
    async fn save_preferences(
        &self,
        permissions: &PermissionSnapshot,
        profile: &Option<String>,
        permissions_changed: bool,
    ) -> Result<()> {
        if self.inner.hosted {
            let id = self.session().await.session_id;
            let root = &self.inner.preferences;
            if permissions_changed {
                save(
                    &root.join(format!("{id}.permissions")),
                    &serde_json::to_vec(permissions)?,
                )?;
            } else {
                save(
                    &root.join(format!("{id}.model")),
                    &serde_json::to_vec(&kyler_protocol::api::models::ModelSelection {
                        profile: profile.clone(),
                    })?,
                )?;
            }
        } else {
            save(
                &self.inner.preferences,
                &serde_json::to_vec(&Preferences {
                    permissions: permissions.clone(),
                    profile: profile.clone(),
                })?,
            )?;
        }
        Ok(())
    }
    pub async fn permissions(
        &self,
        selection: Option<&PermissionSelection>,
    ) -> Result<PermissionState> {
        if let Some(selection) = selection {
            let (reply, receive) = tokio::sync::oneshot::channel();
            self.submit(super::ThreadOp::SelectPermissions {
                selection: selection.clone(),
                reply,
            })
            .await?;
            return receive
                .await
                .context("thread stopped before permission selection")?;
        }
        self.permission_state(None).await
    }
    pub(super) async fn permission_state(
        &self,
        selection: Option<&PermissionSelection>,
    ) -> Result<PermissionState> {
        let environment = self.inner.environment.read().await.clone();
        let host = environment.as_ref().map(|e| e.policy().mode);
        let writes = environment
            .as_ref()
            .is_some_and(|e| !e.policy().filesystem.readwrite_paths.is_empty());
        let full_access = environment.as_ref().is_some_and(|e| e.allows_full_access());
        let options = permission_options(host, writes, full_access);
        let mut selected = self.inner.permissions.write().await;
        if let Some(selection) = selection {
            let mode = selection.mode.unwrap_or(selected.mode).preset();
            ensure!(
                selection.mode.is_none() || options.iter().any(|o| o.mode == mode && o.available),
                "permission mode exceeds the host boundary"
            );
            ensure!(
                selection.mode.is_none()
                    || mode != PermissionMode::FullAccess
                    || selection.confirm_full_access,
                "confirm full access first"
            );
            ensure!(
                mode == selected.mode
                    || (mode != PermissionMode::FullAccess
                        && selected.mode != PermissionMode::FullAccess)
                    || self.inner.active.read().await.is_none(),
                "Wait for the current run to finish before switching full access."
            );
            if mode != selected.mode
                && (mode == PermissionMode::FullAccess
                    || selected.mode == PermissionMode::FullAccess)
            {
                let session = self.session().await.session_id;
                ensure!(
                    self.inner
                        .processes
                        .list(&session)
                        .iter()
                        .all(|p| p.status != "running"),
                    "Stop active process sessions before switching full access."
                );
            }
            let mut approval = selection.approval.clone().unwrap_or_else(|| {
                if selection.mode.is_some() {
                    self.inner.default_approval.clone()
                } else {
                    selected.effective_approval()
                }
            });
            if let Some(error) = approval.validation_error() {
                anyhow::bail!(error);
            }
            if selection.mode.is_some() && selection.approval.is_none() {
                approval.policy = ApprovalPolicy::OnRequest;
            }
            approval.reviewer = ApprovalReviewer::User;
            if mode == PermissionMode::FullAccess {
                approval.policy = ApprovalPolicy::Never;
            }
            let next = PermissionSnapshot {
                mode,
                approval: Some(approval),
                revision: uuid::Uuid::new_v4().to_string(),
            };
            let profile = self.inner.profile.read().await;
            self.save_preferences(&next, &profile, true).await?;
            if let Some(active) = self.inner.active.write().await.as_mut() {
                active.permissions = Some(next.clone());
            }
            *selected = next;
            self.inner
                .approvals
                .update_policy(&self.session().await.session_id, selected.clone());
        }
        Ok(PermissionState {
            sandbox: match selected.mode {
                PermissionMode::ReadOnly if host.is_some() => Some(SandboxMode::ReadOnly),
                PermissionMode::WorkspaceAccess if writes => Some(SandboxMode::WorkspaceWrite),
                PermissionMode::FullAccess if full_access => Some(SandboxMode::DangerFullAccess),
                _ => host,
            },
            approval: selected.effective_approval(),
            selected: selected.clone(),
            active: self
                .inner
                .active
                .read()
                .await
                .as_ref()
                .and_then(|a| a.permissions.clone()),
            options,
            workspace: environment
                .as_ref()
                .map(|e| e.session_environment().cwd.display().to_string()),
            ceiling: format!(
                "Default sandbox: {}. Full access requires explicit confirmation and host authorization.",
                host.map(|mode| mode.label()).unwrap_or("unavailable")
            ),
        })
    }
    async fn thinking_path(&self) -> std::path::PathBuf {
        if self.inner.hosted {
            self.inner
                .preferences
                .join(format!("{}.thinking", self.session().await.session_id))
        } else {
            self.inner.preferences.with_extension("thinking")
        }
    }
    pub async fn thinking(
        &self,
        enabled: Option<bool>,
    ) -> Result<kyler_protocol::api::models::ThinkingState> {
        self.thinking_level(enabled.map(kyler_protocol::api::models::ThinkingLevel::from_enabled))
            .await
    }
    pub async fn thinking_level(
        &self,
        level: Option<kyler_protocol::api::models::ThinkingLevel>,
    ) -> Result<kyler_protocol::api::models::ThinkingState> {
        if let Some(level) = level {
            let (reply, receive) = tokio::sync::oneshot::channel();
            self.submit(super::ThreadOp::SelectThinking {
                level: Some(level),
                reply,
            })
            .await?;
            return receive
                .await
                .context("thread stopped before thinking selection")?;
        }
        self.thinking_state(None).await
    }
    pub async fn reset_thinking(&self) -> Result<kyler_protocol::api::models::ThinkingState> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        self.submit(super::ThreadOp::SelectThinking { level: None, reply })
            .await?;
        receive
            .await
            .context("thread stopped before thinking reset")?
    }
    pub(super) async fn thinking_state(
        &self,
        selection: Option<Option<kyler_protocol::api::models::ThinkingLevel>>,
    ) -> Result<kyler_protocol::api::models::ThinkingState> {
        let profile = self.inner.profile.read().await;
        let snapshot = self
            .inner
            .agent
            .model_registry()
            .context("model management is unavailable")?
            .resolve(profile.as_deref())?;
        self.thinking_for_snapshot(&snapshot, selection).await
    }
    async fn thinking_for_snapshot(
        &self,
        snapshot: &kyler_core::models::ModelSnapshot,
        selection: Option<Option<kyler_protocol::api::models::ThinkingLevel>>,
    ) -> Result<kyler_protocol::api::models::ThinkingState> {
        use kyler_protocol::api::models::{ThinkingLevel, ThinkingState, ThinkingSupport};
        let caps = snapshot.info.capabilities.reasoning.as_ref();
        let status = match caps {
            None => ThinkingSupport::Unknown,
            Some(caps) if !caps.levels.is_empty() => ThinkingSupport::Supported,
            Some(_) => ThinkingSupport::Unsupported,
        };
        let levels = ThinkingLevel::ALL.to_vec();
        let default = ThinkingLevel::Off;
        let path = migrate_thinking_path(&self.thinking_path().await, &snapshot.preference_key())?;
        if let Some(selection) = selection {
            if let Some(level) = selection {
                ensure!(
                    levels.contains(&level),
                    "Thinking level is not supported for this model/provider"
                );
                save(&path, &serde_json::to_vec(&level)?)?;
            } else {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        let saved = read_thinking(&path)?;
        let level = saved
            .filter(|level| levels.contains(level))
            .unwrap_or(default);
        Ok(ThinkingState {
            status,
            source: caps.map(|caps| caps.source.clone()),
            profile: snapshot.info.profile.clone(),
            revision: snapshot.info.revision,
            follows_default: saved.is_none_or(|level| !levels.contains(&level)),
            enabled: level.enabled(),
            supported: status == ThinkingSupport::Supported,
            level,
            levels,
        })
    }

    pub async fn models(&self, reload: bool) -> Result<ModelCatalog> {
        let registry = self
            .inner
            .agent
            .model_registry()
            .context("model management is unavailable")?;
        if reload {
            let config = kyler_config::loader::load_local(self.inner.source.as_deref())?;
            registry.reload(&config)?;
            registry.refresh_discovery().await?;
        }
        Ok(registry.catalog())
    }
    pub async fn model(&self, selection: Option<Option<String>>) -> Result<SessionModel> {
        if let Some(profile) = selection {
            let (reply, receive) = tokio::sync::oneshot::channel();
            self.submit(super::ThreadOp::SelectModel { profile, reply })
                .await?;
            return receive
                .await
                .context("thread stopped before model selection")?;
        }
        self.model_state(None).await
    }
    pub(super) async fn model_state(
        &self,
        selection: Option<Option<String>>,
    ) -> Result<SessionModel> {
        // Use the same lock ordering as permission updates.
        let permissions = self.inner.permissions.read().await;
        let mut profile = self.inner.profile.write().await;
        let target = selection.clone().unwrap_or_else(|| profile.clone());
        ensure!(
            target.as_ref().is_none_or(|name| !name.is_empty()
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))),
            "Invalid model profile name."
        );
        let resolved = self
            .inner
            .agent
            .model_registry()
            .context("model management is unavailable")?
            .resolve(target.as_deref());
        if selection.is_some() {
            if let Err(error) = &resolved {
                anyhow::bail!("{error}");
            }
        }
        let (selected, thinking, error) = match resolved {
            Ok(snapshot) => {
                let reset = selection
                    .as_ref()
                    .map(|_| Some(kyler_protocol::api::models::ThinkingLevel::Off));
                let thinking = self.thinking_for_snapshot(&snapshot, reset).await?;
                (Some(snapshot.info.clone()), Some(thinking), None)
            }
            Err(error) => (None, None, Some(error.to_string())),
        };
        if selection.is_some() {
            self.save_preferences(&permissions, &target, false).await?;
            *profile = target;
        }
        Ok(SessionModel {
            profile: profile.clone(),
            selected,
            thinking,
            error,
            active: self
                .inner
                .active
                .read()
                .await
                .as_ref()
                .and_then(|a| a.model.clone()),
        })
    }
}

fn permission_options(
    host: Option<SandboxMode>,
    writes: bool,
    full_access: bool,
) -> Vec<PermissionOption> {
    PermissionMode::ALL
        .into_iter()
        .map(|mode| PermissionOption {
            mode,
            available: match mode {
                PermissionMode::ReadOnly => host.is_some(),
                PermissionMode::WorkspaceAccess => writes,
                PermissionMode::FullAccess => full_access,
                PermissionMode::AutoReview => false,
            },
            description: match mode {
                PermissionMode::ReadOnly => "Read allowed files without changing them.",
                PermissionMode::WorkspaceAccess => "Work within the host-authorized workspace.",
                PermissionMode::FullAccess if full_access => "Access host files and network as the service user, without workspace isolation. The working directory stays unchanged.",
                PermissionMode::FullAccess => "Full access is unavailable in this execution environment.",
                _ => "Unavailable.",
            }
            .into(),
        })
        .collect()
}

fn save(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("preference path has no parent")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// Model identity includes the provider, so preferences cannot leak across services.
/// A scoped preference is never reused across provider endpoints or wire protocols.
pub(super) fn migrate_thinking_path(base: &Path, identity: &str) -> Result<std::path::PathBuf> {
    let path = base.with_extension(format!("{identity}.thinking"));
    if !path.exists() && base.exists() {
        std::fs::rename(base, &path)?;
    }
    Ok(path)
}

/// Read old boolean preferences and new levels without modifying historical data.
pub(super) fn read_thinking(
    path: &Path,
) -> Result<Option<kyler_protocol::api::models::ThinkingLevel>> {
    use kyler_protocol::api::models::ThinkingLevel;
    match std::fs::read(path) {
        Ok(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            Ok(Some(if let Some(enabled) = value.as_bool() {
                ThinkingLevel::from_enabled(enabled)
            } else {
                serde_json::from_value(value)?
            }))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod thinking_tests {
    use super::*;
    use kyler_protocol::api::models::ThinkingLevel;
    #[test]
    fn old_boolean_and_level_preferences_are_read_without_rewriting() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("thinking");
        for (input, expected) in [
            ("true", ThinkingLevel::High),
            ("false", ThinkingLevel::Off),
            ("\"max\"", ThinkingLevel::Max),
        ] {
            std::fs::write(&path, input).unwrap();
            assert_eq!(read_thinking(&path).unwrap(), Some(expected));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), input);
        }
    }
}
