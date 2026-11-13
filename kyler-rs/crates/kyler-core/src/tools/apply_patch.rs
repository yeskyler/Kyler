use crate::{
    approval::ToolOperation,
    sandbox::FilesystemPolicy,
    tools::{ActivityKind, Tool, ToolContext, ToolExecutionMode},
};

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use anyhow::Context;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;

pub(crate) const MAX_PATCH_ARGUMENT_BYTES: usize = 16 * 1024;
pub(crate) const PATCH_GENERATION_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(120);

#[derive(Debug)]
pub(crate) struct PatchGenerationLimit(pub &'static str);
impl std::fmt::Display for PatchGenerationLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "apply_patch {}. The next patch should change only one named section or function, at most about 40 changed lines and 2 KiB of JSON arguments. The hard argument limit remains 16384 UTF-8 bytes. Read existing files when state is uncertain and update only changed sections; do not resend the rejected whole-file patch or bypass the budget through shell commands.",
            self.0
        )
    }
}
impl std::error::Error for PatchGenerationLimit {}

pub(crate) fn check_argument_size(bytes: usize) -> anyhow::Result<()> {
    if bytes > MAX_PATCH_ARGUMENT_BYTES {
        return Err(PatchGenerationLimit("arguments exceeded the 16 KiB limit").into());
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyPatchArgs {
    /// Literal patch text. Keep the entire JSON argument object within 16 KiB of UTF-8 bytes;
    /// aim for 50–80 changed lines and about 4 KiB or less per call. Example to create a file:
    /// *** Begin Patch
    /// *** Add File: notes.txt
    /// +Hello
    /// +
    /// +World
    /// *** End Patch
    /// To edit an existing file, use *** Update File: notes.txt followed by @@,
    /// -old text and +new text. Context lines start with one space.
    /// Every added line, including blank lines, starts with +; use + alone for an empty line.
    pub input: String,
}

pub async fn apply_patch(args: ApplyPatchArgs) -> anyhow::Result<String> {
    let root = std::env::current_dir().context("failed to determine current directory")?;
    apply_patch_in(&root, &args.input).await
}

pub(crate) async fn apply_patch_in(root: &Path, input: &str) -> anyhow::Result<String> {
    let patch = parse_patch(input)?;
    let mut changed_paths = Vec::with_capacity(patch.operations.len());
    let mut staged = BTreeMap::<PathBuf, Option<String>>::new();
    let mut prepared = Vec::new();

    for operation in patch.operations {
        match operation {
            FileOperation::Add { path, lines } => {
                let path = resolve_path(root, &path)?;
                anyhow::ensure!(
                    !staged_exists(&staged, &path).await?,
                    "cannot add `{}` because it already exists; read the file and use Update File instead",
                    path.display()
                );
                let trailing_newline = !lines.is_empty();
                stage_write(&mut staged, &mut prepared, &path, lines, trailing_newline);
                changed_paths.push(path);
            }
            FileOperation::Delete { path } => {
                let path = resolve_path(root, &path)?;
                anyhow::ensure!(
                    staged_exists(&staged, &path).await?,
                    "cannot delete `{}` because it does not exist; inspect current file state before retrying",
                    path.display()
                );
                if !staged.contains_key(&path) {
                    anyhow::ensure!(
                        fs::metadata(&path).await?.is_file(),
                        "cannot delete `{}`: expected a file",
                        path.display()
                    );
                }
                staged.insert(path.clone(), None);
                prepared.push(PreparedOperation::Remove(path.clone()));
                changed_paths.push(path);
            }
            FileOperation::Update {
                path,
                move_path,
                hunks,
            } => {
                let source_path = resolve_path(root, &path)?;
                let source = match staged.get(&source_path) {
                    Some(Some(content)) => content.clone(),
                    Some(None) => anyhow::bail!("cannot update `{}` because an earlier operation removed it", source_path.display()),
                    None => fs::read_to_string(&source_path).await.with_context(|| format!("failed to read `{}` for Update File; read the current file before retrying", source_path.display()))?,
                };
                let ends_with_newline = source.ends_with('\n');
                let mut lines = split_lines(&source);

                for (index, hunk) in hunks.into_iter().enumerate() {
                    apply_hunk(&mut lines, &hunk).with_context(|| {
                        format!(
                            "patch verification failed for `{}` at hunk {}",
                            source_path.display(),
                            index + 1
                        )
                    })?;
                }

                let destination_path = match move_path {
                    Some(move_path) => resolve_path(root, &move_path)?,
                    None => source_path.clone(),
                };

                if destination_path != source_path {
                    anyhow::ensure!(
                        !staged_exists(&staged, &destination_path).await?,
                        "cannot move `{}` to `{}` because the destination exists",
                        source_path.display(),
                        destination_path.display()
                    );
                }

                stage_write(
                    &mut staged,
                    &mut prepared,
                    &destination_path,
                    lines,
                    ends_with_newline,
                );
                if destination_path != source_path {
                    staged.insert(source_path.clone(), None);
                    prepared.push(PreparedOperation::Remove(source_path));
                }
                changed_paths.push(destination_path);
            }
        }
    }

    // Validate every operation before the first mutation. I/O failures during commit
    // can still be partial; this is not a filesystem transaction.
    for operation in prepared {
        match operation {
            PreparedOperation::Write {
                path,
                lines,
                trailing_newline,
            } => {
                write_lines(&path, &lines, trailing_newline).await?;
            }
            PreparedOperation::Remove(path) => {
                fs::remove_file(&path).await.with_context(|| format!("failed to remove `{}` during patch commit; inspect file state before retrying", path.display()))?;
            }
        }
    }

    let files = changed_paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    Ok(format!(
        "Applied patch to {} file(s): {}",
        files.len(),
        files.join(", ")
    ))
}

enum PreparedOperation {
    Write {
        path: PathBuf,
        lines: Vec<String>,
        trailing_newline: bool,
    },
    Remove(PathBuf),
}

async fn staged_exists(
    staged: &BTreeMap<PathBuf, Option<String>>,
    path: &Path,
) -> anyhow::Result<bool> {
    match staged.get(path) {
        Some(content) => Ok(content.is_some()),
        None => Ok(fs::try_exists(path).await?),
    }
}

fn stage_write(
    staged: &mut BTreeMap<PathBuf, Option<String>>,
    prepared: &mut Vec<PreparedOperation>,
    path: &Path,
    lines: Vec<String>,
    trailing_newline: bool,
) {
    let mut content = lines.join("\n");
    if trailing_newline {
        content.push('\n');
    }
    staged.insert(path.to_owned(), Some(content));
    prepared.push(PreparedOperation::Write {
        path: path.to_owned(),
        lines,
        trailing_newline,
    });
}

pub(crate) fn validate_patch_access(
    root: &Path,
    input: &str,
    policy: &FilesystemPolicy,
) -> anyhow::Result<()> {
    for operation in parse_patch(input)?.operations {
        match operation {
            FileOperation::Add { path, .. } | FileOperation::Delete { path } => {
                policy.check_write(root, &resolve_path(root, &path)?)?;
            }
            FileOperation::Update {
                path, move_path, ..
            } => {
                policy.check_write(root, &resolve_path(root, &path)?)?;
                if let Some(destination) = move_path {
                    policy.check_write(root, &resolve_path(root, &destination)?)?;
                }
            }
        }
    }
    Ok(())
}

/// Return bounded, non-destructive targets for host-side automatic review.
/// Parsing stays shared with execution so the two cannot disagree on operations.
pub(crate) fn automatic_patch_targets(
    root: &Path,
    input: &str,
) -> anyhow::Result<Option<Vec<PathBuf>>> {
    if input.len() > 128 * 1024 {
        return Ok(None);
    }
    let patch = parse_patch(input)?;
    if patch.operations.len() > 20 {
        return Ok(None);
    }
    let mut targets = Vec::new();
    for operation in patch.operations {
        let target = match operation {
            FileOperation::Add { path, .. } => {
                let path = resolve_path(root, &path)?;
                if path.exists() {
                    return Ok(None);
                }
                path
            }
            FileOperation::Update {
                path,
                move_path: None,
                ..
            } => {
                let path = resolve_path(root, &path)?;
                if !path.is_file() {
                    return Ok(None);
                }
                path
            }
            _ => return Ok(None),
        };
        targets.push(target);
    }
    Ok(Some(targets))
}

async fn write_lines(path: &Path, lines: &[String], trailing_newline: bool) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }

    let mut content = lines.join("\n");
    if trailing_newline {
        content.push('\n');
    }
    fs::write(path, content)
        .await
        .with_context(|| format!("failed to write {}", path.display()))
}

