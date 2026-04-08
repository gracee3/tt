//! Thin CLI surface for TT v2.
//!
//! The canonical v2 CLI is a narrow client over the daemon request API rather
//! than a second application layer.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::de::DeserializeOwned;
use tt_daemon::{DaemonRequest, DaemonResponse, DaemonRuntime};
use tt_domain as _;

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
    WorkspaceBinding {
        #[command(subcommand)]
        command: WorkspaceBindingCommand,
    },
    MergeRun {
        #[command(subcommand)]
        command: MergeRunCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    List,
    Get { id_or_slug: String },
    Upsert { file: PathBuf },
    Delete { id_or_slug: String },
}

#[derive(Debug, Subcommand)]
pub enum WorkUnitCommand {
    List { project_id: Option<String> },
    Get { id_or_slug: String },
    Upsert { file: PathBuf },
    Delete { id_or_slug: String },
}

#[derive(Debug, Subcommand)]
pub enum ThreadBindingCommand {
    List,
    Get { codex_thread_id: String },
    Upsert { file: PathBuf },
    Delete { codex_thread_id: String },
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceBindingCommand {
    List,
    Get { id: String },
    Upsert { file: PathBuf },
    Delete { id: String },
}

#[derive(Debug, Subcommand)]
pub enum MergeRunCommand {
    List,
    Get { id: String },
    Upsert { file: PathBuf },
    Delete { id: String },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let cwd = cli.cwd.unwrap_or(std::env::current_dir()?);
    let runtime = DaemonRuntime::open(cwd)?;
    let response = runtime.request(command_to_request(cli.command, runtime.cwd())?)?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

fn command_to_request(command: Command, cwd: &Path) -> Result<DaemonRequest> {
    Ok(match command {
        Command::Status => DaemonRequest::Status,
        Command::Repo => DaemonRequest::RepositorySummary {
            cwd: cwd.to_path_buf(),
        },
        Command::Project { command } => match command {
            ProjectCommand::List => DaemonRequest::ListProjects,
            ProjectCommand::Get { id_or_slug } => DaemonRequest::GetProject { id_or_slug },
            ProjectCommand::Upsert { file } => DaemonRequest::UpsertProject {
                project: read_json(file)?,
            },
            ProjectCommand::Delete { id_or_slug } => DaemonRequest::DeleteProject { id_or_slug },
        },
        Command::WorkUnit { command } => match command {
            WorkUnitCommand::List { project_id } => DaemonRequest::ListWorkUnits { project_id },
            WorkUnitCommand::Get { id_or_slug } => DaemonRequest::GetWorkUnit { id_or_slug },
            WorkUnitCommand::Upsert { file } => DaemonRequest::UpsertWorkUnit {
                work_unit: read_json(file)?,
            },
            WorkUnitCommand::Delete { id_or_slug } => DaemonRequest::DeleteWorkUnit { id_or_slug },
        },
        Command::ThreadBinding { command } => match command {
            ThreadBindingCommand::List => DaemonRequest::ListThreadBindings,
            ThreadBindingCommand::Get { codex_thread_id } => {
                DaemonRequest::GetThreadBinding { codex_thread_id }
            }
            ThreadBindingCommand::Upsert { file } => DaemonRequest::UpsertThreadBinding {
                binding: read_json(file)?,
            },
            ThreadBindingCommand::Delete { codex_thread_id } => {
                DaemonRequest::DeleteThreadBinding { codex_thread_id }
            }
        },
        Command::WorkspaceBinding { command } => match command {
            WorkspaceBindingCommand::List => DaemonRequest::ListWorkspaceBindings,
            WorkspaceBindingCommand::Get { id } => DaemonRequest::GetWorkspaceBinding { id },
            WorkspaceBindingCommand::Upsert { file } => DaemonRequest::UpsertWorkspaceBinding {
                binding: read_json(file)?,
            },
            WorkspaceBindingCommand::Delete { id } => DaemonRequest::DeleteWorkspaceBinding { id },
        },
        Command::MergeRun { command } => match command {
            MergeRunCommand::List => DaemonRequest::ListMergeRuns,
            MergeRunCommand::Get { id } => DaemonRequest::GetMergeRun { id },
            MergeRunCommand::Upsert { file } => DaemonRequest::UpsertMergeRun {
                run: read_json(file)?,
            },
            MergeRunCommand::Delete { id } => DaemonRequest::DeleteMergeRun { id },
        },
    })
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
    let runtime = DaemonRuntime::open(cwd)?;
    let response = runtime.request(command_to_request(cli.command, runtime.cwd())?)?;
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
        let cli = Cli::parse_from(["tt", "project", "list"]);
        assert!(matches!(cli.command, Command::Project { command: ProjectCommand::List }));
    }

    #[test]
    fn parses_cwd_flag() {
        let cli = Cli::parse_from(["tt", "--cwd", "/tmp", "status"]);
        assert!(matches!(cli.command, Command::Status));
        assert_eq!(cli.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
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
