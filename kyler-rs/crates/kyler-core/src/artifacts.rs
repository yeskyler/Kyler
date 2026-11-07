//! Capture explicit final-message file references; never serve arbitrary workspace paths.
mod resources;

use crate::environment::ExecutionEnvironment;

use std::{
    collections::{HashSet, VecDeque},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use percent_encoding::percent_decode_str;
use pulldown_cmark::{Event, Options, Parser, Tag};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use kyler_protocol::api::artifacts::{ArtifactInfo, MAX_ARTIFACT_BYTES};

#[derive(Clone)]
pub struct ArtifactStore {
    root: PathBuf,
}
impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn session(&self, session: &str) -> Result<PathBuf> {
        ensure!(
            Uuid::parse_str(session)?.to_string() == session,
            "invalid session ID"
        );
        Ok(self.root.join(session))
    }
    pub fn get(&self, session: &str, id: &str) -> Result<ArtifactInfo> {
        ensure!(
            Uuid::parse_str(id)?.to_string() == id,
            "invalid artifact ID"
        );
        let directory = self.session(session)?.join(id);
        let metadata = fs::symlink_metadata(&directory)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "artifact is unavailable"
        );
        let mut bytes = Vec::new();
        open_file(&directory, Path::new("metadata.json"))?
            .take(16385)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 16384, "invalid artifact metadata");
        let info: ArtifactInfo = serde_json::from_slice(&bytes)?;
        ensure!(
            info.id == id
                && info.session_id == session
                && info.size_bytes <= MAX_ARTIFACT_BYTES as u64,
            "artifact identity mismatch"
        );
        Ok(info)
    }
    pub fn list(&self, session: &str) -> Result<Vec<ArtifactInfo>> {
        let root = self.session(session)?;
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            if let Ok(info) = self.get(session, &entry.file_name().to_string_lossy()) {
                result.push(info);
            }
        }
        Ok(result)
    }
    pub fn capture(
        &self,
        session: &str,
        execution: &str,
        output: &str,
        environment: Option<&ExecutionEnvironment>,
    ) -> Result<()> {
        ensure!(
            Uuid::parse_str(execution)?.to_string() == execution,
            "invalid execution ID"
        );
        let existing = self.list(session)?;
        let mut seen = existing
            .iter()
            .filter(|info| info.execution_id == execution)
            .map(|info| info.reference.clone())
            .collect::<HashSet<_>>();
        let mut count = existing.len();
        let mut retained_bytes = existing.iter().map(|info| info.size_bytes).sum::<u64>();
        let references = Parser::new_ext(output, Options::all())
            .filter_map(|event| match event {
                Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                    Some(dest_url.into_string())
                }
                _ => None,
            })
            .filter(|reference| reference.len() <= 4096 && local_reference(reference))
            .take(32);
        let mut queue = references
            .map(|reference| (reference, false))
            .collect::<VecDeque<_>>();
        let mut resources = 0usize;
        while let Some((reference, resource)) = queue.pop_front() {
            if !seen.insert(reference.clone()) {
                continue;
            }
            ensure!(count < 256, "session artifact limit reached");
            count += 1;
            let snapshot = environment
                .context("no authorized workspace")
                .and_then(|environment| read_source(&reference, environment))
                .and_then(|snapshot| {
                    ensure!(
                        retained_bytes + snapshot.1.len() as u64 <= 200 * 1024 * 1024,
                        "session artifact byte limit reached"
                    );
                    Ok(snapshot)
                });
            let fallback = reference.split(['?', '#']).next().unwrap_or(&reference);
            let mut info = ArtifactInfo {
                id: Uuid::new_v4().to_string(),
                session_id: session.into(),
                execution_id: execution.into(),
                reference: reference.clone(),
                resource,
                name: Path::new(fallback)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("file")
                    .chars()
                    .take(255)
                    .collect(),
                mime_type: "application/octet-stream".into(),
                size_bytes: 0,
                sha256: None,
                available: false,
                error: Some("File unavailable: it may be missing, outside the workspace, restricted, changing, or exceeds a storage limit (20 MiB per file).".into()),
            };
            let data = match snapshot {
                Ok((name, bytes)) => {
                    if resources < 24 {
                        for asset in resources::references(&name, &bytes) {
                            if let Some(resolved) = resources::resolve(&reference, &asset) {
                                if resources == 24 {
                                    break;
                                }
                                if !seen.contains(&resolved)
                                    && !queue.iter().any(|(reference, _)| reference == &resolved)
                                {
                                    queue.push_back((resolved, true));
                                    resources += 1;
                                }
                            }
                        }
                    }
                    retained_bytes += bytes.len() as u64;
                    info.name = name;
                    info.mime_type = mime(&info.name).into();
                    info.size_bytes = bytes.len() as u64;
                    info.sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
                    info.available = true;
                    info.error = None;
                    Some(bytes)
                }
                Err(_) => None,
            };
            let root = self.session(session)?;
            fs::create_dir_all(&root)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            }
            let stage = tempfile::Builder::new()
                .prefix(".artifact-")
                .tempdir_in(&root)?;
            if let Some(data) = data {
                write_file(&stage.path().join("content"), &data)?;
            }
            write_file(
                &stage.path().join("metadata.json"),
                &serde_json::to_vec(&info)?,
            )?;
            fs::rename(stage.path(), root.join(&info.id))?;
        }
        Ok(())
    }
    pub fn download(&self, session: &str, id: &str) -> Result<(ArtifactInfo, Vec<u8>)> {
        let info = self.get(session, id)?;
        ensure!(info.available, "artifact is unavailable");
        let root = self.session(session)?.join(id);
        let mut bytes = Vec::new();
        open_file(&root, Path::new("content"))?
            .take(MAX_ARTIFACT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == info.size_bytes
                && info.sha256.as_deref() == Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
            "artifact content mismatch"
        );
        Ok((info, bytes))
    }
    pub fn delete_session(&self, session: &str) -> Result<()> {
        let root = self.session(session)?;
        if root.exists() {
            fs::remove_dir_all(root)?;
        }
        Ok(())
    }
}
fn local_reference(reference: &str) -> bool {
    if reference.is_empty() || reference.starts_with(['#', '?']) || reference.starts_with("//") {
        return false;
    }
    !reference
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .contains(':')
}
fn read_source(reference: &str, environment: &ExecutionEnvironment) -> Result<(String, Vec<u8>)> {
    ensure!(reference.len() <= 4096, "file reference is too long");
    let reference = reference.split(['?', '#']).next().unwrap_or(reference);
    let decoded = percent_decode_str(reference).decode_utf8()?;
    ensure!(
        !decoded.chars().any(char::is_control) && !decoded.contains('\\'),
        "invalid file reference"
    );
    let root = &environment.session_environment().workspace_root;
    let path = Path::new(decoded.as_ref());
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        "parent traversal is not allowed"
    );
    let relative = if path.is_absolute() {
        path.strip_prefix(root)?
    } else {
        path
    };
    let relative = relative
        .components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect::<PathBuf>();
    ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "invalid relative path"
    );
    let mut current = root.clone();
    for part in relative.components() {
        current.push(part);
        ensure!(
            !fs::symlink_metadata(&current)?.file_type().is_symlink(),
            "artifact symlinks are not allowed"
        );
    }
    environment
        .policy()
        .filesystem
        .check_read(root, &relative)?;
    let mut file = open_file(root, &relative)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_ARTIFACT_BYTES as u64,
        "invalid file size or type"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_ARTIFACT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    ensure!(
        bytes.len() <= MAX_ARTIFACT_BYTES
            && metadata.len() == after.len()
            && metadata.modified()? == after.modified()?
            && bytes.len() as u64 == after.len(),
        "file changed during publication"
    );
    let name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid file name")?
        .to_owned();
    Ok((name, bytes))
}
fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    Ok(())
}
fn mime(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "zip" => "application/zip",
        "txt" | "md" | "csv" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(unix)]
fn open_file(root: &Path, relative: &Path) -> Result<fs::File> {
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
                "artifact file is not a regular file"
            );
            return Ok(file);
        }
        directory = file;
    }
    anyhow::bail!("empty artifact file path")
}
#[cfg(not(unix))]
fn open_file(root: &Path, relative: &Path) -> Result<fs::File> {
    Ok(fs::File::open(root.join(relative))?)
}