fn split_lines(content: &str) -> Vec<String> {
    content
        .split_terminator('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

fn validate_path_syntax(raw_path: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !raw_path.trim().is_empty(),
        "patch file path must not be empty"
    );
    anyhow::ensure!(
        !Path::new(raw_path)
            .components()
            .any(|component| matches!(component, Component::ParentDir)),
        "patch file path escapes the workspace: `{raw_path}`"
    );
    Ok(())
}

fn resolve_path(root: &Path, raw_path: &str) -> anyhow::Result<PathBuf> {
    validate_path_syntax(raw_path)?;
    let root = std::fs::canonicalize(root).context("failed to resolve patch workspace")?;
    let path = crate::sandbox::paths::resolve_path(&root, Path::new(raw_path))?;
    anyhow::ensure!(
        path != root && path.starts_with(&root),
        "patch file path escapes the workspace: `{raw_path}`"
    );
    Ok(path)
}

fn apply_hunk(lines: &mut Vec<String>, hunk: &Hunk) -> anyhow::Result<()> {
    let expected = hunk
        .lines
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(value) | HunkLine::Remove(value) => Some(value.clone()),
            HunkLine::Add(_) => None,
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !expected.is_empty(),
        "patch hunk must contain context or removed lines"
    );

    let replacement = hunk
        .lines
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(value) | HunkLine::Add(value) => Some(value.clone()),
            HunkLine::Remove(_) => None,
        })
        .collect::<Vec<_>>();

    let matches = lines
        .windows(expected.len())
        .enumerate()
        .filter_map(|(index, window)| (window == expected.as_slice()).then_some(index))
        .collect::<Vec<_>>();

    anyhow::ensure!(
        matches.len() == 1,
        "patch hunk matched {} locations; expected exactly one. Read the current file, use exact context, and add surrounding lines if the match is ambiguous",
        matches.len()
    );

    let index = matches[0];
    lines.splice(index..index + expected.len(), replacement);
    Ok(())
}

