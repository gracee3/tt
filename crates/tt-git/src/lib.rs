//! Git and worktree orchestration for TT v2.
//!
//! This crate owns repository discovery, worktree inspection, and a small
//! merge-readiness model. It is intentionally CLI-driven so the v2 daemon can
//! stay thin.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tt_domain::MergeReadiness;

pub const TT_GIT_SUBSYSTEM: &str = "tt-git";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepository {
    pub repository_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepositoryInspection {
    pub inspected_at: DateTime<Utc>,
    pub repository_root: PathBuf,
    pub current_worktree: Option<PathBuf>,
    pub current_branch: Option<String>,
    pub current_head_commit: Option<String>,
    pub dirty: bool,
    pub upstream: Option<String>,
    pub ahead_by: Option<u64>,
    pub behind_by: Option<u64>,
    pub merge_readiness: MergeReadiness,
    pub worktrees: Vec<GitWorktreeInspection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitWorktreeInspection {
    pub worktree_path: PathBuf,
    pub head_commit: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub locked_reason: Option<String>,
    pub prunable: bool,
}

impl GitRepository {
    pub fn discover(cwd: impl AsRef<Path>) -> Result<Option<Self>> {
        let cwd = cwd.as_ref();
        let Some(repository_root) = git_stdout(cwd, &["rev-parse", "--show-toplevel"])? else {
            return Ok(None);
        };
        Ok(Some(Self {
            repository_root: PathBuf::from(repository_root),
        }))
    }

    pub fn inspect(cwd: impl AsRef<Path>) -> Result<Option<GitRepositoryInspection>> {
        let Some(repository) = Self::discover(cwd.as_ref())? else {
            return Ok(None);
        };
        Ok(Some(repository.inspect_repository()?))
    }

    pub fn inspect_repository(&self) -> Result<GitRepositoryInspection> {
        let inspected_at = Utc::now();
        let current_worktree =
            git_stdout(&self.repository_root, &["rev-parse", "--show-toplevel"])?
                .map(|value| PathBuf::from(value.trim()));
        let current_branch = git_stdout(&self.repository_root, &["branch", "--show-current"])?;
        let current_branch = current_branch.and_then(normalize_non_empty);
        let current_head_commit = git_stdout(&self.repository_root, &["rev-parse", "HEAD"])?;
        let current_head_commit = current_head_commit.and_then(normalize_non_empty);
        let dirty = git_stdout(
            &self.repository_root,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
        )?
        .is_some_and(|stdout| !stdout.trim().is_empty());

        let (upstream, ahead_by, behind_by) = match current_branch.as_deref() {
            Some(_) => {
                let upstream = git_stdout(
                    &self.repository_root,
                    &[
                        "rev-parse",
                        "--abbrev-ref",
                        "--symbolic-full-name",
                        "@{upstream}",
                    ],
                )?
                .and_then(normalize_non_empty);
                let divergence = if upstream.is_some() {
                    compare_revision_distance(&self.repository_root, "@{upstream}", "HEAD")?
                } else {
                    None
                };
                let (ahead_by, behind_by) =
                    divergence.map_or((None, None), |(ahead, behind)| (Some(ahead), Some(behind)));
                (upstream, ahead_by, behind_by)
            }
            None => (None, None, None),
        };

        let merge_readiness = if dirty || current_branch.is_none() || current_head_commit.is_none()
        {
            MergeReadiness::Blocked
        } else if behind_by.is_some_and(|value| value > 0) {
            MergeReadiness::Blocked
        } else {
            MergeReadiness::Ready
        };

        Ok(GitRepositoryInspection {
            inspected_at,
            repository_root: self.repository_root.clone(),
            current_worktree,
            current_branch,
            current_head_commit,
            dirty,
            upstream,
            ahead_by,
            behind_by,
            merge_readiness,
            worktrees: self.list_worktrees()?,
        })
    }

    pub fn list_worktrees(&self) -> Result<Vec<GitWorktreeInspection>> {
        let Some(stdout) = git_stdout(&self.repository_root, &["worktree", "list", "--porcelain"])?
        else {
            return Ok(Vec::new());
        };

        let mut worktrees = Vec::new();
        let mut current = WorktreeSection::default();
        for line in stdout.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                if let Some(entry) = current.finish()? {
                    worktrees.push(entry);
                }
                current = WorktreeSection::default();
                continue;
            }
            if let Some(rest) = line.strip_prefix("worktree ") {
                current.worktree_path = Some(PathBuf::from(rest));
                continue;
            }
            if let Some(rest) = line.strip_prefix("HEAD ") {
                current.head_commit = Some(rest.to_string());
                continue;
            }
            if let Some(rest) = line.strip_prefix("branch ") {
                current.branch = normalize_non_empty(rest.to_string());
                continue;
            }
            if line == "bare" {
                current.bare = true;
                continue;
            }
            if let Some(rest) = line.strip_prefix("locked ") {
                current.locked_reason = Some(rest.to_string());
                continue;
            }
            if line == "prunable" {
                current.prunable = true;
            }
        }
        if let Some(entry) = current.finish()? {
            worktrees.push(entry);
        }
        Ok(worktrees)
    }

    pub fn create_worktree(
        &self,
        worktree_path: impl AsRef<Path>,
        branch_name: &str,
        start_point: Option<&str>,
    ) -> Result<bool> {
        let worktree_path = worktree_path.as_ref();
        let start_point = start_point.unwrap_or("HEAD");
        git_status(
            &self.repository_root,
            &[
                "worktree",
                "add",
                "-b",
                branch_name,
                worktree_path.to_str().context("worktree path utf-8")?,
                start_point,
            ],
        )
    }

    pub fn prune_worktree(&self, worktree_path: impl AsRef<Path>) -> Result<bool> {
        git_status(
            &self.repository_root,
            &[
                "worktree",
                "remove",
                "--force",
                worktree_path
                    .as_ref()
                    .to_str()
                    .context("worktree path utf-8")?,
            ],
        )
    }

    pub fn delete_branch(&self, branch_name: &str) -> Result<bool> {
        if !branch_exists(&self.repository_root, branch_name)? {
            return Ok(false);
        }
        git_status(&self.repository_root, &["branch", "-D", branch_name])
    }
}

