use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

use kyler_protocol::api::run::RunInfo;

pub fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    Ok(())
}
pub async fn initialize(root: PathBuf) -> Result<(File, Vec<RunInfo>)> {
    tokio::task::spawn_blocking(move || {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;

            builder.mode(0o700);
        }
        match builder.create(&root) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let lock = private_file(&root.join(".server.lock"))?;
        lock.try_lock()
            .context("another server owns this Session store")?;
        sync_directory(&root)?;
        if let Some(parent) = root.parent() {
            sync_directory(parent)?;
        }
        let mut runs = Vec::new();
        for item in fs::read_dir(&root)? {
            let path = item?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let info: RunInfo =
                serde_json::from_slice(&fs::read(&path)?).context("invalid run manifest")?;
            ensure!(
                uuid::Uuid::parse_str(&info.run_id)?.to_string() == info.run_id
                    && path.file_stem().and_then(|s| s.to_str()) == Some(info.run_id.as_str()),
                "invalid run manifest ID"
            );
            ensure!(
                uuid::Uuid::parse_str(&info.session_id)?.to_string() == info.session_id,
                "invalid manifest session ID"
            );
            runs.push(info);
        }
        Ok((lock, runs))
    })
    .await?
}
pub async fn save(root: PathBuf, info: RunInfo) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let temporary = root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = private_file(&temporary)?;
            file.write_all(&serde_json::to_vec(&info)?)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, root.join(format!("{}.json", info.run_id)))?;
            sync_directory(&root)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    })
    .await?
}
pub async fn remove(root: &Path, id: &str) -> Result<()> {
    let root = root.to_owned();
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || {
        for path in [
            root.join(format!("{id}.json")),
            root.join("admissions").join(format!("{id}.json")),
        ] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if root.join("admissions").exists() {
            sync_directory(&root.join("admissions"))?;
        }
        sync_directory(&root)
    })
    .await?
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Admission {
    pub run_id: String,
    pub session_id: String,
    pub key: String,
    pub fingerprint: String,
}
pub async fn save_admission(root: PathBuf, admission: Admission) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let root = root.join("admissions");
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let temporary = root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let mut file = private_file(&temporary)?;
        file.write_all(&serde_json::to_vec(&admission)?)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, root.join(format!("{}.json", admission.run_id)))?;
        sync_directory(&root)?;
        if let Some(parent) = root.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    })
    .await?
}
pub async fn admissions(root: PathBuf) -> Result<Vec<Admission>> {
    tokio::task::spawn_blocking(move || {
        let root = root.join("admissions");
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for file in fs::read_dir(root)? {
            let path = file?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let record: Admission = serde_json::from_slice(&fs::read(&path)?)?;
            ensure!(
                uuid::Uuid::parse_str(&record.run_id)?.to_string() == record.run_id
                    && uuid::Uuid::parse_str(&record.session_id)?.to_string() == record.session_id,
                "Invalid admission identity"
            );
            records.push(record);
        }
        Ok(records)
    })
    .await?
}
