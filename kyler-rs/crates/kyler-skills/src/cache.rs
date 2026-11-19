//! Advisory metadata signatures. Content digests, never timestamps, authorize loads.
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};

use kyler_config::SkillSettings;

pub(super) fn signature(settings: &SkillSettings, workspace: Option<&Path>) -> Result<String> {
    let mut hash = Sha256::new();
    if !settings.enabled {
        return Ok("disabled".into());
    }
    root(&settings.user_directory, &mut hash)?;
    if let Some(workspace) = workspace {
        let mut path = workspace.to_owned();
        for component in settings.project_directory.components() {
            path.push(component);
            if let Ok(meta) = fs::symlink_metadata(&path) {
                ensure!(
                    !meta.file_type().is_symlink(),
                    "project skill directory cannot contain symlinks"
                );
            }
        }
        root(&path, &mut hash)?;
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn metadata(path: &Path, hash: &mut Sha256) -> Result<Option<fs::Metadata>> {
    hash.update(path.as_os_str().as_encoded_bytes());
    hash.update([0]);
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            hash.update(b"missing");
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    hash.update([
        u8::from(meta.is_dir()),
        u8::from(meta.is_file()),
        u8::from(meta.file_type().is_symlink()),
    ]);
    hash.update(meta.len().to_le_bytes());
    if let Ok(time) = meta.modified().and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)
    }) {
        hash.update(time.as_nanos().to_le_bytes());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(meta.dev().to_le_bytes());
        hash.update(meta.ino().to_le_bytes());
        hash.update(meta.ctime().to_le_bytes());
        hash.update(meta.ctime_nsec().to_le_bytes());
    }
    Ok(Some(meta))
}
fn entries(path: &Path, limit: usize) -> Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(path)?
        .filter(|entry| {
            entry
                .as_ref()
                .map(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .unwrap_or(true)
        })
        .take(limit + 1)
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries.into_iter().map(|entry| entry.path()).collect())
}
fn root(path: &Path, hash: &mut Sha256) -> Result<()> {
    let Some(meta) = metadata(path, hash)? else {
        return Ok(());
    };
    // A configured user root may use a host-owned alias; package links remain forbidden.
    if !meta.is_dir() && !meta.file_type().is_symlink() {
        return Ok(());
    }
    let entries = entries(path, 128)?;
    ensure!(entries.len() <= 128, "skill directory exceeds 128 entries");
    for path in entries {
        let mut count = 0;
        package(&path, 0, &mut count, hash)?;
    }
    Ok(())
}
fn package(path: &Path, depth: usize, count: &mut usize, hash: &mut Sha256) -> Result<()> {
    let Some(meta) = metadata(path, hash)? else {
        return Ok(());
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Ok(());
    }
    if depth > 8 || *count > 256 {
        hash.update(b"bounded");
        return Ok(());
    }
    // Include hidden package resources too; they are part of the reviewed manifest.
    let mut children = fs::read_dir(path)?
        .take(257)
        .collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        if *count > 256 {
            hash.update(b"bounded");
            break;
        }
        *count += 1;
        package(&entry.path(), depth + 1, count, hash)?;
    }
    Ok(())
}
