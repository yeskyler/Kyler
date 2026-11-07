//! Fixed, bounded Git reads. Repository configuration is never used for diff execution.
use crate::{
    sandbox::{FilesystemPolicy, WorkerRequest},
    subagent::review::validate_path,
};

use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{io::AsyncReadExt, process::Command};

use kyler_protocol::review::{ReviewRequest, ReviewTarget};

pub(super) const FILE_LIMIT: usize = 512 * 1024;
const OUTPUT_LIMIT: usize = 512 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GitContext {
    pub directory: PathBuf,
    pub worktree: PathBuf,
    pub repository_root: PathBuf,
    pub prefix: String,
    pub head: String,
    pub base: Option<String>,
    pub revision: Option<String>,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Discovery {
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub root: PathBuf,
    pub prefix: String,
    pub head: String,
    pub base: Option<String>,
    pub revision: Option<String>,
    pub object_format: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Capture {
    pub files: Vec<String>,
    pub all_files: Vec<String>,
    pub untracked: Vec<String>,
    pub omitted_files: Vec<String>,
    pub patch: String,
    pub fingerprint: String,
    pub head: String,
    pub base: Option<String>,
    pub revision: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum GitRead {
    Discover {
        request: ReviewRequest,
    },
    Capture {
        git: GitContext,
        request: ReviewRequest,
    },
    Read {
        git: GitContext,
        path: String,
    },
    Patch {
        git: GitContext,
        path: String,
    },
}
#[derive(Serialize, Deserialize)]
pub(crate) struct FileContent {
    pub content: String,
    pub fingerprint: String,
    pub deleted: bool,
}

fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
async fn git(cwd: &Path, context: Option<&GitContext>, args: &[String]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .env("LANG", "C.UTF-8")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_GRAFT_FILE", "/dev/null")
        .args([
            "--no-pager",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "submodule.recurse=false",
        ])
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(context) = context {
        command
            .env("GIT_DIR", &context.directory)
            .env("GIT_WORK_TREE", &context.repository_root)
            .env("GIT_INDEX_FILE", context.directory.join("index"));
    }
    let mut child = command
        .spawn()
        .context("Git is unavailable in the review sandbox runtime")?;
    let stdout = child.stdout.take().context("Git stdout unavailable")?;
    let stderr = child.stderr.take().context("Git stderr unavailable")?;
    let read = async move {
        let mut out = Vec::new();
        stdout
            .take((OUTPUT_LIMIT + 1) as u64)
            .read_to_end(&mut out)
            .await?;
        ensure!(
            out.len() <= OUTPUT_LIMIT,
            "Review Git output exceeds 512 KiB; narrow review.paths"
        );
        Ok::<_, anyhow::Error>(out)
    };
    let errors = async move {
        let mut out = Vec::new();
        stderr
            .take((OUTPUT_LIMIT + 1) as u64)
            .read_to_end(&mut out)
            .await?;
        ensure!(
            out.len() <= OUTPUT_LIMIT,
            "Review Git errors exceed output limit"
        );
        Ok::<_, anyhow::Error>(out)
    };
    let run = async {
        let (out, err) = tokio::try_join!(read, errors)?;
        let status = child.wait().await?;
        ensure!(
            status.success(),
            "Git read failed: {}",
            String::from_utf8_lossy(&err)
                .chars()
                .take(500)
                .collect::<String>()
        );
        Ok(out)
    };
    tokio::time::timeout(std::time::Duration::from_secs(20), run)
        .await
        .context("Review Git read timed out")?
}
fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}
async fn text(cwd: &Path, context: Option<&GitContext>, args: &[String]) -> Result<String> {
    Ok(String::from_utf8(git(cwd, context, args).await?)
        .context("Git review requires UTF-8 paths and metadata")?
        .trim_end_matches('\n')
        .to_owned())
}
async fn resolve(cwd: &Path, reference: &str) -> Result<String> {
    let value = text(
        cwd,
        None,
        &words(&["rev-parse", "--verify", &format!("{reference}^{{commit}}")]),
    )
    .await?;
    ensure!(oid(&value), "Review target did not resolve to a commit");
    Ok(value)
}
pub(crate) async fn discover(
    request: &ReviewRequest,
    cwd: &Path,
    fs: &FilesystemPolicy,
) -> Result<Discovery> {
    super::validate_request(request)?;
    let root = PathBuf::from(text(cwd, None, &words(&["rev-parse", "--show-toplevel"])).await?)
        .canonicalize()?;
    ensure!(
        cwd.starts_with(&root),
        "Review workspace is outside its Git worktree"
    );
    let git_dir = fs.check_read(
        cwd,
        Path::new(&text(cwd, None, &words(&["rev-parse", "--absolute-git-dir"])).await?),
    )?;
    let common_path =
        PathBuf::from(text(cwd, None, &words(&["rev-parse", "--git-common-dir"])).await?);
    let common_path = if common_path.is_absolute() {
        common_path
    } else {
        cwd.join(common_path)
    };
    let common_dir = fs.check_read(cwd, &common_path.canonicalize()?)?;
    fs.check_read(cwd, &common_dir.join("objects"))?;
    let prefix = cwd
        .strip_prefix(&root)?
        .to_str()
        .context("Review workspace path is not UTF-8")?
        .replace('\\', "/");
    let prefix = if prefix.is_empty() {
        prefix
    } else {
        format!("{prefix}/")
    };
    let head = resolve(cwd, "HEAD")
        .await
        .context("Review requires an existing local HEAD commit")?;
    let object_format = text(cwd, None, &words(&["rev-parse", "--show-object-format"])).await?;
    ensure!(
        matches!(object_format.as_str(), "sha1" | "sha256"),
        "Unsupported Git object format"
    );
    let (base, revision) = match &request.target {
        ReviewTarget::Uncommitted => (Some(head.clone()), None),
        ReviewTarget::BaseBranch { branch } => {
            let branch = resolve(cwd, branch).await?;
            let base = text(cwd, None, &words(&["merge-base", &branch, &head])).await?;
            ensure!(oid(&base), "Review branches have no unique merge base");
            (Some(base), Some(head.clone()))
        }
        ReviewTarget::Commit { commit } => {
            let commit = resolve(cwd, commit).await?;
            let parents = text(
                cwd,
                None,
                &words(&["rev-list", "--parents", "-n", "1", &commit]),
            )
            .await?;
            let base = parents.split_whitespace().nth(1).map(str::to_owned);
            ensure!(base.as_deref().is_none_or(oid), "Invalid parent commit");
            (base, Some(commit))
        }
    };
    Ok(Discovery {
        root,
        git_dir,
        common_dir,
        prefix,
        head,
        base,
        revision,
        object_format,
    })
}
fn checked_path(root: &Path, fs: &FilesystemPolicy, path: &str) -> Result<PathBuf> {
    validate_path(path)?;
    let target = fs.check_read(root, Path::new(path))?;
    ensure!(
        target.starts_with(root),
        "Review paths cannot follow links outside the workspace"
    );
    Ok(target)
}
fn scoped(paths: &[String]) -> Vec<String> {
    if paths.is_empty() {
        vec![".".into()]
    } else {
        paths.to_vec()
    }
}
fn diff_args(context: &GitContext, paths: &[String], names: bool) -> Vec<String> {
    let mut args = if let Some(base) = &context.base {
        let mut args = words(&[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--relative",
        ]);
        if names {
            args.extend(words(&["--name-only", "-z"]));
        } else {
            args.extend(words(&[
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--unified=5",
            ]));
        }
        args.push(base.clone());
        if let Some(revision) = &context.revision {
            args.push(revision.clone());
        }
        args
    } else {
        let mut args = words(&[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "-r",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--relative",
        ]);
        if names {
            args.extend(words(&["--name-only", "-z"]));
        } else {
            args.extend(words(&[
                "-p",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "--unified=5",
            ]));
        }
        args.push(
            context
                .revision
                .as_ref()
                .expect("root commit revision")
                .clone(),
        );
        args
    };
    args.push("--".into());
    args.extend(scoped(paths));
    args
}
fn paths(bytes: Vec<u8>) -> Result<Vec<String>> {
    let mut result = bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            Ok(std::str::from_utf8(path)
                .context("Review requires UTF-8 file paths")?
                .to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    result.sort();
    result.dedup();
    ensure!(
        result.len() <= 8192,
        "Review file list exceeds 8192 entries; narrow review.paths"
    );
    Ok(result)
}
async fn file_bytes(
    context: &GitContext,
    fs: &FilesystemPolicy,
    path: &str,
) -> Result<(Vec<u8>, bool)> {
    let target = checked_path(&context.worktree, fs, path)?;
    let blob = |revision: &str| format!("{revision}:{}{path}", context.prefix);
    if let Some(revision) = &context.revision {
        return Ok((
            git(
                &context.worktree,
                Some(context),
                &words(&["show", &blob(revision)]),
            )
            .await?,
            false,
        ));
    }
    match tokio::fs::symlink_metadata(&target).await {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Review does not follow symlinks or read directories as files"
            );
            ensure!(
                metadata.len() <= FILE_LIMIT as u64,
                "Review file exceeds 512 KiB; narrow review scope"
            );
            let file = tokio::fs::File::open(&target).await?;
            let mut bytes = Vec::new();
            file.take((FILE_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)
                .await?;
            ensure!(
                bytes.len() <= FILE_LIMIT,
                "Review file grew beyond the read limit"
            );
            Ok((bytes, false))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let base = context
                .base
                .as_deref()
                .context("Deleted review file has no base commit")?;
            Ok((
                git(
                    &context.worktree,
                    Some(context),
                    &words(&["show", &blob(base)]),
                )
                .await?,
                true,
            ))
        }
        Err(error) => Err(error.into()),
    }
}
async fn capture(
    context: &GitContext,
    request: &ReviewRequest,
    fs: &FilesystemPolicy,
) -> Result<Capture> {
    let cwd = &context.worktree;
    let mut files = paths(
        git(
            cwd,
            Some(context),
            &diff_args(context, &request.paths, true),
        )
        .await?,
    )?;
    let untracked = if context.revision.is_none() && request.include_untracked {
        let mut args = words(&["ls-files", "--others", "--exclude-standard", "-z", "--"]);
        args.extend(scoped(&request.paths));
        paths(git(cwd, Some(context), &args).await?)?
    } else {
        Vec::new()
    };
    let mut all_files = if let Some(revision) = &context.revision {
        let mut args = words(&["ls-tree", "-r", "--name-only", "-z", revision, "--"]);
        args.extend(scoped(&request.paths));
        // ls-tree without --full-tree reports paths relative to the selected workspace.
        paths(git(cwd, Some(context), &args).await?)?
    } else {
        let mut args = words(&["ls-files", "--cached", "-z", "--"]);
        args.extend(scoped(&request.paths));
        paths(git(cwd, Some(context), &args).await?)?
    };
    files.extend(untracked.iter().cloned());
    files.sort();
    files.dedup();
    ensure!(
        files.len() <= 128,
        "Review scope exceeds 128 changed files; narrow review.paths"
    );
    for path in &files {
        checked_path(cwd, fs, path)?;
    }
    all_files.extend(files.iter().cloned());
    all_files.sort();
    all_files.dedup();
    // Do not let Git resolve restricted paths in a patch before policy validation.
    let patch = if files.is_empty() {
        String::new()
    } else {
        String::from_utf8(git(cwd, Some(context), &diff_args(context, &files, false)).await?)
            .context("Review patch is not UTF-8")?
    };
    ensure!(
        patch.len() <= 256 * 1024,
        "Review patch exceeds 256 KiB; narrow review.paths"
    );
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(request)?);
    hash.update(&context.head);
    hash.update(context.base.as_deref().unwrap_or(""));
    hash.update(context.revision.as_deref().unwrap_or(""));
    hash.update(&patch);
    let mut omitted_files = Vec::new();
    for path in &files {
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path);
        match file_bytes(context, fs, path).await {
            Ok((bytes, deleted)) => {
                hash.update([u8::from(deleted)]);
                hash.update(Sha256::digest(&bytes));
            }
            Err(_) => {
                omitted_files.push(path.clone());
                hash.update(b"unavailable file content");
            }
        }
    }
    Ok(Capture {
        files,
        all_files,
        untracked,
        omitted_files,
        patch,
        fingerprint: format!("{:x}", hash.finalize()),
        head: context.head.clone(),
        base: context.base.clone(),
        revision: context.revision.clone(),
    })
}
pub(crate) async fn execute(request: &WorkerRequest) -> Result<String> {
    let operation: GitRead = serde_json::from_value(request.arguments.clone())?;
    if let GitRead::Discover { request: target } = &operation {
        return Ok(serde_json::to_string(
            &discover(
                target,
                &request.environment.workspace_root,
                &request.filesystem,
            )
            .await?,
        )?);
    }
    let context = match &operation {
        GitRead::Capture { git, .. } | GitRead::Read { git, .. } | GitRead::Patch { git, .. } => {
            git
        }
        _ => unreachable!(),
    };
    let temporary = PathBuf::from(
        std::env::var_os("TMPDIR").context("Review worker temporary directory is missing")?,
    )
    .canonicalize()?;
    ensure!(
        context.directory.canonicalize()?.starts_with(temporary),
        "Review Git metadata must be host prepared"
    );
    ensure!(
        context.worktree == request.environment.workspace_root
            && context.worktree.starts_with(&context.repository_root)
            && context
                .worktree
                .strip_prefix(&context.repository_root)
                .ok()
                .and_then(|path| path.to_str())
                .map(|prefix| if prefix.is_empty() {
                    String::new()
                } else {
                    format!("{}/", prefix.replace('\\', "/"))
                })
                .as_deref()
                == Some(context.prefix.as_str())
            && oid(&context.head)
            && context.base.as_deref().is_none_or(oid)
            && context.revision.as_deref().is_none_or(oid),
        "Invalid review Git context"
    );
    match operation {
        GitRead::Capture {
            git,
            request: target,
        } => {
            super::validate_request(&target)?;
            Ok(serde_json::to_string(
                &capture(&git, &target, &request.filesystem).await?,
            )?)
        }
        GitRead::Read { git, path } => {
            let (bytes, deleted) = file_bytes(&git, &request.filesystem, &path).await?;
            ensure!(
                bytes.len() <= FILE_LIMIT && !bytes.contains(&0),
                "Binary or oversized review file cannot be inspected as text"
            );
            let fingerprint = format!("{:x}", Sha256::digest(&bytes));
            Ok(serde_json::to_string(&FileContent {
                content: String::from_utf8(bytes).context("Review file is not UTF-8")?,
                fingerprint,
                deleted,
            })?)
        }
        GitRead::Patch { git, path } => {
            checked_path(&git.worktree, &request.filesystem, &path)?;
            Ok(serde_json::to_string(&String::from_utf8(
                git_read_patch(&git, &path).await?,
            )?)?)
        }
        _ => unreachable!(),
    }
}
async fn git_read_patch(context: &GitContext, path: &str) -> Result<Vec<u8>> {
    git(
        &context.worktree,
        Some(context),
        &diff_args(context, &[path.into()], false),
    )
    .await
}
