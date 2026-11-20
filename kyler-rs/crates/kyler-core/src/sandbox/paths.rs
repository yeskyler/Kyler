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

/// Runtime payloads follow the executable, including Cargo's adjacent development layout.
pub fn bundled_runtime_root(worker: &Path) -> Option<PathBuf> {
    let worker = worker.canonicalize().ok()?;
    let parent = worker.parent()?;
    let name = parent.file_name()?.to_str()?;
    let root = match name {
        "bin" => parent.parent()?,
        "debug" | "release" => parent,
        "deps"
            if parent
                .parent()?
                .file_name()
                .is_some_and(|n| n == "debug" || n == "release") =>
        {
            parent.parent()?
        }
        _ => return None,
    };
    Some(root.join("runtime"))
}
pub fn bundled_tool_directories(worker: &Path) -> Vec<PathBuf> {
    let Some(root) = bundled_runtime_root(worker) else {
        return Vec::new();
    };
    [root.join("tools"), root.join("sandbox/bin")]
        .into_iter()
        .filter(|p| p.is_dir())
        .collect()
}
pub fn require_bundled_tools(worker: &Path) -> anyhow::Result<()> {
    let runtime = bundled_runtime_root(worker).context("Bundled runtime is missing; prepare the development runtime or reinstall the complete Kyler bundle")?;
    let mut required = vec![runtime.join("tools/rg")];
    if cfg!(target_os = "linux") {
        required.extend(
            [
                "bwrap",
                "slirp4netns",
                "ip",
                "unshare",
                "nsenter",
                "iptables",
                "ip6tables",
                "iptables-restore",
                "ip6tables-restore",
                "sh",
                "sleep",
            ]
            .map(|n| runtime.join("sandbox/bin").join(n)),
        );
    }
    for path in required {
        ensure!(
            std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()),
            "Bundled tool missing: {}. Reinstall the complete Kyler bundle; host tools are never substituted",
            path.display()
        );
        #[cfg(target_os = "linux")]
        if path
            .parent()
            .is_some_and(|p| p.file_name().is_some_and(|n| n == "bin"))
        {
            let name = path.file_name().unwrap();
            let selected = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|directory| directory.join(name))
                .find(|candidate| candidate.is_file());
            ensure!(
                selected.and_then(|p| p.canonicalize().ok()) == path.canonicalize().ok(),
                "Bundled sandbox helper {} is not selected. Start Kyler through its packaged executable; host helpers are never used",
                name.to_string_lossy()
            );
        }
    }
    Ok(())
}

pub fn bundled_search_path(
    worker: &Path,
    existing: &std::ffi::OsStr,
) -> std::io::Result<std::ffi::OsString> {
    let mut directories = bundled_tool_directories(worker);
    if directories.is_empty() {
        return Ok(existing.to_owned());
    }
    for directory in std::env::split_paths(existing).filter(|p| p.is_absolute()) {
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    for path in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from) {
        if !directories.contains(&path) {
            directories.push(path);
        }
    }
    std::env::join_paths(directories).map_err(std::io::Error::other)
}

pub fn bundled_ripgrep(worker: &Path) -> Option<PathBuf> {
    bundled_tool_directories(worker)
        .into_iter()
        .map(|p| p.join("rg"))
        .find(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file()))
}

#[cfg(test)]
mod bundled_tests {
    use super::*;
    #[test]
    fn runtime_paths_follow_the_executable_and_ignore_cwd_and_relative_path_entries() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("runtime/tools")).unwrap();
        std::fs::create_dir_all(root.join("runtime/sandbox/bin")).unwrap();
        std::fs::write(root.join("bin/kyler"), "fixture").unwrap();
        std::fs::write(root.join("runtime/tools/rg"), "fixture").unwrap();
        let directories = bundled_tool_directories(&root.join("bin/kyler"));
        assert_eq!(
            directories,
            [root.join("runtime/tools"), root.join("runtime/sandbox/bin")]
        );
        let value = bundled_search_path(
            &root.join("bin/kyler"),
            std::ffi::OsStr::new("relative:/usr/bin"),
        )
        .unwrap();
        let paths = std::env::split_paths(&value).collect::<Vec<_>>();
        assert_eq!(&paths[..2], directories);
        assert!(!paths.contains(&PathBuf::from("relative")));
        assert_eq!(
            bundled_search_path(&root.join("bin/kyler"), &value).unwrap(),
            value
        );
        std::fs::write(root.join("other-worker"), "fixture").unwrap();
        assert!(bundled_tool_directories(&root.join("other-worker")).is_empty());
        #[cfg(unix)]
        {
            std::fs::remove_file(root.join("runtime/tools/rg")).unwrap();
            std::os::unix::fs::symlink(root.join("other-worker"), root.join("runtime/tools/rg"))
                .unwrap();
            assert!(bundled_ripgrep(&root.join("bin/kyler")).is_none());
            assert!(require_bundled_tools(&root.join("bin/kyler")).is_err());
            std::fs::remove_file(root.join("runtime/tools/rg")).unwrap();
            std::fs::write(root.join("runtime/tools/rg"), "fixture").unwrap();
        }
        assert_eq!(
            bundled_ripgrep(&root.join("bin/kyler")).unwrap(),
            root.join("runtime/tools/rg")
        );
    }
}