fn parse_patch(input: &str) -> anyhow::Result<Patch> {
    let leading = input.len() - input.trim_start().len();
    let first_line = input[..leading]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let lines = input
        .trim()
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        lines.first() == Some(&"*** Begin Patch"),
        "invalid patch format: start with the literal `*** Begin Patch`, use `*** Add File: path` or `*** Update File: path` (not `File:`), and finish with `*** End Patch`"
    );
    anyhow::ensure!(
        lines.last() == Some(&"*** End Patch"),
        "patch must end with `*** End Patch`; resend a complete patch, splitting large changes into smaller complete calls"
    );

    let mut operations = Vec::new();
    let mut index = 1;
    while index < lines.len() - 1 {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut content = Vec::new();
            while index < lines.len() - 1 && !lines[index].starts_with("*** ") {
                let value = if let Some(value) = lines[index].strip_prefix('+') {
                    value
                } else if lines[index].trim().is_empty() {
                    // A bare blank line has one unambiguous meaning in an Add block.
                    // Never repair nonblank content or ambiguous update hunks.
                    lines[index]
                } else {
                    anyhow::bail!(
                        "invalid added content for `{path}` at patch line {}: added file content must start with `+`; use `+` alone for an empty line",
                        first_line + index
                    );
                };
                content.push(value.to_string());
                index += 1;
            }
            operations.push(FileOperation::Add {
                path: path.to_string(),
                lines: content,
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            operations.push(FileOperation::Delete {
                path: path.to_string(),
            });
            index += 1;
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let mut move_path = None;
            if index < lines.len() - 1 {
                if let Some(path) = lines[index].strip_prefix("*** Move to: ") {
                    move_path = Some(path.to_string());
                    index += 1;
                }
            }

            let mut hunks = Vec::new();
            while index < lines.len() - 1 && !lines[index].starts_with("*** ") {
                anyhow::ensure!(
                    lines[index].starts_with("@@"),
                    "expected `@@` hunk for `{path}` at patch line {}",
                    first_line + index
                );
                index += 1;
                let mut hunk_lines = Vec::new();
                while index < lines.len() - 1
                    && !lines[index].starts_with("@@")
                    && !lines[index].starts_with("*** ")
                {
                    let line = lines[index];
                    let hunk_line = match line.as_bytes().first() {
                        Some(b' ') => HunkLine::Context(line[1..].to_string()),
                        Some(b'-') => HunkLine::Remove(line[1..].to_string()),
                        Some(b'+') => HunkLine::Add(line[1..].to_string()),
                        _ => anyhow::bail!(
                            "invalid hunk for `{path}` at patch line {}: patch hunk lines must start with space, `-`, or `+`; blank context/add/remove lines need the same prefix",
                            first_line + index
                        ),
                    };
                    hunk_lines.push(hunk_line);
                    index += 1;
                }
                anyhow::ensure!(!hunk_lines.is_empty(), "patch hunk must not be empty");
                hunks.push(Hunk { lines: hunk_lines });
            }
            anyhow::ensure!(
                !hunks.is_empty(),
                "updated file must contain at least one hunk"
            );
            operations.push(FileOperation::Update {
                path: path.to_string(),
                move_path,
                hunks,
            });
        } else {
            anyhow::bail!(
                "unexpected patch directive `{line}` at patch line {}",
                first_line + index
            );
        }
    }

    anyhow::ensure!(
        !operations.is_empty(),
        "patch must contain at least one file operation"
    );
    Ok(Patch { operations })
}

