#![allow(unused_crate_dependencies)]

//! TUI frontend for TT v2.
//!
//! This crate owns a minimal interactive terminal dashboard that sits on top of
//! the v2 daemon request API and shared view models.

use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::{bail, Result};
use tt_daemon::{DaemonRequest, DaemonResponse, DaemonRuntime, DaemonStatus};
use tt_domain::{MergeRun, Project, ThreadBinding, WorkUnit, WorkspaceBinding};
use tt_ui_core::{DashboardSummary, GitRepositorySummary};

pub const TT_TUI_PRODUCT: &str = "tt-tui";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiSnapshot {
    pub status: DaemonStatus,
    pub dashboard: DashboardSummary,
    pub repository: Option<GitRepositorySummary>,
}

pub fn load_snapshot(cwd: impl AsRef<Path>) -> Result<TuiSnapshot> {
    let runtime = DaemonRuntime::open(cwd)?;
    load_snapshot_from_runtime(&runtime)
}

pub fn load_snapshot_from_runtime(runtime: &DaemonRuntime) -> Result<TuiSnapshot> {
    let status = match runtime.request(DaemonRequest::Status)? {
        DaemonResponse::Status(status) => status,
        other => bail!("unexpected daemon response for status: {other:?}"),
    };
    let dashboard = match runtime.request(DaemonRequest::DashboardSummary)? {
        DaemonResponse::DashboardSummary(summary) => summary,
        other => bail!("unexpected daemon response for dashboard summary: {other:?}"),
    };
    let repository = match runtime.request(DaemonRequest::RepositorySummary {
        cwd: runtime.cwd().to_path_buf(),
    })? {
        DaemonResponse::RepositorySummary(summary) => summary,
        other => bail!("unexpected daemon response for repository summary: {other:?}"),
    };

    Ok(TuiSnapshot {
        status,
        dashboard,
        repository,
    })
}

