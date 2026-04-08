#![allow(unused_crate_dependencies)]

//! TUI frontend for TT v2.
//!
//! This crate owns a minimal interactive terminal dashboard that sits on top of
//! the v2 daemon request API and shared view models.

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::str::FromStr;

use anyhow::{Result, bail};
use tt_daemon::{DaemonRequest, DaemonResponse, DaemonStatus, request_for_cwd};
use tt_domain::{
    MergeAuthorizationStatus, MergeExecutionStatus, MergeReadiness, MergeRun, Project,
    ProjectStatus, ThreadBinding, ThreadBindingStatus, ThreadRole, WorkUnit, WorkUnitStatus,
    WorkspaceBinding, WorkspaceStatus,
};
use tt_ui_core::{CodexThreadDetail, CodexThreadSummary, DashboardSummary, GitRepositorySummary};

pub const TT_TUI_PRODUCT: &str = "tt-tui";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiSnapshot {
    pub status: DaemonStatus,
    pub dashboard: DashboardSummary,
    pub repository: Option<GitRepositorySummary>,
}

pub fn load_snapshot(cwd: impl AsRef<Path>) -> Result<TuiSnapshot> {
    load_snapshot_from_cwd(cwd)
}

pub fn load_snapshot_from_cwd(cwd: impl AsRef<Path>) -> Result<TuiSnapshot> {
    let cwd = cwd.as_ref();
    let status = match request_for_cwd(cwd, DaemonRequest::Status)? {
        DaemonResponse::Status(status) => status,
        other => bail!("unexpected daemon response for status: {other:?}"),
    };
    let dashboard = match request_for_cwd(cwd, DaemonRequest::DashboardSummary)? {
        DaemonResponse::DashboardSummary(summary) => summary,
        other => bail!("unexpected daemon response for dashboard summary: {other:?}"),
    };
    let repository = match request_for_cwd(
        cwd,
        DaemonRequest::RepositorySummary {
            cwd: cwd.to_path_buf(),
        },
    )? {
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
        output.push_str(&format!(
            "Codex session index: {}\n",
            session_index.display()
        ));
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
        output.push_str(&format!(
            "Dirty: {}\n",
            if repo.dirty { "yes" } else { "no" }
        ));
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

fn render_status(status: &DaemonStatus) -> String {
    let mut output = String::new();
    output.push_str("TT status\n");
    output.push_str("=========\n");
    if let Some(codex_home) = status.codex_home.as_ref() {
        output.push_str(&format!("Codex home: {}\n", codex_home.display()));
    }
    if let Some(codex_state_db) = status.codex_state_db.as_ref() {
        output.push_str(&format!("Codex state db: {}\n", codex_state_db.display()));
    }
    if let Some(codex_session_index) = status.codex_session_index.as_ref() {
        output.push_str(&format!(
            "Codex session index: {}\n",
            codex_session_index.display()
        ));
    }
    output.push_str(&format!("Projects: {}\n", status.project_count));
    output.push_str(&format!("Work units: {}\n", status.work_unit_count));
    output.push_str(&format!("Bound threads: {}\n", status.bound_thread_count));
    output.push_str(&format!(
        "Ready workspaces: {}\n",
        status.ready_workspace_count
    ));
    output
}

fn render_repository_summary(summary: &GitRepositorySummary) -> String {
    let mut output = String::new();
    output.push_str("Repository\n");
    output.push_str("==========\n");
    output.push_str(&format!("Root: {}\n", summary.repository_root));
    output.push_str(&format!(
        "Worktree: {}\n",
        summary.current_worktree.as_deref().unwrap_or("<unset>")
    ));
    output.push_str(&format!(
        "Branch: {}\n",
        summary.current_branch.as_deref().unwrap_or("<detached>")
    ));
    output.push_str(&format!(
        "Head: {}\n",
        summary
            .current_head_commit
            .as_deref()
            .unwrap_or("<unknown>")
    ));
    output.push_str(&format!("Dirty: {}\n", summary.dirty));
    output.push_str(&format!("Merge ready: {}\n", summary.merge_ready));
    output.push_str(&format!("Worktrees: {}\n", summary.worktree_count));
    if let Some(upstream) = summary.upstream.as_ref() {
        output.push_str(&format!("Upstream: {}\n", upstream));
    }
    if let Some(ahead_by) = summary.ahead_by {
        output.push_str(&format!("Ahead by: {}\n", ahead_by));
    }
    if let Some(behind_by) = summary.behind_by {
        output.push_str(&format!("Behind by: {}\n", behind_by));
    }
    output
}

fn render_project(project: &Project) -> String {
    format!(
        "Project\n=======\nID: {}\nSlug: {}\nTitle: {}\nObjective: {}\nStatus: {:?}\nCreated: {}\nUpdated: {}\n",
        project.id,
        project.slug,
        project.title,
        project.objective,
        project.status,
        project.created_at,
        project.updated_at
    )
}

fn render_work_unit(work_unit: &WorkUnit) -> String {
    format!(
        "Work unit\n=========\nID: {}\nProject: {}\nSlug: {}\nTitle: {}\nTask: {}\nStatus: {:?}\nCreated: {}\nUpdated: {}\n",
        work_unit.id,
        work_unit.project_id,
        work_unit.slug.as_deref().unwrap_or("<unset>"),
        work_unit.title,
        work_unit.task,
        work_unit.status,
        work_unit.created_at,
        work_unit.updated_at
    )
}

fn render_thread_binding(binding: &ThreadBinding) -> String {
    format!(
        "Thread binding\n==============\nThread: {}\nWork unit: {}\nRole: {:?}\nStatus: {:?}\nNotes: {}\nCreated: {}\nUpdated: {}\n",
        binding.codex_thread_id,
        binding.work_unit_id.as_deref().unwrap_or("<unbound>"),
        binding.role,
        binding.status,
        binding.notes.as_deref().unwrap_or("<unset>"),
        binding.created_at,
        binding.updated_at
    )
}

pub fn run_interactive(cwd: impl AsRef<Path>) -> Result<()> {
    let cwd = cwd.as_ref().to_path_buf();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    writeln!(
        stdout,
        "{}",
        render_dashboard(&load_snapshot_from_cwd(&cwd)?)
    )?;
    writeln!(stdout, "\n{}", command_help())?;
    stdout.flush()?;

    for line in stdin.lock().lines() {
        let line = line?;
        match handle_command(&cwd, &line)? {
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

fn command_help() -> String {
    [
        "Commands:",
        "  Core: help, refresh, status, repo, quit",
        "  Codex: codex-threads [limit], codex-thread <selector>, codex-thread-read <selector> [include_turns], codex-thread-start [model] [ephemeral], codex-thread-resume <selector> [model]",
        "  Workspace actions: workspace-prepare <id>, workspace-refresh <id>, workspace-merge-prep <id>, workspace-authorize-merge <id>, workspace-execute-landing <id>, workspace-prune <id> [force]",
        "  Workspace lifecycle: workspace-close [selector] [force], workspace-park [selector] [note...], workspace-split <role> [model] [ephemeral]",
        "  Records: projects, project <id>, project-status <id> <status>, work-units [project], work-unit <id>, work-unit-status <id> <status>, thread-bindings [work-unit], thread-binding <thread>, thread-binding-status <thread> <status>, workspace-bindings [thread], workspace-binding <id>, workspace-binding-status <id> <status>, workspace-binding-refresh <id>, merge-runs, merge-run-status <id> <readiness> <authorization> <execution> [head_commit], merge-run-refresh <workspace-binding-id>",
    ]
    .join("\n")
}

fn handle_command(cwd: &Path, input: &str) -> Result<Option<String>> {
    let mut parts = input.split_whitespace();
    let Some(command) = parts.next() else {
        return Ok(Some(String::new()));
    };

    match command {
        "help" => Ok(Some(command_help())),
        "quit" | "exit" => Ok(None),
        "refresh" => Ok(Some(render_dashboard(&load_snapshot_from_cwd(cwd)?))),
        "status" => {
            let response = request_for_cwd(cwd, DaemonRequest::Status)?;
            match response {
                DaemonResponse::Status(status) => Ok(Some(render_status(&status))),
                other => bail!("unexpected daemon response for status: {other:?}"),
            }
        }
        "repo" => {
            let response = request_for_cwd(
                cwd,
                DaemonRequest::RepositorySummary {
                    cwd: cwd.to_path_buf(),
                },
            )?;
            match response {
                DaemonResponse::RepositorySummary(Some(summary)) => {
                    Ok(Some(render_repository_summary(&summary)))
                }
                DaemonResponse::RepositorySummary(None) => {
                    Ok(Some("not inside a git checkout".to_string()))
                }
                other => bail!("unexpected daemon response for repo: {other:?}"),
            }
        }
        "projects" => {
            let projects = match request_for_cwd(cwd, DaemonRequest::ListProjects)? {
                DaemonResponse::Projects(projects) => projects,
                other => bail!("unexpected daemon response for projects: {other:?}"),
            };
            Ok(Some(render_projects(&projects)))
        }
        "project" => {
            let Some(id_or_slug) = parts.next() else {
                bail!("project requires an id or slug");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::GetProject {
                    id_or_slug: id_or_slug.to_string(),
                },
            )?;
            match response {
                DaemonResponse::Project(Some(project)) => Ok(Some(render_project(&project))),
                DaemonResponse::Project(None) => {
                    Ok(Some(format!("project not found: {id_or_slug}")))
                }
                other => bail!("unexpected daemon response for project: {other:?}"),
            }
        }
        "project-status" => {
            let Some(id_or_slug) = parts.next() else {
                bail!("project-status requires an id or slug");
            };
            let Some(raw_status) = parts.next() else {
                bail!("project-status requires a status");
            };
            let status = parse_status::<ProjectStatus>(raw_status)?;
            let response = request_for_cwd(
                cwd,
                DaemonRequest::SetProjectStatus {
                    id_or_slug: id_or_slug.to_string(),
                    status,
                },
            )?;
            match response {
                DaemonResponse::Count(count) => Ok(Some(format!("updated {} project(s)", count))),
                other => bail!("unexpected daemon response for project-status: {other:?}"),
            }
        }
        "work-units" => {
            let project_id = parts.next().map(ToString::to_string);
            let response = request_for_cwd(cwd, DaemonRequest::ListWorkUnits { project_id })?;
            match response {
                DaemonResponse::WorkUnits(work_units) => Ok(Some(render_work_units(&work_units))),
                other => bail!("unexpected daemon response for work units: {other:?}"),
            }
        }
        "work-unit" => {
            let Some(id_or_slug) = parts.next() else {
                bail!("work-unit requires an id or slug");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::GetWorkUnit {
                    id_or_slug: id_or_slug.to_string(),
                },
            )?;
            match response {
                DaemonResponse::WorkUnit(Some(work_unit)) => Ok(Some(render_work_unit(&work_unit))),
                DaemonResponse::WorkUnit(None) => {
                    Ok(Some(format!("work unit not found: {id_or_slug}")))
                }
                other => bail!("unexpected daemon response for work unit: {other:?}"),
            }
        }
        "work-unit-status" => {
            let Some(id_or_slug) = parts.next() else {
                bail!("work-unit-status requires an id or slug");
            };
            let Some(raw_status) = parts.next() else {
                bail!("work-unit-status requires a status");
            };
            let status = parse_status::<WorkUnitStatus>(raw_status)?;
            let response = request_for_cwd(
                cwd,
                DaemonRequest::SetWorkUnitStatus {
                    id_or_slug: id_or_slug.to_string(),
                    status,
                },
            )?;
            match response {
                DaemonResponse::Count(count) => Ok(Some(format!("updated {} work unit(s)", count))),
                other => bail!("unexpected daemon response for work-unit-status: {other:?}"),
            }
        }
        "thread-bindings" => {
            let Some(work_unit_id) = parts.next() else {
                let response = request_for_cwd(cwd, DaemonRequest::ListThreadBindings)?;
                return match response {
                    DaemonResponse::ThreadBindings(bindings) => {
                        Ok(Some(render_thread_bindings(&bindings)))
                    }
                    other => bail!("unexpected daemon response for thread bindings: {other:?}"),
                };
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::ListThreadBindingsForWorkUnit {
                    work_unit_id: work_unit_id.to_string(),
                },
            )?;
            match response {
                DaemonResponse::ThreadBindings(bindings) => {
                    Ok(Some(render_thread_bindings(&bindings)))
                }
                other => bail!("unexpected daemon response for thread bindings: {other:?}"),
            }
        }
        "thread-binding" => {
            let Some(thread_id) = parts.next() else {
                bail!("thread-binding requires a thread id");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::GetThreadBinding {
                    codex_thread_id: thread_id.to_string(),
                },
            )?;
            match response {
                DaemonResponse::ThreadBinding(Some(binding)) => {
                    Ok(Some(render_thread_binding(&binding)))
                }
                DaemonResponse::ThreadBinding(None) => {
                    Ok(Some(format!("thread binding not found: {thread_id}")))
                }
                other => bail!("unexpected daemon response for thread-binding: {other:?}"),
            }
        }
        "thread-binding-status" => {
            let Some(thread_id) = parts.next() else {
                bail!("thread-binding-status requires a thread id");
            };
            let Some(raw_status) = parts.next() else {
                bail!("thread-binding-status requires a status");
            };
            let status = parse_status::<ThreadBindingStatus>(raw_status)?;
            let response = request_for_cwd(
                cwd,
                DaemonRequest::SetThreadBindingStatus {
                    codex_thread_id: thread_id.to_string(),
                    status,
                },
            )?;
            match response {
                DaemonResponse::Count(count) => {
                    Ok(Some(format!("updated {} thread binding(s)", count)))
                }
                other => bail!("unexpected daemon response for thread-binding-status: {other:?}"),
            }
        }
        "workspace-bindings" => {
            let Some(thread_id) = parts.next() else {
                let response = request_for_cwd(cwd, DaemonRequest::ListWorkspaceBindings)?;
                return match response {
                    DaemonResponse::WorkspaceBindings(bindings) => {
                        Ok(Some(render_workspace_bindings(&bindings)))
                    }
                    other => bail!("unexpected daemon response for workspace bindings: {other:?}"),
                };
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::ListWorkspaceBindingsForThread {
                    codex_thread_id: thread_id.to_string(),
                },
            )?;
            match response {
                DaemonResponse::WorkspaceBindings(bindings) => {
                    Ok(Some(render_workspace_bindings(&bindings)))
                }
                other => bail!("unexpected daemon response for workspace bindings: {other:?}"),
            }
        }
        "workspace-binding" => {
            let Some(id) = parts.next() else {
                bail!("workspace-binding requires an id");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::GetWorkspaceBinding { id: id.to_string() },
            )?;
            match response {
                DaemonResponse::WorkspaceBinding(Some(binding)) => {
                    Ok(Some(render_workspace_binding(&binding)))
                }
                DaemonResponse::WorkspaceBinding(None) => {
                    Ok(Some(format!("workspace binding not found: {id}")))
                }
                other => bail!("unexpected daemon response for workspace-binding: {other:?}"),
            }
        }
        "workspace-binding-status" => {
            let Some(id) = parts.next() else {
                bail!("workspace-binding-status requires an id");
            };
            let Some(raw_status) = parts.next() else {
                bail!("workspace-binding-status requires a status");
            };
            let status = parse_status::<WorkspaceStatus>(raw_status)?;
            let response = request_for_cwd(
                cwd,
                DaemonRequest::SetWorkspaceBindingStatus {
                    id: id.to_string(),
                    status,
                },
            )?;
            match response {
                DaemonResponse::Count(count) => {
                    Ok(Some(format!("updated {} workspace binding(s)", count)))
                }
                other => {
                    bail!("unexpected daemon response for workspace-binding-status: {other:?}")
                }
            }
        }
        "workspace-binding-refresh" => {
            let Some(id) = parts.next() else {
                bail!("workspace-binding-refresh requires an id");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::RefreshWorkspaceBinding { id: id.to_string() },
            )?;
            match response {
                DaemonResponse::WorkspaceBinding(Some(binding)) => {
                    Ok(Some(render_workspace_bindings(&[binding])))
                }
                DaemonResponse::WorkspaceBinding(None) => {
                    Ok(Some(format!("workspace binding not found: {id}")))
                }
                other => {
                    bail!("unexpected daemon response for workspace-binding-refresh: {other:?}")
                }
            }
        }
        "workspace-prepare" => {
            let Some(id) = parts.next() else {
                bail!("workspace-prepare requires an id");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::PrepareWorkspaceBinding { id: id.to_string() },
            )?;
            match response {
                DaemonResponse::WorkspaceBinding(Some(binding)) => {
                    Ok(Some(render_workspace_binding(&binding)))
                }
                DaemonResponse::WorkspaceBinding(None) => {
                    Ok(Some(format!("workspace binding not found: {id}")))
                }
                other => bail!("unexpected daemon response for workspace-prepare: {other:?}"),
            }
        }
        "workspace-refresh" => {
            let Some(id) = parts.next() else {
                bail!("workspace-refresh requires an id");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::RefreshWorkspaceBinding { id: id.to_string() },
            )?;
            match response {
                DaemonResponse::WorkspaceBinding(Some(binding)) => {
                    Ok(Some(render_workspace_binding(&binding)))
                }
                DaemonResponse::WorkspaceBinding(None) => {
                    Ok(Some(format!("workspace binding not found: {id}")))
                }
                other => bail!("unexpected daemon response for workspace-refresh: {other:?}"),
            }
        }
        "workspace-merge-prep" => {
            let Some(id) = parts.next() else {
                bail!("workspace-merge-prep requires an id");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::MergePrepWorkspaceBinding { id: id.to_string() },
            )?;
            match response {
                DaemonResponse::MergeRun(Some(run)) => Ok(Some(render_merge_run_detail(&run))),
                DaemonResponse::MergeRun(None) => Ok(Some(format!(
                    "merge run not found for workspace binding: {id}"
                ))),
                other => bail!("unexpected daemon response for workspace-merge-prep: {other:?}"),
            }
        }
        "workspace-authorize-merge" => {
            let Some(id) = parts.next() else {
                bail!("workspace-authorize-merge requires an id");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::AuthorizeMergeWorkspaceBinding { id: id.to_string() },
            )?;
            match response {
                DaemonResponse::MergeRun(Some(run)) => Ok(Some(render_merge_run_detail(&run))),
                DaemonResponse::MergeRun(None) => Ok(Some(format!(
                    "merge run not found for workspace binding: {id}"
                ))),
                other => {
                    bail!("unexpected daemon response for workspace-authorize-merge: {other:?}")
                }
            }
        }
        "workspace-execute-landing" => {
            let Some(id) = parts.next() else {
                bail!("workspace-execute-landing requires an id");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::ExecuteLandingWorkspaceBinding { id: id.to_string() },
            )?;
            match response {
                DaemonResponse::MergeRun(Some(run)) => Ok(Some(render_merge_run_detail(&run))),
                DaemonResponse::MergeRun(None) => Ok(Some(format!(
                    "merge run not found for workspace binding: {id}"
                ))),
                other => {
                    bail!("unexpected daemon response for workspace-execute-landing: {other:?}")
                }
            }
        }
        "workspace-prune" => {
            let Some(id) = parts.next() else {
                bail!("workspace-prune requires an id");
            };
            let force = parts
                .next()
                .map(|value| value == "force" || value == "--force" || value == "true")
                .unwrap_or(false);
            let response = request_for_cwd(
                cwd,
                DaemonRequest::PruneWorkspaceBinding {
                    id: id.to_string(),
                    force,
                },
            )?;
            match response {
                DaemonResponse::WorkspaceBinding(Some(binding)) => Ok(Some(
                    render_workspace_lifecycle_result("workspace-prune", &binding, force, Some(id)),
                )),
                DaemonResponse::WorkspaceBinding(None) => {
                    Ok(Some(format!("workspace binding not found: {id}")))
                }
                other => bail!("unexpected daemon response for workspace-prune: {other:?}"),
            }
        }
        "workspace-close" => {
            let selector = parts.next().map(ToString::to_string);
            let force = parts
                .next()
                .map(|value| value == "force" || value == "--force" || value == "true")
                .unwrap_or(false);
            let response = request_for_cwd(
                cwd,
                DaemonRequest::CloseWorkspace {
                    cwd: cwd.to_path_buf(),
                    selector: selector.clone(),
                    force,
                },
            )?;
            match response {
                DaemonResponse::WorkspaceBinding(Some(binding)) => {
                    Ok(Some(render_workspace_lifecycle_result(
                        "workspace-close",
                        &binding,
                        force,
                        selector.as_deref(),
                    )))
                }
                DaemonResponse::WorkspaceBinding(None) => {
                    Ok(Some("workspace binding not found".to_string()))
                }
                other => bail!("unexpected daemon response for workspace-close: {other:?}"),
            }
        }
        "workspace-park" => {
            let selector = parts.next().map(ToString::to_string);
            let note = parts.next().map(|first| {
                std::iter::once(first)
                    .chain(parts)
                    .collect::<Vec<_>>()
                    .join(" ")
            });
            let response = request_for_cwd(
                cwd,
                DaemonRequest::ParkWorkspace {
                    cwd: cwd.to_path_buf(),
                    selector: selector.clone(),
                    note,
                },
            )?;
            match response {
                DaemonResponse::WorkspaceBinding(Some(binding)) => {
                    Ok(Some(render_workspace_lifecycle_result(
                        "workspace-park",
                        &binding,
                        false,
                        selector.as_deref(),
                    )))
                }
                DaemonResponse::WorkspaceBinding(None) => {
                    Ok(Some("workspace binding not found".to_string()))
                }
                other => bail!("unexpected daemon response for workspace-park: {other:?}"),
            }
        }
        "workspace-split" => {
            let role_raw = parts.next().unwrap_or("develop");
            let model = parts.next().map(ToString::to_string);
            let ephemeral = parts
                .next()
                .and_then(|value| value.parse::<bool>().ok())
                .unwrap_or(false);
            let response = request_for_cwd(
                cwd,
                DaemonRequest::SplitWorkspace {
                    cwd: cwd.to_path_buf(),
                    role: parse_status::<ThreadRole>(role_raw)?,
                    model,
                    ephemeral,
                },
            )?;
            match response {
                DaemonResponse::WorkspaceBinding(Some(binding)) => {
                    Ok(Some(render_workspace_lifecycle_result(
                        "workspace-split",
                        &binding,
                        ephemeral,
                        Some(role_raw),
                    )))
                }
                DaemonResponse::WorkspaceBinding(None) => Ok(Some(
                    "could not split workspace from current cwd".to_string(),
                )),
                other => bail!("unexpected daemon response for workspace-split: {other:?}"),
            }
        }
        "merge-runs" => {
            let response = request_for_cwd(cwd, DaemonRequest::ListMergeRuns)?;
            match response {
                DaemonResponse::MergeRuns(runs) => Ok(Some(render_merge_runs(&runs))),
                other => bail!("unexpected daemon response for merge runs: {other:?}"),
            }
        }
        "merge-run-refresh" => {
            let Some(workspace_binding_id) = parts.next() else {
                bail!("merge-run-refresh requires a workspace binding id");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::RefreshMergeRun {
                    workspace_binding_id: workspace_binding_id.to_string(),
                },
            )?;
            match response {
                DaemonResponse::MergeRun(Some(run)) => Ok(Some(render_merge_run_detail(&run))),
                DaemonResponse::MergeRun(None) => Ok(Some(format!(
                    "merge run not found for workspace binding: {workspace_binding_id}"
                ))),
                other => bail!("unexpected daemon response for merge-run-refresh: {other:?}"),
            }
        }
        "codex-threads" => {
            let limit = parts.next().and_then(|value| value.parse::<usize>().ok());
            let response = request_for_cwd(
                cwd,
                DaemonRequest::ListCodexThreads {
                    cwd: cwd.to_path_buf(),
                    limit,
                },
            )?;
            match response {
                DaemonResponse::CodexThreads(threads) => Ok(Some(render_codex_threads(&threads))),
                other => bail!("unexpected daemon response for codex threads: {other:?}"),
            }
        }
        "codex-thread" => {
            let Some(selector) = parts.next() else {
                bail!("codex-thread requires a selector");
            };
            let response = request_for_cwd(
                cwd,
                DaemonRequest::GetCodexThread {
                    cwd: cwd.to_path_buf(),
                    selector: selector.to_string(),
                },
            )?;
            match response {
                DaemonResponse::CodexThread(Some(thread)) => Ok(Some(render_codex_thread(&thread))),
                DaemonResponse::CodexThread(None) => {
                    Ok(Some(format!("codex thread not found: {selector}")))
                }
                other => bail!("unexpected daemon response for codex thread: {other:?}"),
            }
        }
        "codex-thread-read" => {
            let Some(selector) = parts.next() else {
                bail!("codex-thread-read requires a selector");
            };
            let include_turns = parts
                .next()
                .and_then(|value| value.parse::<bool>().ok())
                .unwrap_or(true);
            let response = request_for_cwd(
                cwd,
                DaemonRequest::ReadCodexThread {
                    cwd: cwd.to_path_buf(),
                    selector: selector.to_string(),
                    include_turns,
                },
            )?;
            match response {
                DaemonResponse::CodexThreadDetail(Some(thread)) => {
                    Ok(Some(render_codex_thread_detail(&thread)))
                }
                DaemonResponse::CodexThreadDetail(None) => {
                    Ok(Some(format!("codex thread not found: {selector}")))
                }
                other => bail!("unexpected daemon response for codex-thread-read: {other:?}"),
            }
        }
        "codex-thread-start" => {
            let model = parts.next().map(ToString::to_string);
            let ephemeral = parts
                .next()
                .and_then(|value| value.parse::<bool>().ok())
                .unwrap_or(false);
            let response = request_for_cwd(
                cwd,
                DaemonRequest::StartCodexThread {
                    cwd: cwd.to_path_buf(),
                    model,
                    ephemeral,
                },
            )?;
            match response {
                DaemonResponse::CodexThreadDetail(Some(thread)) => {
                    Ok(Some(render_codex_thread_detail(&thread)))
                }
                other => bail!("unexpected daemon response for codex-thread-start: {other:?}"),
            }
        }
        "codex-thread-resume" => {
            let Some(selector) = parts.next() else {
                bail!("codex-thread-resume requires a selector");
            };
            let model = parts.next().map(ToString::to_string);
            let response = request_for_cwd(
                cwd,
                DaemonRequest::ResumeCodexThread {
                    cwd: cwd.to_path_buf(),
                    selector: selector.to_string(),
                    model,
                },
            )?;
            match response {
                DaemonResponse::CodexThreadDetail(Some(thread)) => {
                    Ok(Some(render_codex_thread_detail(&thread)))
                }
                DaemonResponse::CodexThreadDetail(None) => {
                    Ok(Some(format!("codex thread not found: {selector}")))
                }
                other => bail!("unexpected daemon response for codex-thread-resume: {other:?}"),
            }
        }
        "merge-run-status" => {
            let Some(id) = parts.next() else {
                bail!("merge-run-status requires an id");
            };
            let Some(raw_readiness) = parts.next() else {
                bail!("merge-run-status requires a readiness");
            };
            let Some(raw_authorization) = parts.next() else {
                bail!("merge-run-status requires an authorization");
            };
            let Some(raw_execution) = parts.next() else {
                bail!("merge-run-status requires an execution status");
            };
            let readiness = parse_status::<MergeReadiness>(raw_readiness)?;
            let authorization = parse_status::<MergeAuthorizationStatus>(raw_authorization)?;
            let execution = parse_status::<MergeExecutionStatus>(raw_execution)?;
            let head_commit = parts.next().map(ToString::to_string);
            let response = request_for_cwd(
                cwd,
                DaemonRequest::SetMergeRunStatus {
                    id: id.to_string(),
                    readiness,
                    authorization,
                    execution,
                    head_commit,
                },
            )?;
            match response {
                DaemonResponse::Count(count) => Ok(Some(format!("updated {} merge run(s)", count))),
                other => bail!("unexpected daemon response for merge-run-status: {other:?}"),
            }
        }
        other => Ok(Some(format!("unknown command: {other}"))),
    }
}

fn parse_status<T>(raw: &str) -> Result<T>
where
    T: FromStr<Err = String>,
{
    T::from_str(raw).map_err(|error| anyhow::anyhow!(error))
}

fn render_projects(projects: &[Project]) -> String {
    if projects.is_empty() {
        return "no projects".to_string();
    }
    let mut output = String::new();
    for project in projects {
        output.push_str(&format!(
            "{} | {} | {}\n",
            project.slug,
            project.title,
            format!("{:?}", project.status)
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
            work_unit.id,
            work_unit.title,
            format!("{:?}", work_unit.status)
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

fn render_workspace_binding(binding: &WorkspaceBinding) -> String {
    format!(
        "Workspace binding\n=================\nID: {}\nRepo root: {}\nThread: {}\nWorktree: {}\nBranch: {}\nBase ref: {}\nBase commit: {}\nLanding target: {}\nStrategy: {:?}\nSync policy: {:?}\nCleanup policy: {:?}\nStatus: {:?}\nCreated: {}\nUpdated: {}\n",
        binding.id,
        binding.repo_root,
        binding.codex_thread_id,
        binding.worktree_path.as_deref().unwrap_or("<unset>"),
        binding.branch_name.as_deref().unwrap_or("<unset>"),
        binding.base_ref.as_deref().unwrap_or("<unset>"),
        binding.base_commit.as_deref().unwrap_or("<unset>"),
        binding.landing_target.as_deref().unwrap_or("<unset>"),
        binding.strategy,
        binding.sync_policy,
        binding.cleanup_policy,
        binding.status,
        binding.created_at,
        binding.updated_at
    )
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

fn render_merge_run_detail(run: &MergeRun) -> String {
    format!(
        "Merge run\n=========\nID: {}\nWorkspace binding: {}\nReadiness: {:?}\nAuthorization: {:?}\nExecution: {:?}\nHead commit: {}\nCreated: {}\nUpdated: {}\n",
        run.id,
        run.workspace_binding_id,
        run.readiness,
        run.authorization,
        run.execution,
        run.head_commit.as_deref().unwrap_or("<unset>"),
        run.created_at,
        run.updated_at
    )
}

fn render_codex_threads(threads: &[CodexThreadSummary]) -> String {
    if threads.is_empty() {
        return "no codex threads".to_string();
    }
    let mut output = String::new();
    for thread in threads {
        output.push_str(&format!(
            "{} | {} | {:?} | work-unit={} | workspaces={}\n",
            thread.thread_id,
            thread.thread_name.as_deref().unwrap_or("<unnamed>"),
            thread.updated_at,
            thread.bound_work_unit_id.as_deref().unwrap_or("<unbound>"),
            thread.workspace_binding_count
        ));
    }
    output
}

fn render_codex_thread(thread: &CodexThreadSummary) -> String {
    format!(
        "{}\n{}\n{:?}\nwork-unit={}\nworkspaces={}\n",
        thread.thread_id,
        thread.thread_name.as_deref().unwrap_or("<unnamed>"),
        thread.updated_at,
        thread.bound_work_unit_id.as_deref().unwrap_or("<unbound>"),
        thread.workspace_binding_count
    )
}

fn render_codex_thread_detail(thread: &CodexThreadDetail) -> String {
    format!(
        "{}\n{}\nstatus={}\ncwd={}\npreview={}\nmodel_provider={}\nephemeral={}\nupdated_at={}\nturn_count={}\nlatest_turn_id={}\nbound_work_unit_id={}\nworkspace_binding_count={}\n",
        thread.thread_id,
        thread.thread_name.as_deref().unwrap_or("<unnamed>"),
        thread.status,
        thread.cwd,
        thread.preview,
        thread.model_provider,
        thread.ephemeral,
        thread.updated_at,
        thread.turn_count,
        thread.latest_turn_id.as_deref().unwrap_or("-"),
        thread.bound_work_unit_id.as_deref().unwrap_or("<unbound>"),
        thread.workspace_binding_count
    )
}

fn render_workspace_lifecycle_result(
    action: &str,
    binding: &WorkspaceBinding,
    flag: bool,
    selector: Option<&str>,
) -> String {
    let mut output = String::new();
    output.push_str(&format!("{}\n", action.replace('-', " ")));
    output.push_str(&format!("Binding: {}\n", binding.id));
    if let Some(selector) = selector {
        output.push_str(&format!("Selector: {}\n", selector));
    }
    output.push_str(&format!("Thread: {}\n", binding.codex_thread_id));
    output.push_str(&format!(
        "Worktree: {}\n",
        binding.worktree_path.as_deref().unwrap_or("<unset>")
    ));
    output.push_str(&format!(
        "Branch: {}\n",
        binding.branch_name.as_deref().unwrap_or("<unset>")
    ));
    output.push_str(&format!("Status: {:?}\n", binding.status));
    output.push_str(&format!("Flag: {}\n", flag));
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::process::Command;
    use tempfile::tempdir;
    use tt_domain::{
        Project, ProjectStatus, ThreadBinding, ThreadBindingStatus, ThreadRole, WorkUnit,
        WorkUnitStatus, WorkspaceBinding, WorkspaceCleanupPolicy, WorkspaceStatus,
        WorkspaceStrategy, WorkspaceSyncPolicy,
    };

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
        request_for_cwd(
            dir.path(),
            DaemonRequest::UpsertProject {
                project: Project {
                    id: "p1".into(),
                    slug: "alpha".into(),
                    title: "Alpha".into(),
                    objective: "Ship".into(),
                    status: ProjectStatus::Active,
                    created_at: ts(),
                    updated_at: ts(),
                },
            },
        )
        .expect("upsert project");

        let output = handle_command(dir.path(), "projects")
            .expect("command")
            .expect("output");
        assert!(output.contains("alpha"));
    }

    #[test]
    fn handle_command_updates_project_status() {
        let dir = tempdir().expect("tempdir");
        request_for_cwd(
            dir.path(),
            DaemonRequest::UpsertProject {
                project: Project {
                    id: "p1".into(),
                    slug: "alpha".into(),
                    title: "Alpha".into(),
                    objective: "Ship".into(),
                    status: ProjectStatus::Active,
                    created_at: ts(),
                    updated_at: ts(),
                },
            },
        )
        .expect("upsert project");

        let output = handle_command(dir.path(), "project-status alpha blocked")
            .expect("command")
            .expect("output");
        assert!(output.contains("updated 1 project(s)"));

        let response = request_for_cwd(
            dir.path(),
            DaemonRequest::GetProject {
                id_or_slug: "alpha".into(),
            },
        )
        .expect("get project");
        match response {
            DaemonResponse::Project(Some(project)) => {
                assert_eq!(project.status, ProjectStatus::Blocked)
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn handle_command_reports_status_and_repo() {
        let dir = tempdir().expect("tempdir");
        let status = handle_command(dir.path(), "status")
            .expect("command")
            .expect("output");
        assert!(status.contains("TT status"));

        let repo_root = dir.path().join("repo");
        std::fs::create_dir_all(&repo_root).expect("create repo");
        let init = Command::new("git")
            .arg("-C")
            .arg(&repo_root)
            .args(["init", "-b", "main"])
            .status()
            .expect("git init");
        assert!(init.success());
        let repo = handle_command(&repo_root, "repo")
            .expect("command")
            .expect("output");
        assert!(repo.contains("Repository"));
    }

    #[test]
    fn handle_command_shows_binding_details() {
        let dir = tempdir().expect("tempdir");
        request_for_cwd(
            dir.path(),
            DaemonRequest::UpsertProject {
                project: Project {
                    id: "p1".into(),
                    slug: "alpha".into(),
                    title: "Alpha".into(),
                    objective: "Ship".into(),
                    status: ProjectStatus::Active,
                    created_at: ts(),
                    updated_at: ts(),
                },
            },
        )
        .expect("upsert project");
        request_for_cwd(
            dir.path(),
            DaemonRequest::UpsertWorkUnit {
                work_unit: WorkUnit {
                    id: "wu1".into(),
                    project_id: "p1".into(),
                    slug: Some("wu-alpha".into()),
                    title: "Work unit".into(),
                    task: "Do the thing".into(),
                    status: WorkUnitStatus::Ready,
                    created_at: ts(),
                    updated_at: ts(),
                },
            },
        )
        .expect("upsert work unit");
        request_for_cwd(
            dir.path(),
            DaemonRequest::UpsertThreadBinding {
                binding: ThreadBinding {
                    codex_thread_id: "thread-1".into(),
                    work_unit_id: Some("wu1".into()),
                    role: ThreadRole::Develop,
                    status: ThreadBindingStatus::Bound,
                    notes: Some("note".into()),
                    created_at: ts(),
                    updated_at: ts(),
                },
            },
        )
        .expect("upsert thread binding");
        request_for_cwd(
            dir.path(),
            DaemonRequest::UpsertWorkspaceBinding {
                binding: WorkspaceBinding {
                    id: "ws1".into(),
                    codex_thread_id: "thread-1".into(),
                    repo_root: "/repo".into(),
                    worktree_path: Some("/repo/worktree".into()),
                    branch_name: Some("tt/main".into()),
                    base_ref: Some("main".into()),
                    base_commit: Some("abc123".into()),
                    landing_target: Some("main".into()),
                    strategy: WorkspaceStrategy::DedicatedWorktree,
                    sync_policy: WorkspaceSyncPolicy::RebaseBeforeLanding,
                    cleanup_policy: WorkspaceCleanupPolicy::PruneAfterLanding,
                    status: WorkspaceStatus::Ready,
                    created_at: ts(),
                    updated_at: ts(),
                },
            },
        )
        .expect("upsert workspace binding");

        let thread_binding = handle_command(dir.path(), "thread-binding thread-1")
            .expect("command")
            .expect("output");
        assert!(thread_binding.contains("Thread binding"));
        assert!(thread_binding.contains("thread-1"));

        let workspace_binding = handle_command(dir.path(), "workspace-binding ws1")
            .expect("command")
            .expect("output");
        assert!(workspace_binding.contains("Workspace binding"));
        assert!(workspace_binding.contains("ws1"));
    }

    #[test]
    fn handle_command_reports_workspace_close_details() {
        let dir = tempdir().expect("tempdir");
        request_for_cwd(
            dir.path(),
            DaemonRequest::UpsertThreadBinding {
                binding: ThreadBinding {
                    codex_thread_id: "thread-1".into(),
                    work_unit_id: None,
                    role: ThreadRole::Develop,
                    status: ThreadBindingStatus::Bound,
                    notes: Some("note".into()),
                    created_at: ts(),
                    updated_at: ts(),
                },
            },
        )
        .expect("upsert thread binding");
        request_for_cwd(
            dir.path(),
            DaemonRequest::UpsertWorkspaceBinding {
                binding: WorkspaceBinding {
                    id: "ws1".into(),
                    codex_thread_id: "thread-1".into(),
                    repo_root: "/repo".into(),
                    worktree_path: Some("/repo/worktree".into()),
                    branch_name: Some("tt/main".into()),
                    base_ref: Some("main".into()),
                    base_commit: Some("abc123".into()),
                    landing_target: Some("main".into()),
                    strategy: WorkspaceStrategy::DedicatedWorktree,
                    sync_policy: WorkspaceSyncPolicy::RebaseBeforeLanding,
                    cleanup_policy: WorkspaceCleanupPolicy::PruneAfterLanding,
                    status: WorkspaceStatus::Ready,
                    created_at: ts(),
                    updated_at: ts(),
                },
            },
        )
        .expect("upsert workspace binding");

        let output = handle_command(dir.path(), "workspace-close ws1 force")
            .expect("command")
            .expect("output");
        assert!(output.contains("workspace close"));
        assert!(output.contains("Status: Pruned"));
        assert!(output.contains("Flag: true"));
    }

    #[test]
    fn handle_command_lists_codex_threads() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".codex")).expect("create codex dir");
        std::fs::write(
            dir.path().join(".codex").join("session_index.jsonl"),
            concat!(
                "{\"id\":\"thread-1\",\"thread_name\":\"alpha\",\"updated_at\":\"2026-04-08T12:00:00Z\"}\n",
                "{\"id\":\"thread-2\",\"thread_name\":\"beta\",\"updated_at\":\"2026-04-08T12:01:00Z\"}\n"
            ),
        )
        .expect("write codex index");

        let output = handle_command(dir.path(), "codex-threads")
            .expect("command")
            .expect("output");
        assert!(output.contains("thread-1"));
        assert!(output.contains("thread-2"));
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
