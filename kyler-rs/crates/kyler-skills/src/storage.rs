//! Validated package writes with conflict detection and rollback on replacement failure.
use super::{manager::SkillManager, package as loader};

use std::{
    collections::BTreeMap,
    fs,
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

use kyler_config::SkillSettings;
use kyler_protocol::api::skills::{
    InstallSkillRequest, SaveSkillRequest, SkillDraftReviewStatus, SkillInfo, SkillScope,
    SkillSource,
};

pub use kyler_protocol::api::skills::{SkillDraft, SkillDraftDetails};
fn review_path(root: &Path, fingerprint: &str) -> Result<PathBuf> {
    ensure!(
        fingerprint.len() == 64 && fingerprint.bytes().all(|c| c.is_ascii_hexdigit()),
        "Invalid skill review fingerprint"
    );
    let directory = root.join(".draft-reviews");
    Ok(directory.join(format!("{fingerprint}.json")))
}
fn read_draft(root: &Path, name: &str, settings: &SkillSettings) -> Result<SkillDraftDetails> {
    ensure!(loader::safe_name(name), "invalid skill draft name");
    checked_directory(root)?;
    let path = root.join(name);
    let package = loader::package(&path, SkillSource::User, settings)?;
    ensure!(
        package.info.name == name,
        "Draft name does not match directory"
    );
    let mut files = BTreeMap::new();
    for (relative, digest) in &package.files {
        let bytes = loader::read_file(
            &package.root,
            Path::new(relative),
            if relative == "SKILL.md" {
                settings.max_instruction_bytes
            } else {
                settings.max_resource_bytes
            },
        )?;
        ensure!(
            loader::digest(&bytes) == *digest,
            "Skill draft changed while reading"
        );
        files.insert(relative.clone(), String::from_utf8(bytes)?);
    }
    Ok(SkillDraftDetails {
        info: SkillDraft {
            name: package.info.name,
            description: package.info.description,
            fingerprint: package.info.fingerprint,
            resources: package.info.resources,
        },
        files,
    })
}

impl SkillManager {
    /// Drafts live outside discovery and cannot shadow or enable installed skills.
    pub async fn save_draft(&self, request: SaveSkillRequest) -> Result<SkillDraft> {
        let _review = self.reviews.lock().await;
        ensure!(
            request.scope == SkillScope::User,
            "Skill drafts currently use user scope only"
        );
        let root = self.destination(None, SkillScope::User)?.join(".drafts");
        let settings = self.settings.clone();
        let _write = self.writes.lock().await;
        tokio::task::spawn_blocking(move || {
            let info = publish(
                &root,
                &request.name,
                SkillScope::User,
                request
                    .files
                    .into_iter()
                    .map(|(path, text)| (path, text.into_bytes()))
                    .collect(),
                request.expected_fingerprint.as_deref(),
                &settings,
            )?;
            Ok(SkillDraft {
                name: info.name,
                description: info.description,
                fingerprint: info.fingerprint,
                resources: info.resources,
            })
        })
        .await?
    }
    pub async fn draft_details(&self, name: &str) -> Result<SkillDraftDetails> {
        let root = self.destination(None, SkillScope::User)?.join(".drafts");
        let settings = self.settings.clone();
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || read_draft(&root, &name, &settings)).await?
    }
    pub async fn drafts(&self) -> Result<Vec<SkillDraft>> {
        let root = self.destination(None, SkillScope::User)?.join(".drafts");
        let settings = self.settings.clone();
        tokio::task::spawn_blocking(move || {
            if !root.exists() {
                return Ok(Vec::new());
            }
            checked_directory(&root)?;
            let mut drafts = Vec::new();
            for entry in fs::read_dir(&root)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                ensure!(drafts.len() < 128, "Skill draft capacity exceeded");
                drafts.push(read_draft(&root, &name, &settings)?.info);
            }
            drafts.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(drafts)
        })
        .await?
    }
    /// Host-only promotion: publish exactly the inspected bytes through normal validation.
    pub async fn publish_draft(
        &self,
        name: &str,
        fingerprint: &str,
        expected_installed: Option<String>,
    ) -> Result<SkillInfo> {
        let _review = self.reviews.lock().await;
        let draft = self.draft_details(name).await?;
        ensure!(
            self.draft_review_status(fingerprint).await? != SkillDraftReviewStatus::Discarded,
            "Skill proposal was discarded"
        );
        ensure!(
            draft.info.fingerprint == fingerprint,
            "Skill draft changed; review the current fingerprint"
        );
        let info = self
            .save(
                None,
                SaveSkillRequest {
                    name: name.into(),
                    scope: SkillScope::User,
                    files: draft.files,
                    expected_fingerprint: expected_installed,
                },
            )
            .await?;
        self.record_draft_review(fingerprint, SkillDraftReviewStatus::Installed)
            .await?;
        Ok(info)
    }
    pub async fn draft_review_status(&self, fingerprint: &str) -> Result<SkillDraftReviewStatus> {
        let path = review_path(&self.destination(None, SkillScope::User)?, fingerprint)?;
        tokio::task::spawn_blocking(move || {
            checked_directory(path.parent().context("Missing skill review directory")?)?;
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    return Ok(SkillDraftReviewStatus::Pending);
                }
                Err(error) => return Err(error.into()),
                Ok(meta) => ensure!(
                    meta.is_file() && !meta.file_type().is_symlink() && meta.len() < 64,
                    "Invalid skill review record"
                ),
            }
            Ok(serde_json::from_slice(&fs::read(path)?)?)
        })
        .await?
    }
    async fn record_draft_review(
        &self,
        fingerprint: &str,
        status: SkillDraftReviewStatus,
    ) -> Result<()> {
        let path = review_path(&self.destination(None, SkillScope::User)?, fingerprint)?;
        tokio::task::spawn_blocking(move || {
            let root = path.parent().context("Missing skill review directory")?;
            checked_directory(root)?;
            let mut stage = tempfile::NamedTempFile::new_in(root)?;
            stage.write_all(&serde_json::to_vec(&status)?)?;
            stage.as_file().sync_all()?;
            stage.persist(path)?;
            Ok(())
        })
        .await?
    }
    pub async fn discard_draft(&self, name: &str, fingerprint: &str) -> Result<()> {
        let _review = self.reviews.lock().await;
        let draft = self.draft_details(name).await?;
        ensure!(
            draft.info.fingerprint == fingerprint,
            "Skill draft changed; review the current fingerprint"
        );
        ensure!(
            self.draft_review_status(fingerprint).await? != SkillDraftReviewStatus::Installed,
            "Skill proposal was already installed"
        );
        self.record_draft_review(fingerprint, SkillDraftReviewStatus::Discarded)
            .await
    }
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

    pub fn check_destination_access(
        &self,
        workspace: Option<&Path>,
        scope: SkillScope,
        name: &str,
        policy: &dyn crate::SkillFileAccess,
    ) -> Result<()> {
        if scope == SkillScope::Project {
            policy.check_replace(&self.destination(workspace, scope)?.join(name))?;
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
        self.install_with_access(workspace, request, None).await
    }

    pub async fn install_with_access(
        &self,
        workspace: Option<&Path>,
        request: InstallSkillRequest,
        policy: Option<std::sync::Arc<dyn crate::SkillFileAccess>>,
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
                loader::package_with_policy(&request.path, source, &settings, policy.as_deref())?;
            if let Some(policy) = &policy {
                policy.check_read(&package.root, &package.root)?;
                for resource in package.files.keys() {
                    policy.check_read(&package.root, &package.root.join(resource))?;
                }
                if request.scope == SkillScope::Project {
                    policy.check_replace(&root.join(&package.info.name))?;
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
                    if let Some(policy) = &policy {
                        policy.check_read(&package.root, &package.root.join(path))?;
                    }
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
