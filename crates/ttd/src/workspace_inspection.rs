use std::path::{Path, PathBuf};

use chrono::Utc;
use tokio::process::Command;

use tt_core::{
    TTError, TTResult,
    authority::TrackedThreadWorkspace,
    ipc::{
        TrackedThreadWorkspaceFilesystemScope, TrackedThreadWorkspaceInspection,
        TrackedThreadWorkspaceInspectionWarning, TrackedThreadWorkspaceRefComparison,
    },
};

#[derive(Debug, Clone)]
struct WorktreeEntry {
    worktree_path: PathBuf,
}

pub async fn inspect_tracked_thread_workspace(
    workspace: &TrackedThreadWorkspace,
) -> TrackedThreadWorkspaceInspection {
    let inspected_at = Utc::now();
    let repository_root = PathBuf::from(&workspace.repository_root);
    let worktree_path = PathBuf::from(&workspace.worktree_path);
    let exists = tokio::fs::try_exists(&worktree_path).await.unwrap_or(false);

    let mut warnings = Vec::new();
    if !exists {
        warnings.push(TrackedThreadWorkspaceInspectionWarning::MissingWorktree);
        return TrackedThreadWorkspaceInspection {
            inspected_at,
            repository_root: workspace.repository_root.clone(),
            worktree_path: workspace.worktree_path.clone(),
            exists,
            is_git_worktree: false,
            current_branch: None,
            current_head_commit: None,
            dirty: None,
            base_ref: Some(workspace.base_ref.clone()),
            base_commit: workspace.base_commit.clone(),
            landing_target: Some(workspace.landing_target.clone()),
            base_commit_comparison: None,
            landing_target_comparison: None,
            warnings,
        };
    }

    let repo_is_git = git_bool(&repository_root, &["rev-parse", "--is-inside-work-tree"])
        .await
        .unwrap_or(false);
    let worktree_entries = if repo_is_git {
        git_worktree_entries(&repository_root)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let is_git_worktree = worktree_entries
        .iter()
        .any(|entry| paths_match(&entry.worktree_path, &worktree_path));
    if !is_git_worktree {
        warnings.push(TrackedThreadWorkspaceInspectionWarning::InvalidWorktree);
    }

    let current_head_commit = git_string(&worktree_path, &["rev-parse", "HEAD"])
        .await
        .ok()
        .flatten()
        .filter(|value| !value.is_empty());
    let current_branch = git_string(&worktree_path, &["branch", "--show-current"])
        .await
        .ok()
        .flatten()
        .filter(|value| !value.is_empty());
    let dirty = git_stdout(
        &worktree_path,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )
    .await
    .ok()
    .flatten()
    .map(|value| !value.trim().is_empty());

    if current_head_commit.is_some() && current_branch.is_none() {
        warnings.push(TrackedThreadWorkspaceInspectionWarning::DetachedHead);
    }
    if dirty == Some(true) {
        warnings.push(TrackedThreadWorkspaceInspectionWarning::DirtyWorkspace);
    }

    let comparison_cwd = if repo_is_git {
        &repository_root
    } else {
        &worktree_path
    };

    let base_commit_comparison =
        match (current_head_commit.as_ref(), workspace.base_commit.as_ref()) {
            (Some(head_commit), Some(base_commit)) => {
                compare_revision_distance(comparison_cwd, base_commit, head_commit)
                    .await
                    .ok()
                    .flatten()
                    .map(|comparison| TrackedThreadWorkspaceRefComparison {
                        reference: base_commit.to_string(),
                        ahead_by: comparison.0,
                        behind_by: comparison.1,
                    })
            }
            _ => None,
        };
    if base_commit_comparison
        .as_ref()
        .is_some_and(|comparison| comparison.behind_by > 0)
    {
        warnings.push(TrackedThreadWorkspaceInspectionWarning::BaseCommitMismatch);
    }

    let landing_target_comparison = match current_head_commit.as_ref() {
        Some(head_commit) => {
            if let Some(landing_commit) =
                resolve_commit(comparison_cwd, &workspace.landing_target).await
            {
                compare_revision_distance(comparison_cwd, &landing_commit, head_commit)
                    .await
                    .ok()
                    .flatten()
                    .map(|comparison| TrackedThreadWorkspaceRefComparison {
                        reference: landing_commit,
                        ahead_by: comparison.0,
                        behind_by: comparison.1,
                    })
            } else {
                None
            }
        }
        None => None,
    };
    if let Some(comparison) = landing_target_comparison.as_ref() {
        if comparison.behind_by > 0 && comparison.ahead_by > 0 {
            warnings.push(TrackedThreadWorkspaceInspectionWarning::DivergedFromLandingTarget);
        } else if comparison.behind_by > 0 {
            warnings.push(TrackedThreadWorkspaceInspectionWarning::BehindLandingTarget);
        }
    }

    TrackedThreadWorkspaceInspection {
        inspected_at,
        repository_root: workspace.repository_root.clone(),
        worktree_path: workspace.worktree_path.clone(),
        exists,
        is_git_worktree,
        current_branch,
        current_head_commit,
        dirty,
        base_ref: Some(workspace.base_ref.clone()),
        base_commit: workspace.base_commit.clone(),
        landing_target: Some(workspace.landing_target.clone()),
        base_commit_comparison,
        landing_target_comparison,
        warnings,
    }
}

pub async fn derive_workspace_filesystem_scope(
    workspace: &TrackedThreadWorkspace,
) -> TrackedThreadWorkspaceFilesystemScope {
    derive_workspace_filesystem_scope_sync(workspace)
}

pub fn derive_workspace_filesystem_scope_sync(
    workspace: &TrackedThreadWorkspace,
) -> TrackedThreadWorkspaceFilesystemScope {
    let worktree_path = PathBuf::from(&workspace.worktree_path);
    let repository_root = PathBuf::from(&workspace.repository_root);
    let worktree_parent = worktree_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| worktree_path.clone());
    let git_dir = git_output_sync(&worktree_path, &["rev-parse", "--git-dir"])
        .map(|value| resolve_git_path(&worktree_path, &value));
    let git_common_dir = git_output_sync(&worktree_path, &["rev-parse", "--git-common-dir"])
        .map(|value| resolve_git_path(&worktree_path, &value));

    let mut worker_turn_roots = vec![normalize_display_path(&worktree_path)];
    if let Some(path) = git_dir.as_ref() {
        worker_turn_roots.push(path.clone());
    }
    if let Some(path) = git_common_dir.as_ref() {
        worker_turn_roots.push(path.clone());
    }
    dedupe_paths(&mut worker_turn_roots);

    let mut workspace_lifecycle_roots = worker_turn_roots.clone();
    workspace_lifecycle_roots.push(normalize_display_path(&repository_root));
    workspace_lifecycle_roots.push(normalize_display_path(&worktree_parent));
    dedupe_paths(&mut workspace_lifecycle_roots);

    TrackedThreadWorkspaceFilesystemScope {
        repository_root: normalize_display_path(&repository_root),
        worktree_path: normalize_display_path(&worktree_path),
        worktree_parent: normalize_display_path(&worktree_parent),
        git_dir,
        git_common_dir,
        worker_turn_roots,
        workspace_lifecycle_roots,
    }
}

