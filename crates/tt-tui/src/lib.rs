#![allow(unused_crate_dependencies)]

//! TUI frontend for TT v2.
//!
//! This crate owns a minimal terminal dashboard that sits on top of the v2
//! daemon and shared view models.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tt_codex as _;
use tt_daemon as _;
use tt_store as _;
use tt_ui_core as _;
use tt_codex::CodexHome;
use tt_daemon::DaemonService;
use tt_store::OverlayStore;
use tt_ui_core::{DashboardSummary, GitRepositorySummary};

pub const TT_TUI_PRODUCT: &str = "tt-tui";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiSnapshot {
    pub dashboard: DashboardSummary,
    pub codex_home: Option<PathBuf>,
    pub repository: Option<GitRepositorySummary>,
}

pub fn load_snapshot(cwd: impl AsRef<Path>) -> Result<TuiSnapshot> {
    let cwd = cwd.as_ref();
    let store = OverlayStore::open_in_dir(cwd)?;
    let codex_home = CodexHome::discover().ok();
    let service = match codex_home.clone() {
        Some(home) => DaemonService::with_codex_home(store, home),
        None => DaemonService::new(store),
    };
    let dashboard = service.dashboard_summary()?;
    let repository = service.repository_summary(cwd)?;

    Ok(TuiSnapshot {
        dashboard,
        codex_home: codex_home.map(|home| home.root().to_path_buf()),
        repository,
    })
}

pub fn render_dashboard(snapshot: &TuiSnapshot) -> String {
    let mut output = String::new();
    output.push_str("TT v2 dashboard\n");
    output.push_str("================\n\n");

    if let Some(codex_home) = snapshot.codex_home.as_ref() {
        output.push_str(&format!("Codex home: {}\n", codex_home.display()));
    } else {
        output.push_str("Codex home: not configured\n");
    }

    if let Some(repo) = snapshot.repository.as_ref() {
        output.push_str(&format!("Repository: {}\n", repo.repository_root));
        output.push_str(&format!(
            "Branch: {}\n",
            repo.current_branch.as_deref().unwrap_or("<detached>")
        ));
        output.push_str(&format!(
            "Head: {}\n",
            repo.current_head_commit.as_deref().unwrap_or("<unknown>")
        ));
        output.push_str(&format!("Dirty: {}\n", if repo.dirty { "yes" } else { "no" }));
        output.push_str(&format!(
            "Merge ready: {}\n",
            if repo.merge_ready { "yes" } else { "no" }
        ));
        output.push_str(&format!(
            "Worktrees: {}\n",
            repo.worktree_count
        ));
        if let Some(upstream) = repo.upstream.as_ref() {
            output.push_str(&format!("Upstream: {}\n", upstream));
        }
        if let Some(ahead_by) = repo.ahead_by {
            output.push_str(&format!("Ahead by: {}\n", ahead_by));
        }
        if let Some(behind_by) = repo.behind_by {
            output.push_str(&format!("Behind by: {}\n", behind_by));
        }
    } else {
        output.push_str("Repository: not inside a git checkout\n");
    }

    output.push_str("\nOverlay counts\n");
    output.push_str("--------------\n");
    output.push_str(&format!(
        "Projects: {}\n",
        snapshot.dashboard.active_projects
    ));
    output.push_str(&format!(
        "Work units: {}\n",
        snapshot.dashboard.active_work_units
    ));
    output.push_str(&format!(
        "Bound threads: {}\n",
        snapshot.dashboard.bound_threads
    ));
    output.push_str(&format!(
        "Ready workspaces: {}\n",
        snapshot.dashboard.ready_workspaces
    ));
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_dashboard_without_repo_or_codex() {
        let snapshot = TuiSnapshot {
            dashboard: DashboardSummary {
                active_projects: 1,
                active_work_units: 2,
                bound_threads: 3,
                ready_workspaces: 4,
            },
            codex_home: None,
            repository: None,
        };

        let rendered = render_dashboard(&snapshot);
        assert!(rendered.contains("TT v2 dashboard"));
        assert!(rendered.contains("Codex home: not configured"));
        assert!(rendered.contains("Projects: 1"));
        assert!(rendered.contains("Ready workspaces: 4"));
    }
}