#[derive(Debug, Default)]
struct WorktreeSection {
    worktree_path: Option<PathBuf>,
    head_commit: Option<String>,
    branch: Option<String>,
    bare: bool,
    locked_reason: Option<String>,
    prunable: bool,
}

impl WorktreeSection {
    fn finish(self) -> Result<Option<GitWorktreeInspection>> {
        let Some(worktree_path) = self.worktree_path else {
            return Ok(None);
        };
        Ok(Some(GitWorktreeInspection {
            worktree_path,
            head_commit: self.head_commit.map(normalize_non_empty).flatten(),
            branch: self.branch.and_then(normalize_non_empty),
            bare: self.bare,
            locked_reason: self.locked_reason.map(normalize_non_empty).flatten(),
            prunable: self.prunable,
        }))
    }
}

fn compare_revision_distance(cwd: &Path, left: &str, right: &str) -> Result<Option<(u64, u64)>> {
    let range = format!("{left}...{right}");
    let Some(stdout) = git_stdout(cwd, &["rev-list", "--left-right", "--count", &range])? else {
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

fn git_stdout(cwd: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .with_context(|| format!("failed to invoke git in {}", cwd.display()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8(output.stdout).context("git output was not valid utf-8")?;
    let stdout = stdout.trim().to_string();
    Ok((!stdout.is_empty()).then_some(stdout))
}

fn git_status(cwd: &Path, args: &[&str]) -> Result<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .status()
        .with_context(|| format!("failed to invoke git in {}", cwd.display()))?;
    Ok(status.success())
}

fn branch_exists(cwd: &Path, branch_name: &str) -> Result<bool> {
    let Some(stdout) = git_stdout(cwd, &["branch", "--list", branch_name])? else {
        return Ok(false);
    };
    Ok(!stdout.trim().is_empty())
}

fn normalize_non_empty(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn setup_repo() -> (PathBuf, PathBuf, String) {
        let root = std::env::temp_dir().join(format!(
            "tt-git-v2-{}-{}",
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

    #[test]
    fn discovers_repo_from_worktree() {
        let (_, worktree, _) = setup_repo();
        let discovered = GitRepository::discover(&worktree)
            .expect("discover")
            .expect("repo");
        assert_eq!(discovered.repository_root, worktree);
    }

    #[test]
    fn inspects_clean_repo_and_worktrees() {
        let (_, worktree, head) = setup_repo();
        let inspection = GitRepository::discover(&worktree)
            .expect("discover")
            .expect("repo")
            .inspect_repository()
            .expect("inspect");

        assert_eq!(inspection.repository_root, worktree);
        assert_eq!(
            inspection
                .current_worktree
                .as_ref()
                .expect("current worktree"),
            &worktree
        );
        assert_eq!(inspection.current_branch.as_deref(), Some("tt/tt-1"));
        assert_eq!(
            inspection.current_head_commit.as_deref(),
            Some(head.as_str())
        );
        assert!(!inspection.dirty);
        assert_eq!(inspection.merge_readiness, MergeReadiness::Ready);
        assert_eq!(inspection.worktrees.len(), 2);
        assert!(
            inspection
                .worktrees
                .iter()
                .any(|entry| entry.worktree_path == worktree)
        );
    }

    #[test]
    fn reports_dirty_worktree_and_blocks_readiness() {
        let (_, worktree, _) = setup_repo();
        std::fs::write(worktree.join("README.md"), "dirty\n").expect("dirty file");

        let inspection = GitRepository::discover(&worktree)
            .expect("discover")
            .expect("repo")
            .inspect_repository()
            .expect("inspect");

        assert!(inspection.dirty);
        assert_eq!(inspection.merge_readiness, MergeReadiness::Blocked);
    }

    #[test]
    fn lists_registered_worktrees() {
        let (_, worktree, _) = setup_repo();
        let repository = GitRepository::discover(&worktree)
            .expect("discover")
            .expect("repo");
        let worktrees = repository.list_worktrees().expect("worktrees");

        assert_eq!(worktrees.len(), 2);
        assert!(
            worktrees
                .iter()
                .any(|entry| entry.worktree_path == worktree)
        );
    }

    #[test]
    fn delete_branch_is_idempotent_for_missing_branch() {
        let (_, worktree, _) = setup_repo();
        let repository = GitRepository::discover(&worktree)
            .expect("discover")
            .expect("repo");
        assert!(
            !repository
                .delete_branch("tt/missing")
                .expect("delete branch")
        );
    }
}
