//! Thin CLI surface for TT v2.
//!
//! The canonical v2 CLI is a narrow client over the daemon request API rather
//! than a second application layer.

use std::fs;
use std::io::IsTerminal;
use std::io::{BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Arg, ArgAction, CommandFactory, Parser, Subcommand};
use tt_codex::managed_project_codex_home;
#[cfg(test)]
use tt_codex::{TT_CODEX_LOGIN_MODE_ENV, load_repo_settings_env, repo_env_var};
use tt_daemon::{
    DaemonRequest, DaemonResponse, ManagedProjectDirectorState, ManagedProjectEvent,
    ManagedProjectEventKind, ManagedProjectThreadAttachment, ManagedProjectThreadControlMode,
    load_managed_project_events, managed_project_events_path, request_for_cwd,
};
use tt_domain as _;
use tt_domain::ThreadRole;

pub const TT_CLI_GENERATION: &str = "v2";

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexLoginMode {
    Auto,
    Interactive,
    DeviceAuth,
    Manual,
}

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
    Init {
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        objective: Option<String>,
        #[arg(long)]
        template: Option<String>,
        #[arg(long)]
        base_branch: Option<String>,
        #[arg(long)]
        worktree_root: Option<PathBuf>,
        #[arg(long)]
        director_model: Option<String>,
        #[arg(long)]
        dev_model: Option<String>,
        #[arg(long)]
        test_model: Option<String>,
        #[arg(long)]
        integration_model: Option<String>,
    },
    Open {
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        objective: Option<String>,
        #[arg(long)]
        base_branch: Option<String>,
        #[arg(long)]
        worktree_root: Option<PathBuf>,
        #[arg(long)]
        director_model: Option<String>,
        #[arg(long)]
        dev_model: Option<String>,
        #[arg(long)]
        test_model: Option<String>,
        #[arg(long)]
        integration_model: Option<String>,
    },
    Docs {
        #[command(subcommand)]
        command: DocsCommand,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Clean {
        #[arg(long = "all", alias = "full")]
        all: bool,
    },
    Events {
        #[arg(long)]
        follow: bool,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    #[command(hide = true)]
    Internal {
        #[command(subcommand)]
        command: InternalCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum DocsCommand {
    ExportCli {
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum InternalCommand {
    Repo,
    Project {
        #[command(subcommand)]
        command: InternalProjectCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum InternalProjectCommand {
    #[command(alias = "status")]
    Inspect,
    Plan {
        #[command(subcommand)]
        command: ProjectPlanCommand,
    },
    Director {
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        objective: Option<String>,
        #[arg(long)]
        base_branch: Option<String>,
        #[arg(long)]
        worktree_root: Option<PathBuf>,
        #[arg(long)]
        director_model: Option<String>,
        #[arg(long)]
        dev_model: Option<String>,
        #[arg(long)]
        test_model: Option<String>,
        #[arg(long)]
        integration_model: Option<String>,
        #[arg(long)]
        role: Vec<String>,
        #[arg(long)]
        binding: Vec<String>,
        #[arg(long)]
        scenario: Option<String>,
        #[arg(long)]
        seed_file: Option<PathBuf>,
    },
    Control {
        #[arg(long)]
        role: String,
        #[arg(long)]
        mode: String,
    },
    Spawn {
        #[arg(long)]
        role: Vec<String>,
    },
    Attach {
        #[arg(long)]
        binding: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProjectPlanCommand {
    Show,
    Refresh,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    if let Command::Events {
        follow,
        json,
        limit,
    } = &cli.command
    {
        let cwd = cli.cwd.unwrap_or(std::env::current_dir()?);
        if *follow {
            follow_events_for_cwd(&cwd, *json, *limit)?;
            return Ok(());
        }
        print!("{}", read_events_output_for_cwd(&cwd, *json, *limit)?);
        return Ok(());
    }
    if let Some(output) = local_command_output(&cli.command, &cli.cwd)? {
        print!("{output}");
        return Ok(());
    }
    let is_open_command = matches!(&cli.command, Command::Open { .. });
    let is_status_command = matches!(&cli.command, Command::Status { .. });
    let is_clean_command = matches!(&cli.command, Command::Clean { .. });
    let status_json = matches!(&cli.command, Command::Status { json: true });
    let cwd = cli.cwd.unwrap_or(std::env::current_dir()?);
    if is_open_command {
        ensure_project_initialized_for_open(&cwd)?;
    }
    let request_cwd = request_cwd_for_command(&cwd, &cli.command);
    let response = request_for_cwd(&request_cwd, command_to_request(cli.command, &cwd)?)
        .map_err(|error| rewrite_open_runtime_error(&cwd, is_open_command, error))?;
    if is_open_command && should_launch_codex_tui() {
        let DaemonResponse::ManagedProject(bootstrap) = response else {
            anyhow::bail!("tt open expected a managed project bootstrap");
        };
        launch_codex_tui_for_director(&cwd, &bootstrap)?;
        return Ok(());
    }
    let output = match (is_status_command, is_clean_command, response) {
        (true, _, DaemonResponse::Status(status)) => {
            let runtime_state = runtime_state_for_cwd(&cwd);
            if status_json {
                render_status_json(&status, runtime_state)
            } else {
                render_status_response(&status, runtime_state, std::io::stdout().is_terminal())
            }
        }
        (_, true, DaemonResponse::Count(count)) => format!("cleaned {count}\n"),
        (_, _, response) => render_response(&response),
    };
    println!("{output}");
    Ok(())
}

fn local_command_output(command: &Command, cli_cwd: &Option<PathBuf>) -> Result<Option<String>> {
    match command {
        Command::Docs {
            command: DocsCommand::ExportCli { output },
        } => {
            let markdown = render_cli_reference_markdown();
            if let Some(path) = output {
                let base = cli_cwd.clone().unwrap_or(
                    std::env::current_dir().context("resolve current working directory")?,
                );
                let resolved = resolve_cli_path(&base, path.clone());
                if let Some(parent) = resolved.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("create docs parent {}", parent.display()))?;
                }
                fs::write(&resolved, markdown.as_bytes())
                    .with_context(|| format!("write CLI reference to {}", resolved.display()))?;
                return Ok(Some(format!(
                    "exported CLI reference to {}\n",
                    resolved.display()
                )));
            }
            Ok(Some(markdown))
        }
        _ => Ok(None),
    }
}

fn read_events_output_for_cwd(cwd: &Path, json: bool, limit: usize) -> Result<String> {
    let repo_root = resolve_repo_root(cwd)
        .ok_or_else(|| anyhow::anyhow!("tt events requires a git repository"))?;
    let events = load_managed_project_events(&repo_root, Some(limit))?;
    if events.is_empty() {
        return Ok("no events yet\n".to_string());
    }
    Ok(if json {
        events
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?
            .join("\n")
            + "\n"
    } else {
        render_events_chat(&events)
    })
}

fn follow_events_for_cwd(cwd: &Path, json: bool, limit: usize) -> Result<()> {
    let repo_root = resolve_repo_root(cwd)
        .ok_or_else(|| anyhow::anyhow!("tt events requires a git repository"))?;
    let initial = read_events_output_for_cwd(&repo_root, json, limit)?;
    print!("{initial}");
    let path = managed_project_events_path(&repo_root);
    let mut offset = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                offset = 0;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.len() < offset {
            offset = 0;
        }
        if metadata.len() == offset {
            continue;
        }
        let mut file = fs::File::open(&path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();
        while reader.read_line(&mut line)? > 0 {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let event: ManagedProjectEvent = serde_json::from_str(trimmed)?;
                if json {
                    println!("{}", serde_json::to_string(&event)?);
                } else {
                    print!("{}", render_events_chat(&[event]));
                }
            }
            line.clear();
        }
        offset = metadata.len();
    }
}

fn resolve_repo_root(cwd: &Path) -> Option<PathBuf> {
    let mut current = Some(cwd);
    while let Some(path) = current {
        if path.join(".git").exists() {
            return Some(path.to_path_buf());
        }
        current = path.parent();
    }
    None
}

fn render_events_chat(events: &[ManagedProjectEvent]) -> String {
    let mut output = String::new();
    for event in events {
        let ts = event.ts.format("%H:%M:%S").to_string();
        output.push_str(&format!("{}        {}\n", event_actor_label(event), ts));
        output.push_str(&event_body_text(event));
        output.push_str("\n\n");
    }
    output
}

fn event_actor_label(event: &ManagedProjectEvent) -> String {
    format_role_label(event.role.as_deref().unwrap_or("tt"))
}

fn format_role_label(role: &str) -> String {
    match role {
        "tt" => "TT".to_string(),
        "dev" => "Dev".to_string(),
        "test" => "Test".to_string(),
        "integration" => "Integration".to_string(),
        "director" => "Director".to_string(),
        "operator" => "Operator".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => "TT".to_string(),
            }
        }
    }
}

fn event_body_text(event: &ManagedProjectEvent) -> String {
    match event.kind {
        ManagedProjectEventKind::PromptSent | ManagedProjectEventKind::ResponseReceived => {
            event.text.clone()
        }
        ManagedProjectEventKind::ParseFailed => {
            let error = event.error.as_deref().unwrap_or("<unknown parse error>");
            format!("{}\n\nParse error: {}", event.text, error)
        }
        ManagedProjectEventKind::TurnFailed => {
            let error = event.error.as_deref().unwrap_or("<unknown error>");
            format!("{}\n\nError: {}", event.text, error)
        }
        ManagedProjectEventKind::PhaseChanged | ManagedProjectEventKind::SystemNote => {
            if let Some(status) = event.status.as_deref() {
                format!("{}\n\nStatus: {}", event.text, status)
            } else {
                event.text.clone()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeState {
    Ready,
    NeedsAuth,
    Unreachable,
}

fn codex_doctor_for_cwd(cwd: &Path, check_listen: bool) -> Option<tt_daemon::CodexDoctorReport> {
    request_for_cwd(
        cwd,
        DaemonRequest::DoctorCodex {
            cwd: cwd.to_path_buf(),
            check_listen,
        },
    )
    .ok()
    .and_then(|response| match response {
        DaemonResponse::CodexDoctor(report) => Some(report),
        _ => None,
    })
}

fn runtime_state_for_cwd(cwd: &Path) -> RuntimeState {
    let codex_doctor = codex_doctor_for_cwd(cwd, false);
    if codex_doctor.as_ref().and_then(|report| report.auth_present) == Some(false) {
        return RuntimeState::NeedsAuth;
    }

    let runtime_ready = request_for_cwd(
        cwd,
        DaemonRequest::InspectCodexAppServers {
            cwd: cwd.to_path_buf(),
        },
    )
    .ok()
    .and_then(|response| match response {
        DaemonResponse::CodexAppServers(servers) => {
            servers.first().map(|server| server.listen_reachable)
        }
        _ => None,
    })
    .unwrap_or(false);

    if runtime_ready {
        RuntimeState::Ready
    } else {
        RuntimeState::Unreachable
    }
}

fn request_cwd_for_command(cwd: &Path, command: &Command) -> PathBuf {
    match command {
        Command::Init { path, .. } => path
            .as_ref()
            .map(|path| resolve_cli_path(cwd, path.clone()))
            .unwrap_or_else(|| cwd.to_path_buf()),
        _ => cwd.to_path_buf(),
    }
}

fn command_to_request(command: Command, cwd: &Path) -> Result<DaemonRequest> {
    Ok(match command {
        Command::Init {
            path,
            title,
            objective,
            template,
            base_branch,
            worktree_root,
            director_model,
            dev_model,
            test_model,
            integration_model,
        } => DaemonRequest::InitManagedProject {
            path: path
                .map(|path| resolve_cli_path(cwd, path))
                .unwrap_or_else(|| cwd.to_path_buf()),
            title,
            objective,
            template,
            base_branch,
            worktree_root,
            director_model,
            dev_model,
            test_model,
            integration_model,
        },
        Command::Open {
            title,
            objective,
            base_branch,
            worktree_root,
            director_model,
            dev_model,
            test_model,
            integration_model,
        } => DaemonRequest::DirectManagedProject {
            cwd: cwd.to_path_buf(),
            title,
            objective,
            base_branch,
            worktree_root,
            director_model,
            dev_model,
            test_model,
            integration_model,
            roles: None,
            bindings: vec![],
            scenario: None,
            seed_file: None,
        },
        Command::Docs { .. } => anyhow::bail!("docs commands are handled locally"),
        Command::Status { .. } => DaemonRequest::Status {
            cwd: cwd.to_path_buf(),
        },
        Command::Clean { all } => DaemonRequest::CleanManagedProject {
            cwd: cwd.to_path_buf(),
            force: all,
        },
        Command::Events { .. } => anyhow::bail!("events commands are handled locally"),
        Command::Internal { command } => match command {
            InternalCommand::Repo => DaemonRequest::RepositorySummary {
                cwd: cwd.to_path_buf(),
            },
            InternalCommand::Project { command } => match command {
                InternalProjectCommand::Inspect => DaemonRequest::InspectManagedProject {
                    cwd: cwd.to_path_buf(),
                },
                InternalProjectCommand::Plan { command } => match command {
                    ProjectPlanCommand::Show => DaemonRequest::InspectManagedProjectPlan {
                        cwd: cwd.to_path_buf(),
                    },
                    ProjectPlanCommand::Refresh => DaemonRequest::RefreshManagedProjectPlan {
                        cwd: cwd.to_path_buf(),
                    },
                },
                InternalProjectCommand::Director {
                    title,
                    objective,
                    base_branch,
                    worktree_root,
                    director_model,
                    dev_model,
                    test_model,
                    integration_model,
                    role,
                    binding,
                    scenario,
                    seed_file,
                } => DaemonRequest::DirectManagedProject {
                    cwd: cwd.to_path_buf(),
                    title,
                    objective,
                    base_branch,
                    worktree_root,
                    director_model,
                    dev_model,
                    test_model,
                    integration_model,
                    roles: if role.is_empty() {
                        None
                    } else {
                        Some(parse_thread_roles(&role)?)
                    },
                    bindings: parse_thread_bindings(&binding)?,
                    scenario,
                    seed_file: seed_file.map(|path| resolve_cli_path(cwd, path)),
                },
                InternalProjectCommand::Control { role, mode } => {
                    DaemonRequest::SetManagedProjectThreadControl {
                        cwd: cwd.to_path_buf(),
                        role: ThreadRole::from_str(&role)
                            .map_err(|error| anyhow::anyhow!(error))?,
                        mode: parse_thread_control_mode(&mode)?,
                    }
                }
                InternalProjectCommand::Spawn { role } => DaemonRequest::SpawnManagedProject {
                    cwd: cwd.to_path_buf(),
                    roles: if role.is_empty() {
                        None
                    } else {
                        Some(parse_thread_roles(&role)?)
                    },
                },
                InternalProjectCommand::Attach { binding } => DaemonRequest::AttachManagedProject {
                    cwd: cwd.to_path_buf(),
                    bindings: parse_thread_bindings(&binding)?,
                },
            },
        },
    })
}

fn rewrite_open_runtime_error(
    cwd: &Path,
    is_open_command: bool,
    error: anyhow::Error,
) -> anyhow::Error {
    if !is_open_command {
        return error;
    }
    let error_text = format!("{error:#}");
    if !error_text.contains("connect to Codex app-server") {
        return error;
    }
    anyhow::anyhow!(
        "cannot open TT project in {} because the project runtime could not be started.\nRun `tt status` for the persisted project snapshot.\n\nOriginal error:\n{error_text}",
        cwd.display()
    )
}

fn render_cli_reference_markdown() -> String {
    let command = Cli::command().name("tt");
    let mut output = String::new();
    output.push_str("# TT CLI Reference\n\n");
    output.push_str("_Generated from the current `tt` Clap command tree._\n\n");
    render_command_markdown(&command, &["tt".to_string()], &mut output);
    output
}

fn render_command_markdown(command: &clap::Command, path: &[String], output: &mut String) {
    let depth = path.len().saturating_sub(1).min(5);
    let heading = "#".repeat(depth + 2);
    output.push_str(&format!("{heading} `{}`\n\n", path.join(" ")));

    if let Some(about) = command.get_about() {
        output.push_str(&format!("{about}\n\n"));
    }

    output.push_str("**Usage**\n\n```text\n");
    let mut usage_command = command.clone();
    output.push_str(&usage_command.render_usage().to_string());
    output.push_str("\n```\n\n");

    let subcommands: Vec<_> = command
        .get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set() && subcommand.get_name() != "help")
        .collect();
    if !subcommands.is_empty() {
        output.push_str("**Subcommands**\n\n");
        for subcommand in &subcommands {
            let mut names = vec![format!("`{}`", subcommand.get_name())];
            let mut aliases = subcommand.get_visible_aliases();
            if let Some(alias) = aliases.next() {
                names.push(format!("alias: `{alias}`"));
            }
            output.push_str(&format!("- {}\n", names.join(" ")));
        }
        output.push('\n');
    }

    let arguments: Vec<_> = command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set())
        .collect();
    if !arguments.is_empty() {
        output.push_str("**Arguments**\n\n");
        for arg in arguments {
            output.push_str(&format_argument_markdown(arg));
        }
        output.push('\n');
    }

    for subcommand in subcommands {
        let mut child_path = path.to_vec();
        child_path.push(subcommand.get_name().to_string());
        render_command_markdown(subcommand, &child_path, output);
    }
}

fn format_argument_markdown(arg: &Arg) -> String {
    let mut line = String::from("- ");
    if let Some(short) = arg.get_short() {
        line.push_str(&format!("`-{short}`"));
        if arg.get_long().is_some() {
            line.push_str(", ");
        }
    }
    if let Some(long) = arg.get_long() {
        line.push_str(&format!("`--{long}`"));
    }
    if arg.get_short().is_none() && arg.get_long().is_none() {
        line.push_str(&format!("`{}`", arg.get_id()));
    }
    let takes_value = !matches!(
        arg.get_action(),
        ArgAction::SetTrue
            | ArgAction::SetFalse
            | ArgAction::Count
            | ArgAction::Help
            | ArgAction::Version
    );
    if takes_value {
        if let Some(value_names) = arg.get_value_names() {
            for value_name in value_names {
                line.push_str(&format!(" `<{}>`", value_name));
            }
        }
    }
    if !arg.is_required_set() {
        line.push_str(" (optional)");
    }
    if let Some(help) = arg.get_help() {
        line.push_str(&format!(": {help}"));
    }
    line.push('\n');
    line
}

fn render_response(response: &DaemonResponse) -> String {
    match response {
        DaemonResponse::Unit => "ok".to_string(),
        DaemonResponse::Count(count) => format!("updated {count}"),
        DaemonResponse::Doctor(report) => format!(
            "doctor\ncwd: {}\ntt_cli_generation: {}\ndaemon_api_version: {}\ntt_project_root: {}\ncodex_project_root: {}\ndaemon_socket: {}\ncodex_auth_json: {}\ncodex_contract_ok: {}\ncodex_listen_url: {}\ncodex_listen_reachable: {}\ncodex_listen_error: {}\ncodex_error: {}\n",
            report.cwd.display(),
            report.tt_cli_generation,
            report.daemon_api_version,
            report
                .tt_project_root
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            report
                .codex_project_root
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            report.daemon_socket_path.display(),
            report
                .codex_auth_json
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unresolved>".to_string()),
            report.codex_contract_ok,
            report.codex_listen_url,
            report
                .codex_listen_reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<not-checked>".to_string()),
            report.codex_listen_error.as_deref().unwrap_or("<none>"),
            report.codex_error.as_deref().unwrap_or("<none>")
        ),
        DaemonResponse::CodexDoctor(report) => format!(
            "codex doctor\ncontract_ok: {}\ncodex_bin: {}\ncodex_version: {}\napp_server_bin: {}\napp_server_version: {}\nauth_json: {}\nlisten_url: {}\nlisten_reachable: {}\nlisten_error: {}\ncodex_home: {}\nerror: {}\n",
            report.contract_ok,
            report
                .codex_bin
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unresolved>".to_string()),
            report.codex_version.as_deref().unwrap_or("<unknown>"),
            report
                .app_server_bin
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unresolved>".to_string()),
            report.app_server_version.as_deref().unwrap_or("<unknown>"),
            report
                .auth_json
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unresolved>".to_string()),
            report.configured_listen_url,
            report
                .listen_reachable
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<not-checked>".to_string()),
            report.listen_error.as_deref().unwrap_or("<none>"),
            report
                .codex_home
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unresolved>".to_string()),
            report.error.as_deref().unwrap_or("<none>")
        ),
        DaemonResponse::Status(status) => {
            render_status_response(status, RuntimeState::Unreachable, false)
        }
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
        DaemonResponse::CodexAppServers(servers) => format!(
            "{}",
            servers
                .iter()
                .map(|server| {
                    format!(
                        "repo={}\ndaemon_socket={}\ndaemon_socket_exists={}\ndaemon_socket_reachable={}\nlisten_url={}\nlisten_source={}\nlisten_reachable={}\nlisten_error={}\nnote={}\n",
                        server.repo_root.display(),
                        server.daemon_socket_path.display(),
                        server.daemon_socket_exists,
                        server.daemon_socket_reachable,
                        server.configured_listen_url,
                        server.source,
                        server.listen_reachable,
                        server.listen_error.as_deref().unwrap_or("-"),
                        server.note.as_deref().unwrap_or("-"),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        ),
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
            "{}\n{}\nstatus={}\ncwd={}\npreview={}\nmodel_provider={}\nephemeral={}\nupdated_at={}\nturn_count={}\nlatest_turn_id={}\nlatest_turn_status={}\nlatest_turn_error={}\nlatest_turn_summary={}\nbound_work_unit_id={}\nworkspace_binding_count={}\n",
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
            thread.latest_turn_status.as_deref().unwrap_or("-"),
            thread.latest_turn_error.as_deref().unwrap_or("-"),
            thread.latest_turn_summary.as_deref().unwrap_or("-"),
            thread.bound_work_unit_id.as_deref().unwrap_or("<unbound>"),
            thread.workspace_binding_count
        ),
        DaemonResponse::CodexThreadDetail(None) => "codex thread not found".to_string(),
        DaemonResponse::ManagedProject(bootstrap) => render_managed_project_bootstrap(bootstrap),
        DaemonResponse::ManagedProjectInspection(inspection) => {
            render_managed_project_inspection(inspection)
        }
        DaemonResponse::ManagedProjectPlan(inspection) => render_managed_project_plan(inspection),
        DaemonResponse::ManagedProjectThreadControl(inspection) => {
            render_managed_project_inspection(inspection)
        }
    }
}

fn render_status_response(
    status: &tt_daemon::DaemonStatus,
    runtime_state: RuntimeState,
    colorize: bool,
) -> String {
    let project_label = if status.project_initialized {
        "Initialized"
    } else {
        "Uninitialized"
    };
    let runtime_label = match runtime_state {
        RuntimeState::Ready => "Ready",
        RuntimeState::NeedsAuth => "NeedsAuth",
        RuntimeState::Unreachable => "Unreachable",
    };
    let repo_value = status
        .repo_root
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<none>".to_string());
    let project_value = if colorize {
        if status.project_initialized {
            "\u{1b}[1;32mInitialized\u{1b}[0m".to_string()
        } else {
            "\u{1b}[1;31mUninitialized\u{1b}[0m".to_string()
        }
    } else {
        project_label.to_string()
    };
    let runtime_value = if colorize {
        match runtime_state {
            RuntimeState::Ready => "\u{1b}[1;32mReady\u{1b}[0m".to_string(),
            RuntimeState::NeedsAuth => "\u{1b}[1;33mNeedsAuth\u{1b}[0m".to_string(),
            RuntimeState::Unreachable => "\u{1b}[1;31mUnreachable\u{1b}[0m".to_string(),
        }
    } else {
        runtime_label.to_string()
    };
    let director_label = match status.director_state {
        ManagedProjectDirectorState::Ready => "Ready",
        ManagedProjectDirectorState::Starting => "Starting",
        ManagedProjectDirectorState::Blocked => "Blocked",
        ManagedProjectDirectorState::Missing => "Missing",
    };
    let director_value = if colorize {
        match status.director_state {
            ManagedProjectDirectorState::Ready => "\u{1b}[1;32mReady\u{1b}[0m".to_string(),
            ManagedProjectDirectorState::Starting => "\u{1b}[1;33mStarting\u{1b}[0m".to_string(),
            ManagedProjectDirectorState::Blocked => "\u{1b}[1;31mBlocked\u{1b}[0m".to_string(),
            ManagedProjectDirectorState::Missing => "\u{1b}[1;31mMissing\u{1b}[0m".to_string(),
        }
    } else {
        director_label.to_string()
    };
    let repo_value = if colorize {
        format!("\u{1b}[1;37m{repo_value}\u{1b}[0m")
    } else {
        repo_value
    };
    format!(
        "project={project_value} runtime={runtime_value} director={director_value} repo={repo_value}\n",
    )
}

fn render_status_json(status: &tt_daemon::DaemonStatus, runtime_state: RuntimeState) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "project": if status.project_initialized { "Initialized" } else { "Uninitialized" },
        "runtime": match runtime_state {
            RuntimeState::Ready => "Ready",
            RuntimeState::NeedsAuth => "NeedsAuth",
            RuntimeState::Unreachable => "Unreachable",
        },
        "director": match status.director_state {
            ManagedProjectDirectorState::Ready => "Ready",
            ManagedProjectDirectorState::Starting => "Starting",
            ManagedProjectDirectorState::Blocked => "Blocked",
            ManagedProjectDirectorState::Missing => "Missing",
        },
        "repo": status
            .repo_root
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
    }))
    .expect("serialize status")
}

fn ensure_project_initialized_for_open(cwd: &Path) -> Result<()> {
    let response = request_for_cwd(
        cwd,
        DaemonRequest::Status {
            cwd: cwd.to_path_buf(),
        },
    )?;
    let DaemonResponse::Status(status) = response else {
        anyhow::bail!("unexpected daemon response while checking project initialization");
    };
    if status.project_initialized {
        Ok(())
    } else {
        anyhow::bail!(
            "fatal: not a TT project (or any of the parent directories): .tt\nRun `tt init` to initialize this repository."
        )
    }
}

fn render_managed_project_plan(inspection: &tt_daemon::ManagedProjectInspection) -> String {
    let mut output = String::new();
    output.push_str("managed project plan\n");
    output.push_str("===================\n");
    output.push_str(&format!(
        "Project: {} ({})\n",
        inspection.bootstrap.project.title, inspection.bootstrap.project.slug
    ));
    output.push_str(&format!(
        "Plan file: {}\n",
        inspection.bootstrap.plan_path.display()
    ));
    output.push_str(&format!("Status: {}\n", inspection.bootstrap.plan.status));
    output.push_str(&format!(
        "Milestones: {}\n",
        inspection.bootstrap.plan.milestones.len()
    ));
    for milestone in &inspection.bootstrap.plan.milestones {
        output.push_str(&format!(
            "- {}: {} (criteria: {})\n",
            milestone.id,
            milestone.title,
            milestone.success_criteria.join(" | ")
        ));
    }
    output.push_str(&format!(
        "Work items: {}\n",
        inspection.bootstrap.plan.work_items.len()
    ));
    for work_item in &inspection.bootstrap.plan.work_items {
        output.push_str(&format!(
            "- {} [{}] owner={} phase={} commit_required={} status={}\n",
            work_item.id,
            work_item.title,
            work_item.owner_role,
            work_item.phase,
            work_item.commit_required,
            work_item.status
        ));
        if !work_item.depends_on.is_empty() {
            output.push_str(&format!(
                "  depends_on: {}\n",
                work_item.depends_on.join(", ")
            ));
        }
        if !work_item.acceptance_criteria.is_empty() {
            output.push_str(&format!(
                "  acceptance: {}\n",
                work_item.acceptance_criteria.join(" | ")
            ));
        }
        if !work_item.validation_commands.is_empty() {
            output.push_str(&format!(
                "  validation: {}\n",
                work_item.validation_commands.join(" ; ")
            ));
        }
    }
    if !inspection.bootstrap.plan.notes.risks.is_empty() {
        output.push_str(&format!(
            "Risks: {}\n",
            inspection.bootstrap.plan.notes.risks.join(" | ")
        ));
    }
    if !inspection.bootstrap.plan.notes.pitfalls.is_empty() {
        output.push_str(&format!(
            "Pitfalls: {}\n",
            inspection.bootstrap.plan.notes.pitfalls.join(" | ")
        ));
    }
    if !inspection.bootstrap.plan.notes.open_questions.is_empty() {
        output.push_str(&format!(
            "Open questions: {}\n",
            inspection.bootstrap.plan.notes.open_questions.join(" | ")
        ));
    }
    if !inspection
        .bootstrap
        .plan
        .notes
        .operator_constraints
        .is_empty()
    {
        output.push_str(&format!(
            "Operator constraints: {}\n",
            inspection
                .bootstrap
                .plan
                .notes
                .operator_constraints
                .join(" | ")
        ));
    }
    if let Some(repository) = inspection.repository_summary.as_ref() {
        output.push_str("\nRepository\n");
        output.push_str("----------\n");
        output.push_str(&format!("root: {}\n", repository.repository_root));
        output.push_str(&format!(
            "branch: {}\n",
            repository.current_branch.as_deref().unwrap_or("<detached>")
        ));
        output.push_str(&format!(
            "head: {}\n",
            repository
                .current_head_commit
                .as_deref()
                .unwrap_or("<unknown>")
        ));
    }
    output
}

fn render_managed_project_inspection(inspection: &tt_daemon::ManagedProjectInspection) -> String {
    let mut output = render_managed_project_bootstrap(&inspection.bootstrap);
    if let Some(repository) = inspection.repository_summary.as_ref() {
        output.push_str("\nRepository\n");
        output.push_str("----------\n");
        output.push_str(&format!("root: {}\n", repository.repository_root));
        output.push_str(&format!(
            "worktree: {}\n",
            repository.current_worktree.as_deref().unwrap_or("<unset>")
        ));
        output.push_str(&format!(
            "branch: {}\n",
            repository.current_branch.as_deref().unwrap_or("<detached>")
        ));
        output.push_str(&format!(
            "head: {}\n",
            repository
                .current_head_commit
                .as_deref()
                .unwrap_or("<unknown>")
        ));
        output.push_str(&format!("dirty: {}\n", repository.dirty));
        output.push_str(&format!("merge-ready: {}\n", repository.merge_ready));
        output.push_str(&format!("worktrees: {}\n", repository.worktree_count));
    }
    output
}

fn render_managed_project_bootstrap(bootstrap: &tt_daemon::ManagedProjectBootstrap) -> String {
    let mut output = String::new();
    output.push_str("managed project\n");
    output.push_str("===============\n");
    output.push_str(&format!(
        "Project: {} ({})\n",
        bootstrap.project.title, bootstrap.project.slug
    ));
    output.push_str(&format!("Repo root: {}\n", bootstrap.repo_root.display()));
    output.push_str(&format!("Base branch: {}\n", bootstrap.base_branch));
    output.push_str(&format!(
        "Worktree root: {}\n",
        bootstrap.worktree_root.display()
    ));
    output.push_str(&format!(
        "Manifest: {}\n",
        bootstrap.manifest_path.display()
    ));
    output.push_str(&format!(
        "Contract: {}\n",
        bootstrap.contract_path.display()
    ));
    output.push_str(&format!(
        "Codex config: {}\n",
        bootstrap.codex_config_path.display()
    ));
    output.push_str(&format!(
        "Project config: {}\n",
        bootstrap.project_config_path.display()
    ));
    output.push_str(&format!("Plan: {}\n", bootstrap.plan_path.display()));
    output.push_str(&format!(
        "Plan summary: status={} milestones={} work_items={} commit_policy={} plan_first={}\n",
        bootstrap.plan.status,
        bootstrap.plan.milestones.len(),
        bootstrap.plan.work_items.len(),
        bootstrap.project_config.commit_policy,
        bootstrap.project_config.plan_first
    ));
    output.push_str(&format!(
        "Liveness config: expected_long_build={} progress_updates_required={} soft_silence_seconds={} hard_ceiling_seconds={}\n",
        bootstrap.project_config.expected_long_build,
        bootstrap.project_config.require_progress_updates,
        bootstrap.project_config.soft_silence_seconds,
        bootstrap.project_config.hard_ceiling_seconds
    ));
    if let Some(tt_runtime_bin) = bootstrap.project_config.tt_runtime_bin.as_deref() {
        output.push_str(&format!("TT runtime bin: {}\n", tt_runtime_bin));
    }
    if !bootstrap
        .project_config
        .default_validation_commands
        .is_empty()
    {
        output.push_str(&format!(
            "Default validation: {}\n",
            bootstrap
                .project_config
                .default_validation_commands
                .join(", ")
        ));
    }
    if !bootstrap.project_config.pitfalls.is_empty() {
        output.push_str(&format!(
            "Pitfalls: {}\n",
            bootstrap.project_config.pitfalls.join(" | ")
        ));
    }
    output.push_str("\nStartup\n");
    output.push_str("-------\n");
    output.push_str(&format!(
        "phase: {}\n",
        match bootstrap.startup.phase {
            tt_daemon::ManagedProjectStartupPhase::Scaffolded => "scaffolded",
            tt_daemon::ManagedProjectStartupPhase::ThreadsStarted => "threads_started",
            tt_daemon::ManagedProjectStartupPhase::WorkerReportsPending => {
                "worker_reports_pending"
            }
            tt_daemon::ManagedProjectStartupPhase::DirectorAckPending => "director_ack_pending",
            tt_daemon::ManagedProjectStartupPhase::Ready => "ready",
            tt_daemon::ManagedProjectStartupPhase::Blocked => "blocked",
        }
    ));
    output.push_str(&format!(
        "updated_at: {}\n",
        bootstrap.startup.updated_at.to_rfc3339()
    ));
    for role in ["dev", "test", "integration"] {
        if let Some(report) = bootstrap.startup.worker_reports.get(role) {
            output.push_str(&format!(
                "{}: status={} turn={} summary={}\n",
                role,
                match report.status {
                    tt_daemon::ManagedProjectStartupRoleStatus::NotStarted => "not_started",
                    tt_daemon::ManagedProjectStartupRoleStatus::Pending => "pending",
                    tt_daemon::ManagedProjectStartupRoleStatus::Reported => "reported",
                    tt_daemon::ManagedProjectStartupRoleStatus::Blocked => "blocked",
                },
                report.turn_id.as_deref().unwrap_or("<none>"),
                report.summary.as_deref().unwrap_or("<none>")
            ));
        }
    }
    if let Some(ack) = bootstrap.startup.director_ack.as_ref() {
        output.push_str(&format!(
            "director_ack: status={} turn={} summary={}\n",
            match ack.status {
                tt_daemon::ManagedProjectStartupAckStatus::Ready => "ready",
                tt_daemon::ManagedProjectStartupAckStatus::Blocked => "blocked",
            },
            ack.turn_id.as_deref().unwrap_or("<none>"),
            ack.summary
        ));
    }
    if let Some(scenario) = bootstrap.scenario.as_ref() {
        let progress_stream = bootstrap
            .repo_root
            .join(".tt")
            .join("scenarios")
            .join(&scenario.scenario_id)
            .join("progress.jsonl");
        output.push_str("\nScenario\n");
        output.push_str("--------\n");
        output.push_str(&format!("id: {}\n", scenario.scenario_id));
        output.push_str(&format!("kind: {}\n", scenario.scenario_kind));
        output.push_str(&format!("phase: {}\n", scenario.current_phase));
        output.push_str(&format!("round: {}\n", scenario.current_round));
        output.push_str(&format!("completed: {}\n", scenario.completed));
        output.push_str(&format!(
                "liveness_policy: expected_long_build={} progress_updates_required={} soft_silence_seconds={} hard_ceiling_seconds={}\n",
                scenario.liveness_policy.expected_long_build,
                scenario.liveness_policy.require_progress_updates,
                scenario.liveness_policy.soft_silence_seconds,
                scenario.liveness_policy.hard_ceiling_seconds
            ));
        output.push_str(&format!("progress_stream: {}\n", progress_stream.display()));
        if let Ok(stream_contents) = fs::read_to_string(&progress_stream) {
            output.push_str(&format!(
                "progress_events: {}\n",
                stream_contents.lines().count()
            ));
        }
        if let Some(approval) = scenario.pending_approval.as_ref() {
            output.push_str(&format!(
                "pending_approval: {} by {} approved={}\n",
                approval.approval_kind, approval.requested_by_role, approval.approved
            ));
        }
        if let Some(watchdog) = scenario.watchdog.as_ref() {
            output.push_str(&format!(
                "watchdog: state={} role={} round={} turn={} silence={}s signal={}\n",
                watchdog.state,
                watchdog.role.as_deref().unwrap_or("<none>"),
                watchdog
                    .round
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "<none>".to_string()),
                watchdog.turn_id.as_deref().unwrap_or("<none>"),
                watchdog.silence_seconds,
                watchdog.last_signal.as_deref().unwrap_or("<none>")
            ));
            output.push_str(&format!(
                "watchdog_summary: {} (last progress: {})\n",
                watchdog.state,
                watchdog.last_signal.as_deref().unwrap_or("<none>")
            ));
            output.push_str(&format!(
                "watchdog_detail: elapsed={}s status={} items={} log_size={} last_observed={} last_progress={}\n",
                watchdog.elapsed_seconds,
                watchdog.turn_status.as_deref().unwrap_or("<unknown>"),
                watchdog.turn_items,
                watchdog
                    .app_server_log_size
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "<none>".to_string()),
                watchdog
                    .last_observed_at
                    .map(|value| value.to_rfc3339())
                    .unwrap_or_else(|| "<none>".to_string()),
                watchdog
                    .last_progress_at
                    .map(|value| value.to_rfc3339())
                    .unwrap_or_else(|| "<none>".to_string()),
            ));
            if let Some(note) = watchdog.note.as_deref() {
                output.push_str(&format!("watchdog_note: {}\n", note));
            }
        }
        let fallback_rounds = scenario
            .rounds
            .iter()
            .flat_map(|round| round.role_handoffs.values())
            .filter(|handoff| handoff.handoff_source == "seeded_fallback")
            .count();
        output.push_str(&format!("fallback_handoffs: {}\n", fallback_rounds));
        output.push_str(&format!(
            "strict_extraction_ready: {}\n",
            if fallback_rounds == 0 {
                "true"
            } else {
                "false"
            }
        ));
        if let Some(round) = scenario.rounds.last() {
            output.push_str(&format!(
                "latest_round_summary: round {} {}\n",
                round.round_number, round.phase
            ));
            for role in ["dev", "test", "integration"] {
                if let Some(handoff) = round.role_handoffs.get(role) {
                    let blockers = if handoff.blockers.is_empty() {
                        "<none>".to_string()
                    } else {
                        handoff.blockers.join(" | ")
                    };
                    output.push_str(&format!(
                        "  {}: source={} status={} blockers={}\n",
                        role,
                        handoff.handoff_source,
                        handoff.status.as_deref().unwrap_or("<unknown>"),
                        blockers
                    ));
                }
            }
        }
    }
    output.push_str("\nRoles\n");
    output.push_str("-----\n");
    let attached_roles = bootstrap
        .roles
        .iter()
        .filter(|role| role.thread_id.is_some())
        .count();
    output.push_str(&format!(
        "state: {} ({}/{})\n\n",
        if attached_roles == 0 {
            "scaffolded"
        } else if attached_roles == bootstrap.roles.len() {
            "attached"
        } else {
            "partial"
        },
        attached_roles,
        bootstrap.roles.len()
    ));
    for role in &bootstrap.roles {
        output.push_str(&format!(
            "{} | work-unit={} | branch={} | worktree={} | agent={} | thread={} | control={} | workspace={}\n",
            role_name(role.role),
            role.work_unit.id,
            role.branch_name.as_deref().unwrap_or("<none>"),
            role.worktree_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            role.agent_path.display(),
            role.thread_id.as_deref().unwrap_or("<none>"),
            role.control_mode,
            role.workspace_binding_id.as_deref().unwrap_or("<none>"),
        ));
    }
    output
}

fn resolve_cli_path(cwd: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn should_launch_codex_tui() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

#[cfg(test)]
impl CodexLoginMode {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "interactive" => Ok(Self::Interactive),
            "device-auth" | "device_auth" => Ok(Self::DeviceAuth),
            "manual" => Ok(Self::Manual),
            other => anyhow::bail!(
                "unknown {} value `{}`; expected auto, interactive, device-auth, or manual",
                TT_CODEX_LOGIN_MODE_ENV,
                other
            ),
        }
    }
}

#[cfg(test)]
fn codex_login_mode_for_cwd(cwd: &Path) -> Result<CodexLoginMode> {
    load_repo_settings_env(cwd)?;
    repo_env_var(TT_CODEX_LOGIN_MODE_ENV)
        .as_deref()
        .map(CodexLoginMode::parse)
        .transpose()
        .map(|mode| mode.unwrap_or(CodexLoginMode::Auto))
}

#[cfg(test)]
fn codex_login_args(mode: CodexLoginMode, interactive_terminal: bool) -> Vec<&'static str> {
    match mode {
        CodexLoginMode::Auto => {
            if interactive_terminal {
                vec!["login"]
            } else {
                vec!["login", "--device-auth"]
            }
        }
        CodexLoginMode::Interactive => vec!["login"],
        CodexLoginMode::DeviceAuth => vec!["login", "--device-auth"],
        CodexLoginMode::Manual => {
            if interactive_terminal {
                vec!["login"]
            } else {
                vec!["login", "--device-auth"]
            }
        }
    }
}

