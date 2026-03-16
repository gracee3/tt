mod service;
mod streaming;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use service::{RuntimeOverrides, SupervisorService};

#[derive(Debug, Parser)]
#[command(name = "orcas")]
struct Cli {
    #[command(flatten)]
    global: GlobalOptions,
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Debug, Clone, Args, Default)]
struct GlobalOptions {
    #[arg(long, global = true)]
    codex_bin: Option<PathBuf>,
    #[arg(long, global = true)]
    listen_url: Option<String>,
    #[arg(long, global = true)]
    cwd: Option<PathBuf>,
    #[arg(long, global = true)]
    model: Option<String>,
    #[arg(long, global = true, default_value_t = false)]
    connect_only: bool,
    #[arg(long, global = true, default_value_t = false)]
    force_spawn: bool,
}

#[derive(Debug, Subcommand)]
enum TopCommand {
    Supervisor {
        #[command(subcommand)]
        command: SupervisorCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SupervisorCommand {
    Doctor,
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
    Threads {
        #[command(subcommand)]
        command: ThreadsCommand,
    },
    Turns {
        #[command(subcommand)]
        command: TurnsCommand,
    },
    Prompt(PromptArgs),
    Quickstart(QuickstartArgs),
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Start,
    Status,
    Restart,
    Stop,
}

#[derive(Debug, Subcommand)]
enum ModelsCommand {
    List,
}

#[derive(Debug, Subcommand)]
enum ThreadsCommand {
    List,
    Read(ThreadRefArgs),
    Start(ThreadStartArgs),
    Resume(ThreadResumeArgs),
}

#[derive(Debug, Subcommand)]
enum TurnsCommand {
    ListActive,
    Get(TurnRefArgs),
}

#[derive(Debug, Clone, Args)]
struct ThreadRefArgs {
    #[arg(long)]
    thread: String,
}

#[derive(Debug, Clone, Args)]
struct ThreadStartArgs {
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, default_value_t = false)]
    ephemeral: bool,
}

#[derive(Debug, Clone, Args)]
struct ThreadResumeArgs {
    #[arg(long)]
    thread: String,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TurnRefArgs {
    #[arg(long)]
    thread: String,
    #[arg(long)]
    turn: String,
}

#[derive(Debug, Clone, Args)]
struct PromptArgs {
    #[arg(long)]
    thread: String,
    #[arg(long)]
    text: String,
}

#[derive(Debug, Clone, Args)]
struct QuickstartArgs {
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    text: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let overrides = RuntimeOverrides {
        codex_bin: cli.global.codex_bin,
        listen_url: cli.global.listen_url,
        cwd: cli.global.cwd,
        model: cli.global.model,
        connect_only: cli.global.connect_only,
        force_spawn: cli.global.force_spawn,
    };
    let service = SupervisorService::load(&overrides).await?;

    match cli.command {
        TopCommand::Supervisor { command } => match command {
            SupervisorCommand::Doctor => service.doctor().await?,
            SupervisorCommand::Daemon { command } => match command {
                DaemonCommand::Start => service.daemon_start(overrides.force_spawn).await?,
                DaemonCommand::Status => service.daemon_status().await?,
                DaemonCommand::Restart => service.daemon_restart().await?,
                DaemonCommand::Stop => service.daemon_stop().await?,
            },
            SupervisorCommand::Models { command } => match command {
                ModelsCommand::List => service.models_list().await?,
            },
            SupervisorCommand::Threads { command } => match command {
                ThreadsCommand::List => service.threads_list().await?,
                ThreadsCommand::Read(args) => service.thread_read(&args.thread).await?,
                ThreadsCommand::Start(args) => {
                    service
                        .thread_start(args.cwd, args.model, args.ephemeral)
                        .await?;
                }
                ThreadsCommand::Resume(args) => {
                    service
                        .thread_resume(&args.thread, args.cwd, args.model)
                        .await?;
                }
            },
            SupervisorCommand::Turns { command } => match command {
                TurnsCommand::ListActive => service.turns_list_active().await?,
                TurnsCommand::Get(args) => service.turn_get(&args.thread, &args.turn).await?,
            },
            SupervisorCommand::Prompt(args) => {
                let _ = service.prompt(&args.thread, &args.text).await?;
            }
            SupervisorCommand::Quickstart(args) => {
                service.quickstart(args.cwd, args.model, &args.text).await?;
            }
        },
    }

    Ok(())
}