async fn git_bool(cwd: &Path, args: &[&str]) -> TTResult<bool> {
    Ok(git_stdout(cwd, args)
        .await?
        .is_some_and(|value| value.trim() == "true"))
}

async fn git_string(cwd: &Path, args: &[&str]) -> TTResult<Option<String>> {
    Ok(git_stdout(cwd, args)
        .await?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

async fn git_stdout(cwd: &Path, args: &[&str]) -> TTResult<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .await
        .map_err(|error| {
            TTError::Transport(format!(
                "failed to inspect git state for {}: {error}",
                cwd.display()
            ))
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()))
}

async fn git_worktree_entries(cwd: &Path) -> TTResult<Vec<WorktreeEntry>> {
    let Some(stdout) = git_stdout(cwd, &["worktree", "list", "--porcelain"]).await? else {
        return Ok(Vec::new());
    };

    let mut entries = Vec::new();
    let mut current_worktree: Option<PathBuf> = None;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            if let Some(worktree_path) = current_worktree.take() {
                entries.push(WorktreeEntry { worktree_path });
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(worktree_path) = current_worktree.take() {
                entries.push(WorktreeEntry { worktree_path });
            }
            current_worktree = Some(PathBuf::from(rest));
        }
    }

    if let Some(worktree_path) = current_worktree.take() {
        entries.push(WorktreeEntry { worktree_path });
    }

    Ok(entries)
}

async fn compare_revision_distance(
    cwd: &Path,
    left: &str,
    right: &str,
) -> TTResult<Option<(u64, u64)>> {
    let range = format!("{left}...{right}");
    let Some(stdout) = git_stdout(cwd, &["rev-list", "--left-right", "--count", &range]).await?
    else {
        return Ok(None);
    };
    let mut parts = stdout.split_whitespace();
    let Some(left_count) = parts.next().and_then(|part| part.parse::<u64>().ok()) else {
        return Ok(None);
    };
    let Some(right_count) = parts.next().and_then(|part| part.parse::<u64>().ok()) else {
        return Ok(None);
    };
    Ok(Some((right_count, left_count)))
}

async fn resolve_commit(cwd: &Path, reference: &str) -> Option<String> {
    git_string(
        cwd,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )
    .await
    .ok()
    .flatten()
}

fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let left = std::fs::canonicalize(left).ok();
    let right = std::fs::canonicalize(right).ok();
    left.is_some() && left == right
}

fn resolve_git_path(base: &Path, value: &str) -> String {
    let candidate = PathBuf::from(value);
    let resolved = if candidate.is_absolute() {
        candidate
    } else {
        base.join(candidate)
    };
    normalize_display_path(&resolved)
}

fn normalize_display_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn dedupe_paths(paths: &mut Vec<String>) {
    let mut deduped = Vec::with_capacity(paths.len());
    for path in paths.drain(..) {
        if !deduped.contains(&path) {
            deduped.push(path);
        }
    }
    *paths = deduped;
}

