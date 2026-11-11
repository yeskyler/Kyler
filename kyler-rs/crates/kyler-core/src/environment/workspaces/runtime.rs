//! Discover host-installed tools without importing the host's shell or credentials.
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use crate::sandbox::policy::RustupRuntime;

#[derive(Default)]
pub(super) struct Runtime {
    pub readonly_paths: Vec<PathBuf>,
    pub search_directories: Vec<PathBuf>,
    pub rustup: Option<RustupRuntime>,
}
impl Runtime {
    pub fn discover() -> Self {
        Self::from_environment(&std::env::vars_os().collect())
    }

    fn from_environment(environment: &BTreeMap<OsString, OsString>) -> Self {
        let value = |name: &str| environment.get(std::ffi::OsStr::new(name));
        let home = value("HOME").map(PathBuf::from);
        let cargo_home = value("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".cargo")));
        let rustup_home = value("RUSTUP_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".rustup")))
            .and_then(|home| home.canonicalize().ok())
            .filter(|home| {
                home.join("settings.toml").is_file() && home.join("toolchains").is_dir()
            });
        let mut directories = value("PATH")
            .map(|path| {
                std::env::split_paths(path)
                    .filter(|path| path.is_absolute())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(home) = cargo_home {
            directories.push(home.join("bin"));
        }
        let mut runtime = Self::default();
        if let Some(binary) = find_executable(&directories, "rg") {
            runtime.add_directory(binary.parent().unwrap().to_path_buf());
            runtime.readonly_paths.push(binary);
        }
        if let Some(home) = rustup_home {
            if let Some(binary) = find_executable(&directories, "rustup") {
                runtime
                    .readonly_paths
                    .extend([home.clone(), binary.clone()]);
                // Linked custom toolchains resolve outside RUSTUP_HOME. Only targets
                // registered by the host's installed rustup are added as runtime roots.
                if let Ok(toolchains) = std::fs::read_dir(home.join("toolchains")) {
                    for entry in toolchains.flatten() {
                        if let Ok(path) = entry.path().canonicalize() {
                            if path.join("bin/rustc").is_file() && !path.starts_with(&home) {
                                for directory in [path.join("bin"), path.join("lib")] {
                                    if directory.is_dir() {
                                        runtime.readonly_paths.push(directory);
                                    }
                                }
                            }
                        }
                    }
                }
                runtime.rustup = Some(RustupRuntime {
                    home,
                    binary,
                    toolchain: value("RUSTUP_TOOLCHAIN")
                        .and_then(|value| value.to_str())
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned),
                });
                if let Some(toolchain) = runtime
                    .rustup
                    .as_ref()
                    .and_then(|rustup| rustup.toolchain.as_ref())
                    .map(PathBuf::from)
                {
                    if toolchain.is_absolute() {
                        if let Ok(path) = toolchain.canonicalize() {
                            for directory in [path.join("bin"), path.join("lib")] {
                                if directory.is_dir() {
                                    runtime.readonly_paths.push(directory);
                                }
                            }
                        }
                    }
                }
            }
        }
        if runtime.rustup.is_none() {
            // Homebrew and system Rust installations do not need rustup proxies.
            for tool in ["cargo", "rustc", "rustdoc", "rustfmt"] {
                if let Some(binary) = find_executable(&directories, tool)
                    .filter(|path| path.file_stem().is_none_or(|name| name != "rustup"))
                {
                    let directory = binary.parent().unwrap();
                    runtime.add_directory(directory.to_path_buf());
                    runtime.readonly_paths.push(directory.to_path_buf());
                    if let Some(root) = directory.parent() {
                        let libraries = root.join("lib");
                        if libraries.is_dir() {
                            runtime.readonly_paths.push(libraries);
                        }
                    }
                }
            }
        }
        runtime.readonly_paths.sort();
        runtime.readonly_paths.dedup();
        runtime
    }
    fn add_directory(&mut self, directory: PathBuf) {
        if !self.search_directories.contains(&directory) {
            self.search_directories.push(directory);
        }
    }
    pub fn search_path(&self) -> String {
        self.search_directories
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .chain(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(str::to_owned))
            .collect::<Vec<_>>()
            .join(":")
    }
}

fn find_executable(directories: &[PathBuf], name: &str) -> Option<PathBuf> {
    directories.iter().find_map(|directory| {
        let candidate = directory.join(name);
        let candidate = candidate.canonicalize().ok()?;
        if !candidate.is_file() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if candidate.metadata().ok()?.permissions().mode() & 0o111 == 0 {
                return None;
            }
        }
        Some(candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::Runtime;

    use std::{collections::BTreeMap, ffi::OsString};

    #[test]
    fn rustup_is_found_without_a_shell_path_and_cargo_credentials_are_not_exposed() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().canonicalize().unwrap();
        let bin = home.join(".cargo/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(home.join(".rustup/toolchains")).unwrap();
        std::fs::write(home.join(".rustup/settings.toml"), "version = '12'\n").unwrap();
        std::fs::write(bin.join("rustup"), "fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("rustup"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let environment = BTreeMap::from([
            (OsString::from("HOME"), home.as_os_str().to_owned()),
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
        ]);
        let runtime = Runtime::from_environment(&environment);
        assert_eq!(runtime.rustup.unwrap().home, home.join(".rustup"));
        assert!(!runtime.readonly_paths.contains(&home));
        assert!(
            !runtime
                .readonly_paths
                .iter()
                .any(|path| home.join(".cargo/credentials.toml").starts_with(path))
        );
    }
}
