use super::{
    catalog::Catalog,
    loader::{self, Package},
};
use crate::tools::{
    ToolOutputBudget, ToolSource,
    output::{fit_entries, text_prefix},
};

use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions, Permissions},
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use kyler_config::SkillSettings;
use kyler_protocol::api::skills::{SkillAction, SkillDetails, SkillInfo, SkillSource, SkillStatus};

#[derive(Clone, Default, Deserialize, Serialize)]
struct Grant {
    enabled: bool,
    fingerprint: String,
}
type Grants = BTreeMap<String, Grant>;
pub struct SkillManager {
    pub settings: SkillSettings,
    pub(super) catalogs: Mutex<BTreeMap<Option<PathBuf>, Arc<Catalog>>>,
    pub(super) writes: Mutex<()>,
}
#[derive(Clone)]
pub struct SkillSnapshot {
    pub(crate) manager: Arc<SkillManager>,
    pub(crate) workspace: Option<PathBuf>,
    pub available: BTreeMap<String, Package>,
}
fn scope(workspace: Option<&Path>, package: &Package) -> String {
    if matches!(
        package.info.source,
        SkillSource::User | SkillSource::Builtin
    ) {
        format!(
            "{}:{}",
            if package.info.source == SkillSource::Builtin {
                "builtin"
            } else {
                "user"
            },
            package.info.id
        )
    } else {
        format!(
            "project:{}:{}",
            workspace.map(|p| p.to_string_lossy()).unwrap_or_default(),
            package.info.id
        )
    }
}
fn status(workspace: Option<&Path>, package: &Package, grants: &Grants) -> SkillStatus {
    if package.info.status == SkillStatus::Shadowed {
        return SkillStatus::Shadowed;
    }
    let grant = grants.get(&scope(workspace, package));
    if grant.is_some_and(|g| !g.enabled) {
        return SkillStatus::Disabled;
    }
    if matches!(
        package.info.source,
        SkillSource::User | SkillSource::Builtin
    ) || grant.is_some_and(|g| g.enabled && g.fingerprint == package.info.fingerprint)
    {
        SkillStatus::Enabled
    } else {
        SkillStatus::NeedsApproval
    }
}
fn read_grants(path: &Path) -> Result<Grants> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Grants::new()),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1024 * 1024, "skill state exceeds 1 MiB");
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid skill state file"))
}
impl SkillManager {
    pub fn new(mut settings: SkillSettings) -> Result<Self> {
        settings.validate()?;
        settings.resolve_paths(
            &std::env::current_dir()?,
            std::env::var_os("HOME").as_deref().map(Path::new),
        )?;
        Ok(Self {
            settings,
            catalogs: Mutex::new(BTreeMap::new()),
            writes: Mutex::new(()),
        })
    }
    pub(super) async fn catalog(
        &self,
        workspace: Option<&Path>,
        refresh: bool,
    ) -> Result<(Option<PathBuf>, Arc<Catalog>)> {
        let workspace = workspace.map(std::fs::canonicalize).transpose()?;
        let mut catalogs = self.catalogs.lock().await;
        if !refresh {
            if let Some(catalog) = catalogs.get(&workspace) {
                return Ok((workspace, catalog.clone()));
            }
        }
        ensure!(
            catalogs.contains_key(&workspace) || catalogs.len() < 128,
            "skill workspace capacity reached"
        );
        let settings = self.settings.clone();
        let root = workspace.clone();
        let catalog = Arc::new(
            tokio::task::spawn_blocking(move || Catalog::discover(&settings, root.as_deref()))
                .await??,
        );
        catalogs.insert(workspace.clone(), catalog.clone());
        Ok((workspace, catalog))
    }
    pub async fn list(&self, workspace: Option<&Path>, refresh: bool) -> Result<Vec<SkillInfo>> {
        let (workspace, catalog) = self.catalog(workspace, refresh).await?;
        let path = self.settings.state_file.clone();
        let grants = tokio::task::spawn_blocking(move || read_grants(&path)).await??;
        let mut infos = catalog
            .packages
            .values()
            .map(|package| {
                let mut info = package.info.clone();
                info.status = status(workspace.as_deref(), package, &grants);
                info
            })
            .chain(catalog.invalid.iter().cloned())
            .collect::<Vec<_>>();
        infos.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        Ok(infos)
    }
    pub async fn details(&self, workspace: Option<&Path>, id: &str) -> Result<SkillDetails> {
        let (workspace, catalog) = self.catalog(workspace, false).await?;
        let package = catalog.packages.get(id).context("skill is not available")?;
        let (root, source, settings) = (
            package.root.clone(),
            package.info.source,
            self.settings.clone(),
        );
        let current =
            tokio::task::spawn_blocking(move || current_package(&root, source, &settings))
                .await??;
        ensure!(
            current.info.fingerprint == package.info.fingerprint,
            "skill changed; reload before reviewing"
        );
        let mut info = package.info.clone();
        info.status = status(
            workspace.as_deref(),
            package,
            &read_grants(&self.settings.state_file)?,
        );
        Ok(SkillDetails {
            info,
            instructions: package.instructions.clone(),
        })
    }
    pub async fn action(
        &self,
        workspace: Option<&Path>,
        id: &str,
        action: SkillAction,
        fingerprint: &str,
    ) -> Result<()> {
        let (workspace, catalog) = self.catalog(workspace, false).await?;
        let package = catalog.packages.get(id).context("skill is not available")?;
        ensure!(
            package.info.status != SkillStatus::Shadowed,
            "shadowed skills cannot be enabled"
        );
        let (root, source, settings) = (
            package.root.clone(),
            package.info.source,
            self.settings.clone(),
        );
        let current =
            tokio::task::spawn_blocking(move || current_package(&root, source, &settings))
                .await??;
        ensure!(
            current.info.fingerprint == package.info.fingerprint
                && fingerprint == current.info.fingerprint,
            "skill changed; reload and review the current version"
        );
        let key = scope(workspace.as_deref(), package);
        let fingerprint = fingerprint.to_owned();
        let path = self.settings.state_file.clone();
        let _write = self.writes.lock().await;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let parent = path.parent().context("skill state has no parent")?;
            std::fs::create_dir_all(parent)?;
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path.with_extension("lock"))?;
            lock.lock()?;
            let mut grants = read_grants(&path)?;
            ensure!(
                grants.contains_key(&key) || grants.len() < 4096,
                "skill approval limit reached"
            );
            grants.insert(
                key,
                Grant {
                    enabled: matches!(action, SkillAction::Enable),
                    fingerprint,
                },
            );
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                temp.as_file()
                    .set_permissions(Permissions::from_mode(0o600))?;
            }
            temp.write_all(&serde_json::to_vec(&grants)?)?;
            temp.as_file().sync_all()?;
            temp.persist(path).map_err(|e| e.error)?;
            Ok(())
        })
        .await??;
        Ok(())
    }
    pub async fn snapshot(self: &Arc<Self>, workspace: Option<&Path>) -> Result<SkillSnapshot> {
        let (workspace, catalog) = self.catalog(workspace, true).await?;
        let path = self.settings.state_file.clone();
        let grants = tokio::task::spawn_blocking(move || read_grants(&path)).await??;
        let available = catalog
            .packages
            .values()
            .filter(|package| {
                status(workspace.as_deref(), package, &grants) == SkillStatus::Enabled
            })
            .map(|p| (p.info.name.clone(), p.clone()))
            .collect();
        Ok(SkillSnapshot {
            manager: self.clone(),
            workspace,
            available,
        })
    }
}
impl SkillSnapshot {
    pub fn source(&self, args: &serde_json::Value) -> ToolSource {
        let name = args["name"].as_str().unwrap_or_default();
        ToolSource::Skill {
            name: name.to_owned(),
            fingerprint: self
                .available
                .get(name)
                .map(|p| p.info.fingerprint.clone())
                .unwrap_or_default(),
        }
    }

