#![allow(warnings)]

mod remote;
mod docs;
mod service;
mod snapshot;
mod skill_runtime;
mod streaming;
mod tui;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use tracing::info;
use tt_core::{
    AppPaths, DecisionType, WorkUnitStatus, WorkstreamStatus, authority, init_file_logger,
};
use tt_core::lane::LaneCleanupScope as LaneCleanupScopeModel;
use tt_core::lane::LanePaths;

use snapshot::{ContextCommand as SnapshotContextCommand, SnapshotCommand as SnapshotRuntimeCommand, WorkspaceCommand as SnapshotWorkspaceCommand};
use remote::{RemoteCommand, run_remote};
use service::{RuntimeOverrides, SupervisorService};
use skill_runtime::TTSkillBackend;
use tt_skills::{SkillCommand as RuntimeSkillCommand, SkillContext as RuntimeSkillContext};

#[derive(Debug, Parser)]
#[command(name = "tt", version, about = "tt control plane")]
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
        env = "TT_SERVER_URL",
        help = "Base URL for the operator server"
    )]
    server_url: Option<String>,
    #[arg(
        long,
        global = true,
        env = "TT_OPERATOR_API_TOKEN",
        help = "Bearer token for operator-server APIs"
    )]
    operator_api_token: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Override the local TT binary path for this command"
    )]
    tt_bin: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Override the upstream TT app-server WebSocket URL"
    )]
    listen_url: Option<String>,
    #[arg(long, global = true, help = "Enable inbox mirroring to a server URL")]
    inbox_mirror_server_url: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Override the default working directory for this command"
    )]
    cwd: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Override the default worktree root for project and TT spawn commands"
    )]
    worktree_root: Option<PathBuf>,
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
        help = "Require attach-only mode for this process"
    )]
    connect_only: bool,
    #[arg(
        long,
        global = true,
        default_value_t = false,
        conflicts_with = "connect_only",
        help = "Legacy runtime override for spawn-capable processes"
    )]
    force_spawn: bool,
}

