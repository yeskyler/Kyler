use super::policy::FilesystemPolicy;

use std::{
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

/// Resolve symlinked existing ancestors, also for not-yet-created paths.
/// Reject parent traversal instead of lexically rewriting it through symlinks.
pub fn resolve_path(base: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(!path.as_os_str().is_empty(), "path cannot be empty");
    ensure!(
        !path.components().any(|c| matches!(c, Component::ParentDir)),
        "parent traversal is not allowed: {}",
        path.display()
    );
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut existing = joined.as_path();
    let mut tail = Vec::new();
    loop {
        match std::fs::canonicalize(existing) {
            Ok(mut resolved) => {
                for part in tail.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                ensure!(
                    !std::fs::symlink_metadata(existing).is_ok_and(|m| m.file_type().is_symlink()),
                    "cannot resolve dangling symlink: {}",
                    existing.display()
                );
                tail.push(
                    existing
                        .file_name()
                        .context("cannot resolve path ancestor")?
                        .to_os_string(),
                );
                existing = existing.parent().context("cannot resolve path ancestor")?;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("cannot resolve {}", joined.display()));
            }
        }
    }
}
impl FilesystemPolicy {
    pub fn normalize(&mut self) -> Result<()> {
        let base = Path::new("/");
        for path in self
            .readonly_paths
            .iter_mut()
            .chain(&mut self.readwrite_paths)
            .chain(&mut self.denied_paths)
            .chain(&mut self.write_protected_paths)
        {
            *path = resolve_path(base, path)?;
        }
        Ok(())
    }
    pub fn check_read(&self, base: &Path, path: &Path) -> Result<PathBuf> {
        let path = resolve_path(base, path)?;
        ensure!(
            !self.denied_paths.iter().any(|p| path.starts_with(p)),
            "sandbox read denied: {}",
            path.display()
        );
        ensure!(
            self.readonly_paths
                .iter()
                .chain(&self.readwrite_paths)
                .any(|p| path.starts_with(p)),
            "sandbox path is outside readable roots: {}",
            path.display()
        );
        Ok(path)
    }
    pub fn check_write(&self, base: &Path, path: &Path) -> Result<PathBuf> {
        let path = self.check_read(base, path)?;
        ensure!(
            !self
                .write_protected_paths
                .iter()
                .any(|root| path.starts_with(root)),
            "sandbox protected path is read-only: {}",
            path.display()
        );
        let write = self
            .readwrite_paths
            .iter()
            .filter(|root| path.starts_with(root))
            .map(|root| root.components().count())
            .max();
        let readonly = self
            .readonly_paths
            .iter()
            .filter(|root| path.starts_with(root))
            .map(|root| root.components().count())
            .max();
        ensure!(
            write.is_some_and(|depth| readonly.is_none_or(|read| depth > read)),
            "sandbox write denied: {}",
            path.display()
        );
        Ok(path)
    }
}
