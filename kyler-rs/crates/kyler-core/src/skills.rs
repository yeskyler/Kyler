//! Compatibility facade. Package discovery and storage live in kyler-skills.
mod render;
pub use render::SkillCatalogView;
#[cfg(test)]
mod tests;

use crate::{sandbox::FilesystemPolicy, tools::ToolSource};

use std::{ops::Deref, path::Path, sync::Arc};

use anyhow::{Result, ensure};
use serde_json::Value;

use kyler_config::SkillSettings;
use kyler_protocol::api::skills::{InstallSkillRequest, SkillScope};
pub use kyler_skills::{
    MAX_SKILL_ARCHIVE_BYTES, catalog, package_directory, references, validate_references,
};

pub struct SkillManager(Arc<kyler_skills::SkillManager>);
impl Deref for SkillManager {
    type Target = kyler_skills::SkillManager;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl SkillManager {
    pub fn new(settings: SkillSettings) -> Result<Self> {
        Ok(Self(Arc::new(kyler_skills::SkillManager::new(settings)?)))
    }
    pub async fn snapshot(self: &Arc<Self>, workspace: Option<&Path>) -> Result<SkillSnapshot> {
        Ok(SkillSnapshot(self.0.snapshot(workspace).await?))
    }
    pub(crate) fn check_destination_policy(
        &self,
        workspace: Option<&Path>,
        scope: SkillScope,
        name: &str,
        policy: &FilesystemPolicy,
    ) -> Result<()> {
        self.0
            .check_destination_access(workspace, scope, name, &HostAccess(policy.clone()))
    }
    pub(crate) async fn install_checked(
        &self,
        workspace: Option<&Path>,
        request: InstallSkillRequest,
        policy: Option<FilesystemPolicy>,
    ) -> Result<kyler_protocol::api::skills::SkillInfo> {
        self.0
            .install_with_access(
                workspace,
                request,
                policy.map(|policy| {
                    Arc::new(HostAccess(policy)) as Arc<dyn kyler_skills::SkillFileAccess>
                }),
            )
            .await
    }
}
#[derive(Clone)]
pub struct SkillSnapshot(kyler_skills::SkillSnapshot);
impl Deref for SkillSnapshot {
    type Target = kyler_skills::SkillSnapshot;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl SkillSnapshot {
    pub fn source(&self, args: &Value) -> ToolSource {
        let name = args["name"].as_str().unwrap_or_default();
        ToolSource::Skill {
            name: name.into(),
            fingerprint: self
                .available
                .get(name)
                .map(|p| p.info.fingerprint.clone())
                .unwrap_or_default(),
        }
    }
    pub fn resource(&self, name: &str, path: &str) -> Result<Value> {
        let resource = self.0.resource_bytes(name, path)?;
        let content = String::from_utf8(resource.bytes).map_err(|_| {
            anyhow::anyhow!("skill resource is not UTF-8 text; use an appropriate artifact tool")
        })?;
        Ok(
            serde_json::json!({"skill":resource.skill,"fingerprint":resource.fingerprint,"path":resource.path,"content":content}),
        )
    }
}
struct HostAccess(FilesystemPolicy);
impl kyler_skills::SkillFileAccess for HostAccess {
    fn check_read(&self, base: &Path, path: &Path) -> Result<()> {
        self.0.check_read(base, path).map(|_| ())
    }
    fn check_write(&self, base: &Path, path: &Path) -> Result<()> {
        self.0.check_write(base, path).map(|_| ())
    }
    fn check_replace(&self, destination: &Path) -> Result<()> {
        let destination = self.0.check_write(destination, destination)?;
        ensure!(
            !self
                .0
                .readonly_paths
                .iter()
                .chain(&self.0.denied_paths)
                .any(|path| path.starts_with(&destination)),
            "skill replacement would modify a protected descendant"
        );
        Ok(())
    }
}
