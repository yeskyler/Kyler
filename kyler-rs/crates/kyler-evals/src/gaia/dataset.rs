use std::{
    collections::HashSet,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use parquet::{
    file::{reader::FileReader, serialized_reader::SerializedFileReader},
    record::Field,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize)]
pub struct Task {
    pub task_id: String,
    #[serde(rename = "Question")]
    pub question: String,
    #[serde(rename = "Level", deserialize_with = "level")]
    pub level: u8,
    #[serde(default, rename = "Final answer")]
    pub answer: Option<String>,
    #[serde(default)]
    pub file_name: String,
}
fn level<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<u8, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    value
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .or_else(|| value.as_str()?.parse().ok())
        .ok_or_else(|| serde::de::Error::custom("Invalid GAIA level"))
}
pub struct Dataset {
    pub tasks: Vec<Task>,
    pub split_directory: PathBuf,
    pub fingerprint: String,
}
pub fn is_lfs(path: &Path) -> Result<bool> {
    let mut prefix = [0; 128];
    let n = File::open(path)?.read(&mut prefix)?;
    Ok(prefix[..n].starts_with(b"version https://git-lfs.github.com/spec/v1"))
}
pub fn digest_file(path: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    let mut reader = BufReader::new(File::open(path)?);
    let mut buffer = [0; 65536];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
impl Dataset {
    pub fn load(directory: &Path, split: &str) -> Result<Self> {
        ensure!(matches!(split, "validation" | "test"), "Invalid GAIA split");
        let split_directory = directory.join(split).canonicalize().context(
            "GAIA split directory not found; select the directory containing validation/ and test/",
        )?;
        let json = split_directory.join("metadata.jsonl");
        let path = if json.exists() {
            json
        } else {
            split_directory.join("metadata.parquet")
        };
        ensure!(
            !is_lfs(&path)?,
            "GAIA metadata is a Git LFS pointer, not downloaded data. Complete the authorized Git LFS download in the dataset repository, then retry."
        );
        ensure!(
            std::fs::metadata(&path)?.len() <= 32 * 1024 * 1024,
            "GAIA metadata exceeds 32 MiB"
        );
        let tasks: Vec<Task> = if path.extension().is_some_and(|e| e == "jsonl") {
            BufReader::new(File::open(&path)?)
                .lines()
                .filter_map(|line| match line {
                    Ok(s) if s.trim().is_empty() => None,
                    other => Some(other),
                })
                .map(|line| Ok(serde_json::from_str(&line?)?))
                .collect::<Result<_>>()?
        } else {
            let reader = SerializedFileReader::new(File::open(&path)?)?;
            reader
                .get_row_iter(None)?
                .map(|row| {
                    let row = row?;
                    let mut object = serde_json::Map::new();
                    for (key, field) in row.get_column_iter() {
                        if !matches!(
                            key.as_str(),
                            "task_id" | "Question" | "Level" | "Final answer" | "file_name"
                        ) {
                            continue;
                        }
                        let value = match field {
                            Field::Null => serde_json::Value::Null,
                            Field::Str(s) => s.clone().into(),
                            Field::Int(n) => (*n).into(),
                            Field::Long(n) => (*n).into(),
                            Field::Byte(n) => (*n as i64).into(),
                            _ => anyhow::bail!("Unsupported GAIA column type for {key}"),
                        };
                        object.insert(key.clone(), value);
                    }
                    Ok(serde_json::from_value(serde_json::Value::Object(object))?)
                })
                .collect::<Result<_>>()?
        };
        ensure!(
            !tasks.is_empty() && tasks.len() <= 10000,
            "GAIA split must contain 1..=10000 tasks"
        );
        let mut ids = HashSet::new();
        for task in &tasks {
            ensure!(
                !task.task_id.is_empty()
                    && task.task_id.len() <= 128
                    && task
                        .task_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
                "Invalid task ID"
            );
            ensure!(ids.insert(&task.task_id), "Duplicate GAIA task ID");
            ensure!(
                (1..=3).contains(&task.level) && !task.question.trim().is_empty(),
                "Invalid GAIA task level or question"
            );
            if split == "validation" {
                ensure!(
                    task.answer.as_ref().is_some_and(|v| !v.trim().is_empty()),
                    "Validation task has no reference answer"
                );
            }
        }
        Ok(Self {
            tasks,
            fingerprint: digest_file(&path)?,
            split_directory,
        })
    }
    pub fn attachment(&self, task: &Task) -> Result<Option<PathBuf>> {
        if task.file_name.is_empty() {
            return Ok(None);
        }
        ensure!(
            !task.file_name.chars().any(char::is_control)
                && !task.file_name.starts_with("metadata.")
                && task.file_name != ".git",
            "Task attachment cannot be metadata or a control path"
        );
        let path = Path::new(&task.file_name);
        ensure!(
            path.components().count() == 1
                && matches!(path.components().next(), Some(Component::Normal(_)))
                && !task.file_name.contains('\\'),
            "Attachment must be a local filename"
        );
        let path = self
            .split_directory
            .join(path)
            .canonicalize()
            .context("Task attachment not found")?;
        ensure!(
            path.starts_with(&self.split_directory) && path.is_file(),
            "Attachment escapes GAIA split"
        );
        ensure!(
            !is_lfs(&path)?,
            "Task attachment is a Git LFS pointer; complete the dataset download"
        );
        Ok(Some(path))
    }
    pub fn select(&self, level: Option<u8>, limit: Option<usize>) -> Result<Vec<&Task>> {
        ensure!(
            level.is_none_or(|n| (1..=3).contains(&n)),
            "level must be 1, 2 or 3"
        );
        ensure!(limit.is_none_or(|n| n > 0), "limit must be positive");
        let tasks: Vec<_> = self
            .tasks
            .iter()
            .filter(|t| level.is_none_or(|n| n == t.level))
            .take(limit.unwrap_or(usize::MAX))
            .collect();
        ensure!(!tasks.is_empty(), "No matching GAIA tasks");
        Ok(tasks)
    }
}
