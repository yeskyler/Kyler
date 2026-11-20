//! Native prerequisites and fixed package-manager commands; never execute repository metadata.
use crate::{CliError, Result};

use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Host {
    Macos,
    Linux,
}
pub(super) struct Environment {
    pub host: Host,
    pub path: OsString,
    pub root: bool,
    pub non_interactive: bool,
}
pub(super) fn command_path(path: &OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path).map(|p| p.join(name)).find(|p| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        {
            p.is_file()
        }
    })
}
impl Environment {
    pub fn current(non_interactive: bool) -> Result<Self> {
        let host = if cfg!(target_os = "macos") {
            Host::Macos
        } else if cfg!(target_os = "linux") {
            Host::Linux
        } else {
            return Err(CliError::new(
                2,
                "Release installation supports macOS and Linux only.",
            ));
        };
        let mut paths = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .collect::<Vec<_>>();
        for extra in ["/usr/sbin", "/sbin", "/opt/homebrew/bin", "/usr/local/bin"] {
            if Path::new(extra).is_dir() && !paths.iter().any(|p| p == Path::new(extra)) {
                paths.push(extra.into());
            }
        }
        let path = std::env::join_paths(paths)
            .map_err(|_| CliError::new(2, "Invalid executable search path."))?;
        #[cfg(unix)]
        let root = unsafe { libc::geteuid() == 0 };
        #[cfg(not(unix))]
        let root = false;
        Ok(Self {
            host,
            path,
            root,
            non_interactive,
        })
    }
    fn command(&self, name: &str) -> Result<Command> {
        let executable = command_path(&self.path, name)
            .ok_or_else(|| CliError::new(2, format!("Required command is missing: {name}")))?;
        let mut command = Command::new(executable);
        command.env("PATH", &self.path);
        Ok(command)
    }
    fn version(&self, name: &str, args: &[&str], log: &mut File) -> Option<String> {
        let output = self.command(name).ok()?.args(args).output().ok()?;
        let _ = log.write_all(&output.stdout);
        let _ = log.write_all(&output.stderr);
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().into())
    }
    fn missing(&self, log: &mut File) -> Result<(Vec<String>, Vec<String>)> {
        let mut missing = Vec::new();
        let mut packages = Vec::new();
        let mut add = |tool: &str, package: &str| {
            missing.push(tool.to_string());
            if !packages.iter().any(|p| p == package) {
                packages.push(package.to_string());
            }
        };
        if self.host == Host::Linux {
            if !self
                .version("bwrap", &["--version"], log)
                .as_deref()
                .is_some_and(bubblewrap_supported)
            {
                add("bwrap", "bubblewrap");
            }
            for (tool, package) in [
                ("slirp4netns", "slirp4netns"),
                (
                    "ip",
                    if matches!(
                        ["apt-get", "dnf", "yum", "pacman", "zypper"]
                            .into_iter()
                            .find(|m| command_path(&self.path, m).is_some()),
                        Some("dnf" | "yum")
                    ) {
                        "iproute"
                    } else {
                        "iproute2"
                    },
                ),
                ("unshare", "util-linux"),
                ("nsenter", "util-linux"),
                ("iptables", "iptables"),
                ("ip6tables", "iptables"),
                ("iptables-restore", "iptables"),
                ("ip6tables-restore", "iptables"),
            ] {
                if command_path(&self.path, tool).is_none() {
                    add(tool, package);
                }
            }
            for tool in ["sh", "sleep"] {
                if command_path(&self.path, tool).is_none() {
                    return Err(CliError::new(
                        2,
                        format!("Required base system command missing: {tool}"),
                    ));
                }
            }
        }
        if self.version("rg", &["--version"], log).is_none() {
            add("rg", "ripgrep");
        }
        Ok((missing, packages))
    }
    pub fn prepare(&self, log: &mut File) -> Result<()> {
        println!(
            "Platform: {} {}",
            if self.host == Host::Macos {
                "Darwin"
            } else {
                "Linux"
            },
            std::env::consts::ARCH
        );
        if self.host == Host::Linux {
            let libc = self
                .version("getconf", &["GNU_LIBC_VERSION"], log)
                .unwrap_or_default();
            if !libc.starts_with("glibc ") {
                return Err(CliError::new(
                    2,
                    "Current Linux bundles require glibc. musl/Alpine need a separately compatible build; no system libraries were changed.",
                ));
            }
            println!("Runtime: {libc}");
        }
        println!(
            "Sandbox: {} (real execution is checked before activation).",
            if self.host == Host::Macos {
                "system Seatbelt"
            } else {
                "Bubblewrap >= 0.5.0 and isolated networking"
            }
        );
        let (missing, packages) = self.missing(log)?;
        if !missing.is_empty() {
            let (manager, args) = if self.host == Host::Macos {
                if command_path(&self.path, "brew").is_none() {
                    return Err(CliError::new(
                        2,
                        format!(
                            "Missing dependencies: {}. Install Homebrew using https://brew.sh, then run: brew install {}. Rerun the installer afterward.",
                            missing.join(" "),
                            packages.join(" ")
                        ),
                    ));
                }
                ("brew", vec!["install"])
            } else {
                let manager = ["apt-get", "dnf", "yum", "pacman", "zypper"].into_iter().find(|m| command_path(&self.path, m).is_some())
                    .ok_or_else(|| CliError::new(2, format!("No supported package manager found. Ask an administrator to install: {}", missing.join(" "))))?;
                let args = match manager {
                    "pacman" => vec!["-S", "--needed", "--noconfirm"],
                    "zypper" => vec!["--non-interactive", "install"],
                    _ => vec!["install", "-y"],
                };
                (manager, args)
            };
            let mut install_args = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            install_args.extend(packages);
            let manual = format!("{manager} {}", install_args.join(" "));
            let elevated = self.host == Host::Linux && !self.root;
            if elevated && command_path(&self.path, "sudo").is_none() {
                return Err(CliError::new(
                    2,
                    format!(
                        "Administrator access is required. Ask an administrator to run: {manual}"
                    ),
                ));
            }
            let run = |arguments: &[String]| -> Result<()> {
                let mut command = if elevated {
                    let mut c = self.command("sudo")?;
                    if self.non_interactive {
                        c.arg("-n");
                    }
                    c.arg(command_path(&self.path, manager).unwrap());
                    c
                } else {
                    self.command(manager)?
                };
                println!(
                    "Installing dependencies. Command: {}{manager} {}",
                    if elevated { "sudo " } else { "" },
                    arguments.join(" ")
                );
                if !command
                    .args(arguments)
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .status()
                    .map_err(CliError::io)?
                    .success()
                {
                    return Err(CliError::new(
                        3,
                        format!(
                            "Dependency installation failed. Retry: {manual}. Linux may require an administrator."
                        ),
                    ));
                }
                Ok(())
            };
            if manager == "apt-get" {
                run(&["update".into()])?;
            }
            run(&install_args)?;
            let (missing, _) = self.missing(log)?;
            if !missing.is_empty() {
                return Err(CliError::new(
                    3,
                    format!(
                        "Dependencies remain unavailable: {}. Check PATH and package repository versions. No host security settings were changed.",
                        missing.join(" ")
                    ),
                ));
            }
        } else {
            println!("Reusing installed sandbox dependencies and ripgrep.");
        }
        if let Some(version) = self.version("rg", &["--version"], log) {
            println!("{}", version.lines().next().unwrap_or("ripgrep ready"));
        }
        Ok(())
    }
}
fn bubblewrap_supported(version: &str) -> bool {
    let mut words = version.split_whitespace();
    if words.next() != Some("bubblewrap") {
        return false;
    }
    let Some(version) = words.next() else {
        return false;
    };
    if words.next().is_some() {
        return false;
    }
    let components = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>();
    components.is_ok_and(|v| v.len() == 3 && (v[0] > 0 || v[1] >= 5))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bubblewrap_versions_must_be_known_and_at_least_five() {
        for value in ["bubblewrap 0.5.0", "bubblewrap 0.9.0", "bubblewrap 1.0.0"] {
            assert!(bubblewrap_supported(value));
        }
        for value in [
            "bubblewrap 0.4.99",
            "unknown",
            "bubblewrap 0.9",
            "bubblewrap 0.9.0 extra",
            "bwrap 0.9.0",
        ] {
            assert!(!bubblewrap_supported(value));
        }
    }
    #[cfg(unix)]
    struct Fixture {
        root: tempfile::TempDir,
        environment: Environment,
    }
    #[cfg(unix)]
    impl Fixture {
        fn new(host: Host) -> Self {
            let root = tempfile::tempdir().unwrap();
            let tools = root.path().join("bin");
            std::fs::create_dir(&tools).unwrap();
            let fixture = Self {
                root,
                environment: Environment {
                    host,
                    path: tools.into_os_string(),
                    root: false,
                    non_interactive: true,
                },
            };
            for (name, body) in [
                ("getconf", "echo 'glibc 2.31'"),
                ("rg", "echo 'ripgrep 15.2.0'"),
                ("bwrap", "echo 'bubblewrap 0.9.0'"),
                ("sudo", "[ \"$1\" != -n ] || shift; exec \"$@\""),
            ] {
                fixture.script(name, body);
            }
            for name in [
                "slirp4netns",
                "ip",
                "unshare",
                "nsenter",
                "iptables",
                "ip6tables",
                "iptables-restore",
                "ip6tables-restore",
                "sleep",
                "sh",
            ] {
                fixture.script(name, "exit 0");
            }
            fixture
        }
        fn tools(&self) -> PathBuf {
            PathBuf::from(&self.environment.path)
        }
        fn script(&self, name: &str, body: &str) {
            use std::os::unix::fs::PermissionsExt;
            let path = self.tools().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        fn shell_path(&self, name: &str) -> String {
            format!(
                "'{}'",
                self.root
                    .path()
                    .join(name)
                    .to_str()
                    .unwrap()
                    .replace('\'', "'\\''")
            )
        }
        fn manager(&self, name: &str, fail: bool) {
            self.script(
                name,
                &format!(
                    "printf '%s\\n' \"$*\" >> {}; {}; /usr/bin/touch {}",
                    self.shell_path("commands"),
                    if fail { "exit 1" } else { ":" },
                    self.shell_path("installed")
                ),
            );
        }
        fn old_bwrap(&self, upgrade: bool) {
            self.script("bwrap", &format!("if [ -f {} ] && [ {} = yes ]; then echo 'bubblewrap 0.9.0'; else echo 'bubblewrap 0.4.0'; fi", self.shell_path("installed"), if upgrade { "yes" } else { "no" }));
        }
        fn missing_rg(&self, upgrade: bool) {
            self.script(
                "rg",
                &format!(
                    "[ -f {} ] && [ {} = yes ] || exit 1; echo 'ripgrep 15.2.0'",
                    self.shell_path("installed"),
                    if upgrade { "yes" } else { "no" }
                ),
            );
        }
        fn run(&self) -> Result<()> {
            let mut log = File::create(self.root.path().join("diagnostics")).unwrap();
            self.environment.prepare(&mut log)
        }
        fn commands(&self) -> String {
            std::fs::read_to_string(self.root.path().join("commands")).unwrap_or_default()
        }
    }
    #[cfg(unix)]
    #[test]
    fn existing_dependencies_require_no_package_manager_on_either_host() {
        for host in [Host::Macos, Host::Linux] {
            let fixture = Fixture::new(host);
            fixture.run().unwrap();
            assert!(fixture.commands().is_empty());
        }
    }
    #[cfg(unix)]
    #[test]
    fn fixed_linux_package_managers_upgrade_and_recheck() {
        for manager in ["apt-get", "dnf", "yum", "pacman", "zypper"] {
            let fixture = Fixture::new(Host::Linux);
            fixture.old_bwrap(true);
            fixture.manager(manager, false);
            fixture.run().unwrap();
            let commands = fixture.commands();
            assert!(commands.contains("bubblewrap"));
            assert!(!commands.contains("ripgrep"));
            assert!(!commands.contains("slirp4netns"));
            if manager == "apt-get" {
                assert!(commands.starts_with("update\n"));
            }
            if manager == "pacman" {
                assert!(!commands.contains("-Sy"));
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn root_needs_no_sudo_and_unprivileged_failure_shows_manual_command() {
        let mut fixture = Fixture::new(Host::Linux);
        fixture.old_bwrap(true);
        fixture.manager("apt-get", false);
        std::fs::remove_file(fixture.tools().join("sudo")).unwrap();
        assert!(fixture.run().unwrap_err().message.contains("administrator"));
        assert!(fixture.commands().is_empty());
        fixture.environment.root = true;
        fixture.run().unwrap();
        assert!(fixture.commands().contains("bubblewrap"));
    }
    #[cfg(unix)]
    #[test]
    fn missing_managers_and_failed_or_old_repositories_stop() {
        let fixture = Fixture::new(Host::Linux);
        fixture.old_bwrap(true);
        assert!(fixture.run().unwrap_err().message.contains("administrator"));
        fixture.manager("apt-get", true);
        assert!(
            fixture
                .run()
                .unwrap_err()
                .message
                .contains("Retry: apt-get")
        );
        assert!(!fixture.commands().contains("install"));
        fixture.manager("apt-get", false);
        fixture.old_bwrap(false);
        assert!(
            fixture
                .run()
                .unwrap_err()
                .message
                .contains("remain unavailable")
        );
    }
    #[cfg(unix)]
    #[test]
    fn networking_tools_and_ripgrep_are_rechecked_without_host_mutations() {
        let fixture = Fixture::new(Host::Linux);
        fixture.manager("apt-get", false);
        std::fs::remove_file(fixture.tools().join("slirp4netns")).unwrap();
        assert!(
            fixture
                .run()
                .unwrap_err()
                .message
                .contains("remain unavailable")
        );
        assert!(fixture.commands().contains("slirp4netns"));
        for (host, manager) in [(Host::Linux, "apt-get"), (Host::Macos, "brew")] {
            let fixture = Fixture::new(host);
            fixture.missing_rg(true);
            fixture.manager(manager, false);
            fixture.run().unwrap();
            assert!(fixture.commands().contains("ripgrep"));
            assert!(!fixture.commands().contains("bubblewrap"));
            fixture.missing_rg(false);
            assert!(
                fixture
                    .run()
                    .unwrap_err()
                    .message
                    .contains("remain unavailable")
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn homebrew_is_only_needed_for_missing_macos_dependencies() {
        let fixture = Fixture::new(Host::Macos);
        fixture.missing_rg(true);
        assert!(
            fixture
                .run()
                .unwrap_err()
                .message
                .contains("https://brew.sh")
        );
        fixture.manager("brew", true);
        assert!(
            fixture
                .run()
                .unwrap_err()
                .message
                .contains("Retry: brew install ripgrep")
        );
        fixture.manager("brew", false);
        std::fs::remove_file(fixture.tools().join("sudo")).unwrap();
        fixture.run().unwrap();
        assert!(fixture.commands().contains("install ripgrep"));
    }
    #[cfg(unix)]
    #[test]
    fn musl_and_unknown_libc_stop_before_package_operations() {
        for value in ["musl", ""] {
            let fixture = Fixture::new(Host::Linux);
            fixture.script("getconf", &format!("echo '{value}'"));
            fixture.manager("apt-get", false);
            assert!(fixture.run().unwrap_err().message.contains("require glibc"));
            assert!(fixture.commands().is_empty());
        }
    }
}
