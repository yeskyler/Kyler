//! Bounded transport packages shared by CLI directory uploads and server ZIP installs.
use super::loader;

use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
    path::{Component, Path},
};

use anyhow::{Context, Result, ensure};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

use kyler_config::SkillSettings;
use kyler_protocol::api::skills::SkillSource;

pub use kyler_protocol::api::skills::MAX_SKILL_ARCHIVE_BYTES;
const MAX_EXPANDED_BYTES: usize = 8 * 1024 * 1024;

/// Read the client's directory, never a path supplied to the remote server.
pub fn package_directory(path: &Path) -> Result<Vec<u8>> {
    let settings = SkillSettings::default();
    let package = loader::package(path, SkillSource::User, &settings)?;
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, digest) in &package.files {
        let limit = if name == "SKILL.md" {
            settings.max_instruction_bytes
        } else {
            settings.max_resource_bytes
        };
        let bytes = loader::read_file(&package.root, Path::new(name), limit)?;
        ensure!(
            &loader::digest(&bytes) == digest,
            "skill changed while packaging"
        );
        zip.start_file(name, SimpleFileOptions::default())?;
        zip.write_all(&bytes)?;
    }
    let bytes = zip.finish()?.into_inner();
    ensure!(
        bytes.len() <= MAX_SKILL_ARCHIVE_BYTES,
        "skill archive exceeds 20 MiB"
    );
    Ok(bytes)
}

pub(super) fn unpack(
    bytes: &[u8],
    settings: &SkillSettings,
) -> Result<(String, BTreeMap<String, Vec<u8>>)> {
    ensure!(
        bytes.len() <= MAX_SKILL_ARCHIVE_BYTES,
        "skill archive exceeds 20 MiB"
    );
    let mut zip = ZipArchive::new(Cursor::new(bytes)).context("invalid skill ZIP")?;
    ensure!(zip.len() <= 512, "skill archive has too many entries");
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().trim_end_matches('/').to_owned();
        ensure!(
            !name.is_empty()
                && !name.contains(['\\', ':'])
                && !name.chars().any(char::is_control)
                && name
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..")
                && Path::new(&name)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "invalid skill archive path"
        );
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        ensure!(
            matches!(mode, 0 | 0o100000 | 0o040000),
            "skill links and special files are not supported"
        );
        if entry.is_dir()
            || name.starts_with("__MACOSX/")
            || name == ".DS_Store"
            || name.ends_with("/.DS_Store")
        {
            continue;
        }
        ensure!(
            mode != 0o040000 && files.len() < 256,
            "invalid skill archive entry"
        );
        let limit = if name == "SKILL.md" || name.ends_with("/SKILL.md") {
            settings.max_instruction_bytes
        } else {
            settings.max_resource_bytes
        };
        ensure!(
            entry.size() <= limit as u64 && entry.size() <= MAX_EXPANDED_BYTES as u64,
            "skill file exceeds size limit"
        );
        let mut data = Vec::new();
        (&mut entry)
            .take(limit.min(MAX_EXPANDED_BYTES) as u64 + 1)
            .read_to_end(&mut data)?;
        total = total
            .checked_add(data.len())
            .context("skill size overflow")?;
        ensure!(
            data.len() <= limit && data.len() as u64 == entry.size() && total <= MAX_EXPANDED_BYTES,
            "expanded skill exceeds size limit"
        );
        ensure!(
            files.insert(name, data).is_none(),
            "duplicate skill archive path"
        );
    }
    let manifests = files
        .keys()
        .filter(|name| name.as_str() == "SKILL.md" || name.ends_with("/SKILL.md"))
        .cloned()
        .collect::<Vec<_>>();
    ensure!(
        manifests.len() == 1,
        "archive must contain exactly one SKILL.md"
    );
    let manifest = &manifests[0];
    let prefix = manifest.strip_suffix("SKILL.md").unwrap();
    ensure!(
        prefix.is_empty() || prefix.trim_end_matches('/').split('/').count() == 1,
        "skill must be at archive root or inside one folder"
    );
    ensure!(
        files.keys().all(|name| name.starts_with(prefix)),
        "files outside skill folder are not supported"
    );
    let (name, _, _) = loader::instructions(&files[manifest])?;
    Ok((
        name,
        files
            .into_iter()
            .map(|(path, data)| (path[prefix.len()..].to_owned(), data))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (path, content) in files {
            zip.start_file(*path, SimpleFileOptions::default()).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    const INSTRUCTIONS: &[u8] =
        b"---\nname: fixture\ndescription: Fixture skill\n---\nHelp the user.";
    #[test]
    fn zip_roots_and_mac_metadata_work_without_extracting_unsafe_paths() {
        let settings = SkillSettings::default();
        for files in [
            vec![
                ("SKILL.md", INSTRUCTIONS),
                ("assets/image.bin", b"\0\xff".as_slice()),
            ],
            vec![
                ("folder/SKILL.md", INSTRUCTIONS),
                ("folder/assets/image.bin", b"\0\xff".as_slice()),
                ("__MACOSX/._SKILL.md", b"metadata".as_slice()),
            ],
        ] {
            let (name, files) = unpack(&zip(&files), &settings).unwrap();
            assert_eq!(name, "fixture");
            assert_eq!(files["assets/image.bin"], b"\0\xff");
        }
        for files in [
            vec![("../SKILL.md", INSTRUCTIONS)],
            vec![("/SKILL.md", INSTRUCTIONS)],
            vec![("folder\\SKILL.md", INSTRUCTIONS)],
            vec![
                ("one/SKILL.md", INSTRUCTIONS),
                ("two/SKILL.md", INSTRUCTIONS),
            ],
            vec![
                ("one/SKILL.md", INSTRUCTIONS),
                ("outside.txt", b"outside".as_slice()),
            ],
            vec![("nested/folder/SKILL.md", INSTRUCTIONS)],
        ] {
            assert!(unpack(&zip(&files), &settings).is_err());
        }
        assert!(unpack(b"not a zip", &settings).is_err());
    }
    #[test]
    fn zip_symlinks_and_expansion_limits_are_rejected() {
        let settings = SkillSettings::default();
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("SKILL.md", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(INSTRUCTIONS).unwrap();
        writer
            .add_symlink("link", "/tmp", SimpleFileOptions::default())
            .unwrap();
        assert!(unpack(&writer.finish().unwrap().into_inner(), &settings).is_err());
        let large = vec![b'x'; settings.max_resource_bytes + 1];
        assert!(
            unpack(
                &zip(&[("SKILL.md", INSTRUCTIONS), ("large.txt", &large)]),
                &settings
            )
            .is_err()
        );
    }
}