#[cfg(test)]
fn render_codex_login_command(codex_home: &Path, codex_bin: &Path, args: &[&str]) -> String {
    let suffix = if args.is_empty() {
        String::new()
    } else {
        format!(" {}", args.join(" "))
    };
    format!(
        "CODEX_HOME={} {}{}",
        codex_home.display(),
        codex_bin.display(),
        suffix
    )
}

fn launch_codex_tui_for_director(
    cwd: &Path,
    bootstrap: &tt_daemon::ManagedProjectBootstrap,
) -> Result<()> {
    let director_thread_id = bootstrap
        .roles
        .iter()
        .find(|role| role.role == ThreadRole::Director)
        .and_then(|role| role.thread_id.as_deref())
        .ok_or_else(|| anyhow::anyhow!("managed project director thread is not attached"))?;
    let codex_doctor = codex_doctor_for_cwd(cwd, true)
        .ok_or_else(|| anyhow::anyhow!("could not resolve Codex runtime launch info"))?;
    if !codex_doctor.contract_ok {
        anyhow::bail!(
            "cannot open TT project in {} because the Codex runtime contract is not satisfied",
            cwd.display()
        );
    }
    let codex_bin = codex_doctor
        .codex_bin
        .ok_or_else(|| anyhow::anyhow!("Codex CLI binary is unavailable"))?;
    let codex_home = managed_project_codex_home(&bootstrap.repo_root);
    let mut command =
        build_codex_resume_command(&codex_bin, &bootstrap.repo_root, Some(director_thread_id));
    command.env("CODEX_HOME", codex_home);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        Err(error).context(format!(
            "exec Codex TUI `{}` for director thread {}",
            codex_bin.display(),
            director_thread_id
        ))
    }

    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .with_context(|| format!("launch Codex TUI `{}`", codex_bin.display()))?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!(
                "Codex TUI `{}` exited unsuccessfully while opening director thread {}",
                codex_bin.display(),
                director_thread_id
            );
        }
    }
}

