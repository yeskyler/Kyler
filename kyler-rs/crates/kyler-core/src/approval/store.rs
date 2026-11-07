//! Private, atomic approval records. Pending calls never resume after restart.
use std::{
    cmp::Reverse,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use anyhow::{Context, Result, ensure};

use kyler_protocol::api::permissions::ApprovalRecord;

pub(super) fn save(root: &Path, record: &ApprovalRecord) -> Result<()> {
    ensure!(
        uuid::Uuid::parse_str(&record.id).is_ok(),
        "Invalid approval ID"
    );
    let temporary = root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec(record)?)?;
        file.sync_all()?;
        fs::rename(&temporary, root.join(format!("{}.json", record.id)))?;
        #[cfg(unix)]
        fs::File::open(root)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
pub(super) fn load(root: &Path) -> Result<Vec<ApprovalRecord>> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        builder.mode(0o700);
    }
    builder.create(root)?;
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            files.push((path.metadata()?.modified()?, path));
        }
    }
    files.sort_by_key(|(time, _)| Reverse(*time));
    let mut records = Vec::new();
    for (index, (_, path)) in files.into_iter().enumerate() {
        if index >= 1024 {
            let _ = fs::remove_file(path);
            continue;
        }
        let record: ApprovalRecord =
            serde_json::from_slice(&fs::read(&path)?).context("Invalid approval audit record")?;
        ensure!(
            path.file_stem().and_then(|s| s.to_str()) == Some(&record.id),
            "Approval record ID mismatch"
        );
        records.push(record);
    }
    Ok(records)
}
