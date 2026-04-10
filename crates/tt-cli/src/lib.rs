//! Thin CLI surface for TT v2.
//!
//! The canonical v2 CLI is a narrow client over the daemon request API rather
//! than a second application layer.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::io::IsTerminal;

use anyhow::{Context, Result};
use clap::{Arg, ArgAction, CommandFactory, Parser, Subcommand};
use tt_daemon::{
    DaemonRequest, DaemonResponse, ManagedProjectThreadAttachment, ManagedProjectThreadControlMode,
    request_for_cwd,
};
use tt_domain as _;
use tt_domain::ThreadRole;

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
    Status,
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
    if let Some(output) = local_command_output(&cli.command, &cli.cwd)? {
        print!("{output}");
        return Ok(());
    }
    let is_open_command = matches!(&cli.command, Command::Open { .. });
    let is_status_command = matches!(&cli.command, Command::Status);
    let cwd = cli.cwd.unwrap_or(std::env::current_dir()?);
    let request_cwd = request_cwd_for_command(&cwd, &cli.command);
    let response = request_for_cwd(&request_cwd, command_to_request(cli.command, &cwd)?)
        .map_err(|error| rewrite_open_runtime_error(&cwd, is_open_command, error))?;
    let output = match (is_status_command, response) {
        (true, DaemonResponse::Status(status)) => {
            let runtime_ready = request_for_cwd(
                &cwd,
                DaemonRequest::InspectCodexAppServers { cwd: cwd.clone() },
            )
            .ok()
            .and_then(|response| match response {
                DaemonResponse::CodexAppServers(servers) => {
                    servers.first().map(|server| server.listen_reachable)
                }
                _ => None,
            })
            .unwrap_or(false);
            render_status_response(&status, runtime_ready, std::io::stdout().is_terminal())
        }
        (_, response) => render_response(&response),
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
        Command::Status => DaemonRequest::Status {
            cwd: cwd.to_path_buf(),
        },
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
        "cannot open TT project in {} because the project runtime is unreachable.\nRun `tt status` for the persisted project snapshot; the hidden internal runtime probe is available to e2e/debug.\n\nOriginal error:\n{error_text}",
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
        DaemonResponse::Status(status) => render_status_response(status, false, false),
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
    runtime_ready: bool,
    colorize: bool,
) -> String {
    let runtime_label = if colorize {
        if runtime_ready {
            "\u{1b}[32mready\u{1b}[0m"
        } else {
            "\u{1b}[31munreachable\u{1b}[0m"
        }
    } else if runtime_ready {
        "ready"
    } else {
        "unreachable"
    };
    format!(
        "status\nrepo_root: {}\nproject_initialized: {}\nproject_state: {}\nruntime: {}\nprojects: {}\nwork-units: {}\nbound-threads: {}\nready-workspaces: {}\n",
        status
            .repo_root
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        status.project_initialized,
        status.project_state.as_deref().unwrap_or("<none>"),
        runtime_label,
        status.project_count,
        status.work_unit_count,
        status.bound_thread_count,
        status.ready_workspace_count
    )
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
    let is_status_command = matches!(&cli.command, Command::Status);
    let cwd = cli.cwd.unwrap_or(std::env::current_dir()?);
    let request_cwd = request_cwd_for_command(&cwd, &cli.command);
    let response = request_for_cwd(&request_cwd, command_to_request(cli.command, &cwd)?)
        .map_err(|error| rewrite_open_runtime_error(&cwd, is_open_command, error))?;
    let output = match (is_status_command, response) {
        (true, DaemonResponse::Status(status)) => {
            let runtime_ready = request_for_cwd(
                &cwd,
                DaemonRequest::InspectCodexAppServers { cwd: cwd.clone() },
            )
            .ok()
            .and_then(|response| match response {
                DaemonResponse::CodexAppServers(servers) => {
                    servers.first().map(|server| server.listen_reachable)
                }
                _ => None,
            })
            .unwrap_or(false);
            render_status_response(&status, runtime_ready, std::io::stdout().is_terminal())
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
                branch_name: Some("tt/alpha/dev".into()),
                worktree_path: Some("/repo/.tt-worktrees/alpha/dev".into()),
                thread_id: Some("thread-1".into()),
                thread_name: Some("alpha-dev".into()),
                workspace_binding_id: Some("alpha:dev".into()),
            },
        ];
        let bootstrap = tt_daemon::ManagedProjectBootstrap {
            project,
            repo_root: "/repo".into(),
            base_branch: "main".into(),
            worktree_root: "/repo/.tt-worktrees/alpha".into(),
            manifest_path: "/repo/.tt/state.toml".into(),
            project_config_path: "/repo/.tt/project.toml".into(),
            plan_path: "/repo/.tt/plan.toml".into(),
            contract_path: "/repo/.tt/contracts/worker-contract.md".into(),
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
            worktree_root: "/repo/.tt-worktrees/alpha".into(),
            manifest_path: "/repo/.tt/state.toml".into(),
            project_config_path: "/repo/.tt/project.toml".into(),
            plan_path: "/repo/.tt/plan.toml".into(),
            contract_path: "/repo/.tt/contracts/worker-contract.md".into(),
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
                project_count: 2,
                work_unit_count: 6,
                bound_thread_count: 4,
                ready_workspace_count: 3,
            },
            true,
            false,
        );

        assert!(text.contains("repo_root: /repo"));
        assert!(text.contains("project_initialized: true"));
        assert!(text.contains("project_state: attached (4/4)"));
        assert!(text.contains("runtime: ready"));
        assert!(text.contains("projects: 2"));
    }

    #[test]
    fn renders_status_when_runtime_is_unreachable() {
        let text = render_status_response(
            &tt_daemon::DaemonStatus {
                repo_root: Some("/repo".into()),
                project_initialized: true,
                project_state: Some("attached (4/4)".into()),
                project_count: 2,
                work_unit_count: 6,
                bound_thread_count: 4,
                ready_workspace_count: 3,
            },
            false,
            false,
        );

        assert!(text.contains("runtime: unreachable"));
    }

    #[test]
    fn renders_status_with_project_snapshot_only() {
        let text = render_response(&DaemonResponse::Status(tt_daemon::DaemonStatus {
            repo_root: Some("/repo".into()),
            project_initialized: true,
            project_state: Some("attached (4/4)".into()),
            project_count: 2,
            work_unit_count: 6,
            bound_thread_count: 4,
            ready_workspace_count: 3,
        }));

        assert!(text.contains("repo_root: /repo"));
        assert!(text.contains("project_initialized: true"));
        assert!(text.contains("project_state: attached (4/4)"));
        assert!(text.contains("runtime: unreachable"));
        assert!(text.contains("projects: 2"));
    }

    #[test]
    fn rewrites_open_runtime_connect_error() {
        let error = anyhow::anyhow!(
            "connect to Codex app-server `ws://127.0.0.1:4500`\n\nCaused by:\n    Connection refused (os error 111)"
        );
        let rewritten = rewrite_open_runtime_error(Path::new("/repo"), true, error);
        let text = format!("{rewritten:#}");
        assert!(text.contains(
            "cannot open TT project in /repo because the project runtime is unreachable"
        ));
        assert!(text.contains("hidden internal runtime probe"));
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
}
