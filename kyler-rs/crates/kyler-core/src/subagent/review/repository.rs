//! Review evidence is read through the narrowed worker, including immutable revision files.
use super::git::{Capture, Discovery, FileContent, GitContext, GitRead};
use crate::environment::ExecutionEnvironment;

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;

use kyler_protocol::review::{ReviewLocation, ReviewRequest, ReviewTarget};

pub(super) struct Repository {
    pub environment: Arc<ExecutionEnvironment>,
    pub request: ReviewRequest,
    pub captured: Capture,
    git: GitContext,
    _metadata: tempfile::TempDir,
    observed: Mutex<BTreeMap<String, (String, bool)>>,
    visible: Mutex<BTreeMap<String, (usize, Vec<(usize, usize)>)>>,
    limitations: Mutex<Vec<String>>,
}
async fn call<T: DeserializeOwned>(
    environment: &ExecutionEnvironment,
    request: &GitRead,
) -> Result<T> {
    serde_json::from_str(
        &environment
            .run_tool("review_git", &serde_json::to_string(request)?)
            .await?,
    )
    .context("Invalid review worker response")
}
impl Repository {
    pub async fn prepare(
        environment: Arc<ExecutionEnvironment>,
        request: ReviewRequest,
    ) -> Result<Arc<Self>> {
        let discovery: Discovery = call(
            &environment,
            &GitRead::Discover {
                request: request.clone(),
            },
        )
        .await?;
        let metadata = environment.prepare_review_git(&discovery)?;
        let git = GitContext {
            directory: metadata.path().to_path_buf(),
            worktree: environment.session_environment().workspace_root.clone(),
            repository_root: discovery.root,
            prefix: discovery.prefix,
            head: discovery.head,
            base: discovery.base,
            revision: discovery.revision,
        };
        let captured: Capture = call(
            &environment,
            &GitRead::Capture {
                git: git.clone(),
                request: request.clone(),
            },
        )
        .await?;
        ensure!(
            captured.files.iter().map(String::len).sum::<usize>() <= 8192,
            "Review file manifest exceeds 8 KiB; narrow review.paths"
        );
        let initial_limitations = if captured.omitted_files.is_empty() {
            Vec::new()
        } else {
            vec![format!(
                "{} changed files exceeded the bounded reader or could not be captured; see omitted_files in the input.",
                captured.omitted_files.len()
            )]
        };
        Ok(Arc::new(Self {
            environment,
            request,
            captured,
            git,
            _metadata: metadata,
            observed: Mutex::new(BTreeMap::new()),
            visible: Mutex::new(BTreeMap::new()),
            limitations: Mutex::new(initial_limitations),
        }))
    }
    pub async fn read(&self, path: &str, observe: bool) -> Result<FileContent> {
        super::validate_path(path)?;
        let result = call::<FileContent>(
            &self.environment,
            &GitRead::Read {
                git: self.git.clone(),
                path: path.into(),
            },
        )
        .await;
        match &result {
            Ok(file) if observe => {
                self.observed
                    .lock()
                    .unwrap()
                    .insert(path.into(), (file.fingerprint.clone(), file.deleted));
            }
            Err(_) => self.limit(format!("Could not inspect {path} as a bounded UTF-8 file.")),
            _ => {}
        }
        result
    }
    pub fn files(&self, prefix: &str) -> Vec<&String> {
        let prefix = if prefix == "." {
            ""
        } else {
            prefix.trim_end_matches('/')
        };
        self.captured
            .all_files
            .iter()
            .filter(|path| {
                prefix.is_empty() || *path == prefix || path.starts_with(&format!("{prefix}/"))
            })
            .collect()
    }
    pub fn note_visible(&self, path: &str, offset: usize, end: usize, total: usize) {
        let mut visible = self.visible.lock().unwrap();
        let entry = visible
            .entry(path.into())
            .or_insert_with(|| (total, Vec::new()));
        entry.0 = total;
        if end >= offset {
            entry.1.push((offset, end));
        }
    }
    pub fn unread_untracked(&self) -> Vec<String> {
        let visible = self.visible.lock().unwrap();
        self.captured
            .untracked
            .iter()
            .filter(|path| {
                let Some((total, ranges)) = visible.get(*path) else {
                    return true;
                };
                if *total == 0 {
                    return false;
                }
                let mut ranges = ranges.clone();
                ranges.sort();
                let mut next = 1;
                for (start, end) in ranges {
                    if start > next {
                        return true;
                    }
                    next = next.max(end.saturating_add(1));
                }
                next <= *total
            })
            .cloned()
            .collect()
    }
    pub fn limit(&self, reason: String) {
        let mut limitations = self.limitations.lock().unwrap();
        if limitations.len() < 20 && !limitations.contains(&reason) {
            limitations.push(reason);
        }
    }
    pub fn limitations(&self) -> Vec<String> {
        self.limitations.lock().unwrap().clone()
    }
    pub async fn instructions(&self) -> Vec<serde_json::Value> {
        let mut instructions = Vec::new();
        for path in ["AGENTS.md", "CLAUDE.md"] {
            // Optional root instructions remain applicable even when review.paths narrows changes.
            let file = call::<FileContent>(
                &self.environment,
                &GitRead::Read {
                    git: self.git.clone(),
                    path: path.into(),
                },
            )
            .await;
            if let Ok(file) = file {
                self.observed
                    .lock()
                    .unwrap()
                    .insert(path.into(), (file.fingerprint, file.deleted));
                if file.content.len() <= 16000 {
                    instructions.push(serde_json::json!({"path":path,"content":file.content}));
                } else {
                    self.limit(format!("{path} exceeded the initial instruction budget; inspect it with read_review_file."));
                }
            }
        }
        instructions
    }
    pub async fn validate_location(&self, location: &ReviewLocation) -> Result<()> {
        super::validate_path(&location.path)?;
        ensure!(
            self.captured.files.contains(&location.path),
            "Finding location is outside the changed files"
        );
        ensure!(
            location.start_line > 0
                && location.end_line >= location.start_line
                && location.end_line - location.start_line < 10,
            "Finding locations must span one to ten lines"
        );
        let file = self.read(&location.path, false).await?;
        ensure!(
            location.end_line <= file.content.lines().count(),
            "Finding location exceeds the reviewed file"
        );
        if self.captured.untracked.contains(&location.path) {
            return Ok(());
        }
        let patch: String = call(
            &self.environment,
            &GitRead::Patch {
                git: self.git.clone(),
                path: location.path.clone(),
            },
        )
        .await?;
        ensure!(
            patch
                .lines()
                .filter_map(|line| hunk(line, file.deleted))
                .any(|(start, count)| count > 0
                    && location.start_line <= start.saturating_add(count - 1)
                    && location.end_line >= start),
            "Finding location must intersect a changed diff hunk"
        );
        Ok(())
    }
    pub async fn stale(&self) -> bool {
        if !self.captured.omitted_files.is_empty() {
            return true;
        }
        let check = async {
            let latest: Discovery = call(
                &self.environment,
                &GitRead::Discover {
                    request: self.request.clone(),
                },
            )
            .await?;
            let immutable = matches!(self.request.target, ReviewTarget::Commit { .. });
            if latest.base != self.git.base
                || latest.revision != self.git.revision
                || (!immutable && latest.head != self.git.head)
            {
                return Ok::<_, anyhow::Error>(true);
            }
            let captured: Capture = call(
                &self.environment,
                &GitRead::Capture {
                    git: self.git.clone(),
                    request: self.request.clone(),
                },
            )
            .await?;
            if captured.fingerprint != self.captured.fingerprint {
                return Ok(true);
            }
            let observed = self.observed.lock().unwrap().clone();
            for (path, (fingerprint, deleted)) in observed {
                let current = self.read(&path, false).await?;
                if current.fingerprint != fingerprint || current.deleted != deleted {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        .await;
        match check {
            Ok(stale) => stale,
            Err(_) => {
                self.limit("The review target could not be revalidated after inspection.".into());
                true
            }
        }
    }
}
fn hunk(line: &str, old: bool) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ ")?;
    let field = rest.split_whitespace().nth(usize::from(!old))?;
    let field = field.strip_prefix(if old { '-' } else { '+' })?;
    let (start, count) = field.split_once(',').unwrap_or((field, "1"));
    Some((start.parse().ok()?, count.parse().ok()?))
}