pub fn render_dashboard(snapshot: &TuiSnapshot) -> String {
    let mut output = String::new();
    output.push_str("TT v2 dashboard\n");
    output.push_str("================\n\n");

    if let Some(codex_home) = snapshot.status.codex_home.as_ref() {
        output.push_str(&format!("Codex home: {}\n", codex_home.display()));
    } else {
        output.push_str("Codex home: not configured\n");
    }
    if let Some(state_db) = snapshot.status.codex_state_db.as_ref() {
        output.push_str(&format!("Codex state db: {}\n", state_db.display()));
    }
    if let Some(session_index) = snapshot.status.codex_session_index.as_ref() {
        output.push_str(&format!("Codex session index: {}\n", session_index.display()));
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
        output.push_str(&format!("Worktrees: {}\n", repo.worktree_count));
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
    output.push_str(&format!("Projects: {}\n", snapshot.dashboard.active_projects));
    output.push_str(&format!(
        "Work units: {}\n",
        snapshot.dashboard.active_work_units
    ));
    output.push_str(&format!("Bound threads: {}\n", snapshot.dashboard.bound_threads));
    output.push_str(&format!(
        "Ready workspaces: {}\n",
        snapshot.dashboard.ready_workspaces
    ));
    output
}

pub fn run_interactive(cwd: impl AsRef<Path>) -> Result<()> {
    let runtime = DaemonRuntime::open(cwd)?;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    writeln!(stdout, "{}", render_dashboard(&load_snapshot_from_runtime(&runtime)?))?;
    writeln!(
        stdout,
        "\nCommands: help, refresh, projects, project <id>, work-units [project], work-unit <id>, thread-bindings [work-unit], workspace-bindings [thread], merge-runs, quit"
    )?;
    stdout.flush()?;

    for line in stdin.lock().lines() {
        let line = line?;
        match handle_command(&runtime, &line)? {
            Some(output) => {
                writeln!(stdout, "{output}")?;
            }
            None => break,
        }
        writeln!(stdout, "\n> ")?;
        stdout.flush()?;
    }

    Ok(())
}

fn handle_command(runtime: &DaemonRuntime, input: &str) -> Result<Option<String>> {
    let mut parts = input.split_whitespace();
    let Some(command) = parts.next() else {
        return Ok(Some(String::new()));
    };

    match command {
        "help" => Ok(Some(
            "Commands: help, refresh, projects, project <id>, work-units [project], work-unit <id>, thread-bindings [work-unit], workspace-bindings [thread], merge-runs, quit".to_string(),
        )),
        "quit" | "exit" => Ok(None),
        "refresh" => Ok(Some(render_dashboard(&load_snapshot_from_runtime(runtime)?))),
        "projects" => {
            let projects = match runtime.request(DaemonRequest::ListProjects)? {
                DaemonResponse::Projects(projects) => projects,
                other => bail!("unexpected daemon response for projects: {other:?}"),
            };
            Ok(Some(render_projects(&projects)))
        }
        "project" => {
            let Some(id_or_slug) = parts.next() else {
                bail!("project requires an id or slug");
            };
            let response = runtime.request(DaemonRequest::GetProject {
                id_or_slug: id_or_slug.to_string(),
            })?;
            match response {
                DaemonResponse::Project(Some(project)) => Ok(Some(format!(
                    "{}\n{}\n{}\n{}\n",
                    project.id, project.slug, project.title, project.objective
                ))),
                DaemonResponse::Project(None) => Ok(Some(format!("project not found: {id_or_slug}"))),
                other => bail!("unexpected daemon response for project: {other:?}"),
            }
        }
        "work-units" => {
            let project_id = parts.next().map(ToString::to_string);
            let response = runtime.request(DaemonRequest::ListWorkUnits { project_id })?;
            match response {
                DaemonResponse::WorkUnits(work_units) => Ok(Some(render_work_units(&work_units))),
                other => bail!("unexpected daemon response for work units: {other:?}"),
            }
        }
        "work-unit" => {
            let Some(id_or_slug) = parts.next() else {
                bail!("work-unit requires an id or slug");
            };
            let response = runtime.request(DaemonRequest::GetWorkUnit {
                id_or_slug: id_or_slug.to_string(),
            })?;
            match response {
                DaemonResponse::WorkUnit(Some(work_unit)) => Ok(Some(format!(
                    "{}\n{}\n{}\n{}\n",
                    work_unit.id, work_unit.title, work_unit.task, work_unit.project_id
                ))),
                DaemonResponse::WorkUnit(None) => {
                    Ok(Some(format!("work unit not found: {id_or_slug}")))
                }
                other => bail!("unexpected daemon response for work unit: {other:?}"),
            }
        }
        "thread-bindings" => {
            let Some(work_unit_id) = parts.next() else {
                let response = runtime.request(DaemonRequest::ListThreadBindings)?;
                return match response {
                    DaemonResponse::ThreadBindings(bindings) => {
                        Ok(Some(render_thread_bindings(&bindings)))
                    }
                    other => bail!("unexpected daemon response for thread bindings: {other:?}"),
                };
            };
            let response = runtime.request(DaemonRequest::ListThreadBindingsForWorkUnit {
                work_unit_id: work_unit_id.to_string(),
            })?;
            match response {
                DaemonResponse::ThreadBindings(bindings) => {
                    Ok(Some(render_thread_bindings(&bindings)))
                }
                other => bail!("unexpected daemon response for thread bindings: {other:?}"),
            }
        }
        "workspace-bindings" => {
            let Some(thread_id) = parts.next() else {
                let response = runtime.request(DaemonRequest::ListWorkspaceBindings)?;
                return match response {
                    DaemonResponse::WorkspaceBindings(bindings) => {
                        Ok(Some(render_workspace_bindings(&bindings)))
                    }
                    other => bail!("unexpected daemon response for workspace bindings: {other:?}"),
                };
            };
            let response = runtime.request(DaemonRequest::ListWorkspaceBindingsForThread {
                codex_thread_id: thread_id.to_string(),
            })?;
            match response {
                DaemonResponse::WorkspaceBindings(bindings) => {
                    Ok(Some(render_workspace_bindings(&bindings)))
                }
                other => bail!("unexpected daemon response for workspace bindings: {other:?}"),
            }
        }
        "merge-runs" => {
            let response = runtime.request(DaemonRequest::ListMergeRuns)?;
            match response {
                DaemonResponse::MergeRuns(runs) => Ok(Some(render_merge_runs(&runs))),
                other => bail!("unexpected daemon response for merge runs: {other:?}"),
            }
        }
        other => Ok(Some(format!("unknown command: {other}"))),
    }
}

fn render_projects(projects: &[Project]) -> String {
    if projects.is_empty() {
        return "no projects".to_string();
    }
    let mut output = String::new();
    for project in projects {
        output.push_str(&format!(
            "{} | {} | {}\n",
            project.slug, project.title, format!("{:?}", project.status)
        ));
    }
    output
}

fn render_work_units(work_units: &[WorkUnit]) -> String {
    if work_units.is_empty() {
        return "no work units".to_string();
    }
    let mut output = String::new();
    for work_unit in work_units {
        output.push_str(&format!(
            "{} | {} | {}\n",
            work_unit.id, work_unit.title, format!("{:?}", work_unit.status)
        ));
    }
    output
}

fn render_thread_bindings(bindings: &[ThreadBinding]) -> String {
    if bindings.is_empty() {
        return "no thread bindings".to_string();
    }
    let mut output = String::new();
    for binding in bindings {
        output.push_str(&format!(
            "{} | {:?} | {:?}\n",
            binding.codex_thread_id, binding.role, binding.status
        ));
    }
    output
}

fn render_workspace_bindings(bindings: &[WorkspaceBinding]) -> String {
    if bindings.is_empty() {
        return "no workspace bindings".to_string();
    }
    let mut output = String::new();
    for binding in bindings {
        output.push_str(&format!(
            "{} | {} | {:?}\n",
            binding.id, binding.repo_root, binding.status
        ));
    }
    output
}

fn render_merge_runs(runs: &[MergeRun]) -> String {
    if runs.is_empty() {
        return "no merge runs".to_string();
    }
    let mut output = String::new();
    for run in runs {
        output.push_str(&format!(
            "{} | {} | {:?} | {:?}\n",
            run.id, run.workspace_binding_id, run.readiness, run.execution
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::process::Command;
    use tempfile::tempdir;
    use tt_domain::{Project, ProjectStatus};

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 8, 12, 0, 0).unwrap()
    }

    #[test]
    fn renders_dashboard_without_repo_or_codex() {
        let snapshot = TuiSnapshot {
            status: DaemonStatus {
                codex_home: None,
                codex_state_db: None,
                codex_session_index: None,
                project_count: 0,
                work_unit_count: 0,
                bound_thread_count: 0,
                ready_workspace_count: 0,
            },
            dashboard: DashboardSummary {
                active_projects: 1,
                active_work_units: 2,
                bound_threads: 3,
                ready_workspaces: 4,
            },
            repository: None,
        };

        let rendered = render_dashboard(&snapshot);
        assert!(rendered.contains("TT v2 dashboard"));
        assert!(rendered.contains("Codex home: not configured"));
        assert!(rendered.contains("Projects: 1"));
        assert!(rendered.contains("Ready workspaces: 4"));
    }

    #[test]
    fn handle_command_lists_projects() {
        let dir = tempdir().expect("tempdir");
        let runtime = DaemonRuntime::open(dir.path()).expect("open runtime");
        runtime
            .request(DaemonRequest::UpsertProject {
                project: Project {
                    id: "p1".into(),
                    slug: "alpha".into(),
                    title: "Alpha".into(),
                    objective: "Ship".into(),
                    status: ProjectStatus::Active,
                    created_at: ts(),
                    updated_at: ts(),
                },
            })
            .expect("upsert project");

        let output = handle_command(&runtime, "projects")
            .expect("command")
            .expect("output");
        assert!(output.contains("alpha"));
    }

    #[test]
    fn interactive_loop_can_render_once_with_repo_summary() {
        let root = std::env::temp_dir().join(format!(
            "tt-tui-v2-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("create repo");
        let status = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["init", "-b", "main"])
            .status()
            .expect("git init");
        assert!(status.success());
    }
}
