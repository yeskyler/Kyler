//! Acquire and validate local or remote bundles before executing their programs.
use super::{Progress, bundle};
use crate::{CliError, Result};

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use flate2::read::GzDecoder;
use tokio::process::Command;

pub(super) struct Prepared {
    pub root: PathBuf,
    _temporary: tempfile::TempDir,
}

pub(super) fn valid_version(value: &str) -> bool {
    let value = value.strip_prefix('v').unwrap_or(value);
    value.starts_with(|c: char| c.is_ascii_digit())
        && value.contains('.')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".+-".contains(&c))
}

fn checksum(text: &str, asset: &str) -> Result<String> {
    let matches = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            let name = fields.next()?.trim_start_matches('*');
            (name == asset && fields.next().is_none()).then_some(hash)
        })
        .collect::<Vec<_>>();
    if matches.len() != 1
        || matches[0].len() != 64
        || !matches[0].bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(CliError::new(
            2,
            "Release checksum entry is missing or invalid.",
        ));
    }
    Ok(matches[0].to_ascii_lowercase())
}

async fn transfer(url: &str, output: &Path, log: &mut File, progress: &mut Progress) -> Result<()> {
    if !url.starts_with("https://") {
        return Err(CliError::new(2, "Release downloads require HTTPS."));
    }
    let mut command = Command::new("curl");
    command
        .args([
            "--silent",
            "--fail",
            "--location",
            "--show-error",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--connect-timeout",
            "15",
            "--speed-limit",
            "1024",
            "--speed-time",
            "60",
            "--retry",
            "2",
            "--retry-delay",
            "2",
        ])
        .arg(url)
        .arg("-o")
        .arg(output)
        .kill_on_drop(true);
    let download = command.output();
    tokio::pin!(download);
    let mut interval = tokio::time::interval(Duration::from_millis(200));
    let result = loop {
        tokio::select! {
            result = &mut download => break result.map_err(CliError::io)?,
            _ = interval.tick() => {
                if let Ok(metadata) = fs::metadata(output) {
                    progress.detail(&format!("{:.1} MiB", metadata.len() as f64 / 1_048_576.0));
                }
            }
        }
    };
    log.write_all(&result.stderr).map_err(CliError::io)?;
    if !result.status.success() {
        return Err(CliError::new(3, "Release download failed."));
    }
    Ok(())
}

fn small_text(path: &Path) -> Result<String> {
    if fs::metadata(path).map_err(CliError::io)?.len() > 1024 * 1024 {
        return Err(CliError::new(2, "Release metadata exceeds its size limit."));
    }
    fs::read_to_string(path).map_err(CliError::io)
}