fn git_output_sync(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use tt_core::authority::{
        TrackedThreadId, TrackedThreadWorkspace, TrackedThreadWorkspaceCleanupPolicy,
        TrackedThreadWorkspaceLandingPolicy, TrackedThreadWorkspaceStatus,
        TrackedThreadWorkspaceStrategy, TrackedThreadWorkspaceSyncPolicy,
    };

    use super::{derive_workspace_filesystem_scope, inspect_tracked_thread_workspace};

    fn run_git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {:?} failed: {status}", args);
    }

    fn git_output(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {:?} failed", args);
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    }

    fn workspace(
        repository_root: &Path,
        worktree_path: &Path,
        base_commit: Option<String>,
    ) -> TrackedThreadWorkspace {
        TrackedThreadWorkspace {
            repository_root: repository_root.display().to_string(),
            owner_tracked_thread_id: TrackedThreadId::parse("tt-1").expect("tracked thread id"),
            strategy: TrackedThreadWorkspaceStrategy::DedicatedThreadWorktree,
            worktree_path: worktree_path.display().to_string(),
            branch_name: "tt/tt-1".to_string(),
            base_ref: "main".to_string(),
            base_commit,
            landing_target: "main".to_string(),
            landing_policy: TrackedThreadWorkspaceLandingPolicy::MergeToMain,
            sync_policy: TrackedThreadWorkspaceSyncPolicy::RebaseBeforeCompletion,
            cleanup_policy: TrackedThreadWorkspaceCleanupPolicy::PruneAfterMerge,
            last_reported_head_commit: None,
            status: TrackedThreadWorkspaceStatus::Requested,
        }
    }

    fn setup_repo() -> (PathBuf, PathBuf, String) {
        let root = std::env::temp_dir().join(format!(
            "tt-git-inspection-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let repo = root.join("repo");
        let worktree = root.join("worktree");
        std::fs::create_dir_all(&repo).expect("create repo");
        run_git(&repo, &["init", "-b", "main"]);
        run_git(&repo, &["config", "user.name", "TT Test"]);
        run_git(&repo, &["config", "user.email", "tt@example.com"]);
        std::fs::write(repo.join("README.md"), "tt\n").expect("write file");
        run_git(&repo, &["add", "README.md"]);
        run_git(&repo, &["commit", "-m", "initial"]);
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "tt/tt-1",
                worktree.to_str().expect("worktree"),
                "HEAD",
            ],
        );
        let head = git_output(&worktree, &["rev-parse", "HEAD"]);
        (repo, worktree, head)
    }

    #[tokio::test]
    async fn inspects_clean_registered_worktree() {
        let (repo, worktree, head) = setup_repo();
        let inspection =
            inspect_tracked_thread_workspace(&workspace(&repo, &worktree, Some(head.clone())))
                .await;

        assert!(inspection.exists);
        assert!(inspection.is_git_worktree);
        assert_eq!(inspection.current_branch.as_deref(), Some("tt/tt-1"));
        assert_eq!(
            inspection.current_head_commit.as_deref(),
            Some(head.as_str())
        );
        assert_eq!(inspection.dirty, Some(false));
        assert!(inspection.warnings.is_empty(), "{inspection:?}");
        assert!(inspection.base_commit_comparison.is_some());
        assert!(inspection.landing_target_comparison.is_some());
    }

    #[tokio::test]
    async fn flags_missing_worktree() {
        let (repo, worktree, head) = setup_repo();
        std::fs::remove_dir_all(&worktree).expect("remove worktree");

        let inspection =
            inspect_tracked_thread_workspace(&workspace(&repo, &worktree, Some(head))).await;

        assert!(!inspection.exists);
        assert!(!inspection.is_git_worktree);
        assert!(inspection.current_head_commit.is_none());
        assert!(
            inspection
                .warnings
                .contains(&tt_core::ipc::TrackedThreadWorkspaceInspectionWarning::MissingWorktree)
        );
    }

    #[tokio::test]
    async fn derives_workspace_filesystem_scope_for_worktree() {
        let (repo, worktree, _) = setup_repo();
        let scope = derive_workspace_filesystem_scope(&workspace(&repo, &worktree, None)).await;

        assert_eq!(scope.repository_root, repo.display().to_string());
        assert_eq!(scope.worktree_path, worktree.display().to_string());
        assert_eq!(
            scope.worktree_parent,
            worktree.parent().expect("parent").display().to_string()
        );
        assert!(
            scope
                .git_dir
                .as_deref()
                .is_some_and(|value| value.contains("/.git/worktrees/"))
        );
        assert!(
            scope
                .git_common_dir
                .as_deref()
                .is_some_and(|value| value.ends_with("/repo/.git"))
        );
        assert!(scope.worker_turn_roots.contains(&scope.worktree_path));
        assert!(
            scope
                .workspace_lifecycle_roots
                .contains(&scope.repository_root)
        );
        assert!(
            scope
                .workspace_lifecycle_roots
                .contains(&scope.worktree_parent)
        );
    }
}
