//! Thin CLI surface for TT v2.
//!
//! The canonical v2 CLI is a narrow client over the daemon request API rather
//! than a second application layer.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::de::DeserializeOwned;
use tt_daemon::{DaemonRequest, DaemonResponse, request_for_cwd};
use tt_domain as _;
use tt_domain::{
    MergeAuthorizationStatus, MergeExecutionStatus, MergeReadiness, ProjectStatus,
    ThreadBindingStatus, ThreadRole, WorkUnitStatus, WorkspaceStatus,
};

pub const TT_CLI_GENERATION: &str = "v2";

#[derive(Debug, Parser)]
#[command(name = "tt", version, about = "TT v2 local client")]
pub struct Cli {
    /// Working directory to open the TT runtime in.
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Status,
    Repo,
    Codex {
        #[command(subcommand)]
        command: CodexCommand,
    },
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    #[command(alias = "legacy")]
    Records {
        #[command(subcommand)]
        command: RecordsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum CodexCommand {
    Threads {
        #[command(subcommand)]
        command: CodexThreadsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum CodexThreadsCommand {
    List {
        limit: Option<usize>,
    },
    Get {
        selector: String,
    },
    Read {
        selector: String,
        #[arg(long, default_value_t = true)]
        include_turns: bool,
    },
    Start {
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value_t = false)]
        ephemeral: bool,
    },
    Resume {
        selector: String,
        #[arg(long)]
        model: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    List,
    Get { id_or_slug: String },
    Upsert { file: PathBuf },
    SetStatus { id_or_slug: String, status: String },
    Delete { id_or_slug: String },
}

#[derive(Debug, Subcommand)]
pub enum WorkUnitCommand {
    List { project_id: Option<String> },
    Get { id_or_slug: String },
    Upsert { file: PathBuf },
    SetStatus { id_or_slug: String, status: String },
    Delete { id_or_slug: String },
}

#[derive(Debug, Subcommand)]
pub enum ThreadBindingCommand {
    List,
    Get {
        codex_thread_id: String,
    },
    Upsert {
        file: PathBuf,
    },
    SetStatus {
        codex_thread_id: String,
        status: String,
    },
    Delete {
        codex_thread_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceBindingCommand {
    List,
    Get { id: String },
    Upsert { file: PathBuf },
    SetStatus { id: String, status: String },
    Refresh { id: String },
    Delete { id: String },
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    Binding {
        #[command(subcommand)]
        command: WorkspaceBindingCommand,
    },
    MergeRun {
        #[command(subcommand)]
        command: MergeRunCommand,
    },
    Action {
        #[command(subcommand)]
        command: WorkspaceActionCommand,
    },
    Lifecycle {
        #[command(subcommand)]
        command: WorkspaceLifecycleCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceActionCommand {
    Prepare {
        id: String,
    },
    Refresh {
        id: String,
    },
    MergePrep {
        id: String,
    },
    AuthorizeMerge {
        id: String,
    },
    ExecuteLanding {
        id: String,
    },
    Prune {
        id: String,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceLifecycleCommand {
    Close {
        selector: Option<String>,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    Park {
        selector: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    Split {
        #[arg(long, default_value = "develop")]
        role: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value_t = false)]
        ephemeral: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum RecordsCommand {
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    WorkUnit {
        #[command(subcommand)]
        command: WorkUnitCommand,
    },
    ThreadBinding {
        #[command(subcommand)]
        command: ThreadBindingCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum MergeRunCommand {
    List,
    Get {
        id: String,
    },
    Upsert {
        file: PathBuf,
    },
    SetStatus {
        id: String,
        readiness: String,
        authorization: String,
        execution: String,
        head_commit: Option<String>,
    },
    Refresh {
        workspace_binding_id: String,
    },
    Delete {
        id: String,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let cwd = cli.cwd.unwrap_or(std::env::current_dir()?);
    let response = request_for_cwd(&cwd, command_to_request(cli.command, &cwd)?)?;
    println!("{}", render_response(&response));
    Ok(())
}

fn command_to_request(command: Command, cwd: &Path) -> Result<DaemonRequest> {
    Ok(match command {
        Command::Status => DaemonRequest::Status,
        Command::Repo => DaemonRequest::RepositorySummary {
            cwd: cwd.to_path_buf(),
        },
        Command::Codex { command } => match command {
            CodexCommand::Threads { command } => match command {
                CodexThreadsCommand::List { limit } => DaemonRequest::ListCodexThreads {
                    cwd: cwd.to_path_buf(),
                    limit,
                },
                CodexThreadsCommand::Get { selector } => DaemonRequest::GetCodexThread {
                    cwd: cwd.to_path_buf(),
                    selector,
                },
                CodexThreadsCommand::Read {
                    selector,
                    include_turns,
                } => DaemonRequest::ReadCodexThread {
                    cwd: cwd.to_path_buf(),
                    selector,
                    include_turns,
                },
                CodexThreadsCommand::Start { model, ephemeral } => {
                    DaemonRequest::StartCodexThread {
                        cwd: cwd.to_path_buf(),
                        model,
                        ephemeral,
                    }
                }
                CodexThreadsCommand::Resume { selector, model } => {
                    DaemonRequest::ResumeCodexThread {
                        cwd: cwd.to_path_buf(),
                        selector,
                        model,
                    }
                }
            },
        },
        Command::Workspace { command } => match command {
            WorkspaceCommand::Binding { command } => match command {
                WorkspaceBindingCommand::List => DaemonRequest::ListWorkspaceBindings,
                WorkspaceBindingCommand::Get { id } => DaemonRequest::GetWorkspaceBinding { id },
                WorkspaceBindingCommand::Upsert { file } => DaemonRequest::UpsertWorkspaceBinding {
                    binding: read_json(file)?,
                },
                WorkspaceBindingCommand::SetStatus { id, status } => {
                    DaemonRequest::SetWorkspaceBindingStatus {
                        id,
                        status: parse_status::<WorkspaceStatus>(&status)?,
                    }
                }
                WorkspaceBindingCommand::Refresh { id } => {
                    DaemonRequest::RefreshWorkspaceBinding { id }
                }
                WorkspaceBindingCommand::Delete { id } => {
                    DaemonRequest::DeleteWorkspaceBinding { id }
                }
            },
            WorkspaceCommand::MergeRun { command } => match command {
                MergeRunCommand::List => DaemonRequest::ListMergeRuns,
                MergeRunCommand::Get { id } => DaemonRequest::GetMergeRun { id },
                MergeRunCommand::Upsert { file } => DaemonRequest::UpsertMergeRun {
                    run: read_json(file)?,
                },
                MergeRunCommand::SetStatus {
                    id,
                    readiness,
                    authorization,
                    execution,
                    head_commit,
                } => DaemonRequest::SetMergeRunStatus {
                    id,
                    readiness: parse_status::<MergeReadiness>(&readiness)?,
                    authorization: parse_status::<MergeAuthorizationStatus>(&authorization)?,
                    execution: parse_status::<MergeExecutionStatus>(&execution)?,
                    head_commit,
                },
                MergeRunCommand::Refresh {
                    workspace_binding_id,
                } => DaemonRequest::RefreshMergeRun {
                    workspace_binding_id,
                },
                MergeRunCommand::Delete { id } => DaemonRequest::DeleteMergeRun { id },
            },
            WorkspaceCommand::Action { command } => match command {
                WorkspaceActionCommand::Prepare { id } => {
                    DaemonRequest::PrepareWorkspaceBinding { id }
                }
                WorkspaceActionCommand::Refresh { id } => {
                    DaemonRequest::RefreshWorkspaceBinding { id }
                }
                WorkspaceActionCommand::MergePrep { id } => {
                    DaemonRequest::MergePrepWorkspaceBinding { id }
                }
                WorkspaceActionCommand::AuthorizeMerge { id } => {
                    DaemonRequest::AuthorizeMergeWorkspaceBinding { id }
                }
                WorkspaceActionCommand::ExecuteLanding { id } => {
                    DaemonRequest::ExecuteLandingWorkspaceBinding { id }
                }
                WorkspaceActionCommand::Prune { id, force } => {
                    DaemonRequest::PruneWorkspaceBinding { id, force }
                }
            },
            WorkspaceCommand::Lifecycle { command } => match command {
                WorkspaceLifecycleCommand::Close { selector, force } => {
                    DaemonRequest::CloseWorkspace {
                        cwd: cwd.to_path_buf(),
                        selector,
                        force,
                    }
                }
                WorkspaceLifecycleCommand::Park { selector, note } => {
                    DaemonRequest::ParkWorkspace {
                        cwd: cwd.to_path_buf(),
                        selector,
                        note,
                    }
                }
                WorkspaceLifecycleCommand::Split {
                    role,
                    model,
                    ephemeral,
                } => DaemonRequest::SplitWorkspace {
                    cwd: cwd.to_path_buf(),
                    role: parse_status::<ThreadRole>(&role)?,
                    model,
                    ephemeral,
                },
            },
        },
        Command::Records { command } => match command {
            RecordsCommand::Project { command } => match command {
                ProjectCommand::List => DaemonRequest::ListProjects,
                ProjectCommand::Get { id_or_slug } => DaemonRequest::GetProject { id_or_slug },
                ProjectCommand::Upsert { file } => DaemonRequest::UpsertProject {
                    project: read_json(file)?,
                },
                ProjectCommand::SetStatus { id_or_slug, status } => {
                    DaemonRequest::SetProjectStatus {
                        id_or_slug,
                        status: parse_status::<ProjectStatus>(&status)?,
                    }
                }
                ProjectCommand::Delete { id_or_slug } => {
                    DaemonRequest::DeleteProject { id_or_slug }
                }
            },
            RecordsCommand::WorkUnit { command } => match command {
                WorkUnitCommand::List { project_id } => DaemonRequest::ListWorkUnits { project_id },
                WorkUnitCommand::Get { id_or_slug } => DaemonRequest::GetWorkUnit { id_or_slug },
                WorkUnitCommand::Upsert { file } => DaemonRequest::UpsertWorkUnit {
                    work_unit: read_json(file)?,
                },
                WorkUnitCommand::SetStatus { id_or_slug, status } => {
                    DaemonRequest::SetWorkUnitStatus {
                        id_or_slug,
                        status: parse_status::<WorkUnitStatus>(&status)?,
                    }
                }
                WorkUnitCommand::Delete { id_or_slug } => {
                    DaemonRequest::DeleteWorkUnit { id_or_slug }
                }
            },
            RecordsCommand::ThreadBinding { command } => match command {
                ThreadBindingCommand::List => DaemonRequest::ListThreadBindings,
                ThreadBindingCommand::Get { codex_thread_id } => {
                    DaemonRequest::GetThreadBinding { codex_thread_id }
                }
                ThreadBindingCommand::Upsert { file } => DaemonRequest::UpsertThreadBinding {
                    binding: read_json(file)?,
                },
                ThreadBindingCommand::SetStatus {
                    codex_thread_id,
                    status,
                } => DaemonRequest::SetThreadBindingStatus {
                    codex_thread_id,
                    status: parse_status::<ThreadBindingStatus>(&status)?,
                },
                ThreadBindingCommand::Delete { codex_thread_id } => {
                    DaemonRequest::DeleteThreadBinding { codex_thread_id }
                }
            },
        },
    })
}

fn parse_status<T>(raw: &str) -> Result<T>
where
    T: FromStr<Err = String>,
{
    T::from_str(raw).map_err(|error| anyhow::anyhow!(error))
}

fn render_response(response: &DaemonResponse) -> String {
    match response {
        DaemonResponse::Unit => "ok".to_string(),
        DaemonResponse::Count(count) => format!("updated {count}"),
        DaemonResponse::Status(status) => format!(
            "status\nprojects: {}\nwork-units: {}\nbound-threads: {}\nready-workspaces: {}\n",
            status.project_count,
            status.work_unit_count,
            status.bound_thread_count,
            status.ready_workspace_count
        ),
        DaemonResponse::DashboardSummary(summary) => format!(
            "dashboard\nprojects: {}\nwork-units: {}\nbound-threads: {}\nready-workspaces: {}\n",
            summary.active_projects,
            summary.active_work_units,
            summary.bound_threads,
            summary.ready_workspaces
        ),
        DaemonResponse::RepositorySummary(Some(summary)) => format!(
            "repository\nroot: {}\nworktree: {}\nbranch: {}\nhead: {}\ndirty: {}\nmerge-ready: {}\nworktrees: {}\n",
            summary.repository_root,
            summary.current_worktree.as_deref().unwrap_or("<unset>"),
            summary.current_branch.as_deref().unwrap_or("<detached>"),
            summary
                .current_head_commit
                .as_deref()
                .unwrap_or("<unknown>"),
            summary.dirty,
            summary.merge_ready,
            summary.worktree_count
        ),
        DaemonResponse::RepositorySummary(None) => "not inside a git checkout".to_string(),
        DaemonResponse::Project(Some(project)) => format!(
            "project\nid: {}\nslug: {}\ntitle: {}\nobjective: {}\nstatus: {:?}\n",
            project.id, project.slug, project.title, project.objective, project.status
        ),
        DaemonResponse::Project(None) => "project not found".to_string(),
        DaemonResponse::WorkUnit(Some(work_unit)) => format!(
            "work-unit\nid: {}\nproject: {}\nslug: {}\ntitle: {}\ntask: {}\nstatus: {:?}\n",
            work_unit.id,
            work_unit.project_id,
            work_unit.slug.as_deref().unwrap_or("<unset>"),
            work_unit.title,
            work_unit.task,
            work_unit.status
        ),
        DaemonResponse::WorkUnit(None) => "work unit not found".to_string(),
        DaemonResponse::ThreadBinding(Some(binding)) => format!(
            "thread-binding\nthread: {}\nwork-unit: {}\nrole: {:?}\nstatus: {:?}\nnotes: {}\n",
            binding.codex_thread_id,
            binding.work_unit_id.as_deref().unwrap_or("<unbound>"),
            binding.role,
            binding.status,
            binding.notes.as_deref().unwrap_or("<unset>")
        ),
        DaemonResponse::ThreadBinding(None) => "thread binding not found".to_string(),
        DaemonResponse::WorkspaceBinding(Some(binding)) => format!(
            "workspace-binding\nid: {}\nrepo-root: {}\nthread: {}\nworktree: {}\nbranch: {}\nstatus: {:?}\n",
            binding.id,
            binding.repo_root,
            binding.codex_thread_id,
            binding.worktree_path.as_deref().unwrap_or("<unset>"),
            binding.branch_name.as_deref().unwrap_or("<unset>"),
            binding.status
        ),
        DaemonResponse::WorkspaceBinding(None) => "workspace binding not found".to_string(),
        DaemonResponse::MergeRun(Some(run)) => format!(
            "merge-run\nid: {}\nworkspace-binding: {}\nreadiness: {:?}\nauthorization: {:?}\nexecution: {:?}\nhead-commit: {}\n",
            run.id,
            run.workspace_binding_id,
            run.readiness,
            run.authorization,
            run.execution,
            run.head_commit.as_deref().unwrap_or("<unset>")
        ),
        DaemonResponse::MergeRun(None) => "merge run not found".to_string(),
        DaemonResponse::Projects(projects) => format!(
            "{}",
            projects
                .iter()
                .map(|project| format!(
                    "{} | {} | {:?}",
                    project.slug, project.title, project.status
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        DaemonResponse::WorkUnits(work_units) => format!(
            "{}",
            work_units
                .iter()
                .map(|work_unit| format!(
                    "{} | {} | {:?}",
                    work_unit.id, work_unit.title, work_unit.status
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        DaemonResponse::ThreadBindings(bindings) => format!(
            "{}",
            bindings
                .iter()
                .map(|binding| format!(
                    "{} | {:?} | {:?}",
                    binding.codex_thread_id, binding.role, binding.status
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        DaemonResponse::WorkspaceBindings(bindings) => format!(
            "{}",
            bindings
                .iter()
                .map(|binding| format!(
                    "{} | {} | {:?}",
                    binding.id, binding.repo_root, binding.status
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        DaemonResponse::MergeRuns(runs) => format!(
            "{}",
            runs.iter()
                .map(|run| format!(
                    "{} | {} | {:?} | {:?}",
                    run.id, run.workspace_binding_id, run.readiness, run.execution
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        DaemonResponse::CodexThreads(threads) => format!(
            "{}",
            threads
                .iter()
                .map(|thread| format!(
                    "{} | {} | {:?} | work-unit={} | workspaces={}",
                    thread.thread_id,
                    thread.thread_name.as_deref().unwrap_or("<unnamed>"),
                    thread.updated_at,
                    thread.bound_work_unit_id.as_deref().unwrap_or("<unbound>"),
                    thread.workspace_binding_count
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        DaemonResponse::CodexThread(Some(thread)) => format!(
            "{}\n{}\n{:?}\nwork-unit={}\nworkspaces={}\n",
            thread.thread_id,
            thread.thread_name.as_deref().unwrap_or("<unnamed>"),
            thread.updated_at,
            thread.bound_work_unit_id.as_deref().unwrap_or("<unbound>"),
            thread.workspace_binding_count
        ),
        DaemonResponse::CodexThread(None) => "codex thread not found".to_string(),
        DaemonResponse::CodexThreadDetails(details) => format!(
            "{}",
            details
                .iter()
                .map(|detail| format!(
                    "{} | {} | {}\n",
                    detail.thread_id,
                    detail.thread_name.as_deref().unwrap_or("<unnamed>"),
                    detail.status
                ))
                .collect::<Vec<_>>()
                .join("")
        ),
        DaemonResponse::CodexThreadDetail(Some(thread)) => format!(
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
        ),
        DaemonResponse::CodexThreadDetail(None) => "codex thread not found".to_string(),
    }
}

fn read_json<T>(path: PathBuf) -> Result<T>
where
    T: DeserializeOwned,
{
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

pub fn run_from_args(args: impl IntoIterator<Item = String>) -> Result<()> {
    let cli = Cli::parse_from(args);
    let cwd = cli.cwd.unwrap_or(std::env::current_dir()?);
    let response = request_for_cwd(&cwd, command_to_request(cli.command, &cwd)?)?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

pub fn response_is_empty(response: &DaemonResponse) -> bool {
    matches!(response, DaemonResponse::Unit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use tt_domain::{
        MergeAuthorizationStatus, MergeExecutionStatus, MergeReadiness, MergeRun, Project,
        ProjectStatus, WorkspaceBinding, WorkspaceCleanupPolicy, WorkspaceStatus,
        WorkspaceStrategy, WorkspaceSyncPolicy,
    };

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 8, 12, 0, 0).unwrap()
    }

    #[test]
    fn parses_project_list_command() {
        let cli = Cli::parse_from(["tt", "records", "project", "list"]);
        assert!(matches!(
            cli.command,
            Command::Records {
                command: RecordsCommand::Project {
                    command: ProjectCommand::List
                }
            }
        ));
    }

    #[test]
    fn parses_legacy_alias_for_records_command() {
        let cli = Cli::parse_from(["tt", "legacy", "project", "list"]);
        assert!(matches!(
            cli.command,
            Command::Records {
                command: RecordsCommand::Project {
                    command: ProjectCommand::List
                }
            }
        ));
    }

    #[test]
    fn parses_cwd_flag() {
        let cli = Cli::parse_from(["tt", "--cwd", "/tmp", "status"]);
        assert!(matches!(cli.command, Command::Status));
        assert_eq!(cli.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
    }

    #[test]
    fn parses_codex_thread_list_command() {
        let cli = Cli::parse_from(["tt", "codex", "threads", "list"]);
        assert!(matches!(
            cli.command,
            Command::Codex {
                command: CodexCommand::Threads {
                    command: CodexThreadsCommand::List { limit: None }
                }
            }
        ));
    }

    #[test]
    fn can_serialize_project_round_trip() {
        let project = Project {
            id: "p1".into(),
            slug: "alpha".into(),
            title: "Alpha".into(),
            objective: "Ship".into(),
            status: ProjectStatus::Active,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&project).expect("serialize");
        let _: Project = serde_json::from_str(&json).expect("deserialize");
    }

    #[test]
    fn renders_workspace_binding_and_merge_run_details() {
        let binding = WorkspaceBinding {
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
        };
        let binding_text = render_response(&DaemonResponse::WorkspaceBinding(Some(binding)));
        assert!(binding_text.contains("workspace-binding"));
        assert!(binding_text.contains("thread-1"));
        assert!(binding_text.contains("tt/main"));

        let run = MergeRun {
            id: "ws1".into(),
            workspace_binding_id: "ws1".into(),
            readiness: MergeReadiness::Ready,
            authorization: MergeAuthorizationStatus::NotRequested,
            execution: MergeExecutionStatus::NotStarted,
            head_commit: Some("abc123".into()),
            created_at: ts(),
            updated_at: ts(),
        };
        let run_text = render_response(&DaemonResponse::MergeRun(Some(run)));
        assert!(run_text.contains("merge-run"));
        assert!(run_text.contains("abc123"));
        assert!(run_text.contains("readiness"));
    }
}
