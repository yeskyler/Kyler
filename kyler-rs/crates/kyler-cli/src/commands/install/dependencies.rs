//! Validate only the release's private tool payload. Never install or substitute host tools.
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
        println!(
            "Platform: {} {}",
            if self.host == Host::Macos {
                "Darwin"
            } else {
                "Linux"
            },
            std::env::consts::ARCH
        );
        let mut tools = vec![(root.join("runtime/tools/rg"), vec!["--version"])];
        if self.host == Host::Linux {
            for name in [
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
            ] {
                let arguments = match name {
                    "sh" => vec!["-c", "exit 0"],
                    "sleep" => vec!["0"],
                    "ip" => vec!["-V"],
                    _ => vec!["--version"],
                };
                tools.push((root.join("runtime/sandbox/bin").join(name), arguments));
            }
        }
        for (path, arguments) in tools {
            if !path.is_file() {
                return Err(CliError::new(
                    2,
                    format!(
                        "Bundled tool missing: {}. Reinstall the complete bundle; no host substitution or package installation is performed.",
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
                        "Bundled tool failed: {}. Repair the bundle; host tools are never substituted.",
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
                    "Bundled Bubblewrap must be at least 0.5.0.",
                ));
            }
            if path.file_name().is_some_and(|n| n == "rg") {
                if !version.starts_with("ripgrep ") {
                    return Err(CliError::new(2, "Invalid bundled ripgrep."));
                }
                println!("{} (bundled)", version.lines().next().unwrap());
            }
        }
        println!(
            "Bundled tools ready. {} will be checked by real sandbox execution.",
            if self.host == Host::Macos {
                "System Seatbelt"
            } else {
                "Host kernel namespaces and security policy"
            }
        );
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
