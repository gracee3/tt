mod service;
mod streaming;

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use orcas_core::{
    AppPaths, DecisionType, WorkUnitStatus, WorkstreamStatus, authority, init_file_logger,
};
use tracing::info;

use service::{RuntimeOverrides, SupervisorService};

#[derive(Debug, Parser)]
#[command(name = "orcas", version, about = "Orcas operator CLI")]
struct Cli {
    #[command(flatten)]
    global: GlobalOptions,
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Debug, Clone, Args, Default)]
struct GlobalOptions {
    #[arg(
        long,
        global = true,
        help = "Override the local Codex binary path for this command"
    )]
    codex_bin: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Override the upstream Codex app-server WebSocket URL"
    )]
    listen_url: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Override the default working directory for this command"
    )]
    cwd: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Override the default model for this command"
    )]
    model: Option<String>,
    #[arg(
        long,
        global = true,
        default_value_t = false,
        conflicts_with = "force_spawn",
        help = "Require connect-only mode instead of spawning a local Codex app-server"
    )]
    connect_only: bool,
    #[arg(
        long,
        global = true,
        default_value_t = false,
        conflicts_with = "connect_only",
        help = "Force spawn mode instead of connect-only mode"
    )]
    force_spawn: bool,
}

#[derive(Debug, Subcommand)]
enum TopCommand {
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Tui,
    Doctor,
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
    Workstreams {
        #[command(subcommand)]
        command: WorkstreamsCommand,
    },
    Workunits {
        #[command(subcommand)]
        command: WorkunitsCommand,
    },
    TrackedThreads {
        #[command(subcommand)]
        command: TrackedThreadsCommand,
    },
    Assignments {
        #[command(subcommand)]
        command: AssignmentsCommand,
    },
    Reports {
        #[command(subcommand)]
        command: ReportsCommand,
    },
    Decisions {
        #[command(subcommand)]
        command: DecisionsCommand,
    },
    Proposals {
        #[command(subcommand)]
        command: ProposalsCommand,
    },
    Codex {
        #[command(subcommand)]
        command: CodexCommand,
    },
    Prompt(PromptArgs),
    Quickstart(QuickstartArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Launch and manage the Orcas daemon")]
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

#[derive(Debug, Subcommand)]
#[command(about = "Canonical authority-backed CRUD for planning workstreams")]
enum WorkstreamsCommand {
    Create(WorkstreamCreateArgs),
    Edit(WorkstreamEditArgs),
    Delete(WorkstreamRefArgs),
    List,
    Get(WorkstreamRefArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Canonical authority-backed CRUD for planning work units")]
enum WorkunitsCommand {
    Create(WorkunitCreateArgs),
    Edit(WorkunitEditArgs),
    Delete(WorkunitRefArgs),
    List(WorkunitListArgs),
    Get(WorkunitRefArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Canonical authority-backed CRUD for tracked-thread planning records")]
enum TrackedThreadsCommand {
    Create(TrackedThreadCreateArgs),
    Edit(TrackedThreadEditArgs),
    Delete(TrackedThreadRefArgs),
    List(TrackedThreadListArgs),
    Get(TrackedThreadRefArgs),
    PrepareWorkspace(TrackedThreadRefArgs),
    RefreshWorkspace(TrackedThreadRefArgs),
    MergePrep(TrackedThreadRefArgs),
    AuthorizeMerge(TrackedThreadRefArgs),
}

#[derive(Debug, Subcommand)]
enum AssignmentsCommand {
    Start(AssignmentStartArgs),
    Get(AssignmentRefArgs),
    Communication(AssignmentRefArgs),
}

#[derive(Debug, Subcommand)]
enum ReportsCommand {
    Get(ReportRefArgs),
    ListForWorkunit(WorkunitRefArgs),
}

#[derive(Debug, Subcommand)]
enum DecisionsCommand {
    Apply(DecisionApplyArgs),
}

#[derive(Debug, Subcommand)]
enum ProposalsCommand {
    Create(ProposalCreateArgs),
    Get(ProposalRefArgs),
    ArtifactSummary(ProposalRefArgs),
    ArtifactDetail(ProposalRefArgs),
    ArtifactExport(ProposalArtifactExportArgs),
    ListForWorkunit(WorkunitRefArgs),
    Approve(ProposalApproveArgs),
    Reject(ProposalRejectArgs),
}

#[derive(Debug, Subcommand)]
enum CodexCommand {
    Review {
        #[command(subcommand)]
        command: CodexReviewCommand,
    },
}

#[derive(Debug, Subcommand)]
enum CodexReviewCommand {
    List(CodexDecisionListArgs),
    Queue(CodexDecisionQueueArgs),
    History(CodexDecisionHistoryArgs),
    Get(CodexDecisionRefArgs),
    ProposeSteer(CodexDecisionProposeSteerArgs),
    ReplacePendingSteer(CodexDecisionReplacePendingSteerArgs),
    RecordNoAction(CodexDecisionRecordNoActionArgs),
    ManualRefresh(CodexDecisionManualRefreshArgs),
    Approve(CodexDecisionApproveArgs),
    Reject(CodexDecisionRejectArgs),
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
struct WorkstreamCreateArgs {
    #[arg(long)]
    title: String,
    #[arg(long)]
    objective: String,
    #[arg(long)]
    priority: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct WorkstreamRefArgs {
    #[arg(long)]
    workstream: String,
}

#[derive(Debug, Clone, Args)]
struct WorkstreamEditArgs {
    #[arg(long)]
    workstream: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    objective: Option<String>,
    #[arg(long, value_enum)]
    status: Option<WorkstreamStatusArg>,
    #[arg(long)]
    priority: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct WorkunitCreateArgs {
    #[arg(long)]
    workstream: String,
    #[arg(long)]
    title: String,
    #[arg(long)]
    task: String,
    #[arg(long = "dependency")]
    dependencies: Vec<String>,
}

#[derive(Debug, Clone, Args, Default)]
struct WorkunitListArgs {
    #[arg(long)]
    workstream: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct WorkunitRefArgs {
    #[arg(long)]
    workunit: String,
}

#[derive(Debug, Clone, Args)]
struct WorkunitEditArgs {
    #[arg(long)]
    workunit: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    task: Option<String>,
    #[arg(long, value_enum)]
    status: Option<WorkUnitStatusArg>,
}

#[derive(Debug, Clone, Args, Default)]
struct TrackedThreadListArgs {
    #[arg(long)]
    workunit: String,
}

#[derive(Debug, Clone, Args)]
struct TrackedThreadRefArgs {
    #[arg(long = "tracked-thread")]
    tracked_thread: String,
}

#[derive(Debug, Clone, Args)]
struct TrackedThreadCreateArgs {
    #[arg(long)]
    workunit: String,
    #[arg(long)]
    title: String,
    #[arg(long = "root-dir")]
    root_dir: String,
    #[arg(long)]
    notes: Option<String>,
    #[arg(long = "upstream-thread")]
    upstream_thread: Option<String>,
    #[arg(long)]
    model: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TrackedThreadEditArgs {
    #[arg(long = "tracked-thread")]
    tracked_thread: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long = "root-dir")]
    root_dir: Option<String>,
    #[arg(long)]
    notes: Option<String>,
    #[arg(long = "upstream-thread")]
    upstream_thread: Option<String>,
    #[arg(long, value_enum)]
    binding_state: Option<TrackedThreadBindingStateArg>,
    #[arg(long)]
    model: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct AssignmentStartArgs {
    #[arg(long)]
    workunit: String,
    #[arg(long)]
    worker: String,
    #[arg(long)]
    instructions: Option<String>,
    #[arg(long)]
    worker_kind: Option<String>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct AssignmentRefArgs {
    #[arg(long)]
    assignment: String,
}

#[derive(Debug, Clone, Args)]
struct ReportRefArgs {
    #[arg(long)]
    report: String,
}

#[derive(Debug, Clone, Args)]
struct ProposalRefArgs {
    #[arg(long)]
    proposal: String,
}

#[derive(Debug, Clone, Args)]
struct ProposalArtifactExportArgs {
    #[arg(long)]
    proposal: String,
    #[arg(long, value_enum, default_value_t = ProposalArtifactExportFormatArg::Json)]
    format: ProposalArtifactExportFormatArg,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
struct ProposalCreateArgs {
    #[arg(long)]
    workunit: String,
    #[arg(long)]
    report: Option<String>,
    #[arg(long)]
    note: Option<String>,
    #[arg(long)]
    requested_by: Option<String>,
    #[arg(long, default_value_t = false)]
    supersede_open: bool,
}

#[derive(Debug, Clone, Args)]
struct ProposalApproveArgs {
    #[arg(long)]
    proposal: String,
    #[arg(long)]
    review_note: Option<String>,
    #[arg(long)]
    reviewed_by: Option<String>,
    #[arg(long = "type", value_enum)]
    decision_type: Option<DecisionTypeArg>,
    #[arg(long)]
    rationale: Option<String>,
    #[arg(long)]
    worker: Option<String>,
    #[arg(long)]
    worker_kind: Option<String>,
    #[arg(long)]
    objective: Option<String>,
    #[arg(long = "instruction")]
    instructions: Vec<String>,
    #[arg(long = "acceptance")]
    acceptance_criteria: Vec<String>,
    #[arg(long = "stop-condition")]
    stop_conditions: Vec<String>,
    #[arg(long = "expected-report-field")]
    expected_report_fields: Vec<String>,
}

#[derive(Debug, Clone, Args)]
struct ProposalRejectArgs {
    #[arg(long)]
    proposal: String,
    #[arg(long)]
    review_note: Option<String>,
    #[arg(long)]
    reviewed_by: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
struct CodexDecisionFilterArgs {
    #[arg(long)]
    thread: Option<String>,
    #[arg(long)]
    assignment: Option<String>,
    #[arg(long)]
    workstream: Option<String>,
    #[arg(long)]
    workunit: Option<String>,
    #[arg(long)]
    supervisor: Option<String>,
    #[arg(long, value_enum)]
    status: Option<CodexDecisionStatusArg>,
    #[arg(long, value_enum)]
    kind: Option<CodexDecisionKindArg>,
    #[arg(long, default_value_t = false)]
    include_superseded: bool,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Args, Default)]
struct CodexDecisionListArgs {
    #[command(flatten)]
    filters: CodexDecisionFilterArgs,
    #[arg(long, default_value_t = false)]
    include_closed: bool,
}

#[derive(Debug, Clone, Args, Default)]
struct CodexDecisionQueueArgs {
    #[command(flatten)]
    filters: CodexDecisionFilterArgs,
}

#[derive(Debug, Clone, Args, Default)]
struct CodexDecisionHistoryArgs {
    #[arg(long)]
    thread: Option<String>,
    #[arg(long)]
    assignment: Option<String>,
    #[arg(long, default_value_t = true)]
    include_superseded: bool,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Args)]
struct CodexDecisionRefArgs {
    #[arg(long)]
    decision: String,
}

#[derive(Debug, Clone, Args)]
struct CodexDecisionProposeSteerArgs {
    #[arg(long)]
    thread: String,
    #[arg(long)]
    text: String,
    #[arg(long)]
    requested_by: Option<String>,
    #[arg(long)]
    rationale_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct CodexDecisionReplacePendingSteerArgs {
    #[arg(long)]
    decision: String,
    #[arg(long)]
    text: String,
    #[arg(long)]
    requested_by: Option<String>,
    #[arg(long)]
    rationale_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct CodexDecisionRecordNoActionArgs {
    #[arg(long)]
    decision: String,
    #[arg(long)]
    reviewed_by: Option<String>,
    #[arg(long)]
    review_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct CodexDecisionManualRefreshArgs {
    #[arg(long)]
    thread: Option<String>,
    #[arg(long)]
    assignment: Option<String>,
    #[arg(long)]
    requested_by: Option<String>,
    #[arg(long)]
    rationale_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct CodexDecisionApproveArgs {
    #[arg(long)]
    decision: String,
    #[arg(long)]
    reviewed_by: Option<String>,
    #[arg(long)]
    review_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct CodexDecisionRejectArgs {
    #[arg(long)]
    decision: String,
    #[arg(long)]
    reviewed_by: Option<String>,
    #[arg(long)]
    review_note: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DecisionTypeArg {
    Accept,
    Continue,
    Redirect,
    MarkComplete,
    EscalateToHuman,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProposalArtifactExportFormatArg {
    Json,
    Md,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WorkstreamStatusArg {
    Active,
    Blocked,
    Completed,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WorkUnitStatusArg {
    Ready,
    Blocked,
    Running,
    AwaitingDecision,
    Accepted,
    NeedsHuman,
    Completed,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TrackedThreadBindingStateArg {
    Unbound,
    Bound,
    Detached,
    Missing,
}

impl From<WorkstreamStatusArg> for WorkstreamStatus {
    fn from(value: WorkstreamStatusArg) -> Self {
        match value {
            WorkstreamStatusArg::Active => WorkstreamStatus::Active,
            WorkstreamStatusArg::Blocked => WorkstreamStatus::Blocked,
            WorkstreamStatusArg::Completed => WorkstreamStatus::Completed,
        }
    }
}

impl From<WorkUnitStatusArg> for WorkUnitStatus {
    fn from(value: WorkUnitStatusArg) -> Self {
        match value {
            WorkUnitStatusArg::Ready => WorkUnitStatus::Ready,
            WorkUnitStatusArg::Blocked => WorkUnitStatus::Blocked,
            WorkUnitStatusArg::Running => WorkUnitStatus::Running,
            WorkUnitStatusArg::AwaitingDecision => WorkUnitStatus::AwaitingDecision,
            WorkUnitStatusArg::Accepted => WorkUnitStatus::Accepted,
            WorkUnitStatusArg::NeedsHuman => WorkUnitStatus::NeedsHuman,
            WorkUnitStatusArg::Completed => WorkUnitStatus::Completed,
        }
    }
}

impl From<TrackedThreadBindingStateArg> for authority::TrackedThreadBindingState {
    fn from(value: TrackedThreadBindingStateArg) -> Self {
        match value {
            TrackedThreadBindingStateArg::Unbound => authority::TrackedThreadBindingState::Unbound,
            TrackedThreadBindingStateArg::Bound => authority::TrackedThreadBindingState::Bound,
            TrackedThreadBindingStateArg::Detached => {
                authority::TrackedThreadBindingState::Detached
            }
            TrackedThreadBindingStateArg::Missing => authority::TrackedThreadBindingState::Missing,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CodexDecisionStatusArg {
    ProposedToHuman,
    Recorded,
    Sent,
    Rejected,
    Stale,
    Superseded,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CodexDecisionKindArg {
    NextTurn,
    SteerActiveTurn,
    InterruptActiveTurn,
    NoAction,
}

impl From<CodexDecisionStatusArg> for orcas_core::SupervisorTurnDecisionStatus {
    fn from(value: CodexDecisionStatusArg) -> Self {
        match value {
            CodexDecisionStatusArg::ProposedToHuman => Self::ProposedToHuman,
            CodexDecisionStatusArg::Recorded => Self::Recorded,
            CodexDecisionStatusArg::Sent => Self::Sent,
            CodexDecisionStatusArg::Rejected => Self::Rejected,
            CodexDecisionStatusArg::Stale => Self::Stale,
            CodexDecisionStatusArg::Superseded => Self::Superseded,
        }
    }
}

impl From<CodexDecisionKindArg> for orcas_core::SupervisorTurnDecisionKind {
    fn from(value: CodexDecisionKindArg) -> Self {
        match value {
            CodexDecisionKindArg::NextTurn => Self::NextTurn,
            CodexDecisionKindArg::SteerActiveTurn => Self::SteerActiveTurn,
            CodexDecisionKindArg::InterruptActiveTurn => Self::InterruptActiveTurn,
            CodexDecisionKindArg::NoAction => Self::NoAction,
        }
    }
}

#[derive(Debug, Clone, Args)]
struct DecisionApplyArgs {
    #[arg(long)]
    workunit: String,
    #[arg(long)]
    rationale: String,
    #[arg(long)]
    report: Option<String>,
    #[arg(long = "type", value_enum)]
    decision_type: DecisionTypeArg,
    #[arg(long)]
    instructions: Option<String>,
    #[arg(long)]
    worker: Option<String>,
    #[arg(long)]
    worker_kind: Option<String>,
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
    let paths = AppPaths::discover()?;
    paths.ensure().await?;
    init_file_logger("orcas", &paths.logs_dir.join("orcas.log"))?;
    info!("starting orcas process");

    let cli = Cli::parse();
    let overrides = RuntimeOverrides {
        codex_bin: cli.global.codex_bin,
        listen_url: cli.global.listen_url,
        cwd: cli.global.cwd,
        model: cli.global.model,
        connect_only: cli.global.connect_only,
        force_spawn: cli.global.force_spawn,
    };
    match cli.command {
        TopCommand::Tui => launch_tui()?,
        TopCommand::Daemon { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                DaemonCommand::Start => service.daemon_start(overrides.force_spawn).await?,
                DaemonCommand::Status => service.daemon_status().await?,
                DaemonCommand::Restart => service.daemon_restart().await?,
                DaemonCommand::Stop => service.daemon_stop().await?,
            }
        }
        TopCommand::Doctor => {
            let service = SupervisorService::load(&overrides).await?;
            service.doctor().await?;
        }
        TopCommand::Models { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                ModelsCommand::List => service.models_list().await?,
            }
        }
        TopCommand::Threads { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
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
            }
        }
        TopCommand::Turns { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                TurnsCommand::ListActive => service.turns_list_active().await?,
                TurnsCommand::Get(args) => service.turn_get(&args.thread, &args.turn).await?,
            }
        }
        TopCommand::Workstreams { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                WorkstreamsCommand::Create(args) => {
                    service
                        .workstream_create(args.title, args.objective, args.priority)
                        .await?;
                }
                WorkstreamsCommand::Edit(args) => {
                    service
                        .workstream_edit(
                            &args.workstream,
                            args.title,
                            args.objective,
                            args.status.map(Into::into),
                            args.priority,
                        )
                        .await?;
                }
                WorkstreamsCommand::Delete(args) => {
                    service.workstream_delete(&args.workstream).await?;
                }
                WorkstreamsCommand::List => service.workstream_list().await?,
                WorkstreamsCommand::Get(args) => service.workstream_get(&args.workstream).await?,
            }
        }
        TopCommand::Workunits { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                WorkunitsCommand::Create(args) => {
                    service
                        .workunit_create(&args.workstream, args.title, args.task, args.dependencies)
                        .await?;
                }
                WorkunitsCommand::Edit(args) => {
                    service
                        .workunit_edit(
                            &args.workunit,
                            args.title,
                            args.task,
                            args.status.map(Into::into),
                        )
                        .await?;
                }
                WorkunitsCommand::Delete(args) => {
                    service.workunit_delete(&args.workunit).await?;
                }
                WorkunitsCommand::List(args) => {
                    service.workunit_list(args.workstream.as_deref()).await?;
                }
                WorkunitsCommand::Get(args) => service.workunit_get(&args.workunit).await?,
            }
        }
        TopCommand::TrackedThreads { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                TrackedThreadsCommand::Create(args) => {
                    service
                        .tracked_thread_create(
                            &args.workunit,
                            args.title,
                            args.root_dir,
                            args.notes,
                            args.upstream_thread,
                            args.model,
                        )
                        .await?;
                }
                TrackedThreadsCommand::Edit(args) => {
                    service
                        .tracked_thread_edit(
                            &args.tracked_thread,
                            args.title,
                            args.root_dir,
                            args.notes,
                            args.upstream_thread,
                            args.binding_state.map(Into::into),
                            args.model,
                        )
                        .await?;
                }
                TrackedThreadsCommand::Delete(args) => {
                    service.tracked_thread_delete(&args.tracked_thread).await?;
                }
                TrackedThreadsCommand::List(args) => {
                    service.tracked_thread_list(&args.workunit).await?;
                }
                TrackedThreadsCommand::Get(args) => {
                    service.tracked_thread_get(&args.tracked_thread).await?;
                }
                TrackedThreadsCommand::PrepareWorkspace(args) => {
                    service
                        .tracked_thread_prepare_workspace(&args.tracked_thread)
                        .await?;
                }
                TrackedThreadsCommand::RefreshWorkspace(args) => {
                    service
                        .tracked_thread_refresh_workspace(&args.tracked_thread)
                        .await?;
                }
                TrackedThreadsCommand::MergePrep(args) => {
                    service
                        .tracked_thread_merge_prep(&args.tracked_thread)
                        .await?;
                }
                TrackedThreadsCommand::AuthorizeMerge(args) => {
                    service
                        .tracked_thread_authorize_merge(&args.tracked_thread)
                        .await?;
                }
            }
        }
        TopCommand::Assignments { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                AssignmentsCommand::Start(args) => {
                    service
                        .assignment_start(
                            &args.workunit,
                            &args.worker,
                            args.instructions,
                            args.worker_kind,
                            args.cwd,
                            args.model,
                        )
                        .await?;
                }
                AssignmentsCommand::Get(args) => service.assignment_get(&args.assignment).await?,
                AssignmentsCommand::Communication(args) => {
                    service
                        .assignment_communication_get(&args.assignment)
                        .await?
                }
            }
        }
        TopCommand::Reports { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                ReportsCommand::Get(args) => service.report_get(&args.report).await?,
                ReportsCommand::ListForWorkunit(args) => {
                    service.report_list_for_workunit(&args.workunit).await?;
                }
            }
        }
        TopCommand::Decisions { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                DecisionsCommand::Apply(args) => {
                    service
                        .decision_apply(
                            &args.workunit,
                            args.report,
                            match args.decision_type {
                                DecisionTypeArg::Accept => DecisionType::Accept,
                                DecisionTypeArg::Continue => DecisionType::Continue,
                                DecisionTypeArg::Redirect => DecisionType::Redirect,
                                DecisionTypeArg::MarkComplete => DecisionType::MarkComplete,
                                DecisionTypeArg::EscalateToHuman => DecisionType::EscalateToHuman,
                            },
                            args.rationale,
                            args.instructions,
                            args.worker,
                            args.worker_kind,
                        )
                        .await?;
                }
            }
        }
        TopCommand::Proposals { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                ProposalsCommand::Create(args) => {
                    service
                        .proposal_create(
                            &args.workunit,
                            args.report,
                            args.note,
                            args.requested_by,
                            args.supersede_open,
                        )
                        .await?;
                }
                ProposalsCommand::Get(args) => service.proposal_get(&args.proposal).await?,
                ProposalsCommand::ArtifactSummary(args) => {
                    service
                        .proposal_artifact_summary_get(&args.proposal)
                        .await?;
                }
                ProposalsCommand::ArtifactDetail(args) => {
                    service.proposal_artifact_detail_get(&args.proposal).await?;
                }
                ProposalsCommand::ArtifactExport(args) => {
                    service
                        .proposal_artifact_export(
                            &args.proposal,
                            match args.format {
                                ProposalArtifactExportFormatArg::Json => {
                                    service::ProposalArtifactExportFormat::Json
                                }
                                ProposalArtifactExportFormatArg::Md => {
                                    service::ProposalArtifactExportFormat::Markdown
                                }
                            },
                            args.output.as_deref(),
                        )
                        .await?;
                }
                ProposalsCommand::ListForWorkunit(args) => {
                    service.proposal_list_for_workunit(&args.workunit).await?;
                }
                ProposalsCommand::Approve(args) => {
                    service
                        .proposal_approve(
                            &args.proposal,
                            args.reviewed_by,
                            args.review_note,
                            args.decision_type.map(|decision_type| match decision_type {
                                DecisionTypeArg::Accept => DecisionType::Accept,
                                DecisionTypeArg::Continue => DecisionType::Continue,
                                DecisionTypeArg::Redirect => DecisionType::Redirect,
                                DecisionTypeArg::MarkComplete => DecisionType::MarkComplete,
                                DecisionTypeArg::EscalateToHuman => DecisionType::EscalateToHuman,
                            }),
                            args.rationale,
                            args.worker,
                            args.worker_kind,
                            args.objective,
                            args.instructions,
                            args.acceptance_criteria,
                            args.stop_conditions,
                            args.expected_report_fields,
                        )
                        .await?;
                }
                ProposalsCommand::Reject(args) => {
                    service
                        .proposal_reject(&args.proposal, args.reviewed_by, args.review_note)
                        .await?;
                }
            }
        }
        TopCommand::Codex { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                CodexCommand::Review { command } => match command {
                    CodexReviewCommand::List(args) => {
                        service
                            .codex_decision_list(
                                args.filters.thread.as_deref(),
                                args.filters.assignment.as_deref(),
                                args.filters.workstream.as_deref(),
                                args.filters.workunit.as_deref(),
                                args.filters.supervisor.as_deref(),
                                args.filters.status.map(Into::into),
                                args.filters.kind.map(Into::into),
                                args.include_closed,
                                args.filters.include_superseded,
                                false,
                                args.filters.limit,
                            )
                            .await?;
                    }
                    CodexReviewCommand::Queue(args) => {
                        service
                            .codex_decision_list(
                                args.filters.thread.as_deref(),
                                args.filters.assignment.as_deref(),
                                args.filters.workstream.as_deref(),
                                args.filters.workunit.as_deref(),
                                args.filters.supervisor.as_deref(),
                                args.filters.status.map(Into::into),
                                args.filters.kind.map(Into::into),
                                false,
                                args.filters.include_superseded,
                                true,
                                args.filters.limit,
                            )
                            .await?;
                    }
                    CodexReviewCommand::History(args) => {
                        service
                            .codex_decision_history(
                                args.thread.as_deref(),
                                args.assignment.as_deref(),
                                args.include_superseded,
                                args.limit,
                            )
                            .await?;
                    }
                    CodexReviewCommand::Get(args) => {
                        service.codex_decision_get(&args.decision).await?;
                    }
                    CodexReviewCommand::ProposeSteer(args) => {
                        service
                            .codex_decision_propose_steer(
                                &args.thread,
                                &args.text,
                                args.requested_by,
                                args.rationale_note,
                            )
                            .await?;
                    }
                    CodexReviewCommand::ReplacePendingSteer(args) => {
                        service
                            .codex_decision_replace_pending_steer(
                                &args.decision,
                                &args.text,
                                args.requested_by,
                                args.rationale_note,
                            )
                            .await?;
                    }
                    CodexReviewCommand::RecordNoAction(args) => {
                        service
                            .codex_decision_record_no_action(
                                &args.decision,
                                args.reviewed_by,
                                args.review_note,
                            )
                            .await?;
                    }
                    CodexReviewCommand::ManualRefresh(args) => {
                        service
                            .codex_decision_manual_refresh(
                                args.thread.as_deref(),
                                args.assignment.as_deref(),
                                args.requested_by,
                                args.rationale_note,
                            )
                            .await?;
                    }
                    CodexReviewCommand::Approve(args) => {
                        service
                            .codex_decision_approve_and_send(
                                &args.decision,
                                args.reviewed_by,
                                args.review_note,
                            )
                            .await?;
                    }
                    CodexReviewCommand::Reject(args) => {
                        service
                            .codex_decision_reject(
                                &args.decision,
                                args.reviewed_by,
                                args.review_note,
                            )
                            .await?;
                    }
                },
            }
        }
        TopCommand::Prompt(args) => {
            let service = SupervisorService::load(&overrides).await?;
            let _ = service.prompt(&args.thread, &args.text).await?;
        }
        TopCommand::Quickstart(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service.quickstart(args.cwd, args.model, &args.text).await?;
        }
    }

    Ok(())
}

fn launch_tui() -> Result<()> {
    let binary = resolve_tui_binary();
    let status = Command::new(&binary)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to launch `{}`", binary.display()))?;

    if status.success() {
        Ok(())
    } else {
        bail!("`{}` exited with status {status}", binary.display())
    }
}

fn resolve_tui_binary() -> PathBuf {
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            let candidate = parent.join(tui_binary_name());
            if candidate.is_file() {
                return candidate;
            }
        }
    }

    PathBuf::from(tui_binary_name())
}

fn tui_binary_name() -> &'static str {
    if cfg!(windows) {
        "orcas-tui.exe"
    } else {
        "orcas-tui"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn top_level_help_mentions_operator_cli() {
        let help = Cli::command().render_help().to_string();

        assert!(help.contains("Orcas operator CLI"));
        assert!(help.contains("--version"));
    }

    #[test]
    fn top_level_version_matches_crate_version() {
        let version = Cli::command().render_version().to_string();

        assert!(version.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn daemon_help_mentions_lifecycle_wrapper() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("daemon")
            .expect("daemon subcommand")
            .render_help()
            .to_string();

        assert!(help.contains("Launch and manage the Orcas daemon"));
    }

    #[test]
    fn global_help_mentions_runtime_override_flags() {
        let help = Cli::command().render_help().to_string();

        assert!(help.contains("--codex-bin"));
        assert!(help.contains("--listen-url"));
        assert!(help.contains("--connect-only"));
        assert!(help.contains("--force-spawn"));
    }

    #[test]
    fn top_level_help_mentions_only_canonical_planning_namespaces() {
        let help = Cli::command().render_help().to_string();

        assert!(help.contains("workstreams"));
        assert!(help.contains("workunits"));
        assert!(help.contains("tracked-threads"));
        assert!(!help.contains("legacy-workstreams"));
        assert!(!help.contains("legacy-workunits"));
    }

    #[test]
    fn workstreams_help_marks_authority_surface_as_canonical() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("workstreams")
            .expect("workstreams subcommand")
            .render_help()
            .to_string();

        assert!(help.contains("Canonical authority-backed CRUD for planning workstreams"));
    }

    #[test]
    fn parses_top_level_daemon_status_command() {
        let cli = Cli::parse_from(["orcas", "daemon", "status"]);

        match cli.command {
            TopCommand::Daemon {
                command: DaemonCommand::Status,
            } => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_tui_command() {
        let cli = Cli::parse_from(["orcas", "tui"]);

        match cli.command {
            TopCommand::Tui => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_doctor_command() {
        let cli = Cli::parse_from(["orcas", "doctor"]);

        match cli.command {
            TopCommand::Doctor => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn global_runtime_mode_flags_conflict() {
        let result = Cli::try_parse_from(["orcas", "--connect-only", "--force-spawn", "doctor"]);

        assert!(result.is_err());
    }

    #[test]
    fn parses_tracked_thread_create_command() {
        let cli = Cli::parse_from([
            "orcas",
            "tracked-threads",
            "create",
            "--workunit",
            "wu-1",
            "--title",
            "Thread record",
            "--root-dir",
            "/tmp/orcas",
            "--model",
            "gpt-5.4",
        ]);

        match cli.command {
            TopCommand::TrackedThreads {
                command: TrackedThreadsCommand::Create(args),
            } => {
                assert_eq!(args.workunit, "wu-1");
                assert_eq!(args.title, "Thread record");
                assert_eq!(args.root_dir, "/tmp/orcas");
                assert_eq!(args.model.as_deref(), Some("gpt-5.4"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tracked_thread_edit_command() {
        let cli = Cli::parse_from([
            "orcas",
            "tracked-threads",
            "edit",
            "--tracked-thread",
            "tt-1",
            "--binding-state",
            "detached",
            "--model",
            "gpt-5.5",
        ]);

        match cli.command {
            TopCommand::TrackedThreads {
                command: TrackedThreadsCommand::Edit(args),
            } => {
                assert_eq!(args.tracked_thread, "tt-1");
                assert!(matches!(
                    args.binding_state,
                    Some(TrackedThreadBindingStateArg::Detached)
                ));
                assert_eq!(args.model.as_deref(), Some("gpt-5.5"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn legacy_workstream_namespace_is_not_exposed() {
        assert!(Cli::try_parse_from(["orcas", "legacy-workstreams", "create"]).is_err());
        assert!(Cli::try_parse_from(["orcas", "legacy-workstreams", "list"]).is_err());
    }

    #[test]
    fn legacy_workunit_namespace_is_not_exposed() {
        assert!(Cli::try_parse_from(["orcas", "legacy-workunits", "create"]).is_err());
        assert!(Cli::try_parse_from(["orcas", "legacy-workunits", "list"]).is_err());
    }

    #[test]
    fn parses_codex_review_propose_steer_command() {
        let cli = Cli::parse_from([
            "orcas",
            "codex",
            "review",
            "propose-steer",
            "--thread",
            "thread-1",
            "--text",
            "stay focused",
            "--requested-by",
            "cli_user",
        ]);

        match cli.command {
            TopCommand::Codex {
                command:
                    CodexCommand::Review {
                        command: CodexReviewCommand::ProposeSteer(args),
                    },
            } => {
                assert_eq!(args.thread, "thread-1");
                assert_eq!(args.text, "stay focused");
                assert_eq!(args.requested_by.as_deref(), Some("cli_user"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_codex_review_replace_pending_steer_command() {
        let cli = Cli::parse_from([
            "orcas",
            "codex",
            "review",
            "replace-pending-steer",
            "--decision",
            "std-7",
            "--text",
            "updated steer text",
        ]);

        match cli.command {
            TopCommand::Codex {
                command:
                    CodexCommand::Review {
                        command: CodexReviewCommand::ReplacePendingSteer(args),
                    },
            } => {
                assert_eq!(args.decision, "std-7");
                assert_eq!(args.text, "updated steer text");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_codex_review_record_no_action_command() {
        let cli = Cli::parse_from([
            "orcas",
            "codex",
            "review",
            "record-no-action",
            "--decision",
            "std-7",
            "--reviewed-by",
            "cli_user",
        ]);

        match cli.command {
            TopCommand::Codex {
                command:
                    CodexCommand::Review {
                        command: CodexReviewCommand::RecordNoAction(args),
                    },
            } => {
                assert_eq!(args.decision, "std-7");
                assert_eq!(args.reviewed_by.as_deref(), Some("cli_user"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_codex_review_manual_refresh_command() {
        let cli = Cli::parse_from([
            "orcas",
            "codex",
            "review",
            "manual-refresh",
            "--thread",
            "thread-1",
            "--requested-by",
            "cli_user",
        ]);

        match cli.command {
            TopCommand::Codex {
                command:
                    CodexCommand::Review {
                        command: CodexReviewCommand::ManualRefresh(args),
                    },
            } => {
                assert_eq!(args.thread.as_deref(), Some("thread-1"));
                assert_eq!(args.assignment, None);
                assert_eq!(args.requested_by.as_deref(), Some("cli_user"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_codex_review_queue_command_with_filters() {
        let cli = Cli::parse_from([
            "orcas",
            "codex",
            "review",
            "queue",
            "--workstream",
            "ws-1",
            "--kind",
            "steer-active-turn",
            "--limit",
            "5",
        ]);

        match cli.command {
            TopCommand::Codex {
                command:
                    CodexCommand::Review {
                        command: CodexReviewCommand::Queue(args),
                    },
            } => {
                assert_eq!(args.filters.workstream.as_deref(), Some("ws-1"));
                assert!(matches!(
                    args.filters.kind,
                    Some(CodexDecisionKindArg::SteerActiveTurn)
                ));
                assert_eq!(args.filters.limit, Some(5));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_codex_review_history_command() {
        let cli = Cli::parse_from([
            "orcas",
            "codex",
            "review",
            "history",
            "--assignment",
            "cta-1",
            "--limit",
            "20",
        ]);

        match cli.command {
            TopCommand::Codex {
                command:
                    CodexCommand::Review {
                        command: CodexReviewCommand::History(args),
                    },
            } => {
                assert_eq!(args.assignment.as_deref(), Some("cta-1"));
                assert_eq!(args.limit, Some(20));
                assert!(args.include_superseded);
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_proposal_artifact_summary_command() {
        let cli = Cli::parse_from([
            "orcas",
            "proposals",
            "artifact-summary",
            "--proposal",
            "proposal-1",
        ]);

        match cli.command {
            TopCommand::Proposals {
                command: ProposalsCommand::ArtifactSummary(args),
            } => {
                assert_eq!(args.proposal, "proposal-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_proposal_artifact_detail_command() {
        let cli = Cli::parse_from([
            "orcas",
            "proposals",
            "artifact-detail",
            "--proposal",
            "proposal-1",
        ]);

        match cli.command {
            TopCommand::Proposals {
                command: ProposalsCommand::ArtifactDetail(args),
            } => {
                assert_eq!(args.proposal, "proposal-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_proposal_artifact_export_command() {
        let cli = Cli::parse_from([
            "orcas",
            "proposals",
            "artifact-export",
            "--proposal",
            "proposal-1",
            "--format",
            "md",
            "--output",
            "/tmp/proposal.md",
        ]);

        match cli.command {
            TopCommand::Proposals {
                command: ProposalsCommand::ArtifactExport(args),
            } => {
                assert_eq!(args.proposal, "proposal-1");
                assert!(matches!(args.format, ProposalArtifactExportFormatArg::Md));
                assert_eq!(args.output, Some(PathBuf::from("/tmp/proposal.md")));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn rejects_supervisor_namespace() {
        assert!(Cli::try_parse_from(["orcas", "supervisor", "doctor"]).is_err());
    }

    #[test]
    fn rejects_codex_decisions_namespace() {
        assert!(Cli::try_parse_from(["orcas", "codex", "decisions", "list"]).is_err());
    }

    #[test]
    fn rejects_flat_codex_review_verbs() {
        assert!(Cli::try_parse_from(["orcas", "codex", "list"]).is_err());
    }
}
