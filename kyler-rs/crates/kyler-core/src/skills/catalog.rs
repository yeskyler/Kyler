use super::loader::{self, Package};

use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use kyler_config::SkillSettings;
use kyler_protocol::api::skills::{SkillInfo, SkillSource, SkillStatus};

#[derive(Clone, Default)]
pub struct Catalog {
    pub packages: BTreeMap<String, Package>,
    pub invalid: Vec<SkillInfo>,
}
impl Catalog {
    pub fn discover(settings: &SkillSettings, workspace: Option<&Path>) -> anyhow::Result<Self> {
        let mut catalog = Self::default();
        if !settings.enabled {
            return Ok(catalog);
        }
        for package in super::builtin::packages(settings)? {
            catalog.packages.insert(package.info.id.clone(), package);
        }
        catalog.scan(&settings.user_directory, SkillSource::User, settings)?;
        if let Some(workspace) = workspace {
            let mut path = workspace.to_path_buf();
            for component in settings.project_directory.components() {
                path.push(component);
                if path.exists() {
                    anyhow::ensure!(
                        !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
                        "project skill directory cannot contain symlinks"
                    );
                }
            }
            catalog.scan(&path, SkillSource::Project, settings)?;
        }
        let priority = |source| match source {
            SkillSource::Builtin => 0,
            SkillSource::User => 1,
            SkillSource::Project => 2,
        };
        let mut winners = BTreeMap::<String, u8>::new();
        for info in catalog
            .packages
            .values()
            .map(|p| &p.info)
            .chain(catalog.invalid.iter())
        {
            let rank = priority(info.source);
            winners
                .entry(info.name.clone())
                .and_modify(|r| *r = (*r).max(rank))
                .or_insert(rank);
        }
        // Invalid higher-priority packages shadow lower-priority copies as well.
        for package in catalog.packages.values_mut() {
            if priority(package.info.source) < winners[&package.info.name] {
                package.info.status = SkillStatus::Shadowed;
            }
        }
        Ok(catalog)
    }
    fn scan(
        &mut self,
        root: &Path,
        source: SkillSource,
        settings: &SkillSettings,
    ) -> anyhow::Result<()> {
        if !root.exists() {
            return Ok(());
        }
        let mut entries = std::fs::read_dir(root)?
            .filter(|entry| {
                entry
                    .as_ref()
                    .map(|e| !e.file_name().to_string_lossy().starts_with('.'))
                    .unwrap_or(true)
            })
            .take(129)
            .collect::<std::io::Result<Vec<_>>>()?;
        anyhow::ensure!(entries.len() <= 128, "skill directory exceeds 128 entries");
        entries.sort_by_key(|e| e.file_name());
        let mut names = HashSet::new();
        for entry in entries {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if !entry.file_type()?.is_dir() && !entry.file_type()?.is_symlink() {
                continue;
            }
            let path = entry.path();
            match loader::package(&path, source, settings) {
                Ok(package) => {
                    if names.insert(package.info.name.clone()) {
                        self.packages.insert(package.info.id.clone(), package);
                    } else {
                        let duplicates=self.packages.iter().filter(|(_,p)|p.info.source==source&&p.info.name==package.info.name).map(|(id,_)|id.clone()).collect::<Vec<_>>();
                        for id in duplicates { let mut info=self.packages.remove(&id).unwrap().info;info.status=SkillStatus::Invalid;info.error=Some("Duplicate skill name in the same directory.".into());self.invalid.push(info); }
                        let mut info=package.info;info.status=SkillStatus::Invalid;info.error=Some("Duplicate skill name in the same directory.".into());self.invalid.push(info);
                    }
                }
                Err(_) => self.invalid.push(SkillInfo {
                    id:loader::digest(path.to_string_lossy().as_bytes()), name:entry.file_name().to_string_lossy().into_owned(), description:String::new(), source, path:path.to_string_lossy().into_owned(), fingerprint:String::new(), status:SkillStatus::Invalid, resources:Vec::new(), error:Some("Invalid skill package: check frontmatter, names, symlinks and size limits.".into()),
                }),
            }
        }
        Ok(())
    }
}
