use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::{TTError, TTResult};
use crate::paths::AppPaths;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LaneCleanupScope {
    #[default]
    Runtime,
    Worktree,
    Repo,
    Lane,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaneManifest {
    #[serde(default = "default_lane_schema_version")]
    pub schema_version: u32,
    pub label: String,
    pub slug: String,
    #[serde(default)]
    pub root_path: String,
    #[serde(default)]
    pub shared_home_path: String,
    #[serde(default)]
    pub repos_root_path: String,
    #[serde(default)]
    pub worktrees_root_path: String,
    #[serde(default)]
    pub runtime_root_path: String,
    pub created_at: String,
}

impl LaneManifest {
    pub fn new(
        label: impl Into<String>,
        slug: impl Into<String>,
        root_path: impl Into<String>,
        shared_home_path: impl Into<String>,
        repos_root_path: impl Into<String>,
        worktrees_root_path: impl Into<String>,
        runtime_root_path: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: default_lane_schema_version(),
            label: label.into(),
            slug: slug.into(),
            root_path: root_path.into(),
            shared_home_path: shared_home_path.into(),
            repos_root_path: repos_root_path.into(),
            worktrees_root_path: worktrees_root_path.into(),
            runtime_root_path: runtime_root_path.into(),
            created_at: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoManifest {
    #[serde(default = "default_lane_schema_version")]
    pub schema_version: u32,
    pub lane_slug: String,
    #[serde(default)]
    pub lane_root_path: String,
    #[serde(default)]
    pub repo_root_path: String,
    pub source_url: String,
    pub org: String,
    pub repo: String,
    pub cloned_at: String,
}

impl RepoManifest {
    pub fn new(
        lane_slug: impl Into<String>,
        lane_root_path: impl Into<String>,
        repo_root_path: impl Into<String>,
        source_url: impl Into<String>,
        org: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: default_lane_schema_version(),
            lane_slug: lane_slug.into(),
            lane_root_path: lane_root_path.into(),
            repo_root_path: repo_root_path.into(),
            source_url: source_url.into(),
            org: org.into(),
            repo: repo.into(),
            cloned_at: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceManifest {
    #[serde(default = "default_lane_schema_version")]
    pub schema_version: u32,
    pub lane_slug: String,
    #[serde(default)]
    pub lane_root_path: String,
    #[serde(default)]
    pub repo_root_path: String,
    pub label: String,
    pub slug: String,
    pub repo: String,
    #[serde(default)]
    pub workspace_root_path: String,
    pub worktree_path: String,
    #[serde(default)]
    pub worktree_root_path: String,
    pub runtime_path: String,
    pub home_path: String,
    pub branch_name: String,
    #[serde(default)]
    pub bound_snapshot_id: Option<String>,
    #[serde(default)]
    pub canonical_snapshot_id: Option<String>,
    #[serde(default)]
    pub bound_commit_sha: Option<String>,
    #[serde(default)]
    pub bound_worktree_path: Option<String>,
    #[serde(default)]
    pub bound_thread_id: Option<String>,
    #[serde(default)]
    pub bound_at: Option<String>,
    #[serde(default)]
    pub promoted_at: Option<String>,
    pub created_at: String,
    pub cleanup_scope: LaneCleanupScope,
    #[serde(default)]
    pub attached_tracked_thread_ids: Vec<String>,
}

impl WorkspaceManifest {
    pub fn new(
        lane_slug: impl Into<String>,
        lane_root_path: impl Into<String>,
        repo_root_path: impl Into<String>,
        label: impl Into<String>,
        slug: impl Into<String>,
        repo: impl Into<String>,
        workspace_root_path: impl Into<String>,
        worktree_path: impl Into<String>,
        worktree_root_path: impl Into<String>,
        runtime_path: impl Into<String>,
        home_path: impl Into<String>,
        branch_name: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: default_lane_schema_version(),
            lane_slug: lane_slug.into(),
            lane_root_path: lane_root_path.into(),
            repo_root_path: repo_root_path.into(),
            label: label.into(),
            slug: slug.into(),
            repo: repo.into(),
            workspace_root_path: workspace_root_path.into(),
            worktree_path: worktree_path.into(),
            worktree_root_path: worktree_root_path.into(),
            runtime_path: runtime_path.into(),
            home_path: home_path.into(),
            branch_name: branch_name.into(),
            bound_snapshot_id: None,
            canonical_snapshot_id: None,
            bound_commit_sha: None,
            bound_worktree_path: None,
            bound_thread_id: None,
            bound_at: None,
            promoted_at: None,
            created_at: Utc::now().to_rfc3339(),
            cleanup_scope: LaneCleanupScope::Runtime,
            attached_tracked_thread_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LanePaths {
    pub root: PathBuf,
    pub manifest_file: PathBuf,
    pub shared_dir: PathBuf,
    pub shared_home_dir: PathBuf,
    pub shared_tt_dir: PathBuf,
    pub shared_codex_dir: PathBuf,
    pub repos_dir: PathBuf,
    pub worktrees_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

impl LanePaths {
    pub fn slugify(label: &str) -> String {
        let mut slug = String::new();
        let mut last_was_dash = false;
        for ch in label.chars() {
            let lowered = ch.to_ascii_lowercase();
            if lowered.is_ascii_alphanumeric() {
                slug.push(lowered);
                last_was_dash = false;
            } else if !last_was_dash {
                slug.push('-');
                last_was_dash = true;
            }
        }
        slug.trim_matches('-').to_string()
    }

    pub fn from_base(base_root: impl AsRef<Path>, lane_slug: &str) -> Self {
        let root = base_root.as_ref().join("lanes").join(lane_slug);
        let shared_dir = root.join("shared");
        let shared_home_dir = shared_dir.join("home");
        let shared_tt_dir = shared_home_dir.join(".tt");
        let shared_codex_dir = shared_home_dir.join(".codex");
        let repos_dir = root.join("repos");
        let worktrees_dir = root.join("worktrees");
        let runtime_dir = root.join("runtime");
        Self {
            manifest_file: root.join("lane.toml"),
            root,
            shared_dir,
            shared_home_dir,
            shared_tt_dir,
            shared_codex_dir,
            repos_dir,
            worktrees_dir,
            runtime_dir,
        }
    }

    pub fn from_app_paths(paths: &AppPaths, lane_slug: &str) -> Self {
        Self::from_base(&paths.data_dir, lane_slug)
    }

    pub fn repo_root(&self, org: &str, repo: &str) -> PathBuf {
        self.repos_dir.join(org).join(repo)
    }

    pub fn repo_manifest_file(&self, org: &str, repo: &str) -> PathBuf {
        self.repo_root(org, repo).join("repo.toml")
    }

    pub fn workspace_root(&self, org: &str, repo: &str, workspace: &str) -> PathBuf {
        self.worktrees_dir.join(org).join(repo).join(workspace)
    }

    pub fn workspace_manifest_file(&self, org: &str, repo: &str, workspace: &str) -> PathBuf {
        self.workspace_root(org, repo, workspace).join("workspace.toml")
    }

    pub fn workspace_runtime_dir(&self, org: &str, repo: &str, workspace: &str) -> PathBuf {
        self.workspace_root(org, repo, workspace).join("runtime")
    }

    pub fn workspace_worktree_dir(&self, org: &str, repo: &str, workspace: &str) -> PathBuf {
        self.workspace_root(org, repo, workspace).join("worktree")
    }

    pub fn workspace_home_dir(&self, org: &str, repo: &str, workspace: &str) -> PathBuf {
        self.workspace_root(org, repo, workspace).join("home")
    }

    pub fn workspace_snapshot_log_file(&self, org: &str, repo: &str, workspace: &str) -> PathBuf {
        self.workspace_root(org, repo, workspace).join("snapshots.jsonl")
    }

    pub fn workspace_snapshot_db_file(&self, org: &str, repo: &str, workspace: &str) -> PathBuf {
        self.workspace_root(org, repo, workspace).join("snapshots.sqlite")
    }

    pub fn workspace_turn_log_file(&self, org: &str, repo: &str, workspace: &str) -> PathBuf {
        self.workspace_root(org, repo, workspace).join("turns.jsonl")
    }

    pub fn ensure(&self) -> TTResult<()> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(&self.shared_tt_dir)?;
        fs::create_dir_all(&self.shared_codex_dir)?;
        fs::create_dir_all(&self.repos_dir)?;
        fs::create_dir_all(&self.worktrees_dir)?;
        fs::create_dir_all(&self.runtime_dir)?;
        Ok(())
    }
}

pub fn render_toml<T: Serialize>(value: &T) -> TTResult<String> {
    Ok(toml::to_string_pretty(value).map_err(|error| TTError::Config(error.to_string()))?)
}

fn default_lane_schema_version() -> u32 {
    1
}

pub fn write_toml<T: Serialize>(path: &Path, value: &T) -> TTResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, render_toml(value)?)?;
    Ok(())
}

pub fn read_toml<T: for<'de> Deserialize<'de>>(path: &Path) -> TTResult<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    Ok(Some(toml::from_str(&raw).map_err(|error| TTError::Config(error.to_string()))?))
}

#[cfg(test)]
mod tests {
    use super::{LanePaths, LaneManifest, RepoManifest, WorkspaceManifest};

    #[test]
    fn slugify_normalizes_labels() {
        assert_eq!(LanePaths::slugify("Directory and worktree requirements"), "directory-and-worktree-requirements");
        assert_eq!(LanePaths::slugify("my random name"), "my-random-name");
    }

    #[test]
    fn manifests_round_trip_to_toml() {
        let lane = LaneManifest::new(
            "My Lane",
            "my-lane",
            "/tmp/lane",
            "/tmp/lane/shared/home",
            "/tmp/lane/repos",
            "/tmp/lane/worktrees",
            "/tmp/lane/runtime",
        );
        let encoded = toml::to_string(&lane).expect("encode lane");
        let decoded: LaneManifest = toml::from_str(&encoded).expect("decode lane");
        assert_eq!(decoded.slug, "my-lane");

        let repo = RepoManifest::new(
            "my-lane",
            "/tmp/lane",
            "/tmp/lane/repos/openai/codex",
            "https://github.com/openai/codex.git",
            "openai",
            "codex",
        );
        let encoded = toml::to_string(&repo).expect("encode repo");
        let decoded: RepoManifest = toml::from_str(&encoded).expect("decode repo");
        assert_eq!(decoded.repo, "codex");

        let workspace = WorkspaceManifest::new(
            "my-lane",
            "/tmp/lane",
            "/tmp/lane/repos/openai/codex",
            "Default",
            "default",
            "openai/codex",
            "/tmp/lane/worktrees/openai/codex/default",
            "/tmp/worktree",
            "/tmp/lane/worktrees/openai/codex/default",
            "/tmp/runtime",
            "/tmp/home",
            "worktree/default",
        );
        let encoded = toml::to_string(&workspace).expect("encode workspace");
        let decoded: WorkspaceManifest = toml::from_str(&encoded).expect("decode workspace");
        assert_eq!(decoded.label, "Default");
        assert!(decoded.attached_tracked_thread_ids.is_empty());
    }
}