async fn remote(
    version: Option<&str>,
    scratch: &Path,
    log: &mut File,
    progress: &mut Progress,
) -> Result<PathBuf> {
    let repository = std::env::var("KYLER_REPOSITORY").unwrap_or_else(|_| "yeskyler/Kyler".into());
    if repository.split('/').count() != 2
        || repository
            .split('/')
            .any(|s| s.is_empty() || s.contains(".."))
        || !repository
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_./-".contains(&c))
    {
        return Err(CliError::new(2, "Invalid release repository."));
    }
    let mirror = "https://www.kyler.work/releases";
    let latest = scratch.join("latest");
    let version = if let Some(version) = version {
        version.to_owned()
    } else if transfer(&format!("{mirror}/latest.txt"), &latest, log, progress)
        .await
        .is_ok()
        && small_text(&latest).is_ok_and(|v| valid_version(v.trim()))
    {
        small_text(&latest)?.trim().to_owned()
    } else {
        writeln!(
            log,
            "Server version lookup failed; checking GitHub Releases."
        )
        .map_err(CliError::io)?;
        let output = Command::new("curl")
            .args([
                "--silent",
                "--fail",
                "--location",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--tlsv1.2",
                "--max-time",
                "45",
                "-o",
                "/dev/null",
                "-w",
                "%{url_effective}",
            ])
            .arg(format!("https://github.com/{repository}/releases/latest"))
            .kill_on_drop(true)
            .output()
            .await
            .map_err(CliError::io)?;
        log.write_all(&output.stderr).map_err(CliError::io)?;
        if !output.status.success() {
            return Err(CliError::new(
                3,
                "Cannot determine latest version. Retry with --version VERSION.",
            ));
        }
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned()
    };
    if !valid_version(&version) {
        return Err(CliError::new(2, "Invalid release version."));
    }
    let version = format!("v{}", version.trim_start_matches('v'));
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        _ => return Err(CliError::new(2, "Unsupported release platform.")),
    };
    let asset = format!("kyler-{version}-{target}.tar.gz");
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
        .join("kyler/downloads")
        .join(&version);
    super::private_directory(&cache)?;
    let manifest = scratch.join("release-checksums");
    for base in [
        format!("{mirror}/{version}"),
        format!("https://github.com/{repository}/releases/download/{version}"),
    ] {
        if transfer(&format!("{base}/SHA256SUMS"), &manifest, log, progress)
            .await
            .is_err()
        {
            writeln!(
                log,
                "Server download failed; switching to GitHub for the same version."
            )
            .map_err(CliError::io)?;
            continue;
        }
        let expected = checksum(&small_text(&manifest)?, &asset)?;
        for candidate in [
            cache.join(&asset),
            std::env::current_dir().map_err(CliError::io)?.join(&asset),
        ] {
            if candidate.is_file() {
                if bundle::digest(&candidate)? == expected {
                    writeln!(log, "Reusing verified bundle; download skipped.")
                        .map_err(CliError::io)?;
                    return Ok(candidate);
                }
                writeln!(
                    log,
                    "Existing bundle checksum differs; a fresh download is required."
                )
                .map_err(CliError::io)?;
            }
        }
        let archive = scratch.join(&asset);
        if transfer(&format!("{base}/{asset}"), &archive, log, progress)
            .await
            .is_err()
        {
            continue;
        }
        if bundle::digest(&archive)? != expected {
            return Err(CliError::new(
                2,
                "Release checksum mismatch. Installation stopped.",
            ));
        }
        let mut staged = tempfile::NamedTempFile::new_in(&cache).map_err(CliError::io)?;
        std::io::copy(
            &mut File::open(&archive).map_err(CliError::io)?,
            &mut staged,
        )
        .map_err(CliError::io)?;
        let destination = cache.join(&asset);
        staged
            .persist(&destination)
            .map_err(|e| CliError::io(e.error))?;
        return Ok(destination);
    }
    writeln!(log, "Manual installation: download {asset} and SHA256SUMS from GitHub Releases, verify SHA256, then run kyler install --bundle /path/to/{asset}.").map_err(CliError::io)?;
    Err(CliError::new(
        3,
        "Both download sources failed. Existing installation was kept.",
    ))
}

fn unpack(source: &Path, destination: &Path) -> Result<PathBuf> {
    let mut archive = tar::Archive::new(GzDecoder::new(File::open(source).map_err(CliError::io)?));
    let mut names = BTreeSet::new();
    let mut root = None;
    for entry in archive.entries().map_err(CliError::io)? {
        let mut entry = entry.map_err(CliError::io)?;
        let path = entry.path().map_err(CliError::io)?.into_owned();
        if path
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
            || path.as_os_str().is_empty()
        {
            return Err(CliError::new(2, "Archive contains unsafe paths."));
        }
        let first = path.components().next().unwrap().as_os_str().to_owned();
        if root.as_ref().is_some_and(|r| *r != first) {
            return Err(CliError::new(2, "Expected one bundle root directory."));
        }
        root = Some(first);
        if !names.insert(path.clone()) {
            return Err(CliError::new(2, "Archive contains duplicate paths."));
        }
        let target = destination.join(&path);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(&target).map_err(CliError::io)?;
        } else if kind.is_file() {
            fs::create_dir_all(target.parent().unwrap()).map_err(CliError::io)?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(entry.header().mode().map_err(CliError::io)? & 0o777);
            }
            let mut output = options.open(target).map_err(CliError::io)?;
            std::io::copy(&mut entry, &mut output).map_err(CliError::io)?;
        } else {
            return Err(CliError::new(
                2,
                "Archive links and special files are not supported.",
            ));
        }
    }
    Ok(destination.join(root.ok_or_else(|| CliError::new(2, "Empty bundle archive."))?))
}