struct Patch {
    operations: Vec<FileOperation>,
}

enum FileOperation {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_path: Option<String>,
        hunks: Vec<Hunk>,
    },
}

struct Hunk {
    lines: Vec<HunkLine>,
}

enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[cfg(test)]
mod tests {
    use super::apply_patch_in;
    use crate::tools::Tool;

    use std::path::PathBuf;

    fn temporary_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kyler-apply-patch-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn patch_argument_budget_counts_utf8_bytes_and_protects_direct_calls() {
        assert!(super::check_argument_size(super::MAX_PATCH_ARGUMENT_BYTES).is_ok());
        assert!(super::check_argument_size(super::MAX_PATCH_ARGUMENT_BYTES + 1).is_err());
        let arguments = serde_json::json!({"input":"界".repeat(6000)});
        let error = super::ApplyPatchTool
            .approval_requirement(&arguments)
            .unwrap_err();
        assert!(
            error
                .downcast_ref::<super::PatchGenerationLimit>()
                .is_some()
        );
    }

    #[tokio::test]
    async fn verifies_all_file_operations_before_writing() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("existing.txt"), "original\n").unwrap();
        for invalid in [
            "*** Update File: existing.txt\n@@\n-missing\n+replacement",
            "*** Add File: existing.txt\n+replacement",
            "*** Delete File: missing.txt",
            "*** Add File: ../outside.txt\n+outside",
        ] {
            let patch =
                format!("*** Begin Patch\n*** Add File: first.txt\n+new\n{invalid}\n*** End Patch");
            let error = apply_patch_in(directory.path(), &patch).await.unwrap_err();
            assert!(!directory.path().join("first.txt").exists(), "{error:#}");
            assert_eq!(
                std::fs::read_to_string(directory.path().join("existing.txt")).unwrap(),
                "original\n"
            );
        }
        let error = apply_patch_in(
            directory.path(),
            "*** Begin Patch\n*** Update File: existing.txt\n@@\n-missing\n+new\n*** End Patch",
        )
        .await
        .unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("existing.txt"));
        assert!(error.contains("hunk 1"));
        assert!(error.contains("Read the current file"));
    }

    #[tokio::test]
    async fn preflight_preserves_ordered_changes_to_the_same_file() {
        let directory = tempfile::tempdir().unwrap();
        apply_patch_in(directory.path(), "*** Begin Patch\n*** Add File: first.txt\n+old\n*** Update File: first.txt\n*** Move to: final.txt\n@@\n-old\n+new\n*** Update File: final.txt\n@@\n-new\n+done\n*** End Patch").await.unwrap();
        assert!(!directory.path().join("first.txt").exists());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("final.txt")).unwrap(),
            "done\n"
        );
    }

    #[tokio::test]
    async fn adds_updates_moves_and_deletes_files() {
        let directory = temporary_directory("operations");
        tokio::fs::create_dir_all(&directory)
            .await
            .expect("create temporary directory");

        apply_patch_in(
            &directory,
            "*** Begin Patch\n*** Add File: first.txt\n+old\n*** End Patch",
        )
        .await
        .expect("add file");
        apply_patch_in(
            &directory,
            "*** Begin Patch\n*** Update File: first.txt\n*** Move to: moved.txt\n@@\n-old\n+new\n*** End Patch",
        )
        .await
        .expect("update and move file");

        assert!(!directory.join("first.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(directory.join("moved.txt"))
                .await
                .expect("read moved file"),
            "new\n"
        );

        apply_patch_in(
            &directory,
            "*** Begin Patch\n*** Delete File: moved.txt\n*** End Patch",
        )
        .await
        .expect("delete file");
        assert!(!directory.join("moved.txt").exists());

        tokio::fs::remove_dir_all(directory)
            .await
            .expect("remove temporary directory");
    }

    #[tokio::test]
    async fn rejects_paths_outside_workspace() {
        let directory = temporary_directory("path");
        tokio::fs::create_dir_all(&directory)
            .await
            .expect("create temporary directory");

        let error = apply_patch_in(
            &directory,
            "*** Begin Patch\n*** Add File: ../outside.txt\n+unsafe\n*** End Patch",
        )
        .await
        .expect_err("parent directory path must fail");

        assert!(error.to_string().contains("escapes the workspace"));
        tokio::fs::remove_dir_all(directory)
            .await
            .expect("remove temporary directory");
    }

    #[test]
    fn rejects_observed_malformed_format_before_permission_review() {
        use crate::tools::Tool;

        let error = super::ApplyPatchTool
            .approval_requirement(&serde_json::json!({
                "input": "Begin Patch\nFile: example.py\n@@\n-old\n+new\nEnd Patch"
            }))
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("invalid patch format"));
        assert!(message.contains("*** Begin Patch"));
        assert!(!message.contains("Sandbox denied"));
    }

    #[tokio::test]
    async fn absolute_workspace_paths_work_for_add_update_move_and_delete() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let first = root.join("nested/first.txt");
        let moved = root.join("nested/moved.txt");
        apply_patch_in(
            &root,
            &format!(
                "\n*** Begin Patch\n*** Add File: {}\n+old\n*** End Patch\n",
                first.display()
            ),
        )
        .await
        .unwrap();
        apply_patch_in(&root, &format!("*** Begin Patch\n*** Update File: {}\n*** Move to: {}\n@@\n-old\n+new\n*** End Patch", first.display(), moved.display())).await.unwrap();
        assert!(!first.exists());
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "new\n");
        apply_patch_in(
            &root,
            &format!(
                "*** Begin Patch\n*** Delete File: {}\n*** End Patch",
                moved.display()
            ),
        )
        .await
        .unwrap();
        assert!(!moved.exists());
    }

    #[tokio::test]
    async fn absolute_and_symlink_paths_cannot_escape_the_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.txt");
        assert!(
            apply_patch_in(
                &root,
                &format!(
                    "*** Begin Patch\n*** Add File: {}\n+bad\n*** End Patch",
                    target.display()
                )
            )
            .await
            .is_err()
        );
        assert!(!target.exists());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
            assert!(
                apply_patch_in(
                    &root,
                    "*** Begin Patch\n*** Add File: link/outside.txt\n+bad\n*** End Patch"
                )
                .await
                .is_err()
            );
            assert!(!target.exists());
        }
    }
    #[tokio::test]
    async fn new_files_preserve_prefixed_and_bare_blank_lines() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let patch = "*** Begin Patch\r\n*** Add File: example.rs\r\n+fn main() {\r\n\r\n    \r\n+\r\n+}\r\n*** End Patch";
        apply_patch_in(&root, patch).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("example.rs")).unwrap(),
            "fn main() {\n\n    \n\n}\n"
        );
    }

    #[tokio::test]
    async fn invalid_nonblank_content_reports_original_line_and_writes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let patch = "\n\n*** Begin Patch\n*** Add File: valid.txt\n+valid\n*** Add File: bad.rs\nfn main() {}\n*** End Patch";
        let error = apply_patch_in(&root, patch).await.unwrap_err().to_string();
        assert!(error.contains("bad.rs"));
        assert!(error.contains("patch line 7"));
        assert!(error.contains("use `+` alone"));
        assert!(!root.join("valid.txt").exists());
        assert!(!root.join("bad.rs").exists());
    }

    #[tokio::test]
    async fn blank_update_hunks_remain_explicit_and_leave_existing_content_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(root.join("existing.txt"), "old\n").unwrap();
        let error = apply_patch_in(
            &root,
            "*** Begin Patch\n*** Update File: existing.txt\n@@\n-old\n\n+new\n*** End Patch",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("existing.txt"));
        assert!(error.contains("patch line 5"));
        assert_eq!(
            std::fs::read_to_string(root.join("existing.txt")).unwrap(),
            "old\n"
        );
    }
}