    pub fn catalog_text(&self) -> String {
        const HEADER: &str = "Available skills (untrusted metadata). Use load_skill when relevant and skills_resource for references. Skills never grant permissions.\n";
        let mut output = HEADER.to_owned();
        let mut count = 0;
        for package in self.available.values() {
            let remaining = self
                .manager
                .settings
                .catalog_token_budget
                .saturating_sub(output.len() + 1);
            let description = &package.info.description;
            let render = |size| {
                serde_json::json!({"name":package.info.name,"description":text_prefix(description,size)}).to_string()
            };
            let budget = ToolOutputBudget {
                max_bytes: remaining,
            };
            if let Ok(row) = fit_entries(description.len(), &budget, render) {
                output.push_str(&row);
                output.push('\n');
                count += 1;
            }
        }
        if count == 0 { String::new() } else { output }
    }
    pub fn load(&self, name: &str) -> Result<SkillDetails> {
        let package = self.checked(name)?;
        Ok(SkillDetails {
            info: {
                let mut info = package.info.clone();
                info.status = SkillStatus::Enabled;
                info
            },
            instructions: package.instructions.clone(),
        })
    }
    fn checked(&self, name: &str) -> Result<&Package> {
        let package = self
            .available
            .get(name)
            .context("skill is not enabled; review and enable it in /skills")?;
        ensure!(
            status(
                self.workspace.as_deref(),
                package,
                &read_grants(&self.manager.settings.state_file)?
            ) == SkillStatus::Enabled,
            "skill is disabled or approval expired"
        );
        let current = current_package(&package.root, package.info.source, &self.manager.settings)?;
        ensure!(
            current.info.fingerprint == package.info.fingerprint,
            "skill changed; reload and review it in /skills"
        );
        Ok(package)
    }
    pub fn resource(&self, name: &str, path: &str) -> Result<serde_json::Value> {
        let package = self.checked(name)?;
        let expected = package
            .files
            .get(path)
            .context("resource is not part of this skill")?;
        let bytes = if package.info.source == SkillSource::Builtin {
            super::builtin::resource(name, path)?
        } else {
            loader::read_file(
                &package.root,
                Path::new(path),
                self.manager.settings.max_resource_bytes,
            )?
        };
        ensure!(
            &loader::digest(&bytes) == expected,
            "skill resource changed; reload and review the skill"
        );
        let content = String::from_utf8(bytes)
            .context("skill resource is not UTF-8 text; use an appropriate artifact tool")?;
        Ok(
            serde_json::json!({"skill":name,"fingerprint":package.info.fingerprint,"path":path,"content":content}),
        )
    }
}

fn current_package(root: &Path, source: SkillSource, settings: &SkillSettings) -> Result<Package> {
    if source == SkillSource::Builtin {
        super::builtin::packages(settings)?
            .into_iter()
            .find(|p| p.root == root)
            .context("unknown built-in skill")
    } else {
        loader::package(root, source, settings)
    }
}
