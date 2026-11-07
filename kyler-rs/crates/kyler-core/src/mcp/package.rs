//! Bounded ZIP connector packages. Installation never executes package code.
use super::McpManager;

use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Component, Path},
};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use kyler_config::{McpServerSettings, McpTransport};
use kyler_protocol::api::mcp::McpServerInfo;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    id: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
}
fn relative(path: &str) -> Result<&Path> {
    let path = Path::new(path);
    ensure!(
        !path.as_os_str().is_empty()
            && !path.to_string_lossy().contains('\\')
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "Package paths must be relative and cannot contain traversal components"
    );
    Ok(path)
}
impl McpManager {
    pub async fn install_package(&self, bytes: Vec<u8>) -> Result<McpServerInfo> {
        ensure!(bytes.len() <= 20 * 1024 * 1024, "Package exceeds 20 MiB");
        let directory = self.package_directory()?;
        let (staging, manifest) = tokio::task::spawn_blocking(move || -> Result<_> {
            fs::create_dir_all(&directory)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
            }
            let staging = tempfile::Builder::new()
                .prefix(".install-")
                .tempdir_in(directory)?;
            let mut zip =
                zip::ZipArchive::new(Cursor::new(bytes)).context("Expected a ZIP package")?;
            ensure!(zip.len() <= 512, "Package exceeds 512 entries");
            let mut total = 0_u64;
            for index in 0..zip.len() {
                let mut entry = zip.by_index(index)?;
                let name = entry.name().trim_end_matches('/');
                let path = relative(name)?;
                let target = staging.path().join(path);
                let mode = entry.unix_mode().unwrap_or(0);
                ensure!(
                    mode & 0o170000 == 0 || matches!(mode & 0o170000, 0o100000 | 0o040000),
                    "Package links and special files are not supported"
                );
                total = total
                    .checked_add(entry.size())
                    .context("Expanded package exceeds size limits")?;
                ensure!(
                    entry.size() <= 16 * 1024 * 1024 && total <= 64 * 1024 * 1024,
                    "Expanded package exceeds size limits"
                );
                if entry.is_dir() {
                    fs::create_dir_all(&target)?;
                    continue;
                }
                fs::create_dir_all(target.parent().unwrap())?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)?;
                let copied =
                    std::io::copy(&mut entry.by_ref().take(16 * 1024 * 1024 + 1), &mut file)?;
                ensure!(copied == entry.size(), "Package entry size mismatch");
                file.flush()?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(
                        &target,
                        fs::Permissions::from_mode(if mode & 0o111 != 0 { 0o700 } else { 0o600 }),
                    )?;
                }
            }
            let metadata = fs::read(staging.path().join("connector.json"))
                .context("Package needs connector.json at its root")?;
            ensure!(
                metadata.len() <= 32 * 1024,
                "Connector manifest is too large"
            );
            let manifest: Manifest =
                serde_json::from_slice(&metadata).context("Invalid connector.json")?;
            ensure!(
                !manifest.id.is_empty()
                    && manifest.id.len() <= 24
                    && manifest
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "Invalid connector ID"
            );
            ensure!(
                manifest.args.len() <= 64
                    && manifest
                        .args
                        .iter()
                        .all(|arg| arg.len() <= 4096 && !arg.contains('\0')),
                "Invalid connector arguments"
            );
            ensure!(
                staging.path().join(relative(&manifest.command)?).is_file(),
                "Package command must name an included executable file"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    staging.path().join(&manifest.command),
                    fs::Permissions::from_mode(0o700),
                )?;
            }
            Ok((staging, manifest))
        })
        .await??;
        let target = staging.path().parent().unwrap().join(&manifest.id);
        // create_dir reserves the ID, including against concurrent installers.
        fs::create_dir(&target).context("Connector package already exists")?;
        let result = (|| -> Result<McpServerSettings> {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o700))?;
            }
            for entry in fs::read_dir(staging.path())? {
                let entry = entry?;
                fs::rename(entry.path(), target.join(entry.file_name()))?;
            }
            Ok(McpServerSettings {
                transport: McpTransport::Stdio,
                command: target
                    .join(&manifest.command)
                    .to_string_lossy()
                    .into_owned(),
                args: manifest
                    .args
                    .iter()
                    .map(|arg| arg.replace("${package}", &target.to_string_lossy()))
                    .collect(),
                auto_connect: false,
                ..Default::default()
            })
        })();
        let result = match result {
            Ok(settings) => self.add_user_connector(manifest.id, settings).await,
            Err(error) => Err(error),
        };
        if result.is_err() {
            let _ = fs::remove_dir_all(&target);
        }
        result
    }
}