fn build_codex_resume_command(
    codex_bin: &Path,
    repo_root: &Path,
    thread_id: Option<&str>,
) -> ProcessCommand {
    let mut command = ProcessCommand::new(codex_bin);
    command.arg("--cd").arg(repo_root).arg("resume");
    if let Some(thread_id) = thread_id {
        command.arg(thread_id);
    }
    command
}

fn role_name(role: ThreadRole) -> &'static str {
    match role {
        ThreadRole::Director => "director",
        ThreadRole::Develop => "dev",
        ThreadRole::Test => "test",
        ThreadRole::Integrate => "integration",
        ThreadRole::Review => "review",
        ThreadRole::Todo => "todo",
        ThreadRole::Chat => "chat",
        ThreadRole::Learn => "learn",
        ThreadRole::Handoff => "handoff",
        ThreadRole::Custom => "custom",
    }
}

fn parse_thread_roles(values: &[String]) -> Result<Vec<ThreadRole>> {
    values
        .iter()
        .map(|value| ThreadRole::from_str(value).map_err(|error| anyhow::anyhow!(error)))
        .collect()
}

fn parse_thread_control_mode(value: &str) -> Result<ManagedProjectThreadControlMode> {
    match value {
        "director" => Ok(ManagedProjectThreadControlMode::Director),
        "manual_next_turn" | "manual-next-turn" => {
            Ok(ManagedProjectThreadControlMode::ManualNextTurn)
        }
        "manual" => Ok(ManagedProjectThreadControlMode::Manual),
        "director_paused" | "director-paused" | "paused" => {
            Ok(ManagedProjectThreadControlMode::DirectorPaused)
        }
        other => anyhow::bail!(
            "unknown managed project thread control mode `{}`; expected director, manual_next_turn, manual, or director_paused",
            other
        ),
    }
}

