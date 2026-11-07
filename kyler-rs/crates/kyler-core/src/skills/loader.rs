use crate::sandbox::FilesystemPolicy;

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, MAIN_SEPARATOR, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use kyler_config::SkillSettings;
use kyler_protocol::api::skills::{SkillInfo, SkillSource, SkillStatus};

#[derive(Clone)]
pub struct Package {
    pub info: SkillInfo,
    pub root: PathBuf,
    pub instructions: String,
    pub files: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct Metadata {
    name: String,
    description: String,
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}
pub fn read_file(root: &Path, relative: &Path, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "skill resource must be a relative path without traversal"
    );
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "skill symlinks are not allowed"
        );
    }
    ensure!(
        path.canonicalize()?.starts_with(root),
        "skill resource escapes its package"
    );
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= limit as u64,
        "skill resource is not a regular file or exceeds its size limit"
    );
    let mut file = open_resource(root, relative)?.take(limit as u64 + 1);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "skill resource exceeds its size limit"
    );
    Ok(bytes)
}
pub fn package(root: &Path, source: SkillSource, settings: &SkillSettings) -> Result<Package> {
    package_with_policy(root, source, settings, None)
}

pub fn package_with_policy(
    root: &Path,
    source: SkillSource,
    settings: &SkillSettings,
    policy: Option<&FilesystemPolicy>,
) -> Result<Package> {
    ensure!(
        !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "skill directories cannot be symlinks"
    );
    let root = root.canonicalize()?;
    ensure!(root.to_str().is_some(), "skill package paths must be UTF-8");
    if let Some(policy) = policy {
        policy.check_read(&root, &root)?;
        policy.check_read(&root, &root.join("SKILL.md"))?;
    }
    let bytes = read_file(&root, Path::new("SKILL.md"), settings.max_instruction_bytes)?;
    let (name, description, instructions) = instructions(&bytes)?;
    let mut files = BTreeMap::new();
    let mut total = 0;
    collect(
        &root,
        Path::new(""),
        0,
        &mut files,
        &mut total,
        settings,
        policy,
    )?;
    ensure!(
        files.get("SKILL.md") == Some(&digest(&bytes)),
        "skill instructions changed while loading"
    );
    let fingerprint = digest(&serde_json::to_vec(&files)?);
    let id = digest(root.to_string_lossy().as_bytes());
    Ok(Package {
        info: SkillInfo {
            id,
            name,
            description,
            source,
            path: root.to_string_lossy().into_owned(),
            fingerprint,
            status: SkillStatus::Disabled,
            resources: files
                .keys()
                .filter(|p| p.as_str() != "SKILL.md")
                .cloned()
                .collect(),
            error: None,
        },
        root,
        instructions,
        files,
    })
}
fn collect(
    root: &Path,
    relative: &Path,
    depth: usize,
    files: &mut BTreeMap<String, String>,
    total: &mut usize,
    settings: &SkillSettings,
    policy: Option<&FilesystemPolicy>,
) -> Result<()> {
    ensure!(depth <= 8, "skill directory nesting exceeds limit");
    if let Some(policy) = policy {
        policy.check_read(root, &root.join(relative))?;
    }
    let mut entries = fs::read_dir(root.join(relative))?
        .take(257)
        .collect::<std::io::Result<Vec<_>>>()?;
    ensure!(entries.len() <= 256, "too many skill files");
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = relative.join(entry.file_name());
        let kind = entry.file_type()?;
        ensure!(!kind.is_symlink(), "skill symlinks are not allowed");
        if kind.is_dir() {
            collect(root, &path, depth + 1, files, total, settings, policy)?;
        } else {
            ensure!(files.len() < 256, "too many skill files");
            let limit = if path == Path::new("SKILL.md") {
                settings.max_instruction_bytes
            } else {
                settings.max_resource_bytes
            };
            if let Some(policy) = policy {
                policy.check_read(root, &root.join(&path))?;
            }
            let bytes = read_file(root, &path, limit)?;
            *total += bytes.len();
            ensure!(*total <= 8 * 1024 * 1024, "skill package exceeds 8 MiB");
            let path = path
                .to_str()
                .context("skill resource paths must be UTF-8")?
                .replace(MAIN_SEPARATOR, "/");
            ensure!(
                !path.chars().any(char::is_control),
                "invalid skill resource path"
            );
            files.insert(path, digest(&bytes));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn open_resource(root: &Path, relative: &Path) -> Result<fs::File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    };

    let mut directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let name = std::ffi::CString::new(component.as_os_str().as_bytes())?;
        let final_component = index + 1 == components.len();
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if final_component {
                0
            } else {
                libc::O_DIRECTORY
            };
        // Each descriptor is opened relative to an already-open directory, never through a symlink.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let file = unsafe { fs::File::from_raw_fd(fd) };
        if final_component {
            ensure!(
                file.metadata()?.is_file(),
                "skill resource is not a regular file"
            );
            return Ok(file);
        }
        directory = file;
    }
    anyhow::bail!("empty skill resource path")
}
#[cfg(not(unix))]
fn open_resource(root: &Path, relative: &Path) -> Result<fs::File> {
    Ok(fs::File::open(root.join(relative))?)
}

pub(super) fn instructions(bytes: &[u8]) -> Result<(String, String, String)> {
    let text = std::str::from_utf8(bytes)
        .context("SKILL.md must be UTF-8")?
        .replace("\r\n", "\n");
    let mut lines = text.split_inclusive('\n');
    ensure!(
        lines.next().is_some_and(|s| s.trim_end() == "---"),
        "SKILL.md requires YAML frontmatter"
    );
    let mut yaml = String::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
    }
    ensure!(closed, "SKILL.md frontmatter is not closed");
    let mut metadata: Metadata = serde_saphyr::from_str(&yaml)
        .map_err(|_| anyhow::anyhow!("invalid skill YAML metadata"))?;
    metadata.description = metadata
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    ensure!(
        safe_name(&metadata.name),
        "invalid skill name; use lowercase letters, digits and hyphens"
    );
    ensure!(
        !metadata.description.trim().is_empty()
            && metadata.description.len() <= 1024
            && !metadata.description.chars().any(char::is_control),
        "invalid skill description"
    );
    let instructions = lines.collect::<String>().trim().to_owned();
    ensure!(
        !instructions.is_empty(),
        "skill instructions cannot be empty"
    );
    Ok((metadata.name, metadata.description, instructions))
}