#[derive(Debug, Subcommand)]
enum TopCommand {
    #[command(about = "Launch and manage the tt daemon")]
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    #[command(about = "Inspect the current TT state and surfaces")]
    Doctor,
    #[command(about = "Export rendered CLI documentation")]
    Docs {
        #[command(subcommand)]
        command: DocsCommand,
    },
    #[command(about = "Run TT commands against a remote runtime")]
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },
    #[command(about = "Inspect the recent TT event stream")]
    Events {
        #[command(subcommand)]
        command: EventsCommand,
    },
    #[command(name = "project")]
    #[command(about = "Manage durable TT project records")]
    Workstream {
        #[command(subcommand)]
        command: WorkstreamCommand,
    },
    #[command(name = "worktree")]
    #[command(about = "Canonical authority-backed CRUD for planning work units")]
    Workunit {
        #[command(subcommand)]
        command: WorkunitCommand,
    },
    #[command(about = "Capture notes, review gaps, and turn TODOs into plans")]
    Todo {
        #[command(subcommand)]
        command: TodoCommand,
    },
    #[command(about = "Start an implementation thread for the current branch")]
    Develop(ModeSpawnArgs),
    #[command(about = "Start a validation thread for the current branch")]
    Test(ModeSpawnArgs),
    #[command(about = "Start a repo-branch coordination thread")]
    Integrate(ModeSpawnArgs),
    #[command(about = "Start a discuss-only thread")]
    Chat(ModeSpawnArgs),
    #[command(about = "Start a recon and gap-finding thread")]
    Learn(ModeSpawnArgs),
    #[command(about = "Start a handoff thread")]
    Handoff(ModeSpawnArgs),
    #[command(about = "Inspect tracked and untracked changes before cleanup")]
    Diff(DiffArgs),
    #[command(about = "Fork a new child thread and worktree from the current context")]
    Split(SplitArgs),
    #[command(about = "Tear down the current worktree according to policy")]
    Close(CloseArgs),
    #[command(about = "Suspend the current worktree without cleanup")]
    Park(ParkArgs),
    #[command(hide = true)]
    Roles {
        #[command(subcommand)]
        command: RolesCommand,
    },
    #[command(about = "Inspect and manage TT-derived git worktrees")]
    Worktrees,
    #[command(about = "Manage the shared tt app-server lifecycle")]
    AppServer {
        #[command(subcommand)]
        command: AppServerCommand,
    },
    #[command(about = "Manage lane-local runtimes and rendered directory state")]
    Lane {
        #[command(subcommand)]
        command: LaneCommand,
    },
    #[command(about = "Create, fork, diff, and prune TT snapshots")]
    Snapshot {
        #[command(subcommand)]
        command: SnapshotRuntimeCommand,
    },
    #[command(about = "Edit snapshot context selection and pinning")]
    Context {
        #[command(subcommand)]
        command: SnapshotContextCommand,
    },
    #[command(about = "Bind snapshots to workspace and git state")]
    Workspace {
        #[command(subcommand)]
        command: SnapshotWorkspaceCommand,
    },
    #[command(about = "Open the tt dashboard TUI")]
    Tui,
    #[command(hide = true)]
    Supervisor {
        #[command(subcommand)]
        command: SupervisorCommand,
    },
    #[command(about = "Invoke the TT app-embedded command surface")]
    App {
        #[command(subcommand)]
        command: AppCommand,
    },
    #[command(about = "Coordinate the desktop window manager")]
    I3 {
        #[command(subcommand)]
        command: I3Command,
    },
    #[command(about = "Run a typed skill runtime command")]
    Skill {
        #[command(subcommand)]
        command: RuntimeSkillCommand,
    },
    #[command(hide = true)]
    TT {
        #[command(subcommand)]
        command: TTCommand,
    },
    #[command(about = "Send a single prompt to a thread")]
    Prompt(PromptArgs),
    #[command(about = "Launch a quick TT session from freeform input")]
    Quickstart(QuickstartArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Launch and manage the tt daemon")]
enum DaemonCommand {
    Start,
    Status,
    Restart,
    Stop,
}

#[derive(Debug, Subcommand)]
#[command(about = "Export rendered CLI documentation")]
enum DocsCommand {
    ExportCli(DocsExportCliArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Manage the shared tt app-server lifecycle")]
enum AppServerCommand {
    /// Register a named app-server instance.
    Add(AppServerNameArgs),
    /// Forget a named app-server instance.
    Remove(AppServerNameArgs),
    /// Start a named app-server instance.
    Start(AppServerNameArgs),
    /// Stop a named app-server instance.
    Stop(AppServerNameArgs),
    /// Restart a named app-server instance.
    Restart(AppServerNameArgs),
    /// Show the status of a named app-server instance.
    Status(AppServerNameArgs),
    /// Show the configuration of a named app-server instance.
    Info(AppServerNameArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Inspect tt role definitions")]
enum RolesCommand {
    /// List available role definitions.
    List,
    /// Show a specific role definition.
    Info(RoleRefArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Inspect available TT models")]
enum ModelsCommand {
    /// List models for a workstream.
    List(ModelsListArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Inspect TT thread records")]
enum TTThreadsCommand {
    /// List threads for a workstream.
    List(ThreadListArgs),
    /// List loaded threads for a workstream.
    ListLoaded(ThreadListArgs),
    /// Read a thread by id.
    Read(ThreadRefArgs),
    /// Start a new thread.
    Start(ThreadStartArgs),
    /// Resume an existing thread.
    Resume(ThreadResumeArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "TT lane worktree lifecycle helpers")]
enum TTWorktreeCommand {
    /// Create a new TT-managed worktree.
    Add(TTWorktreeAddArgs),
    /// Prune a TT-managed worktree.
    Prune(TTWorktreePruneArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Manage lane-local runtimes and rendered directory state")]
enum LaneCommand {
    /// List rendered lane roots and attachment counts.
    List,
    /// Bootstrap a new lane with rendered directory state and repo checkouts.
    Init(LaneInitArgs),
    /// Print the current lane manifest, worktrees, and attachment summary.
    Inspect(LaneInspectArgs),
    /// Bind a tracked thread to a lane workspace.
    Attach(LaneAttachArgs),
    /// Unbind a tracked thread from a lane workspace.
    Detach(LaneDetachArgs),
    /// Clean up lane runtime state according to the requested scope.
    Cleanup(LaneCleanupArgs),
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// Show the currently active session.
    Active,
}

#[derive(Debug, Subcommand)]
enum EventsCommand {
    /// Show recent events.
    Recent(EventsRecentArgs),
    /// Watch events in real time.
    Watch(EventsWatchArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Manage durable tt project records")]
enum WorkstreamCommand {
    /// Add a project record to a repository.
    Add(WorkstreamAddArgs),
    /// Create a durable project record.
    Create(WorkstreamCreateArgs),
    /// Edit a durable project record.
    Edit(WorkstreamEditArgs),
    /// Delete a durable project record.
    Delete(WorkstreamDeleteArgs),
    /// List durable project records.
    List,
    /// Get a durable project record.
    Get(WorkstreamRefArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Canonical authority-backed CRUD for planning work units")]
enum WorkunitCommand {
    /// Create a planning work unit.
    Create(WorkunitCreateArgs),
    /// Edit a planning work unit.
    Edit(WorkunitEditArgs),
    /// Delete a planning work unit.
    Delete(WorkunitRefArgs),
    /// List planning work units.
    List(WorkunitListArgs),
    /// Get a planning work unit.
    Get(WorkunitRefArgs),
    /// Work with tracked-thread planning records.
    Thread {
        #[command(subcommand)]
        command: WorkunitThreadCommand,
    },
    /// Work with tracked-thread planning record workspaces.
    Workspace {
        #[command(subcommand)]
        command: WorkunitWorkspaceCommand,
    },
}

#[derive(Debug, Subcommand)]
#[command(about = "Canonical authority-backed CRUD for tracked-thread planning records")]
enum WorkunitThreadCommand {
    /// Add a tracked thread to a work unit.
    Add(TrackedThreadCreateArgs),
    /// Update a tracked thread.
    Set(TrackedThreadEditArgs),
    /// Remove a tracked thread from a work unit.
    Remove(TrackedThreadRefArgs),
    /// List tracked threads for a work unit.
    List(TrackedThreadListArgs),
    /// Get a tracked thread record.
    Get(TrackedThreadRefArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Workspace operations for tracked-thread planning records")]
enum WorkunitWorkspaceCommand {
    /// Prepare the tracked-thread workspace.
    PrepareWorkspace(TrackedThreadRefArgs),
    /// Refresh the tracked-thread workspace.
    RefreshWorkspace(TrackedThreadRefArgs),
    /// Assess merge readiness for the workspace.
    MergePrep(TrackedThreadRefArgs),
    /// Authorize merging the workspace.
    AuthorizeMerge(TrackedThreadRefArgs),
    /// Execute landing for the workspace.
    ExecuteLanding(TrackedThreadRefArgs),
    /// Prune the tracked-thread workspace.
    PruneWorkspace(TrackedThreadRefArgs),
}

#[derive(Debug, Subcommand)]
#[command(about = "Supervisor-owned planning session orchestration")]
enum PlanCommand {
    #[command(
        about = "Create a draft planning session; readiness must be set later with mark-ready-for-review"
    )]
    Create(PlanningSessionCreateArgs),
    Get(PlanningSessionRefArgs),
    List(PlanningSessionListArgs),
    #[command(
        about = "Update the descriptive planning summary only; use mark-ready-for-review for readiness"
    )]
    UpdateSummary(PlanningSessionUpdateSummaryArgs),
    #[command(about = "Request more supervisor context while the session is still chatting")]
    RequestSupervisorContext(PlanningSessionRequestSupervisorContextArgs),
    #[command(about = "Request the bounded one-turn research assignment for this session")]
    RequestResearch(PlanningSessionRequestResearchArgs),
    #[command(about = "Explicitly transition a chat session into awaiting-approval")]
    MarkReadyForReview(PlanningSessionMarkReadyForReviewArgs),
    #[command(about = "Abort the planning session without mutating canonical plan state")]
    Abort(PlanningSessionAbortArgs),
    #[command(about = "Stage a canonical plan revision proposal from the session summary")]
    Approve(PlanningSessionApproveArgs),
    #[command(about = "Reject the planning session without mutating canonical plan state")]
    Reject(PlanningSessionRejectArgs),
    #[command(about = "Supersede the planning session without mutating canonical plan state")]
    Supersede(PlanningSessionSupersedeArgs),
}

#[derive(Debug, Subcommand)]
enum AssignmentsCommand {
    /// Start a worker assignment.
    Start(AssignmentStartArgs),
    /// Get a worker assignment.
    Get(AssignmentRefArgs),
    /// Inspect assignment communication.
    Communication(AssignmentRefArgs),
}

#[derive(Debug, Subcommand)]
enum ReportsCommand {
    /// Get a report.
    Get(ReportRefArgs),
    /// List reports for a work unit.
    ListForWorkunit(WorkunitRefArgs),
}

#[derive(Debug, Subcommand)]
enum DecisionsCommand {
    /// Apply a supervisor decision.
    Apply(DecisionApplyArgs),
}

#[derive(Debug, Subcommand)]
enum ProposalsCommand {
    /// Create a proposal.
    Create(ProposalCreateArgs),
    /// Get a proposal.
    Get(ProposalRefArgs),
    /// Show the proposal artifact summary.
    ArtifactSummary(ProposalRefArgs),
    /// Show the proposal artifact detail.
    ArtifactDetail(ProposalRefArgs),
    /// Export a proposal artifact.
    ArtifactExport(ProposalArtifactExportArgs),
    /// List proposals for a work unit.
    ListForWorkunit(WorkunitRefArgs),
    /// Approve a proposal.
    Approve(ProposalApproveArgs),
    /// Reject a proposal.
    Reject(ProposalRejectArgs),
}

#[derive(Debug, Subcommand)]
enum TTCommand {
    /// Inspect available TT models.
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
    /// Spawn a role-backed TT thread.
    Spawn(TTSpawnArgs),
    /// Resume a TT thread.
    Resume(TTResumeArgs),
    /// Manage TT worktrees.
    Worktree {
        #[command(subcommand)]
        command: TTWorktreeCommand,
    },
    /// Manage TT thread records.
    Threads {
        #[command(subcommand)]
        command: TTThreadsCommand,
    },
    /// Inspect TT turns.
    Turns {
        #[command(subcommand)]
        command: TurnsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AppCommand {
    /// Invoke the embedded TT command surface.
    TT {
        #[command(subcommand)]
        command: TTCommand,
    },
}

#[derive(Debug, Subcommand)]
enum I3Command {
    /// Report desktop/window-manager status.
    Status,
    /// Start desktop/window-manager integration.
    Start,
    /// Attach to the current desktop/window-manager session.
    Attach,
}

#[derive(Debug, Subcommand)]
enum SupervisorCommand {
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    Work {
        #[command(subcommand)]
        command: SupervisorWorkCommand,
    },
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SupervisorWorkCommand {
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
}

#[derive(Debug, Subcommand)]
enum TurnsCommand {
    /// List the active turns.
    ListActive,
    /// Show recent turns for a thread.
    Recent(TurnsRecentArgs),
    /// Get a specific turn by thread and turn id.
    Get(TurnRefArgs),
}

#[derive(Debug, Subcommand)]
enum ReviewCommand {
    /// List review decisions.
    List(TTDecisionListArgs),
    /// Show the pending review queue.
    Queue(TTDecisionQueueArgs),
    /// Show review decision history.
    History(TTDecisionHistoryArgs),
    /// Get a specific review decision.
    Get(TTDecisionRefArgs),
    /// Propose a steer decision.
    ProposeSteer(TTDecisionProposeSteerArgs),
    /// Replace a pending steer decision.
    ReplacePendingSteer(TTDecisionReplacePendingSteerArgs),
    /// Record a no-action review outcome.
    RecordNoAction(TTDecisionRecordNoActionArgs),
    /// Manually refresh review state.
    ManualRefresh(TTDecisionManualRefreshArgs),
    /// Approve a review decision.
    Approve(TTDecisionApproveArgs),
    /// Reject a review decision.
    Reject(TTDecisionRejectArgs),
}

#[derive(Debug, Clone, Args)]
struct ModelsListArgs {
    #[arg(long, help = "Workstream to inspect models for")]
    workstream: String,
}

#[derive(Debug, Clone, Args)]
struct ThreadListArgs {
    #[arg(long, help = "Workstream to list threads for")]
    workstream: String,
}

#[derive(Debug, Clone, Args)]
struct ThreadRefArgs {
    #[arg(long, help = "Thread id to inspect")]
    thread: String,
}

#[derive(Debug, Clone, Args)]
struct ThreadStartArgs {
    #[arg(long, help = "Working directory to start the thread in")]
    cwd: Option<PathBuf>,
    #[arg(long, help = "Model to use for the spawned thread")]
    model: Option<String>,
    #[arg(long, default_value_t = false, help = "Start the thread without a visible UI")]
    ephemeral: bool,
}

#[derive(Debug, Clone, Args)]
struct ThreadResumeArgs {
    #[arg(long, help = "Thread id to resume")]
    thread: String,
    #[arg(long, help = "Working directory to resume the thread in")]
    cwd: Option<PathBuf>,
    #[arg(long, help = "Model to use while resuming the thread")]
    model: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TTResumeArgs {
    /// Thread id to resume.
    thread: String,
    #[arg(long, help = "Working directory to resume the thread in")]
    cwd: Option<PathBuf>,
    #[arg(long, help = "Model to use while resuming the thread")]
    model: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TTSpawnArgs {
    /// Role to use for the spawned thread.
    role: String,
    #[arg(long, help = "Existing workstream to attach the spawned thread to")]
    workstream: Option<String>,
    #[arg(
        long = "new-workstream",
        help = "Create a new workstream for the spawned thread"
    )]
    new_workstream: Option<String>,
    #[arg(long, help = "Repository root to bind the spawned thread to")]
    repo_root: Option<PathBuf>,
    #[arg(long, default_value_t = false, help = "Spawn the thread without a visible UI")]
    headless: bool,
    #[arg(long, help = "Model to use for the spawned thread")]
    model: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct ModeSpawnArgs {
    #[arg(long, help = "Existing workstream to attach the mode thread to")]
    workstream: Option<String>,
    #[arg(
        long = "new-workstream",
        help = "Create a new workstream for the mode thread"
    )]
    new_workstream: Option<String>,
    #[arg(long, help = "Repository root to bind the mode thread to")]
    repo_root: Option<PathBuf>,
    #[arg(long, default_value_t = false, help = "Spawn the thread without a visible UI")]
    headless: bool,
    #[arg(long, help = "Model to use for the spawned thread")]
    model: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct SplitArgs {
    #[arg(long, help = "Override the role for the child thread")]
    role: Option<String>,
    #[command(flatten)]
    spawn: ModeSpawnArgs,
    #[arg(long, default_value_t = false, help = "Mark the split thread as ephemeral")]
    ephemeral: bool,
}

#[derive(Debug, Clone, Args)]
struct CloseArgs {
    /// Selector describing the thread, branch, or workspace to close.
    selector: String,
    #[arg(long, default_value_t = false, help = "Force close even when safety checks fail")]
    force: bool,
}

#[derive(Debug, Clone, Args)]
struct ParkArgs {
    /// Selector describing the thread, branch, or workspace to park.
    selector: String,
    #[arg(long, help = "Optional note to carry with the parked state")]
    note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct DiffArgs {
    #[arg(long, help = "Optional selector for the branch or worktree to inspect")]
    selector: Option<String>,
    #[arg(long, help = "Optional repository root to inspect")]
    repo_root: Option<PathBuf>,
    #[arg(long, help = "Optional worktree path to inspect")]
    worktree_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
enum TodoCommand {
    /// Ingest notes into the active TODO ledger.
    Note(ModeSpawnArgs),
    /// Ask clarifying questions about the active TODO section.
    Review(ModeSpawnArgs),
    /// Turn the active TODO section into a plan.
    Plan(ModeSpawnArgs),
}

#[derive(Debug, Clone, Args)]
struct TTWorktreeAddArgs {
    /// Repository root to add the worktree under.
    repo_root: PathBuf,
    /// Worktree name to create.
    name: String,
}

#[derive(Debug, Clone, Args)]
struct TTWorktreePruneArgs {
    /// Worktree selector to prune.
    selector: String,
}

#[derive(Debug, Clone, Args)]
struct LaneInitArgs {
    /// Human-readable lane label to normalize into the lane slug.
    label: String,
    #[arg(
        long = "repo",
        help = "Repo to include in the lane in org/repo form; repeat for multiple repos"
    )]
    repos: Vec<String>,
}

#[derive(Debug, Clone, Args)]
struct LaneInspectArgs {
    /// Human-readable lane label to inspect.
    label: String,
}

#[derive(Debug, Clone, Args)]
struct LaneAttachArgs {
    /// Human-readable lane label that owns the workspace.
    label: String,
    #[arg(long, help = "Repo to bind in org/repo form")]
    repo: String,
    #[arg(long, help = "Workspace name within the lane repo; defaults to `default`")]
    workspace: Option<String>,
    #[arg(
        long = "tracked-thread",
        help = "Authority tracked-thread id to attach to the lane workspace"
    )]
    tracked_thread: String,
}

#[derive(Debug, Clone, Args)]
struct LaneDetachArgs {
    /// Human-readable lane label that owns the workspace.
    label: String,
    #[arg(long, help = "Repo to unbind in org/repo form")]
    repo: String,
    #[arg(long, help = "Workspace name within the lane repo; defaults to `default`")]
    workspace: Option<String>,
    #[arg(
        long = "tracked-thread",
        help = "Authority tracked-thread id to detach from the lane workspace"
    )]
    tracked_thread: String,
}

#[derive(Debug, Clone, Args)]
struct LaneCleanupArgs {
    /// Human-readable lane label to clean up.
    label: String,
    #[arg(long, help = "Optional repo scope in org/repo form")]
    repo: Option<String>,
    #[arg(long, help = "Optional workspace name within the lane repo")]
    workspace: Option<String>,
    #[arg(
        long,
        value_enum,
        default_value_t = LaneCleanupScopeArg::Runtime,
        help = "Cleanup scope to apply: runtime, worktree, repo, or lane"
    )]
    scope: LaneCleanupScopeArg,
    #[arg(long, default_value_t = false, help = "Bypass safety checks for dirty or attached state")]
    force: bool,
}

#[derive(Debug, Clone, Args)]
struct DocsExportCliArgs {
    #[arg(
        long,
        default_value = "docs/CLI_REFERENCE.md",
        help = "Write the generated CLI reference to this file"
    )]
    out: PathBuf,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LaneCleanupScopeArg {
    /// Remove only runtime state.
    Runtime,
    /// Remove runtime state and the active worktree.
    Worktree,
    /// Remove runtime state, the worktree, and the repo checkout.
    Repo,
    /// Remove the full lane subtree.
    Lane,
}

#[derive(Debug, Clone, Args)]
struct TurnRefArgs {
    #[arg(long, help = "Thread id to read from")]
    thread: String,
    #[arg(long, help = "Turn id to inspect")]
    turn: String,
}

#[derive(Debug, Clone, Args)]
struct TurnsRecentArgs {
    #[arg(long, help = "Thread id to inspect recent turns for")]
    thread: String,
    #[arg(long, default_value_t = 10, help = "Maximum number of turns to return")]
    limit: usize,
}

#[derive(Debug, Clone, Args)]
struct EventsRecentArgs {
    #[arg(long, default_value_t = 20, help = "Maximum number of events to return")]
    limit: usize,
}

#[derive(Debug, Clone, Args)]
struct EventsWatchArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Include full snapshot data in the event stream"
    )]
    snapshot: bool,
    #[arg(long, help = "Maximum number of events to watch before stopping")]
    count: Option<usize>,
}

#[derive(Debug, Clone, Args)]
struct AppServerNameArgs {
    #[arg(default_value = "default", help = "Named app-server instance to target")]
    name: String,
}

#[derive(Debug, Clone, Args)]
struct RoleRefArgs {
    /// Role id or name to inspect.
    role: String,
}

#[derive(Debug, Clone, Args)]
struct WorkstreamCreateArgs {
    #[arg(long, help = "Title for the durable project record")]
    title: String,
    #[arg(long, help = "Objective for the durable project record")]
    objective: String,
    #[arg(long, help = "Optional priority label")]
    priority: Option<String>,
    #[arg(long, help = "Optional TT home directory override")]
    tt_home: Option<String>,
    #[arg(long, help = "Optional SQLite home directory override")]
    sqlite_home: Option<String>,
    #[arg(long, help = "Optional listen URL override")]
    listen_url: Option<String>,
    #[arg(long, value_enum, help = "Transport used to connect the workstream")]
    transport_kind: Option<WorkstreamTransportKindArg>,
    #[arg(long, value_enum, help = "How app-server instances are managed")]
    app_server_policy: Option<WorkstreamAppServerPolicyArg>,
    #[arg(long, value_enum, help = "How execution connects to the workstream")]
    connection_mode: Option<WorkstreamExecutionConnectionModeArg>,
}

#[derive(Debug, Clone, Args)]
struct WorkstreamAddArgs {
    /// Repository root to add the project record under.
    repo_root: PathBuf,
    /// Project name to add.
    name: String,
}

#[derive(Debug, Clone, Args)]
struct WorkstreamRefArgs {
    #[arg(long, help = "Project record id or slug to inspect")]
    workstream: String,
}

#[derive(Debug, Clone, Args)]
struct WorkstreamDeleteArgs {
    /// Project record id or slug to delete.
    workstream: String,
}

#[derive(Debug, Clone, Args)]
struct WorkstreamEditArgs {
    #[arg(long, help = "Project record id or slug to edit")]
    workstream: String,
    #[arg(long, help = "Updated title")]
    title: Option<String>,
    #[arg(long, help = "Updated objective")]
    objective: Option<String>,
    #[arg(long, value_enum, help = "Updated workstream status")]
    status: Option<WorkstreamStatusArg>,
    #[arg(long, help = "Updated priority label")]
    priority: Option<String>,
    #[arg(long, help = "Updated TT home directory override")]
    tt_home: Option<String>,
    #[arg(long, help = "Updated SQLite home directory override")]
    sqlite_home: Option<String>,
    #[arg(long, help = "Updated listen URL override")]
    listen_url: Option<String>,
    #[arg(long, value_enum, help = "Updated transport kind")]
    transport_kind: Option<WorkstreamTransportKindArg>,
    #[arg(long, value_enum, help = "Updated app-server policy")]
    app_server_policy: Option<WorkstreamAppServerPolicyArg>,
    #[arg(long, value_enum, help = "Updated execution connection mode")]
    connection_mode: Option<WorkstreamExecutionConnectionModeArg>,
    #[arg(long, help = "Clear any execution-scope override")]
    clear_execution_scope: bool,
}

#[derive(Debug, Clone, Args)]
struct WorkunitCreateArgs {
    #[arg(long, help = "Parent workstream id or slug")]
    workstream: String,
    #[arg(long, help = "Work unit title")]
    title: String,
    #[arg(long, help = "Work unit task description")]
    task: String,
    #[arg(long = "dependency", help = "Dependent work unit ids")]
    dependencies: Vec<String>,
}

#[derive(Debug, Clone, Args, Default)]
struct WorkunitListArgs {
    #[arg(long, help = "Optional workstream filter")]
    workstream: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct WorkunitRefArgs {
    #[arg(long, help = "Work unit id or slug to inspect")]
    workunit: String,
}

#[derive(Debug, Clone, Args)]
struct WorkunitEditArgs {
    #[arg(long, help = "Work unit id or slug to edit")]
    workunit: String,
    #[arg(long, help = "Updated title")]
    title: Option<String>,
    #[arg(long, help = "Updated task description")]
    task: Option<String>,
    #[arg(long, value_enum, help = "Updated work unit status")]
    status: Option<WorkUnitStatusArg>,
}

#[derive(Debug, Clone, Args, Default)]
struct TrackedThreadListArgs {
    #[arg(long, help = "Work unit id to list tracked threads for")]
    workunit: String,
}

#[derive(Debug, Clone, Args)]
struct TrackedThreadRefArgs {
    #[arg(long = "tracked-thread", help = "Tracked-thread id to inspect")]
    tracked_thread: String,
    #[arg(long, help = "Optional request note for the operation")]
    request_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TrackedThreadCreateArgs {
    #[arg(long, help = "Parent work unit id")]
    workunit: String,
    #[arg(long, help = "Tracked-thread title")]
    title: String,
    #[arg(long = "root-dir", help = "Root directory for the tracked thread")]
    root_dir: String,
    #[arg(long, help = "Optional thread notes")]
    notes: Option<String>,
    #[arg(long = "upstream-thread", help = "Optional upstream thread id")]
    upstream_thread: Option<String>,
    #[arg(long, help = "Optional model override")]
    model: Option<String>,
    #[command(flatten)]
    workspace: TrackedThreadWorkspaceArgs,
}

#[derive(Debug, Clone, Args)]
struct TrackedThreadEditArgs {
    #[arg(long = "tracked-thread", help = "Tracked-thread id to edit")]
    tracked_thread: String,
    #[arg(long, help = "Updated title")]
    title: Option<String>,
    #[arg(long = "root-dir", help = "Updated root directory")]
    root_dir: Option<String>,
    #[arg(long, help = "Updated thread notes")]
    notes: Option<String>,
    #[arg(long = "upstream-thread", help = "Updated upstream thread id")]
    upstream_thread: Option<String>,
    #[arg(long, value_enum, help = "Updated binding state")]
    binding_state: Option<TrackedThreadBindingStateArg>,
    #[arg(long, help = "Optional model override")]
    model: Option<String>,
    #[command(flatten)]
    workspace: TrackedThreadWorkspaceArgs,
}

#[derive(Debug, Clone, Args, Default)]
struct TrackedThreadWorkspaceArgs {
    #[arg(long = "workspace-repository-root", help = "Workspace repository root")]
    repository_root: Option<String>,
    #[arg(long = "workspace-worktree-path", help = "Workspace worktree path")]
    worktree_path: Option<String>,
    #[arg(long = "workspace-branch-name", help = "Workspace branch name")]
    branch_name: Option<String>,
    #[arg(long = "workspace-base-ref", help = "Workspace base ref")]
    base_ref: Option<String>,
    #[arg(long = "workspace-base-commit", help = "Workspace base commit")]
    base_commit: Option<String>,
    #[arg(long = "workspace-landing-target", help = "Workspace landing target")]
    landing_target: Option<String>,
    #[arg(long = "workspace-strategy", value_enum, help = "Workspace strategy")]
    strategy: Option<TrackedThreadWorkspaceStrategyArg>,
    #[arg(long = "workspace-landing-policy", value_enum, help = "Workspace landing policy")]
    landing_policy: Option<TrackedThreadWorkspaceLandingPolicyArg>,
    #[arg(long = "workspace-sync-policy", value_enum, help = "Workspace sync policy")]
    sync_policy: Option<TrackedThreadWorkspaceSyncPolicyArg>,
    #[arg(long = "workspace-cleanup-policy", value_enum, help = "Workspace cleanup policy")]
    cleanup_policy: Option<TrackedThreadWorkspaceCleanupPolicyArg>,
    #[arg(long = "workspace-status", value_enum, help = "Workspace status")]
    status: Option<TrackedThreadWorkspaceStatusArg>,
    #[arg(
        long = "workspace-last-reported-head-commit",
        help = "Last head commit reported for the workspace"
    )]
    last_reported_head_commit: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionRefArgs {
    #[arg(long = "session", help = "Planning session id to inspect")]
    session: String,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionListArgs {
    #[arg(long, help = "Optional workstream filter")]
    workstream: Option<String>,
    #[arg(long, default_value_t = false, help = "Include closed planning sessions")]
    include_closed: bool,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionSummaryArgs {
    #[arg(long, help = "Objective for the planning session")]
    objective: String,
    #[arg(long = "requirement", help = "Planning session requirements")]
    requirements: Vec<String>,
    #[arg(long = "constraint", help = "Planning session constraints")]
    constraints: Vec<String>,
    #[arg(long = "non-goal", help = "Planning session non-goals")]
    non_goals: Vec<String>,
    #[arg(long = "open-question", help = "Open questions that still need answers")]
    open_questions: Vec<String>,
    #[arg(
        long,
        value_enum,
        default_value_t = PlanningSessionResearchStatusArg::NotRequested,
        help = "Research status for the planning session"
    )]
    research_status: PlanningSessionResearchStatusArg,
    #[arg(long, help = "Draft plan summary")]
    draft_plan_summary: Option<String>,
    #[arg(
        long,
        default_value_t = false,
        help = "Reserved for the explicit mark-ready-for-review transition; create/update should leave this false"
    )]
    ready_for_review: bool,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionCreateArgs {
    #[arg(long, help = "Workstream id to create the session under")]
    workstream: String,
    #[arg(long = "planning-thread", help = "Optional planning thread id")]
    planning_thread_id: Option<String>,
    #[command(flatten)]
    summary: PlanningSessionSummaryArgs,
    #[arg(long, help = "Who created the session")]
    created_by: Option<String>,
    #[arg(long, help = "Optional request note")]
    request_note: Option<String>,
    #[arg(long, help = "Optional model override")]
    model: Option<String>,
    #[arg(long, help = "Optional working directory")]
    cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionUpdateSummaryArgs {
    #[arg(long = "session", help = "Planning session id to update")]
    session: String,
    #[command(flatten)]
    summary: PlanningSessionSummaryArgs,
    #[arg(long, help = "Who updated the session")]
    updated_by: Option<String>,
    #[arg(long, help = "Optional update note")]
    note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionRequestSupervisorContextArgs {
    #[arg(long = "session", help = "Planning session id to request context for")]
    session: String,
    #[arg(long, help = "Who requested the context")]
    requested_by: Option<String>,
    #[arg(long, help = "Optional request note")]
    note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionRequestResearchArgs {
    #[arg(long = "session", help = "Planning session id to request research for")]
    session: String,
    #[arg(long, help = "Worker to assign research to")]
    worker: String,
    #[arg(long, help = "Optional worker kind")]
    worker_kind: Option<String>,
    #[arg(long, help = "Optional model override")]
    model: Option<String>,
    #[arg(long, help = "Optional working directory")]
    cwd: Option<PathBuf>,
    #[arg(long, help = "Who requested the research")]
    requested_by: Option<String>,
    #[arg(long, help = "Optional research request note")]
    request_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionMarkReadyForReviewArgs {
    #[arg(long = "session", help = "Planning session id to mark ready")]
    session: String,
    #[arg(long, help = "Who marked the session ready")]
    updated_by: Option<String>,
    #[arg(long, help = "Optional readiness note")]
    note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionAbortArgs {
    #[arg(long = "session", help = "Planning session id to abort")]
    session: String,
    #[arg(long, help = "Who aborted the session")]
    updated_by: Option<String>,
    #[arg(long, help = "Optional abort note")]
    note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionApproveArgs {
    #[arg(long = "session", help = "Planning session id to approve")]
    session: String,
    #[arg(long, help = "Who approved the session")]
    approved_by: Option<String>,
    #[arg(long, help = "Optional review note")]
    review_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionRejectArgs {
    #[arg(long = "session", help = "Planning session id to reject")]
    session: String,
    #[arg(long, help = "Who rejected the session")]
    rejected_by: Option<String>,
    #[arg(long, help = "Optional review note")]
    review_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct PlanningSessionSupersedeArgs {
    #[arg(long = "session", help = "Planning session id to supersede")]
    session: String,
    #[arg(
        long = "superseded-by-session",
        help = "Replacement planning session id"
    )]
    superseded_by_session: Option<String>,
    #[arg(long, help = "Who superseded the session")]
    updated_by: Option<String>,
    #[arg(long, help = "Optional supersede note")]
    note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct AssignmentStartArgs {
    #[arg(long, help = "Work unit id to start the assignment for")]
    workunit: String,
    #[arg(long, help = "Worker to assign")]
    worker: String,
    #[arg(long, help = "Optional assignment instructions")]
    instructions: Option<String>,
    #[arg(long, help = "Optional worker kind")]
    worker_kind: Option<String>,
    #[arg(long, help = "Optional working directory")]
    cwd: Option<PathBuf>,
    #[arg(long, help = "Optional model override")]
    model: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct AssignmentRefArgs {
    #[arg(long, help = "Assignment id to inspect")]
    assignment: String,
}

#[derive(Debug, Clone, Args)]
struct ReportRefArgs {
    #[arg(long, help = "Report id to inspect")]
    report: String,
}

#[derive(Debug, Clone, Args)]
struct ProposalRefArgs {
    #[arg(long, help = "Proposal id to inspect")]
    proposal: String,
}

#[derive(Debug, Clone, Args)]
struct ProposalArtifactExportArgs {
    #[arg(long, help = "Proposal id to export")]
    proposal: String,
    #[arg(
        long,
        value_enum,
        default_value_t = ProposalArtifactExportFormatArg::Json,
        help = "Export format for the proposal artifact"
    )]
    format: ProposalArtifactExportFormatArg,
    #[arg(long, help = "Optional output path")]
    output: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
struct ProposalCreateArgs {
    #[arg(long, help = "Work unit id to create a proposal for")]
    workunit: String,
    #[arg(long, help = "Optional linked report id")]
    report: Option<String>,
    #[arg(long, help = "Optional creation note")]
    note: Option<String>,
    #[arg(long, help = "Who requested the proposal")]
    requested_by: Option<String>,
    #[arg(long, default_value_t = false, help = "Supersede open proposals for the same work unit")]
    supersede_open: bool,
}

#[derive(Debug, Clone, Args)]
struct ProposalApproveArgs {
    #[arg(long, help = "Proposal id to approve")]
    proposal: String,
    #[arg(long, help = "Optional review note")]
    review_note: Option<String>,
    #[arg(long, help = "Who reviewed the proposal")]
    reviewed_by: Option<String>,
    #[arg(long = "type", value_enum, help = "Decision type to apply")]
    decision_type: Option<DecisionTypeArg>,
    #[arg(long, help = "Optional rationale")]
    rationale: Option<String>,
    #[arg(long, help = "Worker to assign")]
    worker: Option<String>,
    #[arg(long, help = "Optional worker kind")]
    worker_kind: Option<String>,
    #[arg(long, help = "Optional objective override")]
    objective: Option<String>,
    #[arg(long = "instruction", help = "Worker instruction to include")]
    instructions: Vec<String>,
    #[arg(long = "acceptance", help = "Acceptance criteria to include")]
    acceptance_criteria: Vec<String>,
    #[arg(long = "stop-condition", help = "Stop conditions to include")]
    stop_conditions: Vec<String>,
    #[arg(
        long = "expected-report-field",
        help = "Expected report fields to validate"
    )]
    expected_report_fields: Vec<String>,
}

#[derive(Debug, Clone, Args)]
struct ProposalRejectArgs {
    #[arg(long, help = "Proposal id to reject")]
    proposal: String,
    #[arg(long, help = "Optional review note")]
    review_note: Option<String>,
    #[arg(long, help = "Who reviewed the proposal")]
    reviewed_by: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
struct TTDecisionFilterArgs {
    #[arg(long, help = "Thread id to filter by")]
    thread: Option<String>,
    #[arg(long, help = "Assignment id to filter by")]
    assignment: Option<String>,
    #[arg(long, help = "Workstream id to filter by")]
    workstream: Option<String>,
    #[arg(long, help = "Work unit id to filter by")]
    workunit: Option<String>,
    #[arg(long, help = "Supervisor id to filter by")]
    supervisor: Option<String>,
    #[arg(long, value_enum, help = "Decision status to filter by")]
    status: Option<TTDecisionStatusArg>,
    #[arg(long, value_enum, help = "Decision kind to filter by")]
    kind: Option<TTDecisionKindArg>,
    #[arg(long, default_value_t = false, help = "Include superseded decisions")]
    include_superseded: bool,
    #[arg(long, help = "Maximum number of decisions to return")]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Args, Default)]
struct TTDecisionListArgs {
    #[command(flatten)]
    filters: TTDecisionFilterArgs,
    #[arg(long, default_value_t = false, help = "Include closed decisions")]
    include_closed: bool,
}

#[derive(Debug, Clone, Args, Default)]
struct TTDecisionQueueArgs {
    #[command(flatten)]
    filters: TTDecisionFilterArgs,
}

#[derive(Debug, Clone, Args, Default)]
struct TTDecisionHistoryArgs {
    #[arg(long, help = "Thread id to inspect")]
    thread: Option<String>,
    #[arg(long, help = "Assignment id to inspect")]
    assignment: Option<String>,
    #[arg(long, default_value_t = true, help = "Include superseded decisions")]
    include_superseded: bool,
    #[arg(long, help = "Maximum number of decisions to return")]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Args)]
struct TTDecisionRefArgs {
    #[arg(long, help = "Decision id to inspect")]
    decision: String,
}

#[derive(Debug, Clone, Args)]
struct TTDecisionProposeSteerArgs {
    #[arg(long, help = "Thread id to attach the steer proposal to")]
    thread: String,
    #[arg(long, help = "Steer proposal text")]
    text: String,
    #[arg(long, help = "Who requested the steer proposal")]
    requested_by: Option<String>,
    #[arg(long, help = "Optional rationale note")]
    rationale_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TTDecisionReplacePendingSteerArgs {
    #[arg(long, help = "Decision id to replace")]
    decision: String,
    #[arg(long, help = "Replacement steer proposal text")]
    text: String,
    #[arg(long, help = "Who requested the replacement")]
    requested_by: Option<String>,
    #[arg(long, help = "Optional rationale note")]
    rationale_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TTDecisionRecordNoActionArgs {
    #[arg(long, help = "Decision id to record as no action")]
    decision: String,
    #[arg(long, help = "Who reviewed the decision")]
    reviewed_by: Option<String>,
    #[arg(long, help = "Optional review note")]
    review_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TTDecisionManualRefreshArgs {
    #[arg(long, help = "Thread id to refresh")]
    thread: Option<String>,
    #[arg(long, help = "Assignment id to refresh")]
    assignment: Option<String>,
    #[arg(long, help = "Who requested the refresh")]
    requested_by: Option<String>,
    #[arg(long, help = "Optional rationale note")]
    rationale_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TTDecisionApproveArgs {
    #[arg(long, help = "Decision id to approve")]
    decision: String,
    #[arg(long, help = "Who reviewed the decision")]
    reviewed_by: Option<String>,
    #[arg(long, help = "Optional review note")]
    review_note: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct TTDecisionRejectArgs {
    #[arg(long, help = "Decision id to reject")]
    decision: String,
    #[arg(long, help = "Who reviewed the decision")]
    reviewed_by: Option<String>,
    #[arg(long, help = "Optional review note")]
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
enum WorkstreamTransportKindArg {
    LocalAppServer,
    RemoteWebsocket,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WorkstreamAppServerPolicyArg {
    SharedCurrentDaemon,
    DedicatedPerWorkstream,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WorkstreamExecutionConnectionModeArg {
    ConnectOnly,
    SpawnIfNeeded,
    SpawnAlways,
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TrackedThreadWorkspaceStrategyArg {
    Shared,
    DedicatedThreadWorktree,
    Ephemeral,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TrackedThreadWorkspaceLandingPolicyArg {
    MergeToMain,
    MergeToCampaign,
    CherryPickOnly,
    Parked,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TrackedThreadWorkspaceSyncPolicyArg {
    Manual,
    RebaseBeforeCompletion,
    RebaseBeforeEachAssignment,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TrackedThreadWorkspaceCleanupPolicyArg {
    KeepUntilCampaignClosed,
    PruneAfterMerge,
    KeepForAudit,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TrackedThreadWorkspaceStatusArg {
    Requested,
    Ready,
    Dirty,
    Ahead,
    Behind,
    Conflicted,
    Merged,
    Abandoned,
    Pruned,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PlanningSessionResearchStatusArg {
    NotRequested,
    Requested,
    Completed,
    Failed,
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

impl From<WorkstreamTransportKindArg> for authority::WorkstreamTransportKind {
    fn from(value: WorkstreamTransportKindArg) -> Self {
        match value {
            WorkstreamTransportKindArg::LocalAppServer => Self::LocalAppServer,
            WorkstreamTransportKindArg::RemoteWebsocket => Self::RemoteWebsocket,
        }
    }
}

impl From<WorkstreamAppServerPolicyArg> for authority::WorkstreamAppServerPolicy {
    fn from(value: WorkstreamAppServerPolicyArg) -> Self {
        match value {
            WorkstreamAppServerPolicyArg::SharedCurrentDaemon => Self::SharedCurrentDaemon,
            WorkstreamAppServerPolicyArg::DedicatedPerWorkstream => Self::DedicatedPerWorkstream,
        }
    }
}

impl From<WorkstreamExecutionConnectionModeArg> for authority::WorkstreamExecutionConnectionMode {
    fn from(value: WorkstreamExecutionConnectionModeArg) -> Self {
        match value {
            WorkstreamExecutionConnectionModeArg::ConnectOnly => Self::ConnectOnly,
            WorkstreamExecutionConnectionModeArg::SpawnIfNeeded => Self::SpawnIfNeeded,
            WorkstreamExecutionConnectionModeArg::SpawnAlways => Self::SpawnAlways,
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

impl From<TrackedThreadWorkspaceStrategyArg> for authority::TrackedThreadWorkspaceStrategy {
    fn from(value: TrackedThreadWorkspaceStrategyArg) -> Self {
        match value {
            TrackedThreadWorkspaceStrategyArg::Shared => Self::Shared,
            TrackedThreadWorkspaceStrategyArg::DedicatedThreadWorktree => {
                Self::DedicatedThreadWorktree
            }
            TrackedThreadWorkspaceStrategyArg::Ephemeral => Self::Ephemeral,
        }
    }
}

impl From<TrackedThreadWorkspaceLandingPolicyArg>
    for authority::TrackedThreadWorkspaceLandingPolicy
{
    fn from(value: TrackedThreadWorkspaceLandingPolicyArg) -> Self {
        match value {
            TrackedThreadWorkspaceLandingPolicyArg::MergeToMain => Self::MergeToMain,
            TrackedThreadWorkspaceLandingPolicyArg::MergeToCampaign => Self::MergeToCampaign,
            TrackedThreadWorkspaceLandingPolicyArg::CherryPickOnly => Self::CherryPickOnly,
            TrackedThreadWorkspaceLandingPolicyArg::Parked => Self::Parked,
        }
    }
}

impl From<TrackedThreadWorkspaceSyncPolicyArg> for authority::TrackedThreadWorkspaceSyncPolicy {
    fn from(value: TrackedThreadWorkspaceSyncPolicyArg) -> Self {
        match value {
            TrackedThreadWorkspaceSyncPolicyArg::Manual => Self::Manual,
            TrackedThreadWorkspaceSyncPolicyArg::RebaseBeforeCompletion => {
                Self::RebaseBeforeCompletion
            }
            TrackedThreadWorkspaceSyncPolicyArg::RebaseBeforeEachAssignment => {
                Self::RebaseBeforeEachAssignment
            }
        }
    }
}

impl From<TrackedThreadWorkspaceCleanupPolicyArg>
    for authority::TrackedThreadWorkspaceCleanupPolicy
{
    fn from(value: TrackedThreadWorkspaceCleanupPolicyArg) -> Self {
        match value {
            TrackedThreadWorkspaceCleanupPolicyArg::KeepUntilCampaignClosed => {
                Self::KeepUntilCampaignClosed
            }
            TrackedThreadWorkspaceCleanupPolicyArg::PruneAfterMerge => Self::PruneAfterMerge,
            TrackedThreadWorkspaceCleanupPolicyArg::KeepForAudit => Self::KeepForAudit,
        }
    }
}

impl From<TrackedThreadWorkspaceStatusArg> for authority::TrackedThreadWorkspaceStatus {
    fn from(value: TrackedThreadWorkspaceStatusArg) -> Self {
        match value {
            TrackedThreadWorkspaceStatusArg::Requested => Self::Requested,
            TrackedThreadWorkspaceStatusArg::Ready => Self::Ready,
            TrackedThreadWorkspaceStatusArg::Dirty => Self::Dirty,
            TrackedThreadWorkspaceStatusArg::Ahead => Self::Ahead,
            TrackedThreadWorkspaceStatusArg::Behind => Self::Behind,
            TrackedThreadWorkspaceStatusArg::Conflicted => Self::Conflicted,
            TrackedThreadWorkspaceStatusArg::Merged => Self::Merged,
            TrackedThreadWorkspaceStatusArg::Abandoned => Self::Abandoned,
            TrackedThreadWorkspaceStatusArg::Pruned => Self::Pruned,
        }
    }
}

impl From<PlanningSessionResearchStatusArg> for tt_core::PlanningSessionResearchStatus {
    fn from(value: PlanningSessionResearchStatusArg) -> Self {
        match value {
            PlanningSessionResearchStatusArg::NotRequested => Self::NotRequested,
            PlanningSessionResearchStatusArg::Requested => Self::Requested,
            PlanningSessionResearchStatusArg::Completed => Self::Completed,
            PlanningSessionResearchStatusArg::Failed => Self::Failed,
        }
    }
}

impl TrackedThreadWorkspaceArgs {
    fn is_empty(&self) -> bool {
        self.repository_root.is_none()
            && self.worktree_path.is_none()
            && self.branch_name.is_none()
            && self.base_ref.is_none()
            && self.base_commit.is_none()
            && self.landing_target.is_none()
            && self.strategy.is_none()
            && self.landing_policy.is_none()
            && self.sync_policy.is_none()
            && self.cleanup_policy.is_none()
            && self.status.is_none()
            && self.last_reported_head_commit.is_none()
    }

    fn try_into_workspace(
        self,
        owner_tracked_thread_id: authority::TrackedThreadId,
    ) -> Result<Option<authority::TrackedThreadWorkspace>> {
        if self.is_empty() {
            return Ok(None);
        }

        let repository_root = self.repository_root.ok_or_else(|| {
            anyhow::anyhow!(
                "--workspace-repository-root is required when declaring a tracked-thread workspace"
            )
        })?;
        let worktree_path = self.worktree_path.ok_or_else(|| {
            anyhow::anyhow!(
                "--workspace-worktree-path is required when declaring a tracked-thread workspace"
            )
        })?;
        let branch_name = self.branch_name.ok_or_else(|| {
            anyhow::anyhow!(
                "--workspace-branch-name is required when declaring a tracked-thread workspace"
            )
        })?;
        let base_ref = self.base_ref.ok_or_else(|| {
            anyhow::anyhow!(
                "--workspace-base-ref is required when declaring a tracked-thread workspace"
            )
        })?;
        let landing_target = self.landing_target.ok_or_else(|| {
            anyhow::anyhow!(
                "--workspace-landing-target is required when declaring a tracked-thread workspace"
            )
        })?;
        let strategy = self
            .strategy
            .unwrap_or(TrackedThreadWorkspaceStrategyArg::DedicatedThreadWorktree);
        let landing_policy = self
            .landing_policy
            .unwrap_or(TrackedThreadWorkspaceLandingPolicyArg::MergeToMain);
        let sync_policy = self
            .sync_policy
            .unwrap_or(TrackedThreadWorkspaceSyncPolicyArg::Manual);
        let cleanup_policy = self
            .cleanup_policy
            .unwrap_or(TrackedThreadWorkspaceCleanupPolicyArg::KeepUntilCampaignClosed);
        let status = self
            .status
            .unwrap_or(TrackedThreadWorkspaceStatusArg::Requested);

        Ok(Some(authority::TrackedThreadWorkspace {
            repository_root,
            owner_tracked_thread_id,
            strategy: strategy.into(),
            worktree_path,
            branch_name,
            base_ref,
            base_commit: self.base_commit,
            landing_target,
            landing_policy: landing_policy.into(),
            sync_policy: sync_policy.into(),
            cleanup_policy: cleanup_policy.into(),
            last_reported_head_commit: self.last_reported_head_commit,
            status: status.into(),
        }))
    }
}
#[derive(Debug, Clone, Copy, ValueEnum)]
enum TTDecisionStatusArg {
    ProposedToHuman,
    Recorded,
    Sent,
    Rejected,
    Stale,
    Superseded,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TTDecisionKindArg {
    NextTurn,
    SteerActiveTurn,
    InterruptActiveTurn,
    NoAction,
}

impl From<TTDecisionStatusArg> for tt_core::SupervisorTurnDecisionStatus {
    fn from(value: TTDecisionStatusArg) -> Self {
        match value {
            TTDecisionStatusArg::ProposedToHuman => Self::ProposedToHuman,
            TTDecisionStatusArg::Recorded => Self::Recorded,
            TTDecisionStatusArg::Sent => Self::Sent,
            TTDecisionStatusArg::Rejected => Self::Rejected,
            TTDecisionStatusArg::Stale => Self::Stale,
            TTDecisionStatusArg::Superseded => Self::Superseded,
        }
    }
}

impl From<TTDecisionKindArg> for tt_core::SupervisorTurnDecisionKind {
    fn from(value: TTDecisionKindArg) -> Self {
        match value {
            TTDecisionKindArg::NextTurn => Self::NextTurn,
            TTDecisionKindArg::SteerActiveTurn => Self::SteerActiveTurn,
            TTDecisionKindArg::InterruptActiveTurn => Self::InterruptActiveTurn,
            TTDecisionKindArg::NoAction => Self::NoAction,
        }
    }
}

#[derive(Debug, Clone, Args)]
struct DecisionApplyArgs {
    #[arg(long, help = "Work unit id to apply the decision to")]
    workunit: String,
    #[arg(long, help = "Rationale for the decision")]
    rationale: String,
    #[arg(long, help = "Optional linked report id")]
    report: Option<String>,
    #[arg(long = "type", value_enum, help = "Decision type to apply")]
    decision_type: DecisionTypeArg,
    #[arg(long, help = "Optional instructions for the decision")]
    instructions: Option<String>,
    #[arg(long, help = "Optional worker to route to")]
    worker: Option<String>,
    #[arg(long, help = "Optional worker kind")]
    worker_kind: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct PromptArgs {
    #[arg(long, help = "Target thread id to receive the prompt")]
    thread: String,
    #[arg(long, help = "Prompt text to send to the thread")]
    text: String,
}

#[derive(Debug, Clone, Args)]
struct QuickstartArgs {
    #[arg(long, help = "Optional working directory for the quickstart session")]
    cwd: Option<PathBuf>,
    #[arg(long, help = "Optional model override for the quickstart session")]
    model: Option<String>,
    #[arg(long, help = "Freeform text used to seed the session")]
    text: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let paths = AppPaths::discover()?;
    paths.ensure().await?;
    init_file_logger("tt", &paths.logs_dir.join("tt.log"))?;
    info!("starting tt process");

    let cli = Cli::parse();
    let global = cli.global.clone();
    let overrides = RuntimeOverrides {
        // Server-facing remote client commands use the dedicated `remote`
        // subtree and do not depend on the local daemon surface.
        tt_bin: global.tt_bin.clone(),
        listen_url: global.listen_url.clone(),
        inbox_mirror_server_url: global.inbox_mirror_server_url.clone(),
        cwd: global.cwd.clone(),
        worktree_root: global.worktree_root.clone(),
        model: global.model.clone(),
        connect_only: global.connect_only,
        force_spawn: global.force_spawn,
        ..Default::default()
    };
    match cli.command {
        TopCommand::Remote { command } => {
            run_remote(&global, command).await?;
        }
        TopCommand::Daemon { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                DaemonCommand::Start => service.daemon_start(overrides.force_spawn).await?,
                DaemonCommand::Status => service.daemon_status().await?,
                DaemonCommand::Restart => service.daemon_restart().await?,
                DaemonCommand::Stop => service.daemon_stop().await?,
            }
        }
        TopCommand::AppServer { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                AppServerCommand::Add(args) => service.app_server_add(&args.name).await?,
                AppServerCommand::Remove(args) => service.app_server_remove(&args.name).await?,
                AppServerCommand::Start(args) => service.app_server_start(&args.name).await?,
                AppServerCommand::Stop(args) => service.app_server_stop(&args.name).await?,
                AppServerCommand::Restart(args) => service.app_server_restart(&args.name).await?,
                AppServerCommand::Status(args) => service.app_server_status(&args.name).await?,
                AppServerCommand::Info(args) => service.app_server_info(&args.name).await?,
            }
        }
        TopCommand::Lane { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                LaneCommand::List => service.lane_list().await?,
                LaneCommand::Init(args) => service.lane_init(&args.label, &args.repos).await?,
                LaneCommand::Inspect(args) => service.lane_inspect(&args.label).await?,
                LaneCommand::Attach(args) => {
                    service
                        .lane_attach(
                            &args.label,
                            &args.repo,
                            args.workspace.as_deref(),
                            &args.tracked_thread,
                        )
                        .await?
                }
                LaneCommand::Detach(args) => {
                    service
                        .lane_detach(
                            &args.label,
                            &args.repo,
                            args.workspace.as_deref(),
                            &args.tracked_thread,
                        )
                        .await?
                }
                LaneCommand::Cleanup(args) => {
                    let scope = match args.scope {
                        LaneCleanupScopeArg::Runtime => LaneCleanupScopeModel::Runtime,
                        LaneCleanupScopeArg::Worktree => LaneCleanupScopeModel::Worktree,
                        LaneCleanupScopeArg::Repo => LaneCleanupScopeModel::Repo,
                        LaneCleanupScopeArg::Lane => LaneCleanupScopeModel::Lane,
                    };
                    service
                        .lane_cleanup(
                            &args.label,
                            args.repo.as_deref(),
                            args.workspace.as_deref(),
                            scope,
                            args.force,
                        )
                        .await?;
                }
            }
        }
        TopCommand::Snapshot { command } => {
            snapshot::snapshot_command(paths.clone(), command).await?;
        }
        TopCommand::Context { command } => {
            snapshot::context_command(paths.clone(), command).await?;
        }
        TopCommand::Workspace { command } => {
            snapshot::workspace_command(paths.clone(), command).await?;
        }
        TopCommand::Doctor => {
            let service = SupervisorService::load(&overrides).await?;
            service.doctor().await?;
        }
        TopCommand::Docs { command } => match command {
            DocsCommand::ExportCli(args) => {
                docs::export_cli_markdown(&args.out)?;
            }
        },
        TopCommand::Events { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                EventsCommand::Recent(args) => service.events_recent(args.limit).await?,
                EventsCommand::Watch(args) => {
                    service.events_watch(args.snapshot, args.count).await?
                }
            }
        }
        TopCommand::Workstream { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                WorkstreamCommand::Add(args) => {
                    service.workstream_add(args.repo_root, args.name).await?;
                }
                WorkstreamCommand::Create(args) => {
                    service
                        .workstream_create(
                            args.title,
                            args.objective,
                            args.priority,
                            args.tt_home,
                            args.sqlite_home,
                            args.listen_url,
                            args.transport_kind.map(Into::into),
                            args.app_server_policy.map(Into::into),
                            args.connection_mode.map(Into::into),
                        )
                        .await?;
                }
                WorkstreamCommand::Edit(args) => {
                    service
                        .workstream_edit(
                            &args.workstream,
                            args.title,
                            args.objective,
                            args.status.map(Into::into),
                            args.priority,
                            args.tt_home,
                            args.sqlite_home,
                            args.listen_url,
                            args.transport_kind.map(Into::into),
                            args.app_server_policy.map(Into::into),
                            args.connection_mode.map(Into::into),
                            args.clear_execution_scope,
                        )
                        .await?;
                }
                WorkstreamCommand::Delete(args) => {
                    service.workstream_delete(&args.workstream).await?;
                }
                WorkstreamCommand::List => service.workstream_list().await?,
                WorkstreamCommand::Get(args) => service.workstream_get(&args.workstream).await?,
            }
        }
        TopCommand::Workunit { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                WorkunitCommand::Create(args) => {
                    service
                        .workunit_create(&args.workstream, args.title, args.task, args.dependencies)
                        .await?;
                }
                WorkunitCommand::Edit(args) => {
                    service
                        .workunit_edit(
                            &args.workunit,
                            args.title,
                            args.task,
                            args.status.map(Into::into),
                        )
                        .await?;
                }
                WorkunitCommand::Delete(args) => {
                    service.workunit_delete(&args.workunit).await?;
                }
                WorkunitCommand::List(args) => {
                    service.workunit_list(args.workstream.as_deref()).await?;
                }
                WorkunitCommand::Get(args) => service.workunit_get(&args.workunit).await?,
                WorkunitCommand::Thread { command } => match command {
                    WorkunitThreadCommand::Add(args) => {
                        let tracked_thread_id = authority::TrackedThreadId::new();
                        let workspace = args
                            .workspace
                            .try_into_workspace(tracked_thread_id.clone())?;
                        service
                            .tracked_thread_create(
                                &args.workunit,
                                args.title,
                                args.root_dir,
                                args.notes,
                                args.upstream_thread,
                                args.model,
                                tracked_thread_id,
                                workspace,
                            )
                            .await?;
                    }
                    WorkunitThreadCommand::Set(args) => {
                        let tracked_thread_id =
                            authority::TrackedThreadId::parse(args.tracked_thread.clone())?;
                        let workspace = args
                            .workspace
                            .try_into_workspace(tracked_thread_id.clone())?;
                        service
                            .tracked_thread_edit(
                                &args.tracked_thread,
                                args.title,
                                args.root_dir,
                                args.notes,
                                args.upstream_thread,
                                args.binding_state.map(Into::into),
                                args.model,
                                workspace,
                            )
                            .await?;
                    }
                    WorkunitThreadCommand::Remove(args) => {
                        service.tracked_thread_delete(&args.tracked_thread).await?;
                    }
                    WorkunitThreadCommand::List(args) => {
                        service.tracked_thread_list(&args.workunit).await?;
                    }
                    WorkunitThreadCommand::Get(args) => {
                        service.tracked_thread_get(&args.tracked_thread).await?;
                    }
                },
                WorkunitCommand::Workspace { command } => match command {
                    WorkunitWorkspaceCommand::PrepareWorkspace(args) => {
                        service
                            .tracked_thread_prepare_workspace(
                                &args.tracked_thread,
                                args.request_note,
                            )
                            .await?;
                    }
                    WorkunitWorkspaceCommand::RefreshWorkspace(args) => {
                        service
                            .tracked_thread_refresh_workspace(
                                &args.tracked_thread,
                                args.request_note,
                            )
                            .await?;
                    }
                    WorkunitWorkspaceCommand::MergePrep(args) => {
                        service
                            .tracked_thread_merge_prep(&args.tracked_thread, args.request_note)
                            .await?;
                    }
                    WorkunitWorkspaceCommand::AuthorizeMerge(args) => {
                        service
                            .tracked_thread_authorize_merge(&args.tracked_thread, args.request_note)
                            .await?;
                    }
                    WorkunitWorkspaceCommand::ExecuteLanding(args) => {
                        service
                            .tracked_thread_execute_landing(&args.tracked_thread, args.request_note)
                            .await?;
                    }
                    WorkunitWorkspaceCommand::PruneWorkspace(args) => {
                        service
                            .tracked_thread_prune_workspace(&args.tracked_thread, args.request_note)
                            .await?;
                    }
                },
            }
        }
        TopCommand::Todo { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                TodoCommand::Note(args) => {
                    service
                        .spawn_role_session(
                            "todo-note",
                            args.workstream.as_deref(),
                            args.new_workstream.as_deref(),
                            args.repo_root,
                            args.headless,
                            false,
                            args.model,
                        )
                        .await?;
                }
                TodoCommand::Review(args) => {
                    service
                        .spawn_role_session(
                            "todo-review",
                            args.workstream.as_deref(),
                            args.new_workstream.as_deref(),
                            args.repo_root,
                            args.headless,
                            false,
                            args.model,
                        )
                        .await?;
                }
                TodoCommand::Plan(args) => {
                    service
                        .spawn_role_session(
                            "todo-plan",
                            args.workstream.as_deref(),
                            args.new_workstream.as_deref(),
                            args.repo_root,
                            args.headless,
                            false,
                            args.model,
                        )
                        .await?;
                }
            }
        }
        TopCommand::Develop(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service
                .spawn_role_session(
                    "develop",
                    args.workstream.as_deref(),
                    args.new_workstream.as_deref(),
                    args.repo_root,
                    args.headless,
                    false,
                    args.model,
                )
                .await?;
        }
        TopCommand::Test(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service
                .spawn_role_session(
                    "test",
                    args.workstream.as_deref(),
                    args.new_workstream.as_deref(),
                    args.repo_root,
                    args.headless,
                    false,
                    args.model,
                )
                .await?;
        }
        TopCommand::Integrate(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service
                .spawn_role_session(
                    "integrate",
                    args.workstream.as_deref(),
                    args.new_workstream.as_deref(),
                    args.repo_root,
                    args.headless,
                    false,
                    args.model,
                )
                .await?;
        }
        TopCommand::Chat(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service
                .spawn_role_session(
                    "chat",
                    args.workstream.as_deref(),
                    args.new_workstream.as_deref(),
                    args.repo_root,
                    args.headless,
                    true,
                    args.model,
                )
                .await?;
        }
        TopCommand::Learn(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service
                .spawn_role_session(
                    "learn",
                    args.workstream.as_deref(),
                    args.new_workstream.as_deref(),
                    args.repo_root,
                    args.headless,
                    false,
                    args.model,
                )
                .await?;
        }
        TopCommand::Handoff(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service
                .spawn_role_session(
                    "handoff",
                    args.workstream.as_deref(),
                    args.new_workstream.as_deref(),
                    args.repo_root,
                    args.headless,
                    false,
                    args.model,
                )
                .await?;
        }
        TopCommand::Diff(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service
                .worktree_diff(args.selector.as_deref(), args.repo_root, args.worktree_path)
                .await?;
        }
        TopCommand::Split(args) => {
            let service = SupervisorService::load(&overrides).await?;
            let role = args.role.as_deref().unwrap_or("develop");
            service
                .spawn_role_session(
                    role,
                    args.spawn.workstream.as_deref(),
                    args.spawn.new_workstream.as_deref(),
                    args.spawn.repo_root,
                    args.spawn.headless,
                    args.ephemeral,
                    args.spawn.model,
                )
                .await?;
        }
        TopCommand::Close(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service.close_worktree(&args.selector, args.force).await?;
        }
        TopCommand::Park(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service.park_worktree(&args.selector, args.note).await?;
        }
        TopCommand::Supervisor { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                SupervisorCommand::Session { command } => match command {
                    SessionCommand::Active => service.session_active().await?,
                },
                SupervisorCommand::Plan { command } => match command {
                    PlanCommand::Create(args) => {
                        service
                            .planning_session_create(
                                &args.workstream,
                                args.planning_thread_id,
                                args.summary.objective,
                                args.summary.research_status.into(),
                                args.summary.requirements,
                                args.summary.constraints,
                                args.summary.non_goals,
                                args.summary.open_questions,
                                args.summary.draft_plan_summary,
                                args.summary.ready_for_review,
                                args.created_by,
                                args.request_note,
                                args.model,
                                args.cwd,
                            )
                            .await?;
                    }
                    PlanCommand::Get(args) => {
                        service.planning_session_get(&args.session).await?;
                    }
                    PlanCommand::List(args) => {
                        service
                            .planning_session_list(args.workstream, args.include_closed)
                            .await?;
                    }
                    PlanCommand::UpdateSummary(args) => {
                        service
                            .planning_session_update_summary(
                                &args.session,
                                args.summary.objective,
                                args.summary.requirements,
                                args.summary.constraints,
                                args.summary.non_goals,
                                args.summary.open_questions,
                                args.summary.research_status.into(),
                                args.summary.draft_plan_summary,
                                args.summary.ready_for_review,
                                args.updated_by,
                                args.note,
                            )
                            .await?;
                    }
                    PlanCommand::RequestSupervisorContext(args) => {
                        service
                            .planning_session_request_supervisor_context(
                                &args.session,
                                args.requested_by,
                                args.note,
                            )
                            .await?;
                    }
                    PlanCommand::RequestResearch(args) => {
                        service
                            .planning_session_request_research(
                                &args.session,
                                &args.worker,
                                args.worker_kind,
                                args.model,
                                args.cwd,
                                args.requested_by,
                                args.request_note,
                            )
                            .await?;
                    }
                    PlanCommand::MarkReadyForReview(args) => {
                        service
                            .planning_session_mark_ready_for_review(
                                &args.session,
                                args.updated_by,
                                args.note,
                            )
                            .await?;
                    }
                    PlanCommand::Abort(args) => {
                        service
                            .planning_session_abort(&args.session, args.updated_by, args.note)
                            .await?;
                    }
                    PlanCommand::Approve(args) => {
                        service
                            .planning_session_approve(
                                &args.session,
                                args.approved_by,
                                args.review_note,
                            )
                            .await?;
                    }
                    PlanCommand::Reject(args) => {
                        service
                            .planning_session_reject(
                                &args.session,
                                args.rejected_by,
                                args.review_note,
                            )
                            .await?;
                    }
                    PlanCommand::Supersede(args) => {
                        service
                            .planning_session_supersede(
                                &args.session,
                                args.superseded_by_session,
                                args.updated_by,
                                args.note,
                            )
                            .await?;
                    }
                },
                SupervisorCommand::Work { command } => match command {
                    SupervisorWorkCommand::Assignments { command } => match command {
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
                        AssignmentsCommand::Get(args) => {
                            service.assignment_get(&args.assignment).await?
                        }
                        AssignmentsCommand::Communication(args) => {
                            service
                                .assignment_communication_get(&args.assignment)
                                .await?
                        }
                    },
                    SupervisorWorkCommand::Reports { command } => match command {
                        ReportsCommand::Get(args) => service.report_get(&args.report).await?,
                        ReportsCommand::ListForWorkunit(args) => {
                            service.report_list_for_workunit(&args.workunit).await?;
                        }
                    },
                    SupervisorWorkCommand::Decisions { command } => match command {
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
                                        DecisionTypeArg::EscalateToHuman => {
                                            DecisionType::EscalateToHuman
                                        }
                                    },
                                    args.rationale,
                                    args.instructions,
                                    args.worker,
                                    args.worker_kind,
                                )
                                .await?;
                        }
                    },
                    SupervisorWorkCommand::Proposals { command } => match command {
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
                                        DecisionTypeArg::EscalateToHuman => {
                                            DecisionType::EscalateToHuman
                                        }
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
                    },
                },
                SupervisorCommand::Review { command } => match command {
                    ReviewCommand::List(args) => {
                        service
                            .tt_decision_list(
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
                    ReviewCommand::Queue(args) => {
                        service
                            .tt_decision_list(
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
                    ReviewCommand::History(args) => {
                        service
                            .tt_decision_history(
                                args.thread.as_deref(),
                                args.assignment.as_deref(),
                                args.include_superseded,
                                args.limit,
                            )
                            .await?;
                    }
                    ReviewCommand::Get(args) => {
                        service.tt_decision_get(&args.decision).await?;
                    }
                    ReviewCommand::ProposeSteer(args) => {
                        service
                            .tt_decision_propose_steer(
                                &args.thread,
                                &args.text,
                                args.requested_by,
                                args.rationale_note,
                            )
                            .await?;
                    }
                    ReviewCommand::ReplacePendingSteer(args) => {
                        service
                            .tt_decision_replace_pending_steer(
                                &args.decision,
                                &args.text,
                                args.requested_by,
                                args.rationale_note,
                            )
                            .await?;
                    }
                    ReviewCommand::RecordNoAction(args) => {
                        service
                            .tt_decision_record_no_action(
                                &args.decision,
                                args.reviewed_by,
                                args.review_note,
                            )
                            .await?;
                    }
                    ReviewCommand::ManualRefresh(args) => {
                        service
                            .tt_decision_manual_refresh(
                                args.thread.as_deref(),
                                args.assignment.as_deref(),
                                args.requested_by,
                                args.rationale_note,
                            )
                            .await?;
                    }
                    ReviewCommand::Approve(args) => {
                        service
                            .tt_decision_approve_and_send(
                                &args.decision,
                                args.reviewed_by,
                                args.review_note,
                            )
                            .await?;
                    }
                    ReviewCommand::Reject(args) => {
                        service
                            .tt_decision_reject(&args.decision, args.reviewed_by, args.review_note)
                            .await?;
                    }
                },
            }
        }
        TopCommand::Roles { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                RolesCommand::List => service.roles_list().await?,
                RolesCommand::Info(args) => service.roles_info(&args.role).await?,
            }
        }
        TopCommand::Worktrees => {
            let service = SupervisorService::load(&overrides).await?;
            service.worktrees_list().await?;
        }
        TopCommand::Tui => {
            let service = SupervisorService::load(&overrides).await?;
            tui::run_dashboard(service).await?;
        }
        TopCommand::App { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                AppCommand::TT { command } => match command {
                    TTCommand::Models { command } => match command {
                        ModelsCommand::List(args) => service.models_list(&args.workstream).await?,
                    },
                    TTCommand::Spawn(args) => {
                        service
                            .tt_spawn(
                                &args.role,
                                args.workstream.as_deref(),
                                args.new_workstream.as_deref(),
                                args.repo_root,
                                args.headless,
                                false,
                                args.model,
                            )
                            .await?;
                    }
                    TTCommand::Resume(args) => {
                        service
                            .thread_resume(&args.thread, args.cwd, args.model)
                            .await?;
                    }
                    TTCommand::Worktree { command } => match command {
                        TTWorktreeCommand::Add(args) => {
                            service.workstream_add(args.repo_root, args.name).await?;
                        }
                        TTWorktreeCommand::Prune(args) => {
                            service.tt_worktree_prune(&args.selector).await?;
                        }
                    },
                    TTCommand::Threads { command } => match command {
                        TTThreadsCommand::List(args) => {
                            service.threads_list(&args.workstream).await?
                        }
                        TTThreadsCommand::ListLoaded(args) => {
                            service.threads_list_loaded(&args.workstream).await?
                        }
                        TTThreadsCommand::Read(args) => service.thread_read(&args.thread).await?,
                        TTThreadsCommand::Start(args) => {
                            service
                                .thread_start(args.cwd, args.model, args.ephemeral)
                                .await?;
                        }
                        TTThreadsCommand::Resume(args) => {
                            service
                                .thread_resume(&args.thread, args.cwd, args.model)
                                .await?;
                        }
                    },
                    TTCommand::Turns { command } => match command {
                        TurnsCommand::ListActive => service.turns_list_active().await?,
                        TurnsCommand::Recent(args) => {
                            service.turns_recent(&args.thread, args.limit).await?
                        }
                        TurnsCommand::Get(args) => {
                            service.turn_get(&args.thread, &args.turn).await?
                        }
                    },
                },
            }
        }
        TopCommand::I3 { command } => match command {
            I3Command::Status => println!("i3 adapter: not yet implemented"),
            I3Command::Start => println!("i3 adapter: not yet implemented"),
            I3Command::Attach => println!("i3 adapter: not yet implemented"),
        },
        TopCommand::Skill { command } => {
            let context = RuntimeSkillContext {
                server_url: global.server_url.clone(),
                operator_api_token: global.operator_api_token.clone(),
                tt_bin: overrides.tt_bin.clone(),
                listen_url: overrides.listen_url.clone(),
                inbox_mirror_server_url: overrides.inbox_mirror_server_url.clone(),
                cwd: overrides.cwd.clone(),
                worktree_root: overrides.worktree_root.clone(),
                model: overrides.model.clone(),
                connect_only: overrides.connect_only,
                force_spawn: overrides.force_spawn,
            };
            let backend = TTSkillBackend::new(overrides.clone());
            let _ = tt_skills::dispatch(&backend, &context, command).await?;
        }
        TopCommand::TT { command } => {
            let service = SupervisorService::load(&overrides).await?;
            match command {
                TTCommand::Models { command } => match command {
                    ModelsCommand::List(args) => service.models_list(&args.workstream).await?,
                },
                TTCommand::Spawn(args) => {
                    service
                        .tt_spawn(
                            &args.role,
                            args.workstream.as_deref(),
                            args.new_workstream.as_deref(),
                            args.repo_root,
                            args.headless,
                            false,
                            args.model,
                        )
                        .await?;
                }
                TTCommand::Resume(args) => {
                    service
                        .thread_resume(&args.thread, args.cwd, args.model)
                        .await?;
                }
                TTCommand::Worktree { command } => match command {
                    TTWorktreeCommand::Add(args) => {
                        service.workstream_add(args.repo_root, args.name).await?;
                    }
                    TTWorktreeCommand::Prune(args) => {
                        service.tt_worktree_prune(&args.selector).await?;
                    }
                },
                TTCommand::Threads { command } => match command {
                    TTThreadsCommand::List(args) => service.threads_list(&args.workstream).await?,
                    TTThreadsCommand::ListLoaded(args) => {
                        service.threads_list_loaded(&args.workstream).await?
                    }
                    TTThreadsCommand::Read(args) => service.thread_read(&args.thread).await?,
                    TTThreadsCommand::Start(args) => {
                        service
                            .thread_start(args.cwd, args.model, args.ephemeral)
                            .await?;
                    }
                    TTThreadsCommand::Resume(args) => {
                        service
                            .thread_resume(&args.thread, args.cwd, args.model)
                            .await?;
                    }
                },
                TTCommand::Turns { command } => match command {
                    TurnsCommand::ListActive => service.turns_list_active().await?,
                    TurnsCommand::Recent(args) => {
                        service.turns_recent(&args.thread, args.limit).await?
                    }
                    TurnsCommand::Get(args) => service.turn_get(&args.thread, &args.turn).await?,
                },
            }
        }
        TopCommand::Prompt(args) => {
            let service = SupervisorService::load(&overrides).await?;
            let prompt = service.prompt(&args.thread, &args.text).await?;
            println!("{prompt}");
        }
        TopCommand::Quickstart(args) => {
            let service = SupervisorService::load(&overrides).await?;
            service.quickstart(args.cwd, args.model, &args.text).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn top_level_help_mentions_operator_cli() {
        let help = Cli::command().render_help().to_string();

        assert!(help.contains("tt control plane"));
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

        assert!(help.contains("Launch and manage the tt daemon"));
    }

    #[test]
    fn global_help_mentions_runtime_override_flags() {
        let help = Cli::command().render_help().to_string();

        assert!(help.contains("--tt-bin"));
        assert!(help.contains("--listen-url"));
        assert!(help.contains("--worktree-root"));
        assert!(help.contains("--connect-only"));
        assert!(help.contains("--force-spawn"));
    }

    #[test]
    fn top_level_help_mentions_only_canonical_namespace_groups() {
        let help = Cli::command().render_help().to_string();

        assert!(help.contains("project"));
        assert!(help.contains("worktree"));
        assert!(help.contains("worktrees"));
        assert!(help.contains("app-server"));
        assert!(help.contains("lane"));
        assert!(help.contains("snapshot"));
        assert!(help.contains("context"));
        assert!(help.contains("workspace"));
        assert!(help.contains("tui"));
        assert!(help.contains("todo"));
        assert!(help.contains("develop"));
        assert!(help.contains("test"));
        assert!(help.contains("integrate"));
        assert!(help.contains("chat"));
        assert!(help.contains("learn"));
        assert!(help.contains("handoff"));
        assert!(help.contains("diff"));
        assert!(help.contains("split"));
        assert!(help.contains("close"));
        assert!(help.contains("park"));
        assert!(!help.contains("roles"));
        assert!(!help.contains("supervisor"));
        assert!(!help.contains("tracked-threads"));
        assert!(!help.contains("planning-sessions"));
        assert!(!help.contains("legacy-workstreams"));
        assert!(!help.contains("legacy-workunits"));
        assert!(!help.contains("workunits"));
    }

    #[test]
    fn project_help_marks_surface_as_durable() {
        let about = Cli::command()
            .find_subcommand("project")
            .expect("project subcommand")
            .get_about()
            .expect("project about")
            .to_string();

        assert!(about.contains("Manage durable TT project records"));
    }

    #[test]
    fn parses_top_level_daemon_status_command() {
        let cli = Cli::parse_from(["tt", "daemon", "status"]);

        match cli.command {
            TopCommand::Daemon {
                command: DaemonCommand::Status,
            } => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_app_server_start_command() {
        let cli = Cli::parse_from(["tt", "app-server", "start", "default"]);

        match cli.command {
            TopCommand::AppServer {
                command: AppServerCommand::Start(_args),
            } => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_app_server_status_command() {
        let cli = Cli::parse_from(["tt", "app-server", "status"]);

        match cli.command {
            TopCommand::AppServer {
                command: AppServerCommand::Status(args),
            } => {
                assert_eq!(args.name, "default");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_app_server_info_command() {
        let cli = Cli::parse_from(["tt", "app-server", "info", "default"]);

        match cli.command {
            TopCommand::AppServer {
                command: AppServerCommand::Info(args),
            } => {
                assert_eq!(args.name, "default");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_lane_init_command() {
        let cli = Cli::parse_from([
            "tt",
            "lane",
            "init",
            "directory and worktree requirements",
            "--repo",
            "openai/codex",
        ]);

        match cli.command {
            TopCommand::Lane {
                command: LaneCommand::Init(args),
            } => {
                assert_eq!(args.label, "directory and worktree requirements");
                assert_eq!(args.repos, vec!["openai/codex"]);
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_lane_list_command() {
        let cli = Cli::parse_from(["tt", "lane", "list"]);

        match cli.command {
            TopCommand::Lane {
                command: LaneCommand::List,
            } => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_lane_attach_command() {
        let cli = Cli::parse_from([
            "tt",
            "lane",
            "attach",
            "directory and worktree requirements",
            "--repo",
            "openai/codex",
            "--tracked-thread",
            "tt-1",
        ]);

        match cli.command {
            TopCommand::Lane {
                command: LaneCommand::Attach(args),
            } => {
                assert_eq!(args.label, "directory and worktree requirements");
                assert_eq!(args.repo, "openai/codex");
                assert_eq!(args.tracked_thread, "tt-1");
                assert!(args.workspace.is_none());
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_lane_detach_command() {
        let cli = Cli::parse_from([
            "tt",
            "lane",
            "detach",
            "directory and worktree requirements",
            "--repo",
            "openai/codex",
            "--tracked-thread",
            "tt-1",
        ]);

        match cli.command {
            TopCommand::Lane {
                command: LaneCommand::Detach(args),
            } => {
                assert_eq!(args.label, "directory and worktree requirements");
                assert_eq!(args.repo, "openai/codex");
                assert_eq!(args.tracked_thread, "tt-1");
                assert!(args.workspace.is_none());
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_lane_cleanup_force_command() {
        let cli = Cli::parse_from([
            "tt",
            "lane",
            "cleanup",
            "directory and worktree requirements",
            "--repo",
            "openai/codex",
            "--force",
        ]);

        match cli.command {
            TopCommand::Lane {
                command: LaneCommand::Cleanup(args),
            } => {
                assert_eq!(args.label, "directory and worktree requirements");
                assert_eq!(args.repo.as_deref(), Some("openai/codex"));
                assert!(args.force);
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_snapshot_create_command() {
        let cli = Cli::parse_from([
            "tt",
            "snapshot",
            "create",
            "--lane",
            "my lane",
            "--repo",
            "openai/codex",
            "--workspace",
            "default",
            "--thread",
            "thread-1",
        ]);

        match cli.command {
            TopCommand::Snapshot {
                command: SnapshotRuntimeCommand::Create(args),
            } => {
                assert_eq!(args.scope.lane, "my lane");
                assert_eq!(args.scope.repo, "openai/codex");
                assert_eq!(args.scope.workspace, "default");
                assert_eq!(args.thread, "thread-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_context_include_command() {
        let cli = Cli::parse_from([
            "tt",
            "context",
            "include",
            "--from",
            "snapshot-1",
            "--include-turn",
            "turn-1",
        ]);

        match cli.command {
            TopCommand::Context {
                command: SnapshotContextCommand::Include(args),
            } => {
                assert_eq!(args.from_snapshot, "snapshot-1");
                assert_eq!(args.selection.include_turn, vec!["turn-1"]);
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_workspace_bind_command() {
        let cli = Cli::parse_from([
            "tt",
            "workspace",
            "bind",
            "--lane",
            "my lane",
            "--repo",
            "openai/codex",
            "--workspace",
            "default",
            "--snapshot",
            "snapshot-1",
        ]);

        match cli.command {
            TopCommand::Workspace {
                command: SnapshotWorkspaceCommand::Bind(args),
            } => {
                assert_eq!(args.scope.workspace, "default");
                assert_eq!(args.snapshot_id.as_deref(), Some("snapshot-1"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_todo_note_command() {
        let cli = Cli::parse_from([
            "tt",
            "todo",
            "note",
            "--new-workstream",
            "notes-v1",
            "--repo-root",
            "/tmp/repo",
        ]);

        match cli.command {
            TopCommand::Todo {
                command: TodoCommand::Note(args),
            } => {
                assert_eq!(args.new_workstream.as_deref(), Some("notes-v1"));
                assert_eq!(args.repo_root, Some(PathBuf::from("/tmp/repo")));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_develop_command() {
        let cli = Cli::parse_from(["tt", "develop", "--workstream", "ws-1", "--headless"]);

        match cli.command {
            TopCommand::Develop(args) => {
                assert_eq!(args.workstream.as_deref(), Some("ws-1"));
                assert!(args.headless);
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_split_command_with_role_override() {
        let cli = Cli::parse_from([
            "tt",
            "split",
            "--role",
            "test",
            "--new-workstream",
            "child",
            "--repo-root",
            "/tmp/repo",
        ]);

        match cli.command {
            TopCommand::Split(args) => {
                assert_eq!(args.role.as_deref(), Some("test"));
                assert_eq!(args.spawn.new_workstream.as_deref(), Some("child"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_close_command() {
        let cli = Cli::parse_from(["tt", "close", "workstream-1", "--force"]);

        match cli.command {
            TopCommand::Close(args) => {
                assert_eq!(args.selector, "workstream-1");
                assert!(args.force);
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_park_command() {
        let cli = Cli::parse_from(["tt", "park", "workstream-1", "--note", "pause for review"]);

        match cli.command {
            TopCommand::Park(args) => {
                assert_eq!(args.selector, "workstream-1");
                assert_eq!(args.note.as_deref(), Some("pause for review"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_roles_list_command() {
        let cli = Cli::parse_from(["tt", "roles", "list"]);

        match cli.command {
            TopCommand::Roles {
                command: RolesCommand::List,
            } => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_roles_info_command() {
        let cli = Cli::parse_from(["tt", "roles", "info", "plan"]);

        match cli.command {
            TopCommand::Roles {
                command: RolesCommand::Info(args),
            } => {
                assert_eq!(args.role, "plan");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_worktrees_command() {
        let cli = Cli::parse_from(["tt", "worktrees"]);

        match cli.command {
            TopCommand::Worktrees => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_tui_command() {
        let cli = Cli::parse_from(["tt", "tui"]);

        match cli.command {
            TopCommand::Tui => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_top_level_doctor_command() {
        let cli = Cli::parse_from(["tt", "doctor"]);

        match cli.command {
            TopCommand::Doctor => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_supervisor_session_active_command() {
        let cli = Cli::parse_from(["tt", "supervisor", "session", "active"]);

        match cli.command {
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Session {
                        command: SessionCommand::Active,
                    },
            } => {}
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_turns_recent_command() {
        let cli = Cli::parse_from(["tt", "tt", "turns", "recent", "--thread", "thread-1"]);

        match cli.command {
            TopCommand::TT {
                command:
                    TTCommand::Turns {
                        command: TurnsCommand::Recent(args),
                    },
            } => {
                assert_eq!(args.thread, "thread-1");
                assert_eq!(args.limit, 10);
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_spawn_command() {
        let cli = Cli::parse_from([
            "tt",
            "tt",
            "spawn",
            "plan",
            "--new-workstream",
            "realign-v1",
            "--repo-root",
            "/tmp/repo",
            "--headless",
        ]);

        match cli.command {
            TopCommand::TT {
                command: TTCommand::Spawn(args),
            } => {
                assert_eq!(args.role, "plan");
                assert_eq!(args.new_workstream.as_deref(), Some("realign-v1"));
                assert_eq!(args.repo_root, Some(PathBuf::from("/tmp/repo")));
                assert!(args.headless);
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_resume_command() {
        let cli = Cli::parse_from(["tt", "tt", "resume", "thread-1", "--model", "gpt-5.4"]);

        match cli.command {
            TopCommand::TT {
                command: TTCommand::Resume(args),
            } => {
                assert_eq!(args.thread, "thread-1");
                assert_eq!(args.model.as_deref(), Some("gpt-5.4"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_worktree_add_command() {
        let cli = Cli::parse_from(["tt", "tt", "worktree", "add", "/tmp/repo", "ws-1"]);

        match cli.command {
            TopCommand::TT {
                command:
                    TTCommand::Worktree {
                        command: TTWorktreeCommand::Add(args),
                    },
            } => {
                assert_eq!(args.repo_root, PathBuf::from("/tmp/repo"));
                assert_eq!(args.name, "ws-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_worktree_prune_command() {
        let cli = Cli::parse_from(["tt", "tt", "worktree", "prune", "testing"]);

        match cli.command {
            TopCommand::TT {
                command:
                    TTCommand::Worktree {
                        command: TTWorktreeCommand::Prune(args),
                    },
            } => {
                assert_eq!(args.selector, "testing");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_events_watch_command() {
        let cli = Cli::parse_from(["tt", "events", "watch", "--snapshot", "--count", "5"]);

        match cli.command {
            TopCommand::Events {
                command: EventsCommand::Watch(args),
            } => {
                assert!(args.snapshot);
                assert_eq!(args.count, Some(5));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn global_runtime_mode_flags_conflict() {
        let result = Cli::try_parse_from(["tt", "--connect-only", "--force-spawn", "doctor"]);

        assert!(result.is_err());
    }

    #[test]
    fn parses_workunit_thread_add_command() {
        let cli = Cli::parse_from([
            "tt",
            "worktree",
            "thread",
            "add",
            "--workunit",
            "wu-1",
            "--title",
            "Thread record",
            "--root-dir",
            "/tmp/tt",
            "--model",
            "gpt-5.4",
            "--workspace-repository-root",
            "/tmp/tt",
            "--workspace-worktree-path",
            "/tmp/tt/worktrees/thread-1",
            "--workspace-branch-name",
            "tt/thread-1",
            "--workspace-base-ref",
            "main",
            "--workspace-landing-target",
            "main",
        ]);

        match cli.command {
            TopCommand::Workunit {
                command:
                    WorkunitCommand::Thread {
                        command: WorkunitThreadCommand::Add(args),
                    },
            } => {
                assert_eq!(args.workunit, "wu-1");
                assert_eq!(args.title, "Thread record");
                assert_eq!(args.root_dir, "/tmp/tt");
                assert_eq!(args.model.as_deref(), Some("gpt-5.4"));
                assert_eq!(args.workspace.repository_root.as_deref(), Some("/tmp/tt"));
                assert_eq!(
                    args.workspace.worktree_path.as_deref(),
                    Some("/tmp/tt/worktrees/thread-1")
                );
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_workstream_delete_with_positional_selector() {
        let cli = Cli::parse_from(["tt", "project", "delete", "testing"]);

        match cli.command {
            TopCommand::Workstream {
                command: WorkstreamCommand::Delete(args),
            } => {
                assert_eq!(args.workstream, "testing");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn rejects_legacy_flagged_workstream_delete_selector() {
        let result = Cli::try_parse_from(["tt", "project", "delete", "--workstream", "testing"]);

        assert!(result.is_err());
    }

    #[test]
    fn parses_workunit_thread_set_command() {
        let cli = Cli::parse_from([
            "tt",
            "worktree",
            "thread",
            "set",
            "--tracked-thread",
            "tt-1",
            "--binding-state",
            "detached",
            "--model",
            "gpt-5.5",
        ]);

        match cli.command {
            TopCommand::Workunit {
                command:
                    WorkunitCommand::Thread {
                        command: WorkunitThreadCommand::Set(args),
                    },
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
    fn parses_workunit_workspace_merge_prep_command() {
        let cli = Cli::parse_from([
            "tt",
            "worktree",
            "workspace",
            "merge-prep",
            "--tracked-thread",
            "tt-1",
        ]);

        match cli.command {
            TopCommand::Workunit {
                command:
                    WorkunitCommand::Workspace {
                        command: WorkunitWorkspaceCommand::MergePrep(args),
                    },
            } => {
                assert_eq!(args.tracked_thread, "tt-1");
                assert!(args.request_note.is_none());
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn legacy_workstream_namespace_is_not_exposed() {
        assert!(Cli::try_parse_from(["tt", "legacy-workstreams", "create"]).is_err());
        assert!(Cli::try_parse_from(["tt", "legacy-workstreams", "list"]).is_err());
    }

    #[test]
    fn legacy_workunit_namespace_is_not_exposed() {
        assert!(Cli::try_parse_from(["tt", "legacy-workunits", "create"]).is_err());
        assert!(Cli::try_parse_from(["tt", "legacy-workunits", "list"]).is_err());
    }

    #[test]
    fn rejects_removed_workunits_namespace() {
        assert!(Cli::try_parse_from(["tt", "workunits", "list"]).is_err());
        assert!(Cli::try_parse_from(["tt", "workunits", "get", "--workunit", "wu-1"]).is_err());
    }

    #[test]
    fn rejects_removed_tracked_threads_namespace() {
        assert!(Cli::try_parse_from(["tt", "tracked-threads", "create"]).is_err());
        assert!(Cli::try_parse_from(["tt", "tracked-threads", "list"]).is_err());
    }

    #[test]
    fn rejects_removed_planning_sessions_namespace() {
        assert!(Cli::try_parse_from(["tt", "planning-sessions", "create"]).is_err());
        assert!(Cli::try_parse_from(["tt", "planning-sessions", "list"]).is_err());
    }

    #[test]
    fn parses_supervisor_review_propose_steer_command() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
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
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Review {
                        command: ReviewCommand::ProposeSteer(args),
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
    fn parses_supervisor_plan_create_command() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
            "plan",
            "create",
            "--workstream",
            "ws-1",
            "--objective",
            "Plan a bounded change",
            "--created-by",
            "cli_user",
        ]);

        match cli.command {
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Plan {
                        command: PlanCommand::Create(args),
                    },
            } => {
                assert_eq!(args.workstream, "ws-1");
                assert_eq!(args.summary.objective, "Plan a bounded change");
                assert_eq!(args.created_by.as_deref(), Some("cli_user"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_supervisor_plan_mark_ready_for_review_command() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
            "plan",
            "mark-ready-for-review",
            "--session",
            "ps-1",
            "--updated-by",
            "cli_user",
        ]);

        match cli.command {
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Plan {
                        command: PlanCommand::MarkReadyForReview(args),
                    },
            } => {
                assert_eq!(args.session, "ps-1");
                assert_eq!(args.updated_by.as_deref(), Some("cli_user"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_models_list_command() {
        let cli = Cli::parse_from(["tt", "tt", "models", "list", "--workstream", "ws-1"]);

        match cli.command {
            TopCommand::TT {
                command:
                    TTCommand::Models {
                        command: ModelsCommand::List(args),
                    },
            } => {
                assert_eq!(args.workstream, "ws-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_threads_start_command() {
        let cli = Cli::parse_from([
            "tt",
            "tt",
            "threads",
            "start",
            "--cwd",
            "/tmp/tt",
            "--model",
            "gpt-5.4",
            "--ephemeral",
        ]);

        match cli.command {
            TopCommand::TT {
                command:
                    TTCommand::Threads {
                        command: TTThreadsCommand::Start(args),
                    },
            } => {
                assert_eq!(args.cwd, Some(PathBuf::from("/tmp/tt")));
                assert_eq!(args.model.as_deref(), Some("gpt-5.4"));
                assert!(args.ephemeral);
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_threads_resume_command() {
        let cli = Cli::parse_from([
            "tt", "tt", "threads", "resume", "--thread", "thread-1", "--model", "gpt-5.4",
        ]);

        match cli.command {
            TopCommand::TT {
                command:
                    TTCommand::Threads {
                        command: TTThreadsCommand::Resume(args),
                    },
            } => {
                assert_eq!(args.thread, "thread-1");
                assert_eq!(args.model.as_deref(), Some("gpt-5.4"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_threads_read_command() {
        let cli = Cli::parse_from(["tt", "tt", "threads", "read", "--thread", "thread-1"]);

        match cli.command {
            TopCommand::TT {
                command:
                    TTCommand::Threads {
                        command: TTThreadsCommand::Read(args),
                    },
            } => {
                assert_eq!(args.thread, "thread-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_threads_list_command() {
        let cli = Cli::parse_from(["tt", "tt", "threads", "list", "--workstream", "ws-1"]);

        match cli.command {
            TopCommand::TT {
                command:
                    TTCommand::Threads {
                        command: TTThreadsCommand::List(args),
                    },
            } => {
                assert_eq!(args.workstream, "ws-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_tt_threads_list_loaded_command() {
        let cli = Cli::parse_from(["tt", "tt", "threads", "list-loaded", "--workstream", "ws-1"]);

        match cli.command {
            TopCommand::TT {
                command:
                    TTCommand::Threads {
                        command: TTThreadsCommand::ListLoaded(args),
                    },
            } => {
                assert_eq!(args.workstream, "ws-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_workstream_add_command() {
        let cli = Cli::parse_from(["tt", "project", "add", "/tmp/repo", "ws-1"]);

        match cli.command {
            TopCommand::Workstream {
                command: WorkstreamCommand::Add(args),
            } => {
                assert_eq!(args.repo_root, PathBuf::from("/tmp/repo"));
                assert_eq!(args.name, "ws-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn rejects_removed_top_level_models_namespace() {
        assert!(Cli::try_parse_from(["tt", "models", "list"]).is_err());
    }

    #[test]
    fn rejects_removed_top_level_thread_runtime_commands() {
        assert!(Cli::try_parse_from(["tt", "threads", "read", "--thread", "thread-1"]).is_err());
        assert!(Cli::try_parse_from(["tt", "threads", "start"]).is_err());
        assert!(Cli::try_parse_from(["tt", "threads", "resume", "--thread", "thread-1"]).is_err());
        assert!(Cli::try_parse_from(["tt", "threads", "list-loaded"]).is_err());
    }

    #[test]
    fn rejects_removed_top_level_turns_namespace() {
        assert!(Cli::try_parse_from(["tt", "turns", "recent", "--thread", "thread-1"]).is_err());
        assert!(Cli::try_parse_from(["tt", "turns", "list-active"]).is_err());
        assert!(
            Cli::try_parse_from([
                "tt", "turns", "get", "--thread", "thread-1", "--turn", "t-1"
            ])
            .is_err()
        );
    }

    #[test]
    fn rejects_removed_daemon_app_server_namespace() {
        assert!(Cli::try_parse_from(["tt", "daemon", "discover-app-servers"]).is_err());
        assert!(Cli::try_parse_from(["tt", "daemon", "reap-app-servers"]).is_err());
    }

    #[test]
    fn rejects_removed_tt_review_namespace() {
        assert!(Cli::try_parse_from(["tt", "tt", "review", "list"]).is_err());
        assert!(Cli::try_parse_from(["tt", "tt", "review", "get"]).is_err());
    }

    #[test]
    fn parses_supervisor_review_replace_pending_steer_command() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
            "review",
            "replace-pending-steer",
            "--decision",
            "std-7",
            "--text",
            "updated steer text",
        ]);

        match cli.command {
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Review {
                        command: ReviewCommand::ReplacePendingSteer(args),
                    },
            } => {
                assert_eq!(args.decision, "std-7");
                assert_eq!(args.text, "updated steer text");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_supervisor_review_record_no_action_command() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
            "review",
            "record-no-action",
            "--decision",
            "std-7",
            "--reviewed-by",
            "cli_user",
        ]);

        match cli.command {
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Review {
                        command: ReviewCommand::RecordNoAction(args),
                    },
            } => {
                assert_eq!(args.decision, "std-7");
                assert_eq!(args.reviewed_by.as_deref(), Some("cli_user"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_supervisor_review_manual_refresh_command() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
            "review",
            "manual-refresh",
            "--thread",
            "thread-1",
            "--requested-by",
            "cli_user",
        ]);

        match cli.command {
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Review {
                        command: ReviewCommand::ManualRefresh(args),
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
    fn parses_supervisor_review_queue_command_with_filters() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
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
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Review {
                        command: ReviewCommand::Queue(args),
                    },
            } => {
                assert_eq!(args.filters.workstream.as_deref(), Some("ws-1"));
                assert!(matches!(
                    args.filters.kind,
                    Some(TTDecisionKindArg::SteerActiveTurn)
                ));
                assert_eq!(args.filters.limit, Some(5));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_supervisor_review_history_command() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
            "review",
            "history",
            "--assignment",
            "cta-1",
            "--limit",
            "20",
        ]);

        match cli.command {
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Review {
                        command: ReviewCommand::History(args),
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
    fn parses_supervisor_proposal_artifact_summary_command() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
            "work",
            "proposals",
            "artifact-summary",
            "--proposal",
            "proposal-1",
        ]);

        match cli.command {
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Work {
                        command:
                            SupervisorWorkCommand::Proposals {
                                command: ProposalsCommand::ArtifactSummary(args),
                            },
                    },
            } => {
                assert_eq!(args.proposal, "proposal-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_supervisor_proposal_artifact_detail_command() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
            "work",
            "proposals",
            "artifact-detail",
            "--proposal",
            "proposal-1",
        ]);

        match cli.command {
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Work {
                        command:
                            SupervisorWorkCommand::Proposals {
                                command: ProposalsCommand::ArtifactDetail(args),
                            },
                    },
            } => {
                assert_eq!(args.proposal, "proposal-1");
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn parses_supervisor_proposal_artifact_export_command() {
        let cli = Cli::parse_from([
            "tt",
            "supervisor",
            "work",
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
            TopCommand::Supervisor {
                command:
                    SupervisorCommand::Work {
                        command:
                            SupervisorWorkCommand::Proposals {
                                command: ProposalsCommand::ArtifactExport(args),
                            },
                    },
            } => {
                assert_eq!(args.proposal, "proposal-1");
                assert!(matches!(args.format, ProposalArtifactExportFormatArg::Md));
                assert_eq!(args.output, Some(PathBuf::from("/tmp/proposal.md")));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }

    #[test]
    fn rejects_removed_top_level_supervisor_peer_namespaces() {
        assert!(Cli::try_parse_from(["tt", "plan", "create"]).is_err());
        assert!(Cli::try_parse_from(["tt", "assignments", "start"]).is_err());
        assert!(Cli::try_parse_from(["tt", "reports", "get", "--report", "r-1"]).is_err());
        assert!(Cli::try_parse_from(["tt", "proposals", "create", "--workunit", "wu-1"]).is_err());
        assert!(Cli::try_parse_from(["tt", "decisions", "apply", "--workunit", "wu-1"]).is_err());
        assert!(Cli::try_parse_from(["tt", "review", "list"]).is_err());
        assert!(Cli::try_parse_from(["tt", "session", "active"]).is_err());
    }

    #[test]
    fn rejects_tt_decisions_namespace() {
        assert!(Cli::try_parse_from(["tt", "tt", "decisions", "list"]).is_err());
    }

    #[test]
    fn rejects_flat_tt_review_verbs() {
        assert!(Cli::try_parse_from(["tt", "tt", "list"]).is_err());
    }

    #[test]
    fn parses_skill_agent_spawn_with_default_role() {
        let cli = Cli::try_parse_from(["tt", "skill", "agent", "spawn"]).expect("parse skill");
        match cli.command {
            TopCommand::Skill {
                command: RuntimeSkillCommand::Agent { command },
            } => match command {
                tt_skills::AgentCommand::Spawn(args) => {
                    assert_eq!(args.role, "agent");
                }
                other => panic!("unexpected agent skill command: {other:?}"),
            },
            other => panic!("unexpected top command: {other:?}"),
        }
    }

    #[test]
    fn parses_skill_process_restart_command() {
        let cli = Cli::try_parse_from([
            "tt",
            "skill",
            "process",
            "restart",
            "--name",
            "my-worker",
            "--cwd",
            "/tmp",
            "bash",
            "-lc",
            "sleep 1",
        ])
        .expect("parse skill");
        match cli.command {
            TopCommand::Skill {
                command: RuntimeSkillCommand::Process { command },
            } => match command {
                tt_skills::ProcessCommand::Restart(args) => {
                    assert_eq!(args.name.as_deref(), Some("my-worker"));
                    assert_eq!(args.cwd, Some(PathBuf::from("/tmp")));
                    assert_eq!(args.command, vec!["bash", "-lc", "sleep 1"]);
                }
                other => panic!("unexpected process skill command: {other:?}"),
            },
            other => panic!("unexpected top command: {other:?}"),
        }
    }

    #[test]
    fn parses_skill_process_signal_command() {
        let cli = Cli::try_parse_from([
            "tt", "skill", "process", "signal", "--pid", "42", "--signal", "HUP",
        ])
        .expect("parse skill");
        match cli.command {
            TopCommand::Skill {
                command: RuntimeSkillCommand::Process { command },
            } => match command {
                tt_skills::ProcessCommand::Signal(args) => {
                    assert_eq!(args.pid, Some(42));
                    assert_eq!(args.signal, "HUP");
                }
                other => panic!("unexpected process skill command: {other:?}"),
            },
            other => panic!("unexpected top command: {other:?}"),
        }
    }

    #[test]
    fn parses_skill_services_status_command() {
        let cli = Cli::try_parse_from(["tt", "skill", "services", "status", "daemon"])
            .expect("parse skill");
        match cli.command {
            TopCommand::Skill {
                command: RuntimeSkillCommand::Services { command },
            } => match command {
                tt_skills::ServicesCommand::Status(args) => {
                    assert!(matches!(
                        args.service,
                        tt_skills::ManagedServiceKind::Daemon
                    ));
                }
                other => panic!("unexpected services skill command: {other:?}"),
            },
            other => panic!("unexpected top command: {other:?}"),
        }
    }

    #[test]
    fn parses_skill_git_worktree_command() {
        let cli = Cli::try_parse_from([
            "tt",
            "skill",
            "git",
            "worktree",
            "current",
            "--repo-root",
            "/tmp/repo",
            "--worktree-path",
            "/tmp/repo/worktrees/tt-1",
        ])
        .expect("parse skill");
        match cli.command {
            TopCommand::Skill {
                command: RuntimeSkillCommand::Git { command },
            } => match command {
                tt_skills::GitCommand::Worktree { command } => match command {
                    tt_skills::GitWorktreeCommand::Current(args) => {
                        assert_eq!(args.repo_root, Some(PathBuf::from("/tmp/repo")));
                        assert_eq!(
                            args.worktree_path,
                            Some(PathBuf::from("/tmp/repo/worktrees/tt-1"))
                        );
                    }
                    other => panic!("unexpected git worktree command: {other:?}"),
                },
                other => panic!("unexpected git skill command: {other:?}"),
            },
            other => panic!("unexpected top command: {other:?}"),
        }
    }

    #[test]
    fn parses_skill_git_branch_list_command() {
        let cli = Cli::try_parse_from([
            "tt",
            "skill",
            "git",
            "branch",
            "list",
            "--repo-root",
            "/tmp/repo",
        ])
        .expect("parse skill");
        match cli.command {
            TopCommand::Skill {
                command: RuntimeSkillCommand::Git { command },
            } => match command {
                tt_skills::GitCommand::Branch { command } => match command {
                    tt_skills::GitBranchCommand::List(args) => {
                        assert_eq!(args.repo_root, Some(PathBuf::from("/tmp/repo")));
                    }
                    other => panic!("unexpected git branch command: {other:?}"),
                },
                other => panic!("unexpected git skill command: {other:?}"),
            },
            other => panic!("unexpected top command: {other:?}"),
        }
    }

    #[test]
    fn parses_skill_i3_workspace_list_command() {
        let cli =
            Cli::try_parse_from(["tt", "skill", "i3", "workspace", "list"]).expect("parse skill");
        match cli.command {
            TopCommand::Skill {
                command: RuntimeSkillCommand::I3 { command },
            } => match command {
                tt_skills::I3Command::Workspace { command } => match command {
                    tt_skills::I3WorkspaceCommand::List(_) => {}
                    other => panic!("unexpected i3 workspace command: {other:?}"),
                },
                other => panic!("unexpected i3 skill command: {other:?}"),
            },
            other => panic!("unexpected top command: {other:?}"),
        }
    }

    #[test]
    fn parses_skill_i3_window_info_command() {
        let cli = Cli::try_parse_from([
            "tt",
            "skill",
            "i3",
            "window",
            "info",
            "--criteria",
            r#"["class"="kitty"]"#,
        ])
        .expect("parse skill");
        match cli.command {
            TopCommand::Skill {
                command: RuntimeSkillCommand::I3 { command },
            } => match command {
                tt_skills::I3Command::Window { command } => match command {
                    tt_skills::I3WindowCommand::Info(args) => {
                        assert_eq!(args.criteria, r#"["class"="kitty"]"#);
                    }
                    other => panic!("unexpected i3 window command: {other:?}"),
                },
                other => panic!("unexpected i3 skill command: {other:?}"),
            },
            other => panic!("unexpected top command: {other:?}"),
        }
    }

    #[test]
    fn parses_skill_i3_window_focus_command() {
        let cli = Cli::try_parse_from([
            "tt",
            "skill",
            "i3",
            "window",
            "focus",
            "--criteria",
            r#"["class"="kitty"]"#,
        ])
        .expect("parse skill");
        match cli.command {
            TopCommand::Skill {
                command: RuntimeSkillCommand::I3 { command },
            } => match command {
                tt_skills::I3Command::Window { command } => match command {
                    tt_skills::I3WindowCommand::Focus(args) => {
                        assert_eq!(args.criteria, r#"["class"="kitty"]"#);
                    }
                    other => panic!("unexpected i3 window command: {other:?}"),
                },
                other => panic!("unexpected i3 skill command: {other:?}"),
            },
            other => panic!("unexpected top command: {other:?}"),
        }
    }
}