fn parse_thread_bindings(values: &[String]) -> Result<Vec<ManagedProjectThreadAttachment>> {
    values
        .iter()
        .map(|value| {
            let Some((role_name, thread_id)) = value.split_once('=') else {
                anyhow::bail!("binding `{value}` must be formatted as role=thread_id");
            };
            Ok(ManagedProjectThreadAttachment {
                role: ThreadRole::from_str(role_name).map_err(|error| anyhow::anyhow!(error))?,
                thread_id: thread_id.to_string(),
            })
        })
        .collect()
}

pub fn run_from_args(args: impl IntoIterator<Item = String>) -> Result<()> {
    let cli = Cli::parse_from(args);
    if let Some(output) = local_command_output(&cli.command, &cli.cwd)? {
        print!("{output}");
        return Ok(());
    }
    let is_open_command = matches!(&cli.command, Command::Open { .. });
    let is_status_command = matches!(&cli.command, Command::Status { .. });
    let status_json = matches!(&cli.command, Command::Status { json: true });
    let cwd = cli.cwd.unwrap_or(std::env::current_dir()?);
    if is_open_command {
        ensure_project_initialized_for_open(&cwd)?;
    }
    let request_cwd = request_cwd_for_command(&cwd, &cli.command);
    let response = request_for_cwd(&request_cwd, command_to_request(cli.command, &cwd)?)
        .map_err(|error| rewrite_open_runtime_error(&cwd, is_open_command, error))?;
    if is_open_command && should_launch_codex_tui() {
        let DaemonResponse::ManagedProject(bootstrap) = response else {
            anyhow::bail!("tt open expected a managed project bootstrap");
        };
        launch_codex_tui_for_director(&cwd, &bootstrap)?;
        return Ok(());
    }
    let output = match (is_status_command, response) {
        (true, DaemonResponse::Status(status)) => {
            let runtime_state = runtime_state_for_cwd(&cwd);
            if status_json {
                render_status_json(&status, runtime_state)
            } else {
                render_status_response(&status, runtime_state, std::io::stdout().is_terminal())
            }
        }
        (_, response) => serde_json::to_string_pretty(&response)?,
    };
    println!("{output}");
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
    fn parses_open_command() {
        let cli = Cli::parse_from(["tt", "open", "--title", "Alpha"]);
        assert!(matches!(
            cli.command,
            Command::Open {
                title: Some(ref title),
                ..
            } if title == "Alpha"
        ));
    }

    #[test]
    fn parses_init_command_without_path() {
        let cli = Cli::parse_from(["tt", "init", "--title", "Alpha"]);
        assert!(matches!(
            cli.command,
            Command::Init {
                path: None,
                title: Some(ref title),
                ..
            } if title == "Alpha"
        ));
    }

    #[test]
    fn parses_internal_project_status_alias_for_inspect_command() {
        let cli = Cli::parse_from(["tt", "internal", "project", "status"]);
        assert!(matches!(
            cli.command,
            Command::Internal {
                command: InternalCommand::Project {
                    command: InternalProjectCommand::Inspect
                }
            }
        ));
    }

    #[test]
    fn parses_internal_project_plan_show_command() {
        let cli = Cli::parse_from(["tt", "internal", "project", "plan", "show"]);
        assert!(matches!(
            cli.command,
            Command::Internal {
                command: InternalCommand::Project {
                    command: InternalProjectCommand::Plan {
                        command: ProjectPlanCommand::Show
                    }
                }
            }
        ));
    }

    #[test]
    fn parses_internal_project_director_command() {
        let cli = Cli::parse_from([
            "tt",
            "internal",
            "project",
            "director",
            "--role",
            "dev",
            "--binding",
            "director=thread-1",
        ]);
        assert!(matches!(
            cli.command,
            Command::Internal {
                command: InternalCommand::Project {
                    command: InternalProjectCommand::Director { ref role, ref binding, .. }
                }
            } if role == &vec!["dev".to_string()] && binding == &vec!["director=thread-1".to_string()]
        ));
    }

    #[test]
    fn project_init_without_path_uses_cwd() {
        let cwd = Path::new("/repo");
        let request = command_to_request(
            Command::Init {
                path: None,
                title: Some("Alpha".into()),
                objective: None,
                template: None,
                base_branch: None,
                worktree_root: None,
                director_model: None,
                dev_model: None,
                test_model: None,
                integration_model: None,
            },
            cwd,
        )
        .expect("request");

        match request {
            DaemonRequest::InitManagedProject { path, title, .. } => {
                assert_eq!(path, PathBuf::from("/repo"));
                assert_eq!(title.as_deref(), Some("Alpha"));
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn parses_internal_project_spawn_command() {
        let cli = Cli::parse_from([
            "tt", "internal", "project", "spawn", "--role", "dev", "--role", "test",
        ]);
        assert!(matches!(
            cli.command,
            Command::Internal {
                command: InternalCommand::Project {
                    command: InternalProjectCommand::Spawn { ref role }
                }
            } if role == &vec!["dev".to_string(), "test".to_string()]
        ));
    }

    #[test]
    fn parses_internal_project_attach_command() {
        let cli = Cli::parse_from([
            "tt",
            "internal",
            "project",
            "attach",
            "--binding",
            "dev=thread-123",
            "--binding",
            "test=thread-456",
        ]);
        assert!(matches!(
            cli.command,
            Command::Internal {
                command: InternalCommand::Project {
                    command: InternalProjectCommand::Attach { ref binding }
                }
            } if binding == &vec![
                "dev=thread-123".to_string(),
                "test=thread-456".to_string()
            ]
        ));
    }

    #[test]
    fn parses_cwd_flag() {
        let cli = Cli::parse_from(["tt", "--cwd", "/tmp", "status"]);
        assert!(matches!(cli.command, Command::Status { json: false }));
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

    #[test]
    fn renders_managed_project_inspection_details() {
        let project = Project {
            id: "p-inspect".into(),
            slug: "alpha".into(),
            title: "Alpha".into(),
            objective: "Ship".into(),
            status: ProjectStatus::Active,
            created_at: ts(),
            updated_at: ts(),
        };
        let roles = vec![
            tt_daemon::ManagedProjectRoleBootstrap {
                role: tt_domain::ThreadRole::Director,
                work_unit: tt_domain::WorkUnit {
                    id: "wu-director".into(),
                    project_id: project.id.clone(),
                    slug: Some("director".into()),
                    title: "Director".into(),
                    task: "Coordinate".into(),
                    status: tt_domain::WorkUnitStatus::Ready,
                    created_at: ts(),
                    updated_at: ts(),
                },
                agent_path: "/repo/.codex/agents/director.toml".into(),
                model: Some("director-model".into()),
                reasoning_effort: Some("medium".into()),
                control_mode: tt_daemon::ManagedProjectThreadControlMode::Director,
                branch_name: None,
                worktree_path: None,
                thread_id: None,
                thread_name: None,
                workspace_binding_id: None,
            },
            tt_daemon::ManagedProjectRoleBootstrap {
                role: tt_domain::ThreadRole::Develop,
                work_unit: tt_domain::WorkUnit {
                    id: "wu-dev".into(),
                    project_id: project.id.clone(),
                    slug: Some("dev".into()),
                    title: "Dev".into(),
                    task: "Implement".into(),
                    status: tt_domain::WorkUnitStatus::Ready,
                    created_at: ts(),
                    updated_at: ts(),
                },
                agent_path: "/repo/.codex/agents/dev.toml".into(),
                model: Some("dev-model".into()),
                reasoning_effort: Some("medium".into()),
                control_mode: tt_daemon::ManagedProjectThreadControlMode::Manual,
                branch_name: Some("tt/dev".into()),
                worktree_path: Some("/repo/.tt/worktrees/dev".into()),
                thread_id: Some("thread-1".into()),
                thread_name: Some("alpha-dev".into()),
                workspace_binding_id: Some("alpha:dev".into()),
            },
        ];
        let bootstrap = tt_daemon::ManagedProjectBootstrap {
            project,
            repo_root: "/repo".into(),
            base_branch: "main".into(),
            worktree_root: "/repo/.tt/worktrees".into(),
            manifest_path: "/repo/.tt/state.toml".into(),
            project_config_path: "/repo/.tt/project.toml".into(),
            plan_path: "/repo/.tt/plan.toml".into(),
            contract_path: "/repo/.tt/contract.md".into(),
            codex_config_path: "/repo/.codex/config.toml".into(),
            project_config: tt_daemon::ManagedProjectProjectConfig {
                schema: "tt-managed-project-config-v1".into(),
                title: "Alpha".into(),
                objective: "Ship".into(),
                base_branch: "main".into(),
                branch_prefix: "tt".into(),
                tt_runtime_bin: Some("./target/debug/tt-cli".into()),
                plan_first: true,
                commit_policy: "checkpoint-enforced".into(),
                require_operator_merge_approval: true,
                expected_long_build: true,
                require_progress_updates: true,
                soft_silence_seconds: 900,
                hard_ceiling_seconds: 7200,
                default_validation_commands: vec!["cargo test".into()],
                smoke_validation_commands: vec!["cargo check".into()],
                checkpoint_triggers: vec![
                    "after_plan".into(),
                    "after_develop".into(),
                    "after_test".into(),
                    "before_merge".into(),
                ],
                pitfalls: vec!["clean target directories are slow".into()],
                hints: vec!["use incremental builds".into()],
                exceptions: vec!["merge approval required".into()],
            },
            plan: tt_daemon::ManagedProjectPlan {
                schema: "tt-managed-project-plan-v1".into(),
                status: "draft".into(),
                objective: "Ship".into(),
                updated_at: ts().to_rfc3339(),
                milestones: vec![tt_daemon::ManagedProjectPlanMilestone {
                    id: "milestone-1".into(),
                    title: "Plan".into(),
                    success_criteria: vec!["plan exists".into()],
                    evidence: vec![],
                }],
                work_items: vec![tt_daemon::ManagedProjectPlanWorkItem {
                    id: "alpha-director".into(),
                    title: "Director".into(),
                    owner_role: "director".into(),
                    phase: "plan".into(),
                    depends_on: vec![],
                    acceptance_criteria: vec!["director has plan".into()],
                    validation_commands: vec!["cargo test".into()],
                    commit_required: false,
                    status: "planned".into(),
                }],
                notes: tt_daemon::ManagedProjectPlanNotes::default(),
            },
            startup: tt_daemon::ManagedProjectStartupState {
                phase: tt_daemon::ManagedProjectStartupPhase::Ready,
                updated_at: ts(),
                worker_reports: std::collections::BTreeMap::new(),
                director_ack: None,
            },
            scenario: Some(tt_daemon::ManagedProjectScenarioState {
                scenario_id: "scn-1".into(),
                scenario_kind: "rust-taskflow-four-round".into(),
                current_round: 4,
                current_phase: "completed".into(),
                liveness_policy: tt_daemon::ManagedProjectLivenessPolicy::default(),
                watchdog: Some(tt_daemon::ManagedProjectWatchdogState {
                    state: "healthy".into(),
                    last_signal: Some("worker progress".into()),
                    last_observed_at: Some(ts()),
                    last_progress_at: Some(ts()),
                    role: Some("dev".into()),
                    round: Some(4),
                    turn_id: Some("turn-dev".into()),
                    elapsed_seconds: 42,
                    silence_seconds: 0,
                    turn_status: Some("InProgress".into()),
                    turn_items: 8,
                    app_server_log_modified_at: Some(123),
                    app_server_log_size: Some(456),
                    note: Some("progress is moving".into()),
                }),
                operator_seed: "build taskflow".into(),
                pending_approval: Some(tt_daemon::ManagedProjectApprovalState {
                    approval_kind: "landing".into(),
                    requested_by_role: "director".into(),
                    prompt: "ready to land".into(),
                    approved: true,
                    response: Some("approved".into()),
                }),
                rounds: vec![tt_daemon::ManagedProjectRoundState {
                    round_number: 4,
                    phase: "merge".into(),
                    director_turn_id: Some("turn-4".into()),
                    director_summary: Some("finalize landing".into()),
                    role_handoffs: std::collections::BTreeMap::from([
                        (
                            "dev".into(),
                            tt_daemon::ManagedProjectRoleHandoff {
                                role: "dev".into(),
                                thread_id: "thread-1".into(),
                                turn_id: Some("turn-dev".into()),
                                prompt_summary: "dev prompt".into(),
                                handoff_summary: Some("{}".into()),
                                status: Some("complete".into()),
                                changed_files: vec!["src/lib.rs".into()],
                                tests_run: vec!["cargo test".into()],
                                blockers: vec![],
                                next_step: Some("wait".into()),
                                handoff_source: "extracted".into(),
                                handoff_parse_error: None,
                                raw_handoff_text: Some("{\"status\":\"complete\"}".into()),
                                completed: true,
                            },
                        ),
                        (
                            "test".into(),
                            tt_daemon::ManagedProjectRoleHandoff {
                                role: "test".into(),
                                thread_id: "thread-2".into(),
                                turn_id: Some("turn-test".into()),
                                prompt_summary: "test prompt".into(),
                                handoff_summary: Some("{}".into()),
                                status: Some("complete".into()),
                                changed_files: vec!["tests/taskflow.rs".into()],
                                tests_run: vec!["cargo test".into()],
                                blockers: vec![],
                                next_step: Some("report".into()),
                                handoff_source: "seeded_fallback".into(),
                                handoff_parse_error: Some("no agent message found".into()),
                                raw_handoff_text: None,
                                completed: true,
                            },
                        ),
                    ]),
                }],
                completed: true,
            }),
            roles,
        };
        let inspection = tt_daemon::ManagedProjectInspection {
            bootstrap,
            repository_summary: Some(tt_ui_core::GitRepositorySummary {
                repository_root: "/repo".into(),
                current_worktree: Some("/repo".into()),
                current_branch: Some("main".into()),
                current_head_commit: Some("abc123".into()),
                dirty: false,
                upstream: Some("origin/main".into()),
                ahead_by: Some(0),
                behind_by: Some(0),
                merge_ready: true,
                worktree_count: 3,
            }),
        };
        let text = render_response(&DaemonResponse::ManagedProjectInspection(inspection));
        assert!(text.contains("managed project"));
        assert!(text.contains("Project config: /repo/.tt/project.toml"));
        assert!(text.contains("Plan: /repo/.tt/plan.toml"));
        assert!(text.contains("Plan summary: status=draft milestones=1 work_items=1"));
        assert!(text.contains("TT runtime bin: ./target/debug/tt-cli"));
        assert!(text.contains("state: partial"));
        assert!(text.contains("Repository"));
        assert!(text.contains("merge-ready: true"));
        assert!(text.contains("dev"));
        assert!(text.contains("thread-1"));
        assert!(text.contains("liveness_policy: expected_long_build=false"));
        assert!(text.contains("progress_stream: /repo/.tt/scenarios/scn-1/progress.jsonl"));
        assert!(text.contains("watchdog: state=healthy"));
        assert!(text.contains("watchdog_summary: healthy"));
        assert!(
            text.contains("watchdog_detail: elapsed=42s status=InProgress items=8 log_size=456")
        );
        assert!(text.contains("watchdog_note: progress is moving"));
        assert!(text.contains("fallback_handoffs: 1"));
        assert!(text.contains("strict_extraction_ready: false"));
        assert!(text.contains("latest_round_summary: round 4 merge"));
        assert!(text.contains("dev: source=extracted status=complete"));
        assert!(text.contains("test: source=seeded_fallback status=complete"));
        assert!(text.contains("control=director"));
        assert!(text.contains("control=manual"));
    }

    #[test]
    fn parses_thread_control_mode_commands() {
        let mode = parse_thread_control_mode("manual_next_turn").expect("mode");
        assert_eq!(
            mode,
            tt_daemon::ManagedProjectThreadControlMode::ManualNextTurn
        );
        let mode = parse_thread_control_mode("paused").expect("mode");
        assert_eq!(
            mode,
            tt_daemon::ManagedProjectThreadControlMode::DirectorPaused
        );
    }

    #[test]
    fn project_control_command_maps_to_daemon_request() {
        let request = command_to_request(
            Command::Internal {
                command: InternalCommand::Project {
                    command: InternalProjectCommand::Control {
                        role: "dev".into(),
                        mode: "manual_next_turn".into(),
                    },
                },
            },
            Path::new("/repo"),
        )
        .expect("request");
        assert_eq!(
            request,
            DaemonRequest::SetManagedProjectThreadControl {
                cwd: PathBuf::from("/repo"),
                role: ThreadRole::Develop,
                mode: tt_daemon::ManagedProjectThreadControlMode::ManualNextTurn,
            }
        );
    }

    #[test]
    fn parses_clean_command() {
        let cli = Cli::try_parse_from(["tt", "clean", "--all"]).expect("parse");
        match cli.command {
            Command::Clean { all } => assert!(all),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn clean_command_maps_to_daemon_request() {
        let request =
            command_to_request(Command::Clean { all: true }, Path::new("/repo")).expect("request");
        assert_eq!(
            request,
            DaemonRequest::CleanManagedProject {
                cwd: PathBuf::from("/repo"),
                force: true,
            }
        );
    }

    #[test]
    fn renders_managed_project_plan_details() {
        let project = Project {
            id: "p-plan".into(),
            slug: "alpha".into(),
            title: "Alpha".into(),
            objective: "Ship".into(),
            status: ProjectStatus::Active,
            created_at: ts(),
            updated_at: ts(),
        };
        let bootstrap = tt_daemon::ManagedProjectBootstrap {
            project,
            repo_root: "/repo".into(),
            base_branch: "main".into(),
            worktree_root: "/repo/.tt/worktrees".into(),
            manifest_path: "/repo/.tt/state.toml".into(),
            project_config_path: "/repo/.tt/project.toml".into(),
            plan_path: "/repo/.tt/plan.toml".into(),
            contract_path: "/repo/.tt/contract.md".into(),
            codex_config_path: "/repo/.codex/config.toml".into(),
            project_config: tt_daemon::ManagedProjectProjectConfig {
                schema: "tt-managed-project-config-v1".into(),
                title: "Alpha".into(),
                objective: "Ship".into(),
                base_branch: "main".into(),
                branch_prefix: "tt".into(),
                tt_runtime_bin: Some("./target/debug/tt-cli".into()),
                plan_first: true,
                commit_policy: "checkpoint-enforced".into(),
                require_operator_merge_approval: true,
                expected_long_build: true,
                require_progress_updates: true,
                soft_silence_seconds: 900,
                hard_ceiling_seconds: 7200,
                default_validation_commands: vec!["cargo test".into()],
                smoke_validation_commands: vec!["cargo check".into()],
                checkpoint_triggers: vec!["after_plan".into(), "before_merge".into()],
                pitfalls: vec!["slow clean builds".into()],
                hints: vec!["run cargo test".into()],
                exceptions: vec!["merge approval required".into()],
            },
            plan: tt_daemon::ManagedProjectPlan {
                schema: "tt-managed-project-plan-v1".into(),
                status: "draft".into(),
                objective: "Ship".into(),
                updated_at: ts().to_rfc3339(),
                milestones: vec![tt_daemon::ManagedProjectPlanMilestone {
                    id: "milestone-1".into(),
                    title: "Plan".into(),
                    success_criteria: vec!["plan exists".into()],
                    evidence: vec!["file created".into()],
                }],
                work_items: vec![tt_daemon::ManagedProjectPlanWorkItem {
                    id: "alpha-director".into(),
                    title: "Director".into(),
                    owner_role: "director".into(),
                    phase: "plan".into(),
                    depends_on: vec![],
                    acceptance_criteria: vec!["director has plan".into()],
                    validation_commands: vec!["cargo test".into()],
                    commit_required: false,
                    status: "planned".into(),
                }],
                notes: tt_daemon::ManagedProjectPlanNotes {
                    risks: vec!["slow test lane".into()],
                    pitfalls: vec!["clean builds are expensive".into()],
                    open_questions: vec!["should we split validation?".into()],
                    operator_constraints: vec!["wait for approval".into()],
                },
            },
            startup: tt_daemon::ManagedProjectStartupState {
                phase: tt_daemon::ManagedProjectStartupPhase::Ready,
                updated_at: ts(),
                worker_reports: std::collections::BTreeMap::new(),
                director_ack: None,
            },
            scenario: None,
            roles: vec![],
        };
        let text = render_response(&DaemonResponse::ManagedProjectPlan(
            tt_daemon::ManagedProjectInspection {
                bootstrap,
                repository_summary: Some(tt_ui_core::GitRepositorySummary {
                    repository_root: "/repo".into(),
                    current_worktree: Some("/repo".into()),
                    current_branch: Some("main".into()),
                    current_head_commit: Some("abc123".into()),
                    dirty: false,
                    upstream: Some("origin/main".into()),
                    ahead_by: Some(0),
                    behind_by: Some(0),
                    merge_ready: true,
                    worktree_count: 3,
                }),
            },
        ));
        assert!(text.contains("managed project plan"));
        assert!(text.contains("Plan file: /repo/.tt/plan.toml"));
        assert!(text.contains("Milestones: 1"));
        assert!(text.contains("Work items: 1"));
        assert!(text.contains("Risks: slow test lane"));
        assert!(text.contains("Operator constraints: wait for approval"));
        assert!(text.contains("Repository"));
    }

    #[test]
    fn renders_codex_thread_detail_diagnostics() {
        let detail = tt_ui_core::CodexThreadDetail {
            thread_id: "thread-1".into(),
            thread_name: Some("director".into()),
            preview: "preview".into(),
            status: "systemError".into(),
            cwd: "/repo".into(),
            model_provider: "openai".into(),
            ephemeral: false,
            updated_at: 123,
            turn_count: 2,
            latest_turn_id: Some("turn-2".into()),
            latest_turn_status: Some("Failed".into()),
            latest_turn_error: Some("model backend failed".into()),
            latest_turn_summary: Some("plan\nnext_step".into()),
            bound_work_unit_id: Some("wu-1".into()),
            workspace_binding_count: 1,
        };
        let text = render_response(&DaemonResponse::CodexThreadDetail(Some(detail)));
        assert!(text.contains("latest_turn_status=Failed"));
        assert!(text.contains("latest_turn_error=model backend failed"));
        assert!(text.contains("latest_turn_summary=plan"));
    }

    #[test]
    fn renders_status_with_runtime_summary() {
        let text = render_status_response(
            &tt_daemon::DaemonStatus {
                repo_root: Some("/repo".into()),
                project_initialized: true,
                project_state: Some("attached (4/4)".into()),
                director_state: tt_daemon::ManagedProjectDirectorState::Ready,
                project_count: 2,
                work_unit_count: 6,
                bound_thread_count: 4,
                ready_workspace_count: 3,
            },
            RuntimeState::Ready,
            false,
        );

        assert_eq!(
            text,
            "project=Initialized runtime=Ready director=Ready repo=/repo\n"
        );
    }

    #[test]
    fn renders_status_when_runtime_is_unreachable() {
        let text = render_status_response(
            &tt_daemon::DaemonStatus {
                repo_root: Some("/repo".into()),
                project_initialized: true,
                project_state: Some("attached (4/4)".into()),
                director_state: tt_daemon::ManagedProjectDirectorState::Ready,
                project_count: 2,
                work_unit_count: 6,
                bound_thread_count: 4,
                ready_workspace_count: 3,
            },
            RuntimeState::Unreachable,
            false,
        );

        assert_eq!(
            text,
            "project=Initialized runtime=Unreachable director=Ready repo=/repo\n"
        );
    }

    #[test]
    fn renders_status_when_runtime_needs_auth() {
        let text = render_status_response(
            &tt_daemon::DaemonStatus {
                repo_root: Some("/repo".into()),
                project_initialized: true,
                project_state: Some("attached (4/4)".into()),
                director_state: tt_daemon::ManagedProjectDirectorState::Ready,
                project_count: 2,
                work_unit_count: 6,
                bound_thread_count: 4,
                ready_workspace_count: 3,
            },
            RuntimeState::NeedsAuth,
            false,
        );

        assert_eq!(
            text,
            "project=Initialized runtime=NeedsAuth director=Ready repo=/repo\n"
        );
    }

    #[test]
    fn renders_status_with_project_snapshot_only() {
        let text = render_response(&DaemonResponse::Status(tt_daemon::DaemonStatus {
            repo_root: Some("/repo".into()),
            project_initialized: true,
            project_state: Some("attached (4/4)".into()),
            director_state: tt_daemon::ManagedProjectDirectorState::Ready,
            project_count: 2,
            work_unit_count: 6,
            bound_thread_count: 4,
            ready_workspace_count: 3,
        }));

        assert_eq!(
            text,
            "project=Initialized runtime=Unreachable director=Ready repo=/repo\n"
        );
    }

    #[test]
    fn renders_status_as_json() {
        let text = render_status_json(
            &tt_daemon::DaemonStatus {
                repo_root: Some("/repo".into()),
                project_initialized: true,
                project_state: Some("scaffolded (0/4)".into()),
                director_state: tt_daemon::ManagedProjectDirectorState::Missing,
                project_count: 2,
                work_unit_count: 6,
                bound_thread_count: 4,
                ready_workspace_count: 3,
            },
            RuntimeState::Unreachable,
        );

        assert_eq!(
            text,
            "{\n  \"director\": \"Missing\",\n  \"project\": \"Initialized\",\n  \"repo\": \"/repo\",\n  \"runtime\": \"Unreachable\"\n}"
        );
    }

    #[test]
    fn renders_status_as_json_when_auth_is_missing() {
        let text = render_status_json(
            &tt_daemon::DaemonStatus {
                repo_root: Some("/repo".into()),
                project_initialized: true,
                project_state: Some("scaffolded (0/4)".into()),
                director_state: tt_daemon::ManagedProjectDirectorState::Starting,
                project_count: 2,
                work_unit_count: 6,
                bound_thread_count: 4,
                ready_workspace_count: 3,
            },
            RuntimeState::NeedsAuth,
        );

        assert_eq!(
            text,
            "{\n  \"director\": \"Starting\",\n  \"project\": \"Initialized\",\n  \"repo\": \"/repo\",\n  \"runtime\": \"NeedsAuth\"\n}"
        );
    }

    #[test]
    fn codex_login_mode_defaults_to_auto() {
        assert_eq!(
            codex_login_mode_for_cwd(Path::new("/repo")).expect("mode"),
            CodexLoginMode::Auto
        );
    }

    #[test]
    fn codex_login_args_follow_mode_and_tty() {
        assert_eq!(codex_login_args(CodexLoginMode::Auto, true), vec!["login"]);
        assert_eq!(
            codex_login_args(CodexLoginMode::Auto, false),
            vec!["login", "--device-auth"]
        );
        assert_eq!(
            codex_login_args(CodexLoginMode::DeviceAuth, true),
            vec!["login", "--device-auth"]
        );
        assert_eq!(
            codex_login_args(CodexLoginMode::Interactive, false),
            vec!["login"]
        );
    }

    #[test]
    fn renders_repo_local_device_auth_command() {
        let command = render_codex_login_command(
            Path::new("/repo/.codex"),
            Path::new("/bin/codex"),
            &["login", "--device-auth"],
        );
        assert_eq!(
            command,
            "CODEX_HOME=/repo/.codex /bin/codex login --device-auth"
        );
    }

    #[test]
    fn rewrites_open_runtime_connect_error() {
        let error = anyhow::anyhow!(
            "connect to Codex app-server `ws://127.0.0.1:4500`\n\nCaused by:\n    Connection refused (os error 111)"
        );
        let rewritten = rewrite_open_runtime_error(Path::new("/repo"), true, error);
        let text = format!("{rewritten:#}");
        assert!(text.contains(
            "cannot open TT project in /repo because the project runtime could not be started"
        ));
        assert!(text.contains("Run `tt status` for the persisted project snapshot."));
    }

    #[test]
    fn open_requires_initialized_project_with_git_like_error() {
        let error = anyhow::anyhow!(
            "fatal: not a TT project (or any of the parent directories): .tt\nRun `tt init` to initialize this repository."
        );
        let text = format!("{error:#}");
        assert!(text.contains("fatal: not a TT project"));
        assert!(text.contains("Run `tt init`"));
    }

    #[test]
    fn builds_codex_resume_command_for_director_thread() {
        let command = build_codex_resume_command(
            Path::new("/usr/local/bin/codex"),
            Path::new("/repo"),
            Some("thread-123"),
        );
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(command.get_program(), Path::new("/usr/local/bin/codex"));
        assert_eq!(
            args,
            vec![
                "--cd".to_string(),
                "/repo".to_string(),
                "resume".to_string(),
                "thread-123".to_string(),
            ]
        );
    }

    #[test]
    fn builds_codex_resume_command_without_thread_id_for_picker() {
        let command =
            build_codex_resume_command(Path::new("/usr/local/bin/codex"), Path::new("/repo"), None);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(command.get_program(), Path::new("/usr/local/bin/codex"));
        assert_eq!(
            args,
            vec![
                "--cd".to_string(),
                "/repo".to_string(),
                "resume".to_string(),
            ]
        );
    }

    #[test]
    fn renders_cli_reference_markdown() {
        let markdown = render_cli_reference_markdown();
        assert!(markdown.contains("# TT CLI Reference"));
        assert!(markdown.contains("## `tt`"));
        assert!(markdown.contains("### `tt init`"));
        assert!(markdown.contains("### `tt open`"));
        assert!(!markdown.contains("tt codex"));
        assert!(markdown.contains("`docs`"));
        assert!(markdown.contains("`export-cli`"));
        assert!(!markdown.contains("`internal`"));
        assert!(!markdown.contains("`project`"));
    }

    #[test]
    fn parses_events_command() {
        let cli = Cli::parse_from(["tt", "events", "--follow", "--json", "--limit", "10"]);
        match cli.command {
            Command::Events {
                follow,
                json,
                limit,
            } => {
                assert!(follow);
                assert!(json);
                assert_eq!(limit, 10);
            }
            other => panic!("expected events command, got {other:?}"),
        }
    }

    #[test]
    fn renders_events_as_chat() {
        let events = vec![
            tt_daemon::ManagedProjectEvent {
                ts: ts(),
                project_id: "p1".into(),
                phase: "startup".into(),
                kind: tt_daemon::ManagedProjectEventKind::PromptSent,
                role: Some("director".into()),
                counterparty_role: Some("dev".into()),
                thread_id: Some("thread-1".into()),
                turn_id: Some("turn-1".into()),
                text: "this is an example prompt".into(),
                status: None,
                error: None,
            },
            tt_daemon::ManagedProjectEvent {
                ts: ts(),
                project_id: "p1".into(),
                phase: "startup".into(),
                kind: tt_daemon::ManagedProjectEventKind::ResponseReceived,
                role: Some("dev".into()),
                counterparty_role: Some("director".into()),
                thread_id: Some("thread-2".into()),
                turn_id: Some("turn-2".into()),
                text: "this is an example response".into(),
                status: Some("reported".into()),
                error: None,
            },
        ];
        let text = render_events_chat(&events);
        assert!(text.contains("Director"));
        assert!(text.contains("Dev"));
        assert!(text.contains("this is an example prompt"));
        assert!(text.contains("this is an example response"));
    }

    #[test]
    fn renders_parse_failed_event_with_raw_output() {
        let event = tt_daemon::ManagedProjectEvent {
            ts: ts(),
            project_id: "p1".into(),
            phase: "worker_reports_pending".into(),
            kind: tt_daemon::ManagedProjectEventKind::ParseFailed,
            role: Some("test".into()),
            counterparty_role: Some("director".into()),
            thread_id: Some("thread-3".into()),
            turn_id: Some("turn-3".into()),
            text: "{\"role\":\"test\",\"status\":\"ready\"}".into(),
            status: Some("parse_failed".into()),
            error: Some("expected value at line 1 column 1".into()),
        };
        let text = render_events_chat(&[event]);
        assert!(text.contains("Parse error: expected value at line 1 column 1"));
        assert!(text.contains("{\"role\":\"test\",\"status\":\"ready\"}"));
    }
}
