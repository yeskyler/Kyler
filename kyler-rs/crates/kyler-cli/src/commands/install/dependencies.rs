//! Validate bundled ripgrep and system Linux sandbox dependencies without installing packages.
use crate::{CliError, Result};

use std::{ffi::OsString, fs::File, io::Write, path::PathBuf, process::Command};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Host {
    Macos,
    Linux,
}
pub(super) struct Environment {
    pub host: Host,
    pub path: OsString,
    pub bundle: Option<PathBuf>,
}
impl Environment {
    pub fn current(_: bool) -> Result<Self> {
        let host = if cfg!(target_os = "macos") {
            Host::Macos
        } else if cfg!(target_os = "linux") {
            Host::Linux
        } else {
            return Err(CliError::new(
                2,
                "Native installation supports macOS and Linux only.",
            ));
        };
        Ok(Self {
            host,
            path: std::env::var_os("PATH").unwrap_or_default(),
            bundle: None,
        })
    }
    pub fn prepare(&self, log: &mut File) -> Result<()> {
        let root = self
            .bundle
            .as_ref()
            .ok_or_else(|| CliError::new(2, "Verified runtime payload is missing."))?;
        writeln!(
            log,
            "Platform: {} {}",
            if self.host == Host::Macos {
                "Darwin"
            } else {
                "Linux"
            },
            std::env::consts::ARCH
        )
        .map_err(CliError::io)?;
        let mut tools = vec![(root.join("runtime/tools/rg"), vec!["--version"], true)];
        if self.host == Host::Linux {
            let system_tools = kyler_core::sandbox::paths::system_sandbox_tools()
                .map_err(|error| CliError::new(2, format!("{error:#}")))?;
            for (name, path) in system_tools {
                writeln!(log, "System sandbox helper: {}", path.display()).map_err(CliError::io)?;
                let arguments = match name {
                    "sh" => vec!["-c", "exit 0"],
                    "sleep" => vec!["0"],
                    "ip" => vec!["-V"],
                    _ => vec!["--version"],
                };
                tools.push((path, arguments, false));
            }
        }
        for (path, arguments, bundled) in tools {
            let source = if bundled { "Bundled" } else { "System sandbox" };
            if !path.is_file() {
                return Err(CliError::new(
                    2,
                    format!(
                        "{source} tool missing: {}. Repair the package or system dependencies; no packages are installed automatically.",
                        path.display()
                    ),
                ));
            }
            let output = Command::new(&path)
                .args(arguments)
                .env("PATH", &self.path)
                .output()
                .map_err(CliError::io)?;
            log.write_all(&output.stdout)
                .and_then(|_| log.write_all(&output.stderr))
                .map_err(CliError::io)?;
            if !output.status.success() {
                return Err(CliError::new(
                    3,
                    format!(
                        "{source} tool failed: {}. Repair the package or system dependencies.",
                        path.display()
                    ),
                ));
            }
            let version = String::from_utf8_lossy(&output.stdout);
            if path.file_name().is_some_and(|n| n == "bwrap")
                && !bubblewrap_supported(version.trim())
            {
                return Err(CliError::new(
                    2,
                    "System Bubblewrap must be at least 0.5.0.",
                ));
            }
            if path.file_name().is_some_and(|n| n == "rg") {
                if !version.starts_with("ripgrep ") {
                    return Err(CliError::new(2, "Invalid bundled ripgrep."));
                }
                writeln!(log, "{} (bundled)", version.lines().next().unwrap())
                    .map_err(CliError::io)?;
            }
        }
        writeln!(
            log,
            "Bundled ripgrep ready. {} will be checked by real sandbox execution.",
            if self.host == Host::Macos {
                "System Seatbelt"
            } else {
                "System Bubblewrap, network tools and host security policy"
            }
        )
        .map_err(CliError::io)?;
        Ok(())
    }
}
fn bubblewrap_supported(version: &str) -> bool {
    let Some(value) = version.strip_prefix("bubblewrap ") else {
        return false;
    };
    let components = value
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>();
    components.is_ok_and(|v| v.len() == 3 && (v[0] > 0 || v[1] >= 5))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_floor_and_missing_payload_fail_without_host_fallback() {
        assert!(bubblewrap_supported("bubblewrap 0.5.0"));
        assert!(!bubblewrap_supported("bubblewrap 0.4.99"));
        assert!(!bubblewrap_supported("bubblewrap unknown"));
        let directory = tempfile::tempdir().unwrap();
        let env = Environment {
            host: Host::Macos,
            path: std::env::var_os("PATH").unwrap_or_default(),
            bundle: Some(directory.path().into()),
        };
        let mut log = tempfile::tempfile().unwrap();
        assert!(
            env.prepare(&mut log)
                .unwrap_err()
                .message
                .contains("Bundled tool missing")
        );
    }
}
