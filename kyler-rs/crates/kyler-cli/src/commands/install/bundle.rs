//! Verify every regular bundle file before execution and again after staging.
use crate::{CliError, Result};

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

pub(super) fn digest(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(CliError::io)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(CliError::io)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn regular_files(root: &Path, directory: &Path, files: &mut BTreeSet<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(CliError::io)? {
        let path = entry.map_err(CliError::io)?.path();
        let kind = fs::symlink_metadata(&path)
            .map_err(CliError::io)?
            .file_type();
        if kind.is_dir() {
            regular_files(root, &path, files)?;
        } else if kind.is_file() {
            files.insert(path.strip_prefix(root).unwrap().to_path_buf());
        } else {
            return Err(CliError::new(
                2,
                "Bundle links and special files are not supported.",
            ));
        }
    }
    Ok(())
}
pub(super) fn verify(root: &Path) -> Result<String> {
    let mut files = BTreeSet::new();
    regular_files(root, root, &mut files)?;
    let manifest = root.join("SHA256SUMS");
    let text = fs::read_to_string(&manifest)
        .map_err(|_| CliError::new(2, "Bundle SHA256SUMS is missing or unreadable."))?;
    let mut expected = BTreeMap::new();
    for line in text.lines().filter(|s| !s.is_empty()) {
        let (hash, name) = line
            .split_once("  ")
            .or_else(|| line.split_once(" *"))
            .ok_or_else(|| CliError::new(2, "Invalid bundle checksum entry."))?;
        let path = PathBuf::from(name);
        if hash.len() != 64
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || path.as_os_str().is_empty()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || path == Path::new("SHA256SUMS")
            || expected.insert(path, hash.to_ascii_lowercase()).is_some()
        {
            return Err(CliError::new(
                2,
                "Unsafe or duplicate bundle checksum entry.",
            ));
        }
    }
    for required in ["bin/kyler", "runtime/node", "web/server.js"] {
        if !expected.contains_key(Path::new(required)) {
            return Err(CliError::new(2, format!("Missing checksum for {required}")));
        }
    }
    files.remove(Path::new("SHA256SUMS"));
    if files != expected.keys().cloned().collect() {
        return Err(CliError::new(
            2,
            "Bundle manifest must describe every regular file.",
        ));
    }
    for (path, hash) in expected {
        if digest(&root.join(path))? != hash {
            return Err(CliError::new(2, "Bundle checksum mismatch."));
        }
    }
    digest(&manifest)
}
pub(super) fn copy(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source).map_err(CliError::io)? {
        let path = entry.map_err(CliError::io)?.path();
        let target = destination.join(path.file_name().unwrap());
        let kind = fs::symlink_metadata(&path)
            .map_err(CliError::io)?
            .file_type();
        if kind.is_dir() {
            fs::create_dir(&target).map_err(CliError::io)?;
            copy(&path, &target)?;
        } else if kind.is_file() {
            fs::copy(path, target).map_err(CliError::io)?;
        } else {
            return Err(CliError::new(
                2,
                "Bundle changed during copy; links are not supported.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let mut manifest = String::new();
        for name in ["bin/kyler", "runtime/node", "web/server.js"] {
            let path = root.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, name).unwrap();
            manifest.push_str(&format!("{}  {name}\n", digest(&path).unwrap()));
        }
        fs::write(root.path().join("SHA256SUMS"), manifest).unwrap();
        root
    }
    #[test]
    fn rejects_modified_unlisted_traversal_and_duplicate_files() {
        let root = fixture();
        let hash = verify(root.path()).unwrap();
        let copy_root = tempfile::tempdir().unwrap();
        copy(root.path(), copy_root.path()).unwrap();
        assert_eq!(verify(copy_root.path()).unwrap(), hash);
        fs::write(root.path().join("extra"), "unverified").unwrap();
        assert!(verify(root.path()).is_err());
        fs::remove_file(root.path().join("extra")).unwrap();
        let manifest = fs::read_to_string(root.path().join("SHA256SUMS")).unwrap();
        for extra in [format!("{}  ../escape\n", "a".repeat(64)), manifest.clone()] {
            fs::write(root.path().join("SHA256SUMS"), format!("{manifest}{extra}")).unwrap();
            assert!(verify(root.path()).is_err());
        }
        fs::write(root.path().join("SHA256SUMS"), manifest).unwrap();
        fs::write(root.path().join("bin/kyler"), "tampered").unwrap();
        assert!(verify(root.path()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_bundle_symlinks() {
        let root = fixture();
        std::os::unix::fs::symlink("bin/kyler", root.path().join("link")).unwrap();
        assert!(verify(root.path()).is_err());
    }
}
