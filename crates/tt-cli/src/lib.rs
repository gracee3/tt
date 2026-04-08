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
    ThreadBindingStatus, WorkUnitStatus, WorkspaceStatus,
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
    Legacy {
        #[command(subcommand)]
        command: LegacyCommand,
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
    List { limit: Option<usize> },
    Get { selector: String },
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
}

#[derive(Debug, Subcommand)]
pub enum LegacyCommand {
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
    println!("{}", serde_json::to_string_pretty(&response)?);
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
                WorkspaceBindingCommand::Get { id } => {
                    DaemonRequest::GetWorkspaceBinding { id }
                }
                WorkspaceBindingCommand::Upsert { file } => {
                    DaemonRequest::UpsertWorkspaceBinding {
                        binding: read_json(file)?,
                    }
                }
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
        },
        Command::Legacy { command } => match command {
            LegacyCommand::Project { command } => match command {
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
            LegacyCommand::WorkUnit { command } => match command {
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
            LegacyCommand::ThreadBinding { command } => match command {
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
    use tt_domain::{Project, ProjectStatus};

    #[test]
    fn parses_project_list_command() {
        let cli = Cli::parse_from(["tt", "legacy", "project", "list"]);
        assert!(matches!(
            cli.command,
            Command::Legacy {
                command: LegacyCommand::Project {
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
}