pub struct ApplyPatchTool;

#[async_trait::async_trait]
impl Tool for ApplyPatchTool {
    fn approval_requirement(&self, arguments: &Value) -> anyhow::Result<ToolOperation> {
        check_argument_size(serde_json::to_vec(arguments)?.len())?;
        let args: ApplyPatchArgs = serde_json::from_value(arguments.clone()).context(
            "invalid apply_patch arguments; provide an input string containing the patch",
        )?;
        let patch = parse_patch(&args.input).context("invalid apply_patch input")?;
        for operation in patch.operations {
            match operation {
                FileOperation::Add { path, .. } | FileOperation::Delete { path } => {
                    validate_path_syntax(&path)?
                }
                FileOperation::Update {
                    path, move_path, ..
                } => {
                    validate_path_syntax(&path)?;
                    if let Some(path) = move_path {
                        validate_path_syntax(&path)?;
                    }
                }
            }
        }
        Ok(ToolOperation::Write)
    }

    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Edit
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Worker
    }
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Create, update, move or delete files using literal patch text in the input argument. \
         Required format: *** Begin Patch\n*** Add File: notes.txt\n+Hello\n*** End Patch. \
         Hard limit: 16 KiB of UTF-8 JSON arguments per call. Aim for 50–80 changed lines and about 4 KiB or less, not the hard maximum. For large new files add a concise runnable structure first, then update one function/style/section at a time. Do not generate a whole application in the first Add File or minify code to fit. \
         Prefer small coherent edits; related file operations may share a patch. Split large new files into complete patches when useful; do not repeat unchanged content. \
         Add File never overwrites: if the file exists, read it and use Update File. After a malformed patch, inspect file state and retry a smaller complete patch. After a size/time interruption, the next complete patch should address one named part, at most about 40 changed lines and 2 KiB; do not resend the full rejected patch or move it into shell arguments. \
         For edits, read the file first, then use *** Update File: notes.txt\n@@\n-old text\n+new text. \
         Normally include three unchanged lines before and after each edit, enough to identify exactly one location. Do not duplicate overlapping context between adjacent edits; combine them into one hunk. Context lines start with one space; added/removed lines start with +/-. \
         Every added line, including blank lines, must start with +; use + alone for an empty line. \
         Use *** Delete File: path to delete and *** Move to: path after an Update header to rename. \
         Paths are relative to workspace_root, or absolute paths inside that workspace. \
         Do not use bare Begin Patch, File:, unified diff headers or Markdown fences."
    }

    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(ApplyPatchArgs))
            .expect("ApplyPatchArgs schema is always serializable")
    }

    async fn execute(
        &self,
        args_json: &str,
        _context: &mut ToolContext<'_>,
    ) -> anyhow::Result<String> {
        check_argument_size(args_json.len())?;
        if let Some(environment) = _context.environment {
            return environment.run_tool(self.name(), args_json).await;
        }
        let args: ApplyPatchArgs =
            serde_json::from_str(args_json).context("failed to parse apply_patch arguments")?;
        apply_patch(args).await
    }
}