pub(super) async fn prepare(
    local: Option<&Path>,
    version: Option<&str>,
    log: &mut File,
    progress: &mut Progress,
) -> Result<Prepared> {
    let temporary = tempfile::Builder::new()
        .prefix("kyler-install-source-")
        .tempdir()
        .map_err(CliError::io)?;
    progress.begin("Prepare package");
    let source = if let Some(local) = local {
        local.canonicalize().map_err(CliError::io)?
    } else {
        remote(version, temporary.path(), log, progress).await?
    };
    let prepared = tokio::task::spawn_blocking(move || {
        let root = if source.is_dir() {
            source
        } else {
            let destination = temporary.path().join("unpack");
            fs::create_dir(&destination).map_err(CliError::io)?;
            unpack(&source, &destination)?
        };
        bundle::verify(&root)?;
        Ok::<_, CliError>(Prepared {
            root,
            _temporary: temporary,
        })
    })
    .await
    .map_err(|_| CliError::new(3, "Bundle preparation could not complete."))??;
    progress.complete();
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn archive(entries: &[(&str, tar::EntryType)]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let encoder =
            flate2::write::GzEncoder::new(file.reopen().unwrap(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for (path, kind) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_mode(0o755);
            header.set_size(0);
            header.set_path(path).unwrap();
            if kind.is_symlink() {
                header.set_link_name("/outside").unwrap();
            }
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        file
    }

    #[test]
    fn archives_reject_links_special_entries_duplicates_and_multiple_roots() {
        for entries in [
            vec![("bundle/link", tar::EntryType::Symlink)],
            vec![("bundle/fifo", tar::EntryType::Fifo)],
            vec![
                ("bundle/a", tar::EntryType::Regular),
                ("bundle/a", tar::EntryType::Regular),
            ],
            vec![
                ("first/a", tar::EntryType::Regular),
                ("second/b", tar::EntryType::Regular),
            ],
        ] {
            let source = archive(&entries);
            let directory = tempfile::tempdir().unwrap();
            assert!(unpack(source.path(), directory.path()).is_err());
        }
        let source = archive(&[("bundle/bin/tool", tar::EntryType::Regular)]);
        let directory = tempfile::tempdir().unwrap();
        assert!(
            unpack(source.path(), directory.path())
                .unwrap()
                .join("bin/tool")
                .is_file()
        );
    }

    #[test]
    fn archive_paths_cannot_escape_the_extraction_directory() {
        let source = tempfile::NamedTempFile::new().unwrap();
        let encoder =
            flate2::write::GzEncoder::new(source.reopen().unwrap(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o644);
        header.as_mut_bytes()[..9].copy_from_slice(b"../escape");
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        let directory = tempfile::tempdir().unwrap();
        assert!(unpack(source.path(), directory.path()).is_err());
        assert!(!directory.path().parent().unwrap().join("escape").exists());
    }
    #[test]
    fn release_metadata_cannot_escape_paths_or_select_ambiguous_hashes() {
        assert!(valid_version("v0.5.0-rc.1"));
        for value in ["../0.5.0", "0.5.0/x", "-0.5", "0.5\n"] {
            assert!(!valid_version(value));
        }
        let line = format!("{}  bundle.tar.gz\n", "a".repeat(64));
        assert_eq!(checksum(&line, "bundle.tar.gz").unwrap(), "a".repeat(64));
        assert!(checksum(&(line.clone() + &line), "bundle.tar.gz").is_err());
        assert!(checksum("bad  bundle.tar.gz", "bundle.tar.gz").is_err());
    }
}
