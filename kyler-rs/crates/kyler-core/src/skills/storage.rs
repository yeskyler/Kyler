//! Validated package writes with conflict detection and rollback on replacement failure.
use super::{loader, manager::SkillManager};

use std::{
    collections::BTreeMap,
    fs,
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

use kyler_config::SkillSettings;
use kyler_protocol::api::skills::{
    InstallSkillRequest, SaveSkillRequest, SkillInfo, SkillScope, SkillSource,
};

impl SkillManager {
    /// Install one user package without overwriting an existing skill.
    pub async fn install_archive(&self, bytes: Vec<u8>) -> Result<SkillInfo> {
        let root = self.destination(None, SkillScope::User)?;
        let settings = self.settings.clone();
        let _write = self.writes.lock().await;
        let info = tokio::task::spawn_blocking(move || {
            let (name, files) = super::archive::unpack(&bytes, &settings)?;
            publish(&root, &name, SkillScope::User, files, None, &settings)
        })
        .await??;
        self.catalogs.lock().await.clear();
        self.list(None, true)
            .await?
            .into_iter()
            .find(|skill| skill.id == info.id)
            .context("installed skill was not discovered")
    }

    /// Only user packages can be removed; a matching fingerprint prevents stale deletion.
    pub async fn remove_user(&self, id: &str, fingerprint: &str) -> Result<()> {
        let _write = self.writes.lock().await;
        let (_, catalog) = self.catalog(None, true).await?;
        let package = catalog.packages.get(id).context("user skill not found")?;
        ensure!(
            package.info.source == SkillSource::User,
            "system skills cannot be removed"
        );
        let root = self.destination(None, SkillScope::User)?;
        let source = package.root.clone();
        let fingerprint = fingerprint.to_owned();
        let settings = self.settings.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            checked_directory(&root)?;
            let mut options = fs::OpenOptions::new();
            options.create(true).truncate(false).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .mode(0o600);
            }
            let lock = options.open(root.join(".write.lock"))?;
            ensure!(lock.metadata()?.is_file(), "invalid skill lock");
            lock.lock()?;
            ensure!(
                source.parent() == Some(root.canonicalize()?.as_path()),
                "skill is outside user directory"
            );
            let current = loader::package(&source, SkillSource::User, &settings)?;
            ensure!(
                current.info.fingerprint == fingerprint,
                "skill changed; inspect it before removing"
            );
            let removed = tempfile::Builder::new()
                .prefix(".skill-remove-")
                .tempdir_in(&root)?;
            fs::rename(&source, removed.path().join("package"))?;
            removed.close()?;
            Ok(())
        })
        .await??;
        self.catalogs.lock().await.clear();
        Ok(())
    }

    pub fn destination(&self, workspace: Option<&Path>, scope: SkillScope) -> Result<PathBuf> {
        ensure!(self.settings.enabled, "skills are disabled");
        Ok(match scope {
            SkillScope::User => self.settings.user_directory.clone(),
            SkillScope::Project => workspace
                .context("project skill requires a workspace")?
                .join(&self.settings.project_directory),
        })
    }

    pub(crate) fn check_destination_policy(
        &self,
        workspace: Option<&Path>,
        scope: SkillScope,
        name: &str,
        policy: &crate::sandbox::FilesystemPolicy,
    ) -> Result<()> {
        if scope == SkillScope::Project {
            check_write_policy(policy, &self.destination(workspace, scope)?.join(name))?;
        }
        Ok(())
    }

    pub async fn save(
        &self,
        workspace: Option<&Path>,
        request: SaveSkillRequest,
    ) -> Result<SkillInfo> {
        let root = self.destination(workspace, request.scope)?;
        let settings = self.settings.clone();
        let _write = self.writes.lock().await;
        let info = tokio::task::spawn_blocking(move || {
            publish(
                &root,
                &request.name,
                request.scope,
                request
                    .files
                    .into_iter()
                    .map(|(p, s)| (p, s.into_bytes()))
                    .collect(),
                request.expected_fingerprint.as_deref(),
                &settings,
            )
        })
        .await??;
        self.catalogs.lock().await.clear();
        self.catalog(workspace, true).await?;
        // Return the effective status, including shadowing and project approval.
        self.list(workspace, false)
            .await?
            .into_iter()
            .find(|p| p.id == info.id)
            .context("saved skill was not discovered")
    }

    pub async fn install(
        &self,
        workspace: Option<&Path>,
        request: InstallSkillRequest,
    ) -> Result<SkillInfo> {
        self.install_checked(workspace, request, None).await
    }

    pub(crate) async fn install_checked(
        &self,
        workspace: Option<&Path>,
        request: InstallSkillRequest,
        policy: Option<crate::sandbox::FilesystemPolicy>,
    ) -> Result<SkillInfo> {
        ensure!(
            request.path.is_absolute(),
            "skill source must be an absolute directory path"
        );
        let root = self.destination(workspace, request.scope)?;
        let settings = self.settings.clone();
        let _write = self.writes.lock().await;
        let info = tokio::task::spawn_blocking(move || {
            let source = match request.scope {
                SkillScope::User => SkillSource::User,
                SkillScope::Project => SkillSource::Project,
            };
            let package =
                loader::package_with_policy(&request.path, source, &settings, policy.as_ref())?;
            if let Some(policy) = &policy {
                policy.check_read(&package.root, &package.root)?;
                for resource in package.files.keys() {
                    policy.check_read(&package.root, &package.root.join(resource))?;
                }
                if request.scope == SkillScope::Project {
                    check_write_policy(policy, &root.join(&package.info.name))?;
                    for resource in package.files.keys() {
                        policy.check_write(&root, &root.join(&package.info.name).join(resource))?;
                    }
                }
            }
            let files = package
                .files
                .iter()
                .map(|(path, digest)| {
                    let limit = if path == "SKILL.md" {
                        settings.max_instruction_bytes
                    } else {
                        settings.max_resource_bytes
                    };
                    let bytes = loader::read_file(&package.root, Path::new(path), limit)?;
                    ensure!(
                        &loader::digest(&bytes) == digest,
                        "source skill changed while installing"
                    );
                    Ok((path.clone(), bytes))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            publish(
                &root,
                &package.info.name,
                request.scope,
                files,
                request.expected_fingerprint.as_deref(),
                &settings,
            )
        })
        .await??;
        self.catalogs.lock().await.clear();
        self.catalog(workspace, true).await?;
        self.list(workspace, false)
            .await?
            .into_iter()
            .find(|p| p.id == info.id)
            .context("installed skill was not discovered")
    }
}

fn checked_directory(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "skill destination must be absolute");
    let mut current = PathBuf::new();
    for component in path.components() {
        ensure!(
            !matches!(component, Component::ParentDir),
            "skill directory cannot contain traversal"
        );
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                // macOS exposes temporary directories through these system-owned aliases.
                let system_alias = cfg!(target_os = "macos")
                    && (current == Path::new("/var") || current == Path::new("/tmp"));
                ensure!(
                    (meta.is_dir() && !meta.file_type().is_symlink())
                        || (system_alias && current.is_dir()),
                    "skill directory cannot contain symlinks or non-directories"
                );
            }
            Err(error) if error.kind() == ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn publish(
    root: &Path,
    name: &str,
    scope: SkillScope,
    files: BTreeMap<String, Vec<u8>>,
    expected: Option<&str>,
    settings: &SkillSettings,
) -> Result<SkillInfo> {
    ensure!(loader::safe_name(name), "invalid skill name");
    ensure!(
        files.contains_key("SKILL.md") && files.len() <= 256,
        "package requires SKILL.md and at most 256 files"
    );
    let mut total = 0usize;
    for (path, bytes) in &files {
        ensure!(
            !path.contains('\\')
                && !path.chars().any(char::is_control)
                && Path::new(path)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)))
                && !path.is_empty(),
            "resource path must be relative without traversal"
        );
        ensure!(
            Path::new(path).components().count() <= 9,
            "skill directory nesting exceeds limit"
        );
        let limit = if path == "SKILL.md" {
            settings.max_instruction_bytes
        } else {
            settings.max_resource_bytes
        };
        ensure!(bytes.len() <= limit, "skill file exceeds size limit");
        total = total
            .checked_add(bytes.len())
            .context("skill package size overflow")?;
        ensure!(total <= 8 * 1024 * 1024, "skill package exceeds 8 MiB");
    }
    checked_directory(root)?;
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600);
    }
    let lock = options.open(root.join(".write.lock"))?;
    ensure!(
        lock.metadata()?.is_file(),
        "skill lock must be a regular file"
    );
    lock.lock()?;
    let destination = root.join(name);
    let source = match scope {
        SkillScope::User => SkillSource::User,
        SkillScope::Project => SkillSource::Project,
    };
    let entries = fs::read_dir(root)?
        .filter(|entry| {
            entry
                .as_ref()
                .map(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .unwrap_or(true)
        })
        .take(129)
        .collect::<std::io::Result<Vec<_>>>()?;
    ensure!(
        entries.len() <= 128 && (destination.exists() || entries.len() < 128),
        "skill directory capacity reached"
    );
    for entry in entries {
        if entry.path() == destination || entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if entry.file_type()?.is_dir() {
            if let Ok(other) = loader::package(&entry.path(), source, settings) {
                ensure!(
                    other.info.name != name,
                    "skill name already exists in a different directory"
                );
            }
        }
    }
    let exists = fs::symlink_metadata(&destination).is_ok();
    if exists {
        let current = loader::package(&destination, source, settings)?;
        ensure!(
            current.info.name == name && expected == Some(current.info.fingerprint.as_str()),
            "skill already exists or changed; review it and provide its current expected_fingerprint"
        );
    } else {
        ensure!(expected.is_none(), "skill to update no longer exists");
    }
    let stage = tempfile::Builder::new()
        .prefix(".skill-stage-")
        .tempdir_in(root)?;
    for (path, bytes) in files {
        let path = stage.path().join(path);
        fs::create_dir_all(path.parent().context("resource has no parent")?)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    let staged = loader::package(stage.path(), source, settings)?;
    ensure!(
        staged.info.name == name,
        "skill name must match its destination directory"
    );
    // A hidden backup is kept until the staged package has replaced the old directory.
    let backup = tempfile::Builder::new()
        .prefix(".skill-backup-")
        .tempdir_in(root)?;
    let previous = backup.path().join("previous");
    if exists {
        fs::rename(&destination, &previous)?;
    }
    if let Err(error) = fs::rename(stage.path(), &destination) {
        if exists {
            if let Err(restore) = fs::rename(&previous, &destination) {
                let retained = backup.keep();
                anyhow::bail!(
                    "skill replacement failed: {error}; restore failed: {restore}; previous package retained at {}",
                    retained.display()
                );
            }
        }
        return Err(error.into());
    }
    Ok(loader::package(&destination, source, settings)?.info)
}

fn check_write_policy(policy: &crate::sandbox::FilesystemPolicy, destination: &Path) -> Result<()> {
    let destination = policy.check_write(destination, destination)?;
    ensure!(
        !policy
            .readonly_paths
            .iter()
            .chain(&policy.denied_paths)
            .any(|path| path.starts_with(&destination)),
        "skill replacement would modify a protected descendant"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::SkillManager;
    use crate::sandbox::FilesystemPolicy;

    use std::{collections::BTreeMap, fs};

    use kyler_protocol::api::skills::{InstallSkillRequest, SaveSkillRequest, SkillScope};

    #[tokio::test]
    async fn installation_obeys_source_and_destination_descendant_restrictions() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let workspace = root.join("workspace");
        let source = workspace.join("source");
        fs::create_dir_all(source.join("references")).unwrap();
        fs::write(
            source.join("SKILL.md"),
            "---\nname: imported\ndescription: Test imported package\n---\nInstructions",
        )
        .unwrap();
        fs::write(source.join("references/private.txt"), "Private reference").unwrap();
        let settings = kyler_config::SkillSettings {
            user_directory: root.join("user"),
            state_file: root.join("state.json"),
            ..Default::default()
        };
        let manager = SkillManager::new(settings).unwrap();
        let mut policy = FilesystemPolicy {
            readwrite_paths: vec![workspace.clone()],
            denied_paths: vec![source.join("references/private.txt")],
            ..Default::default()
        };
        let request = InstallSkillRequest {
            path: source.clone(),
            scope: SkillScope::Project,
            expected_fingerprint: None,
        };
        assert!(
            manager
                .install_checked(Some(&workspace), request.clone(), Some(policy.clone()))
                .await
                .is_err()
        );
        let destination = workspace.join(".kyler/skills/imported");
        assert!(!destination.exists());
        policy.denied_paths.clear();
        policy
            .readonly_paths
            .push(destination.join("protected.txt"));
        assert!(
            manager
                .install_checked(Some(&workspace), request.clone(), Some(policy.clone()))
                .await
                .is_err()
        );
        assert!(
            manager
                .check_destination_policy(
                    Some(&workspace),
                    SkillScope::Project,
                    "imported",
                    &policy
                )
                .is_err()
        );
        assert!(!destination.exists());
        policy.readonly_paths.clear();
        assert!(
            manager
                .install_checked(Some(&workspace), request, Some(policy))
                .await
                .is_ok()
        );
        assert!(destination.join("SKILL.md").is_file());
    }

    #[tokio::test]
    async fn rejected_updates_leave_existing_package_and_resources_intact() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let settings = kyler_config::SkillSettings {
            user_directory: root.join("user"),
            state_file: root.join("state.json"),
            ..Default::default()
        };
        let manager = SkillManager::new(settings.clone()).unwrap();
        let mut request = SaveSkillRequest {
            name: "existing".into(), scope: SkillScope::User, expected_fingerprint: None,
            files: BTreeMap::from([("SKILL.md".into(), "---\nname: existing\ndescription: Existing package\n---\nPreserved instructions".into()), ("references/guide.md".into(), "Preserved resource".into())]),
        };
        let info = manager.save(None, request.clone()).await.unwrap();
        request.expected_fingerprint = Some(info.fingerprint);
        request
            .files
            .insert("SKILL.md".into(), "invalid metadata".into());
        assert!(manager.save(None, request).await.is_err());
        assert!(
            fs::read_to_string(settings.user_directory.join("existing/SKILL.md"))
                .unwrap()
                .contains("Preserved instructions")
        );
        assert_eq!(
            fs::read_to_string(settings.user_directory.join("existing/references/guide.md"))
                .unwrap(),
            "Preserved resource"
        );
    }
}
