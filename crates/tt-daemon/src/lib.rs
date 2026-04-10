//! Local orchestration for TT v2.
//!
//! The daemon coordinates TT overlay state, Codex runtime state, and git state.
//! It owns the local request/response API used by the TUI and CLI.

use std::collections::BTreeMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::Arc;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    thread,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap as _;
use codex_app_server_protocol as protocol;
use codex_protocol::openai_models::ReasoningEffort;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tt_codex::{
    CodexHome, CodexRuntimeClient, CodexThreadRuntimeSnapshot, apply_repo_settings_env,
    configured_app_server_listen_url, managed_project_auth_is_present,
    managed_project_auth_json_path, managed_project_codex_home, repo_env_var, repo_env_var_os,
    upsert_session_index_entry, validate_runtime_contract,
};
use tt_domain::{
    MergeAuthorizationStatus, MergeExecutionStatus, MergeReadiness, MergeRun, Project,
    ProjectStatus, ThreadBinding, ThreadBindingStatus, ThreadRole, WorkUnit, WorkUnitStatus,
    WorkspaceBinding, WorkspaceCleanupPolicy, WorkspaceStatus, WorkspaceStrategy,
    WorkspaceSyncPolicy,
};
use tt_git::GitRepository;
use tt_store::OverlayStore;
use tt_ui_core::{CodexThreadDetail, CodexThreadSummary, DashboardSummary, GitRepositorySummary};
use url::Url;

pub const TT_DAEMON_API_VERSION: &str = "v2";
pub const TT_DAEMON_SOCKET_NAME: &str = "tt-daemon.sock";
pub const TT_CONTRACT_FILE_NAME: &str = "contract.md";
pub const TT_CODEX_APP_SERVER_LOG_FILE_NAME: &str = "codex-app-server.log";
pub const TT_EVENTS_FILE_NAME: &str = "events.jsonl";
const CODEX_CONFIG_DEFAULTS_FILE_NAME: &str = "config.defaults.toml";
const CODEX_CONFIG_LOCAL_FILE_NAME: &str = "config.local.toml";
const CODEX_CONFIG_FILE_NAME: &str = "config.toml";
const DEFAULT_AGENT_CONFIG_MAX_THREADS: usize = 6;
const DEFAULT_AGENT_CONFIG_MAX_DEPTH: usize = 1;
const DOCTOR_LISTEN_TIMEOUT_MS: u64 = 1500;
const LIVE_TURN_MAX_ATTEMPTS: usize = 3;
const DAEMON_SPAWN_WAIT_MS: u64 = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub repo_root: Option<PathBuf>,
    pub project_initialized: bool,
    pub project_state: Option<String>,
    pub director_state: ManagedProjectDirectorState,
    pub project_count: usize,
    pub work_unit_count: usize,
    pub bound_thread_count: usize,
    pub ready_workspace_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ManagedProjectDirectorState {
    Ready,
    Starting,
    Blocked,
    Missing,
}

impl Default for ManagedProjectDirectorState {
    fn default() -> Self {
        Self::Missing
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexDoctorReport {
    pub contract_ok: bool,
    pub codex_bin: Option<PathBuf>,
    pub app_server_bin: Option<PathBuf>,
    pub auth_json: Option<PathBuf>,
    pub auth_present: Option<bool>,
    pub codex_version: Option<String>,
    pub app_server_version: Option<String>,
    pub configured_listen_url: String,
    pub listen_reachable: Option<bool>,
    pub listen_error: Option<String>,
    pub codex_home: Option<PathBuf>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexAppServerSummary {
    pub repo_root: PathBuf,
    pub daemon_socket_path: PathBuf,
    pub daemon_socket_exists: bool,
    pub daemon_socket_reachable: bool,
    pub configured_listen_url: String,
    pub listen_reachable: bool,
    pub listen_error: Option<String>,
    pub source: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub cwd: PathBuf,
    pub tt_cli_generation: String,
    pub daemon_api_version: String,
    pub tt_project_root: Option<PathBuf>,
    pub codex_project_root: Option<PathBuf>,
    pub daemon_socket_path: PathBuf,
    pub codex_auth_json: Option<PathBuf>,
    pub codex_auth_present: Option<bool>,
    pub codex_contract_ok: bool,
    pub codex_error: Option<String>,
    pub codex_listen_url: String,
    pub codex_listen_reachable: Option<bool>,
    pub codex_listen_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum DaemonRequest {
    Doctor {
        cwd: PathBuf,
        check_listen: bool,
    },
    DoctorCodex {
        cwd: PathBuf,
        check_listen: bool,
    },
    Status {
        cwd: PathBuf,
    },
    DashboardSummary,
    RepositorySummary {
        cwd: PathBuf,
    },
    InspectManagedProject {
        cwd: PathBuf,
    },
    InspectManagedProjectPlan {
        cwd: PathBuf,
    },
    RefreshManagedProjectPlan {
        cwd: PathBuf,
    },
    CleanManagedProject {
        cwd: PathBuf,
        force: bool,
    },
    SetManagedProjectThreadControl {
        cwd: PathBuf,
        role: ThreadRole,
        mode: ManagedProjectThreadControlMode,
    },
    ListProjects,
    GetProject {
        id_or_slug: String,
    },
    UpsertProject {
        project: Project,
    },
    SetProjectStatus {
        id_or_slug: String,
        status: ProjectStatus,
    },
    DeleteProject {
        id_or_slug: String,
    },
    ListWorkUnits {
        project_id: Option<String>,
    },
    GetWorkUnit {
        id_or_slug: String,
    },
    UpsertWorkUnit {
        work_unit: WorkUnit,
    },
    SetWorkUnitStatus {
        id_or_slug: String,
        status: WorkUnitStatus,
    },
    DeleteWorkUnit {
        id_or_slug: String,
    },
    ListThreadBindings,
    GetThreadBinding {
        codex_thread_id: String,
    },
    UpsertThreadBinding {
        binding: ThreadBinding,
    },
    SetThreadBindingStatus {
        codex_thread_id: String,
        status: ThreadBindingStatus,
    },
    DeleteThreadBinding {
        codex_thread_id: String,
    },
    ListThreadBindingsForWorkUnit {
        work_unit_id: String,
    },
    ListWorkspaceBindings,
    GetWorkspaceBinding {
        id: String,
    },
    UpsertWorkspaceBinding {
        binding: WorkspaceBinding,
    },
    SetWorkspaceBindingStatus {
        id: String,
        status: WorkspaceStatus,
    },
    DeleteWorkspaceBinding {
        id: String,
    },
    ListWorkspaceBindingsForThread {
        codex_thread_id: String,
    },
    RefreshWorkspaceBinding {
        id: String,
    },
    PrepareWorkspaceBinding {
        id: String,
    },
    MergePrepWorkspaceBinding {
        id: String,
    },
    AuthorizeMergeWorkspaceBinding {
        id: String,
    },
    ExecuteLandingWorkspaceBinding {
        id: String,
    },
    PruneWorkspaceBinding {
        id: String,
        force: bool,
    },
    CloseWorkspace {
        cwd: PathBuf,
        selector: Option<String>,
        force: bool,
    },
    ParkWorkspace {
        cwd: PathBuf,
        selector: Option<String>,
        note: Option<String>,
    },
    SplitWorkspace {
        cwd: PathBuf,
        role: ThreadRole,
        model: Option<String>,
        ephemeral: bool,
    },
    ListMergeRuns,
    GetMergeRun {
        id: String,
    },
    UpsertMergeRun {
        run: MergeRun,
    },
    RefreshMergeRun {
        workspace_binding_id: String,
    },
    SetMergeRunStatus {
        id: String,
        readiness: MergeReadiness,
        authorization: MergeAuthorizationStatus,
        execution: MergeExecutionStatus,
        head_commit: Option<String>,
    },
    DeleteMergeRun {
        id: String,
    },
    ListCodexThreads {
        cwd: PathBuf,
        limit: Option<usize>,
    },
    GetCodexThread {
        cwd: PathBuf,
        selector: String,
    },
    ReadCodexThread {
        cwd: PathBuf,
        selector: String,
        include_turns: bool,
    },
    InspectCodexAppServers {
        cwd: PathBuf,
    },
    StartCodexThread {
        cwd: PathBuf,
        model: Option<String>,
        ephemeral: bool,
    },
    ResumeCodexThread {
        cwd: PathBuf,
        selector: String,
        model: Option<String>,
    },
    OpenManagedProject {
        cwd: PathBuf,
        title: Option<String>,
        objective: Option<String>,
        base_branch: Option<String>,
        worktree_root: Option<PathBuf>,
        director_model: Option<String>,
        dev_model: Option<String>,
        test_model: Option<String>,
        integration_model: Option<String>,
    },
    InitManagedProject {
        path: PathBuf,
        title: Option<String>,
        objective: Option<String>,
        template: Option<String>,
        base_branch: Option<String>,
        worktree_root: Option<PathBuf>,
        director_model: Option<String>,
        dev_model: Option<String>,
        test_model: Option<String>,
        integration_model: Option<String>,
    },
    DirectManagedProject {
        cwd: PathBuf,
        title: Option<String>,
        objective: Option<String>,
        base_branch: Option<String>,
        worktree_root: Option<PathBuf>,
        director_model: Option<String>,
        dev_model: Option<String>,
        test_model: Option<String>,
        integration_model: Option<String>,
        roles: Option<Vec<ThreadRole>>,
        bindings: Vec<ManagedProjectThreadAttachment>,
        scenario: Option<String>,
        seed_file: Option<PathBuf>,
    },
    SpawnManagedProject {
        cwd: PathBuf,
        roles: Option<Vec<ThreadRole>>,
    },
    AttachManagedProject {
        cwd: PathBuf,
        bindings: Vec<ManagedProjectThreadAttachment>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum DaemonResponse {
    Unit,
    Count(usize),
    Doctor(DoctorReport),
    CodexDoctor(CodexDoctorReport),
    Status(DaemonStatus),
    DashboardSummary(DashboardSummary),
    RepositorySummary(Option<GitRepositorySummary>),
    ManagedProjectInspection(ManagedProjectInspection),
    ManagedProjectPlan(ManagedProjectInspection),
    ManagedProjectThreadControl(ManagedProjectInspection),
    Projects(Vec<Project>),
    Project(Option<Project>),
    WorkUnits(Vec<WorkUnit>),
    WorkUnit(Option<WorkUnit>),
    ThreadBindings(Vec<ThreadBinding>),
    ThreadBinding(Option<ThreadBinding>),
    WorkspaceBindings(Vec<WorkspaceBinding>),
    WorkspaceBinding(Option<WorkspaceBinding>),
    MergeRuns(Vec<MergeRun>),
    MergeRun(Option<MergeRun>),
    CodexThreads(Vec<CodexThreadSummary>),
    CodexThread(Option<CodexThreadSummary>),
    CodexThreadDetails(Vec<CodexThreadDetail>),
    CodexThreadDetail(Option<CodexThreadDetail>),
    CodexAppServers(Vec<CodexAppServerSummary>),
    ManagedProject(ManagedProjectBootstrap),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectRoleBootstrap {
    pub role: ThreadRole,
    pub work_unit: WorkUnit,
    pub agent_path: PathBuf,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub control_mode: ManagedProjectThreadControlMode,
    pub branch_name: Option<String>,
    pub worktree_path: Option<PathBuf>,
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
    pub workspace_binding_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectBootstrap {
    pub project: Project,
    pub repo_root: PathBuf,
    pub base_branch: String,
    pub worktree_root: PathBuf,
    pub manifest_path: PathBuf,
    pub project_config_path: PathBuf,
    pub plan_path: PathBuf,
    pub contract_path: PathBuf,
    pub codex_config_path: PathBuf,
    pub project_config: ManagedProjectProjectConfig,
    pub plan: ManagedProjectPlan,
    pub startup: ManagedProjectStartupState,
    pub scenario: Option<ManagedProjectScenarioState>,
    pub roles: Vec<ManagedProjectRoleBootstrap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectInspection {
    pub bootstrap: ManagedProjectBootstrap,
    pub repository_summary: Option<GitRepositorySummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectThreadAttachment {
    pub role: ThreadRole,
    pub thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectStartupState {
    pub phase: ManagedProjectStartupPhase,
    pub updated_at: DateTime<Utc>,
    pub worker_reports: BTreeMap<String, ManagedProjectStartupRoleState>,
    pub director_ack: Option<ManagedProjectStartupDirectorAck>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedProjectStartupPhase {
    Scaffolded,
    ThreadsStarted,
    WorkerReportsPending,
    DirectorAckPending,
    Ready,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectStartupRoleState {
    pub status: ManagedProjectStartupRoleStatus,
    pub updated_at: DateTime<Utc>,
    pub turn_id: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedProjectStartupRoleStatus {
    NotStarted,
    Pending,
    Reported,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectStartupDirectorAck {
    pub status: ManagedProjectStartupAckStatus,
    pub updated_at: DateTime<Utc>,
    pub turn_id: Option<String>,
    pub summary: String,
    pub received_roles: Vec<String>,
    pub missing_roles: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedProjectStartupAckStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedProjectThreadControlMode {
    Director,
    ManualNextTurn,
    Manual,
    DirectorPaused,
}

impl Default for ManagedProjectThreadControlMode {
    fn default() -> Self {
        Self::Director
    }
}

impl std::fmt::Display for ManagedProjectThreadControlMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Director => "director",
            Self::ManualNextTurn => "manual_next_turn",
            Self::Manual => "manual",
            Self::DirectorPaused => "director_paused",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectScenarioState {
    pub scenario_id: String,
    pub scenario_kind: String,
    pub operator_seed: String,
    pub current_round: usize,
    pub current_phase: String,
    #[serde(default)]
    pub liveness_policy: ManagedProjectLivenessPolicy,
    pub watchdog: Option<ManagedProjectWatchdogState>,
    pub pending_approval: Option<ManagedProjectApprovalState>,
    pub rounds: Vec<ManagedProjectRoundState>,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectLivenessPolicy {
    pub expected_long_build: bool,
    pub require_progress_updates: bool,
    pub soft_silence_seconds: u64,
    pub hard_ceiling_seconds: u64,
}

impl Default for ManagedProjectLivenessPolicy {
    fn default() -> Self {
        Self {
            expected_long_build: false,
            require_progress_updates: true,
            soft_silence_seconds: 900,
            hard_ceiling_seconds: 7_200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ManagedProjectProjectConfig {
    pub schema: String,
    pub title: String,
    pub objective: String,
    pub base_branch: String,
    pub branch_prefix: String,
    pub tt_runtime_bin: Option<String>,
    pub plan_first: bool,
    pub commit_policy: String,
    pub require_operator_merge_approval: bool,
    pub expected_long_build: bool,
    pub require_progress_updates: bool,
    pub soft_silence_seconds: u64,
    pub hard_ceiling_seconds: u64,
    pub default_validation_commands: Vec<String>,
    pub smoke_validation_commands: Vec<String>,
    pub checkpoint_triggers: Vec<String>,
    pub pitfalls: Vec<String>,
    pub hints: Vec<String>,
    pub exceptions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ManagedProjectPlan {
    pub schema: String,
    pub status: String,
    pub objective: String,
    pub updated_at: String,
    pub milestones: Vec<ManagedProjectPlanMilestone>,
    pub work_items: Vec<ManagedProjectPlanWorkItem>,
    pub notes: ManagedProjectPlanNotes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectPlanMilestone {
    pub id: String,
    pub title: String,
    pub success_criteria: Vec<String>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectPlanWorkItem {
    pub id: String,
    pub title: String,
    pub owner_role: String,
    pub phase: String,
    pub depends_on: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub validation_commands: Vec<String>,
    pub commit_required: bool,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ManagedProjectPlanNotes {
    pub risks: Vec<String>,
    pub pitfalls: Vec<String>,
    pub open_questions: Vec<String>,
    pub operator_constraints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectWatchdogState {
    pub state: String,
    pub last_signal: Option<String>,
    pub last_observed_at: Option<DateTime<Utc>>,
    pub last_progress_at: Option<DateTime<Utc>>,
    pub role: Option<String>,
    pub round: Option<usize>,
    pub turn_id: Option<String>,
    pub elapsed_seconds: u64,
    pub silence_seconds: u64,
    pub turn_status: Option<String>,
    pub turn_items: usize,
    pub app_server_log_modified_at: Option<i64>,
    pub app_server_log_size: Option<u64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectApprovalState {
    pub approval_kind: String,
    pub requested_by_role: String,
    pub prompt: String,
    pub approved: bool,
    pub response: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectProgressEvent {
    pub event: String,
    pub scenario_id: String,
    pub scenario_kind: String,
    pub phase: String,
    pub round: usize,
    pub role: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub state: Option<String>,
    pub signal: Option<String>,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedProjectEventKind {
    PromptSent,
    ResponseReceived,
    ParseFailed,
    TurnFailed,
    PhaseChanged,
    SystemNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectEvent {
    pub ts: DateTime<Utc>,
    pub project_id: String,
    pub phase: String,
    pub kind: ManagedProjectEventKind,
    pub role: Option<String>,
    pub counterparty_role: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub text: String,
    pub status: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectRoundState {
    pub round_number: usize,
    pub phase: String,
    pub director_turn_id: Option<String>,
    pub director_summary: Option<String>,
    pub role_handoffs: BTreeMap<String, ManagedProjectRoleHandoff>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectRoleHandoff {
    pub role: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub prompt_summary: String,
    pub handoff_summary: Option<String>,
    pub status: Option<String>,
    pub changed_files: Vec<String>,
    pub tests_run: Vec<String>,
    pub blockers: Vec<String>,
    pub next_step: Option<String>,
    pub handoff_source: String,
    pub handoff_parse_error: Option<String>,
    pub raw_handoff_text: Option<String>,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StructuredWorkerHandoff {
    status: String,
    changed_files: Vec<String>,
    tests_run: Vec<String>,
    blockers: Vec<String>,
    next_step: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WorkerHandoffExtraction {
    handoff: Option<StructuredWorkerHandoff>,
    raw_text: Option<String>,
    source: WorkerHandoffSource,
    parse_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkerHandoffSource {
    Extracted,
    SeededFallback,
}

fn handoff_source_string(source: &WorkerHandoffSource) -> &'static str {
    match source {
        WorkerHandoffSource::Extracted => "extracted",
        WorkerHandoffSource::SeededFallback => "seeded_fallback",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedProjectScenarioSeed {
    operator_seed: String,
    landing_approval: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedProjectManifest {
    schema: String,
    project_id: String,
    slug: String,
    title: String,
    objective: String,
    repo_root: String,
    base_branch: String,
    worktree_root: String,
    #[serde(default)]
    project_config_path: String,
    #[serde(default)]
    plan_path: String,
    #[serde(default)]
    project_config_sha256: String,
    #[serde(default)]
    plan_sha256: String,
    contract_path: String,
    codex_config_path: String,
    #[serde(default = "default_managed_project_startup_state")]
    startup: ManagedProjectStartupState,
    scenario: Option<ManagedProjectScenarioState>,
    roles: BTreeMap<String, ManagedProjectManifestRole>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedProjectManifestRole {
    work_unit_id: String,
    agent_path: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    #[serde(default)]
    control_mode: ManagedProjectThreadControlMode,
    branch_name: Option<String>,
    worktree_path: Option<String>,
    thread_id: Option<String>,
    thread_name: Option<String>,
    workspace_binding_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedAgentFile {
    name: String,
    description: String,
    model: Option<String>,
    model_reasoning_effort: Option<String>,
    sandbox_mode: String,
    developer_instructions: String,
}

#[derive(Debug, Clone, Copy)]
struct ManagedProjectRoundSpec {
    round_number: usize,
    phase: &'static str,
    director_goal: &'static str,
    dev_goal: &'static str,
    test_goal: &'static str,
    integration_goal: &'static str,
    requires_landing_approval: bool,
}

#[derive(Debug, Clone)]
struct ManagedProjectTurnOutcome {
    turn_id: String,
    summary: String,
    extraction: WorkerHandoffExtraction,
    attempts: Vec<ManagedProjectTurnAttempt>,
    watchdog: Option<ManagedProjectWatchdogState>,
}

#[derive(Debug, Clone)]
struct ManagedProjectTurnAttempt {
    attempt_number: usize,
    model: String,
    reasoning_effort: String,
    thread_id: String,
    turn_id: Option<String>,
    status: Option<String>,
    failure_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedProjectBootstrapWorkerReport {
    role: String,
    cwd: String,
    worktree: String,
    branch: String,
    contract_loaded: bool,
    plan_loaded: bool,
    status: String,
    blocker: Option<String>,
    summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedProjectBootstrapDirectorAckPayload {
    status: String,
    received_roles: Vec<String>,
    missing_roles: Vec<String>,
    summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectThreadControlState {
    pub role: String,
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
    pub workspace_binding_id: Option<String>,
    pub mode: ManagedProjectThreadControlMode,
}

#[derive(Debug, Clone)]
pub struct DaemonService {
    store: Arc<OverlayStore>,
    codex_home: Option<CodexHome>,
}

#[derive(Debug, Clone)]
pub struct DaemonRuntime {
    cwd: PathBuf,
    service: DaemonService,
}

#[derive(Debug, Clone)]
pub struct DaemonClient {
    socket_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DaemonServer {
    runtime: DaemonRuntime,
    socket_path: PathBuf,
}

impl DaemonRuntime {
    pub fn open(cwd: impl AsRef<Path>) -> Result<Self> {
        let cwd = cwd.as_ref().to_path_buf();
        let store = OverlayStore::open_in_dir(&cwd)?;
        let codex_home = CodexHome::discover_in(&cwd)?;
        let service = DaemonService::with_codex_home(store, codex_home);
        Ok(Self { cwd, service })
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn service(&self) -> &DaemonService {
        &self.service
    }

    pub fn request(&self, request: DaemonRequest) -> Result<DaemonResponse> {
        self.service.handle_request(request)
    }
}

impl DaemonClient {
    pub fn connect(socket_path: impl AsRef<Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();
        if !socket_path.exists() {
            anyhow::bail!("daemon socket {} does not exist", socket_path.display());
        }
        Ok(Self { socket_path })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn request(&self, request: DaemonRequest) -> Result<DaemonResponse> {
        let mut stream = UnixStream::connect(&self.socket_path)?;
        send_request(&mut stream, &request)?;
        recv_response(&mut stream)
    }
}

impl DaemonServer {
    pub fn new(runtime: DaemonRuntime) -> Self {
        let socket_path = socket_path_for(runtime.cwd());
        Self {
            runtime,
            socket_path,
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn serve(&self) -> Result<()> {
        if let Some(parent) = self.socket_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if self.socket_path.exists() {
            let _ = fs::remove_file(&self.socket_path);
        }
        let listener = UnixListener::bind(&self.socket_path)?;
        for incoming in listener.incoming() {
            let runtime = self.runtime.clone();
            match incoming {
                Ok(stream) => {
                    thread::spawn(move || {
                        if let Err(error) = handle_connection(&runtime, stream) {
                            eprintln!("tt daemon connection error: {error:?}");
                        }
                    });
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

impl DaemonService {
    pub fn new(store: OverlayStore) -> Self {
        Self {
            store: Arc::new(store),
            codex_home: None,
        }
    }

    pub fn with_codex_home(store: OverlayStore, codex_home: CodexHome) -> Self {
        Self {
            store: Arc::new(store),
            codex_home: Some(codex_home),
        }
    }

    pub fn codex_home(&self) -> Option<&CodexHome> {
        self.codex_home.as_ref()
    }

    pub fn store(&self) -> &OverlayStore {
        self.store.as_ref()
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        self.store.list_projects()
    }

    pub fn get_project(&self, id_or_slug: &str) -> Result<Option<Project>> {
        self.store.get_project(id_or_slug)
    }

    pub fn upsert_project(&self, project: &Project) -> Result<()> {
        self.store.upsert_project(project)
    }

    pub fn set_project_status(&self, id_or_slug: &str, status: ProjectStatus) -> Result<usize> {
        self.store.set_project_status(id_or_slug, status)
    }

    pub fn delete_project(&self, id_or_slug: &str) -> Result<usize> {
        self.store.delete_project(id_or_slug)
    }

    pub fn list_work_units(&self, project_id: Option<&str>) -> Result<Vec<WorkUnit>> {
        self.store.list_work_units(project_id)
    }

    pub fn get_work_unit(&self, id_or_slug: &str) -> Result<Option<WorkUnit>> {
        self.store.get_work_unit(id_or_slug)
    }

    pub fn upsert_work_unit(&self, work_unit: &WorkUnit) -> Result<()> {
        self.store.upsert_work_unit(work_unit)
    }

    pub fn set_work_unit_status(&self, id_or_slug: &str, status: WorkUnitStatus) -> Result<usize> {
        self.store.set_work_unit_status(id_or_slug, status)
    }

    pub fn delete_work_unit(&self, id_or_slug: &str) -> Result<usize> {
        self.store.delete_work_unit(id_or_slug)
    }

    pub fn list_thread_bindings(&self) -> Result<Vec<ThreadBinding>> {
        self.store.list_thread_bindings()
    }

    pub fn get_thread_binding(&self, codex_thread_id: &str) -> Result<Option<ThreadBinding>> {
        self.store.get_thread_binding(codex_thread_id)
    }

    pub fn upsert_thread_binding(&self, binding: &ThreadBinding) -> Result<()> {
        self.store.upsert_thread_binding(binding)
    }

    pub fn set_thread_binding_status(
        &self,
        codex_thread_id: &str,
        status: ThreadBindingStatus,
    ) -> Result<usize> {
        self.store
            .set_thread_binding_status(codex_thread_id, status)
    }

    pub fn delete_thread_binding(&self, codex_thread_id: &str) -> Result<usize> {
        self.store.delete_thread_binding(codex_thread_id)
    }

    pub fn list_thread_bindings_for_work_unit(
        &self,
        work_unit_id: &str,
    ) -> Result<Vec<ThreadBinding>> {
        self.store.list_thread_bindings_for_work_unit(work_unit_id)
    }

    pub fn list_workspace_bindings(&self) -> Result<Vec<WorkspaceBinding>> {
        self.store.list_workspace_bindings()
    }

    pub fn get_workspace_binding(&self, id: &str) -> Result<Option<WorkspaceBinding>> {
        self.store.get_workspace_binding(id)
    }

    pub fn upsert_workspace_binding(&self, binding: &WorkspaceBinding) -> Result<()> {
        self.store.upsert_workspace_binding(binding)
    }

    pub fn set_workspace_binding_status(&self, id: &str, status: WorkspaceStatus) -> Result<usize> {
        self.store.set_workspace_binding_status(id, status)
    }

    pub fn delete_workspace_binding(&self, id: &str) -> Result<usize> {
        self.store.delete_workspace_binding(id)
    }

    pub fn list_workspace_bindings_for_thread(
        &self,
        codex_thread_id: &str,
    ) -> Result<Vec<WorkspaceBinding>> {
        self.store
            .list_workspace_bindings_for_thread(codex_thread_id)
    }

    pub fn refresh_workspace_binding(&self, id: &str) -> Result<Option<WorkspaceBinding>> {
        let Some(mut binding) = self.get_workspace_binding(id)? else {
            return Ok(None);
        };
        let inspection_cwd = binding
            .worktree_path
            .as_deref()
            .map(Path::new)
            .unwrap_or_else(|| Path::new(&binding.repo_root));
        let Some(inspection) = tt_git::GitRepository::inspect(inspection_cwd)? else {
            binding.status = WorkspaceStatus::Requested;
            binding.updated_at = Utc::now();
            self.upsert_workspace_binding(&binding)?;
            return Ok(Some(binding));
        };

        binding.branch_name = inspection.current_branch.clone();
        binding.base_commit = inspection.current_head_commit.clone();
        binding.status = workspace_status_from_git(&inspection);
        binding.updated_at = Utc::now();
        self.upsert_workspace_binding(&binding)?;
        Ok(Some(binding))
    }

    pub fn prepare_workspace_binding(&self, id: &str) -> Result<Option<WorkspaceBinding>> {
        let Some(binding) = self.refresh_workspace_binding(id)? else {
            return Ok(None);
        };
        self.store
            .record_workspace_lifecycle_event(&binding.id, None, "prepare", None)?;
        Ok(Some(binding))
    }

    pub fn merge_prep_workspace_binding(&self, id: &str) -> Result<Option<MergeRun>> {
        let Some(binding) = self.refresh_workspace_binding(id)? else {
            return Ok(None);
        };
        self.store
            .record_workspace_lifecycle_event(&binding.id, None, "merge-prep", None)?;
        self.refresh_merge_run(&binding.id)
    }

    pub fn authorize_merge_workspace_binding(&self, id: &str) -> Result<Option<MergeRun>> {
        let Some(binding) = self.refresh_workspace_binding(id)? else {
            return Ok(None);
        };
        let Some(mut run) = self.refresh_merge_run(&binding.id)? else {
            return Ok(None);
        };
        if run.readiness != MergeReadiness::Ready {
            anyhow::bail!(
                "workspace binding `{}` is not ready for merge authorization",
                binding.id
            );
        }
        run.authorization = MergeAuthorizationStatus::Authorized;
        run.updated_at = Utc::now();
        self.upsert_merge_run(&run)?;
        self.store
            .record_workspace_lifecycle_event(&binding.id, None, "authorize-merge", None)?;
        Ok(Some(run))
    }

    pub fn execute_landing_workspace_binding(&self, id: &str) -> Result<Option<MergeRun>> {
        let Some(binding) = self.refresh_workspace_binding(id)? else {
            return Ok(None);
        };
        let Some(mut run) = self.refresh_merge_run(&binding.id)? else {
            return Ok(None);
        };
        if run.readiness != MergeReadiness::Ready
            || run.authorization != MergeAuthorizationStatus::Authorized
        {
            anyhow::bail!(
                "workspace binding `{}` is not authorized for landing",
                binding.id
            );
        }
        run.execution = MergeExecutionStatus::Succeeded;
        run.head_commit = binding.base_commit.clone();
        run.updated_at = Utc::now();
        self.upsert_merge_run(&run)?;
        let mut updated_binding = binding.clone();
        updated_binding.status = WorkspaceStatus::Merged;
        updated_binding.updated_at = Utc::now();
        self.upsert_workspace_binding(&updated_binding)?;
        self.store.record_workspace_lifecycle_event(
            &updated_binding.id,
            None,
            "execute-landing",
            None,
        )?;
        Ok(Some(run))
    }

    pub fn prune_workspace_binding(
        &self,
        id: &str,
        force: bool,
    ) -> Result<Option<WorkspaceBinding>> {
        let Some(mut binding) = self.get_workspace_binding(id)? else {
            return Ok(None);
        };
        let inspection_cwd = binding
            .worktree_path
            .as_deref()
            .map(Path::new)
            .unwrap_or_else(|| Path::new(&binding.repo_root));
        let Some(inspection) = tt_git::GitRepository::inspect(inspection_cwd)? else {
            if !force {
                anyhow::bail!(
                    "workspace binding `{}` has no inspectable git repository",
                    binding.id
                );
            }
            binding.status = WorkspaceStatus::Pruned;
            binding.updated_at = Utc::now();
            self.upsert_workspace_binding(&binding)?;
            self.store
                .record_workspace_lifecycle_event(&binding.id, None, "prune", None)?;
            return Ok(Some(binding));
        };

        if inspection.dirty && !force {
            anyhow::bail!(
                "workspace binding `{}` has dirty changes; pass force to prune",
                binding.id
            );
        }

        let worktree_path = inspection
            .current_worktree
            .clone()
            .or_else(|| binding.worktree_path.as_ref().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(&binding.repo_root));
        let repository = tt_git::GitRepository {
            repository_root: inspection.repository_root.clone(),
        };
        if !repository.prune_worktree(&worktree_path)? {
            anyhow::bail!("failed to prune worktree {}", worktree_path.display());
        }
        if let Some(branch_name) = inspection.current_branch.as_deref()
            && !repository.delete_branch(branch_name)?
        {
            anyhow::bail!("failed to delete branch {}", branch_name);
        }
        binding.status = WorkspaceStatus::Pruned;
        binding.updated_at = Utc::now();
        self.upsert_workspace_binding(&binding)?;
        self.store
            .record_workspace_lifecycle_event(&binding.id, None, "prune", None)?;
        Ok(Some(binding))
    }

    pub fn close_workspace(
        &self,
        cwd: impl AsRef<Path>,
        selector: Option<&str>,
        force: bool,
    ) -> Result<Option<WorkspaceBinding>> {
        let Some(binding) = self.resolve_workspace_binding(cwd.as_ref(), selector)? else {
            return Ok(None);
        };
        let Some(binding) = self.prune_workspace_binding(&binding.id, force)? else {
            return Ok(None);
        };
        self.store.record_workspace_lifecycle_event(
            &binding.id,
            None,
            "close",
            if force { Some("forced") } else { None },
        )?;
        Ok(Some(binding))
    }

    pub fn clean_managed_project(&self, cwd: impl AsRef<Path>, force: bool) -> Result<usize> {
        let cwd = cwd.as_ref();
        let Some(repo_root) = managed_project_repo_root(cwd)? else {
            anyhow::bail!("managed project clean requires a git repository");
        };
        if !repo_root.join(".tt").is_dir() {
            return Ok(0);
        }

        let repo_root_text = repo_root.display().to_string();
        let manifest_path = repo_root.join(".tt").join("state.toml");
        let manifest = load_managed_project_manifest(&manifest_path).ok();
        let repository = GitRepository::discover(&repo_root)?
            .ok_or_else(|| anyhow::anyhow!("managed project clean requires a git repository"))?;
        let mut worktree_targets: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
        if let Some(manifest) = manifest.as_ref() {
            for role in manifest.roles.values() {
                if let Some(worktree_path) = role.worktree_path.as_ref() {
                    worktree_targets.insert(PathBuf::from(worktree_path), role.branch_name.clone());
                }
            }
        }
        let workspace_bindings = self
            .list_workspace_bindings()?
            .into_iter()
            .filter(|binding| binding.repo_root == repo_root_text)
            .collect::<Vec<_>>();
        for binding in &workspace_bindings {
            if let Some(worktree_path) = binding.worktree_path.as_ref() {
                worktree_targets
                    .entry(PathBuf::from(worktree_path))
                    .or_insert_with(|| binding.branch_name.clone());
            }
        }
        let registered_worktrees = repository.list_worktrees()?;

        if !force {
            for worktree_path in worktree_targets.keys() {
                if !worktree_path.exists() {
                    continue;
                }
                let is_registered = registered_worktrees
                    .iter()
                    .any(|entry| entry.worktree_path == *worktree_path);
                if is_registered {
                    let Some(inspection) = tt_git::GitRepository::inspect(&worktree_path)? else {
                        anyhow::bail!(
                            "workspace binding `{}` has no inspectable git repository; pass --all to clean",
                            worktree_path.display()
                        );
                    };
                    if inspection.dirty {
                        anyhow::bail!(
                            "workspace binding `{}` has dirty changes; pass --all to clean",
                            worktree_path.display()
                        );
                    }
                }
            }
        }
        let mut removed = 0usize;
        for (worktree_path, branch_name) in &worktree_targets {
            let is_registered = registered_worktrees
                .iter()
                .any(|entry| entry.worktree_path == *worktree_path);
            if is_registered {
                if repository.prune_worktree(worktree_path)? {
                    removed += 1;
                } else if !force {
                    anyhow::bail!(
                        "failed to prune worktree {}; pass --all to force cleanup",
                        worktree_path.display()
                    );
                }
            } else if worktree_path.exists() {
                removed += remove_if_exists(worktree_path.to_path_buf())?;
            }
            if let Some(branch_name) = branch_name.as_deref() {
                let branch_deleted = repository.delete_branch(branch_name)?;
                if branch_deleted {
                    removed += 1;
                }
            }
        }

        for binding in &workspace_bindings {
            removed += self.delete_workspace_binding(&binding.id)?;
        }

        for project in self.list_projects()? {
            removed += self.delete_project(&project.id)?;
        }

        removed += remove_if_exists(repo_root.join(".tt").join("state.toml"))?;
        removed += remove_if_exists(repo_root.join(".tt").join(TT_EVENTS_FILE_NAME))?;
        removed += remove_if_exists(repo_root.join(".tt").join(TT_DAEMON_SOCKET_NAME))?;
        removed += remove_if_exists(
            repo_root
                .join(".tt")
                .join(TT_CODEX_APP_SERVER_LOG_FILE_NAME),
        )?;
        removed += remove_if_exists(repo_root.join(".tt").join("runtime"))?;
        removed += remove_if_exists(repo_root.join(".tt").join("contracts"))?;
        if force {
            removed += prune_repo_codex_runtime_artifacts(&repo_root.join(".codex"))?;
        }
        Ok(removed)
    }

    pub fn park_workspace(
        &self,
        cwd: impl AsRef<Path>,
        selector: Option<&str>,
        note: Option<String>,
    ) -> Result<Option<WorkspaceBinding>> {
        let Some(mut binding) = self.resolve_workspace_binding(cwd.as_ref(), selector)? else {
            return Ok(None);
        };
        binding.status = WorkspaceStatus::Abandoned;
        binding.updated_at = Utc::now();
        self.upsert_workspace_binding(&binding)?;
        self.store
            .record_workspace_lifecycle_event(&binding.id, None, "park", note.as_deref())?;
        Ok(Some(binding))
    }

    pub fn split_workspace(
        &self,
        cwd: impl AsRef<Path>,
        role: ThreadRole,
        model: Option<String>,
        ephemeral: bool,
    ) -> Result<Option<WorkspaceBinding>> {
        let cwd = cwd.as_ref();
        let Some(repository) = tt_git::GitRepository::inspect(cwd)? else {
            return Ok(None);
        };
        let repository_handle = tt_git::GitRepository {
            repository_root: repository.repository_root.clone(),
        };
        let client = self.codex_runtime_client(cwd)?;
        let split_id = uuid::Uuid::new_v4().to_string();
        let branch_name = format!("tt/{}", sanitize_branch_component(&split_id));
        let worktree_root = repository
            .repository_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| repository.repository_root.clone());
        let short_id = split_id.chars().take(8).collect::<String>();
        let worktree_path = worktree_root.join(format!("tt-{short_id}"));
        if !repository_handle.create_worktree(
            &worktree_path,
            &branch_name,
            repository.current_head_commit.as_deref(),
        )? {
            anyhow::bail!("failed to create worktree {}", worktree_path.display());
        }
        let snapshot = client.start_thread(&worktree_path, model, ephemeral)?;
        let now = Utc::now();
        let thread_binding = ThreadBinding {
            codex_thread_id: snapshot.thread_id.clone(),
            work_unit_id: None,
            role,
            status: ThreadBindingStatus::Bound,
            notes: Some("split from current workspace".to_string()),
            created_at: now,
            updated_at: now,
        };
        self.upsert_thread_binding(&thread_binding)?;
        let workspace_binding = WorkspaceBinding {
            id: split_id,
            codex_thread_id: snapshot.thread_id.clone(),
            repo_root: repository.repository_root.display().to_string(),
            worktree_path: Some(worktree_path.display().to_string()),
            branch_name: Some(branch_name),
            base_ref: repository.current_branch.clone(),
            base_commit: repository.current_head_commit.clone(),
            landing_target: repository.current_branch.clone(),
            strategy: WorkspaceStrategy::DedicatedWorktree,
            sync_policy: WorkspaceSyncPolicy::RebaseBeforeLanding,
            cleanup_policy: WorkspaceCleanupPolicy::PruneAfterLanding,
            status: if repository.dirty {
                WorkspaceStatus::Requested
            } else {
                WorkspaceStatus::Ready
            },
            created_at: now,
            updated_at: now,
        };
        self.upsert_workspace_binding(&workspace_binding)?;
        self.store.record_workspace_lifecycle_event(
            &workspace_binding.id,
            None,
            "split",
            Some(snapshot.thread_id.as_str()),
        )?;
        Ok(Some(workspace_binding))
    }

    fn resolve_workspace_binding(
        &self,
        cwd: &Path,
        selector: Option<&str>,
    ) -> Result<Option<WorkspaceBinding>> {
        if let Some(selector) = selector {
            if let Some(binding) = self.get_workspace_binding(selector)? {
                return Ok(Some(binding));
            }
            if let Some(binding) = self
                .list_workspace_bindings_for_thread(selector)?
                .into_iter()
                .next()
            {
                return Ok(Some(binding));
            }
        }
        if let Some(inspection) = tt_git::GitRepository::inspect(cwd)? {
            let repo_root = inspection.repository_root.display().to_string();
            let current_worktree = inspection
                .current_worktree
                .as_ref()
                .map(|path| path.display().to_string());
            for binding in self.list_workspace_bindings()? {
                if binding.repo_root == repo_root
                    || current_worktree.as_ref().is_some_and(|worktree| {
                        binding.worktree_path.as_deref() == Some(worktree.as_str())
                    })
                {
                    return Ok(Some(binding));
                }
            }
        }
        Ok(None)
    }

    pub fn list_merge_runs(&self) -> Result<Vec<MergeRun>> {
        self.store.list_merge_runs()
    }

    pub fn get_merge_run(&self, id: &str) -> Result<Option<MergeRun>> {
        self.store.get_merge_run(id)
    }

    pub fn upsert_merge_run(&self, run: &MergeRun) -> Result<()> {
        self.store.upsert_merge_run(run)
    }

    pub fn set_merge_run_status(
        &self,
        id: &str,
        readiness: MergeReadiness,
        authorization: MergeAuthorizationStatus,
        execution: MergeExecutionStatus,
        head_commit: Option<String>,
    ) -> Result<usize> {
        self.store
            .set_merge_run_status(id, readiness, authorization, execution, head_commit)
    }

    pub fn delete_merge_run(&self, id: &str) -> Result<usize> {
        self.store.delete_merge_run(id)
    }

    pub fn refresh_merge_run(&self, workspace_binding_id: &str) -> Result<Option<MergeRun>> {
        let Some(binding) = self.get_workspace_binding(workspace_binding_id)? else {
            return Ok(None);
        };
        let inspection_cwd = binding
            .worktree_path
            .as_deref()
            .map(Path::new)
            .unwrap_or_else(|| Path::new(&binding.repo_root));
        let Some(inspection) = tt_git::GitRepository::inspect(inspection_cwd)? else {
            let now = Utc::now();
            let run = MergeRun {
                id: workspace_binding_id.to_string(),
                workspace_binding_id: workspace_binding_id.to_string(),
                readiness: MergeReadiness::Unknown,
                authorization: MergeAuthorizationStatus::NotRequested,
                execution: MergeExecutionStatus::NotStarted,
                head_commit: None,
                created_at: now,
                updated_at: now,
            };
            self.upsert_merge_run(&run)?;
            return Ok(Some(run));
        };
        let now = Utc::now();
        let existing = self
            .store
            .get_merge_run_for_workspace_binding(workspace_binding_id)?;
        let mut run = existing.unwrap_or_else(|| MergeRun {
            id: workspace_binding_id.to_string(),
            workspace_binding_id: workspace_binding_id.to_string(),
            readiness: MergeReadiness::Unknown,
            authorization: MergeAuthorizationStatus::NotRequested,
            execution: MergeExecutionStatus::NotStarted,
            head_commit: None,
            created_at: now,
            updated_at: now,
        });
        run.readiness = inspection.merge_readiness;
        run.authorization = match binding.status {
            WorkspaceStatus::Merged => MergeAuthorizationStatus::Authorized,
            WorkspaceStatus::Abandoned | WorkspaceStatus::Pruned => {
                MergeAuthorizationStatus::Rejected
            }
            _ => run.authorization,
        };
        run.execution = match binding.status {
            WorkspaceStatus::Merged => MergeExecutionStatus::Succeeded,
            WorkspaceStatus::Abandoned | WorkspaceStatus::Pruned => MergeExecutionStatus::Failed,
            _ => run.execution,
        };
        run.head_commit = inspection.current_head_commit.clone();
        run.updated_at = now;
        self.upsert_merge_run(&run)?;
        Ok(Some(run))
    }

    pub fn codex_catalog(&self) -> Result<Option<tt_codex::CodexSessionCatalog>> {
        self.codex_home
            .as_ref()
            .map(|home| home.session_catalog())
            .transpose()
    }

    pub fn codex_threads(
        &self,
        cwd: impl AsRef<Path>,
        limit: Option<usize>,
    ) -> Result<Vec<CodexThreadSummary>> {
        let Some(catalog) = CodexHome::discover_in(cwd)?.session_catalog().ok() else {
            return Ok(Vec::new());
        };
        let limit = limit.unwrap_or(catalog.all_threads().len());
        Ok(catalog
            .recent_threads(limit)
            .into_iter()
            .map(|thread| {
                let bound_work_unit_id = self
                    .get_thread_binding(&thread.thread_id)
                    .ok()
                    .flatten()
                    .and_then(|binding| binding.work_unit_id);
                let workspace_binding_count = self
                    .list_workspace_bindings_for_thread(&thread.thread_id)
                    .map(|bindings| bindings.len())
                    .unwrap_or(0);
                CodexThreadSummary {
                    thread_id: thread.thread_id,
                    thread_name: thread.thread_name,
                    updated_at: thread.updated_at.map(|value| value.to_rfc3339()),
                    bound_work_unit_id,
                    workspace_binding_count,
                }
            })
            .collect())
    }

    pub fn codex_thread(
        &self,
        cwd: impl AsRef<Path>,
        selector: &str,
    ) -> Result<Option<CodexThreadSummary>> {
        let Some(catalog) = CodexHome::discover_in(cwd)?.session_catalog().ok() else {
            return Ok(None);
        };
        let Some(thread) = catalog.resolve_thread(selector) else {
            return Ok(None);
        };
        let bound_work_unit_id = self
            .get_thread_binding(&thread.thread_id)?
            .and_then(|binding| binding.work_unit_id);
        let workspace_binding_count = self
            .list_workspace_bindings_for_thread(&thread.thread_id)?
            .len();
        Ok(Some(CodexThreadSummary {
            thread_id: thread.thread_id.clone(),
            thread_name: thread.thread_name.clone(),
            updated_at: thread.updated_at.map(|value| value.to_rfc3339()),
            bound_work_unit_id,
            workspace_binding_count,
        }))
    }

    pub fn read_codex_thread(
        &self,
        cwd: impl AsRef<Path>,
        selector: &str,
        include_turns: bool,
    ) -> Result<Option<CodexThreadDetail>> {
        let client = self.codex_runtime_client(cwd.as_ref())?;
        if include_turns {
            let Some(thread) = client.read_thread_full(selector, true)? else {
                return Ok(None);
            };
            return self.enrich_codex_thread(thread).map(Some);
        }
        let Some(snapshot) = client.read_thread(selector, false)? else {
            return Ok(None);
        };
        Ok(Some(self.enrich_codex_snapshot(snapshot)?))
    }

    pub fn start_codex_thread(
        &self,
        cwd: impl AsRef<Path>,
        model: Option<String>,
        ephemeral: bool,
    ) -> Result<CodexThreadDetail> {
        let client = self.codex_runtime_client(cwd.as_ref())?;
        let snapshot = client.start_thread(cwd.as_ref(), model, ephemeral)?;
        self.enrich_codex_snapshot(snapshot)
    }

    pub fn resume_codex_thread(
        &self,
        cwd: impl AsRef<Path>,
        selector: &str,
        model: Option<String>,
    ) -> Result<Option<CodexThreadDetail>> {
        let client = self.codex_runtime_client(cwd.as_ref())?;
        let Some(snapshot) = client.resume_thread(selector, Some(cwd.as_ref()), model)? else {
            return Ok(None);
        };
        Ok(Some(self.enrich_codex_snapshot(snapshot)?))
    }

    pub fn status(&self, cwd: impl AsRef<Path>) -> Result<DaemonStatus> {
        let cwd = cwd.as_ref();
        let repo_root = managed_project_repo_root(cwd)?;
        let (project_initialized, project_state) = match &repo_root {
            Some(repo_root) => managed_project_status_for_repo(repo_root)?,
            None => (false, None),
        };
        let director_state = match &repo_root {
            Some(repo_root) => managed_project_director_state_for_repo(self, repo_root)?,
            None => ManagedProjectDirectorState::Missing,
        };
        Ok(DaemonStatus {
            repo_root,
            project_initialized,
            project_state,
            director_state,
            project_count: self.store.count_projects()?,
            work_unit_count: self.store.count_work_units()?,
            bound_thread_count: self.store.count_bound_threads()?,
            ready_workspace_count: self.store.count_ready_workspaces()?,
        })
    }

    pub fn codex_doctor(&self, cwd: impl AsRef<Path>) -> CodexDoctorReport {
        codex_doctor_for_cwd(cwd, false)
    }

    pub fn codex_doctor_with_listen_check(&self, cwd: impl AsRef<Path>) -> CodexDoctorReport {
        codex_doctor_for_cwd(cwd, true)
    }

    pub fn inspect_codex_app_servers(
        &self,
        cwd: impl AsRef<Path>,
    ) -> Result<Vec<CodexAppServerSummary>> {
        Ok(vec![codex_app_server_summary_for_cwd(cwd)])
    }

    pub fn doctor(&self, cwd: impl AsRef<Path>, check_listen: bool) -> DoctorReport {
        doctor_for_cwd(cwd, check_listen)
    }

    pub fn dashboard_summary(&self) -> Result<DashboardSummary> {
        let status = self.status(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))?;
        Ok(DashboardSummary {
            active_projects: status.project_count,
            active_work_units: status.work_unit_count,
            bound_threads: status.bound_thread_count,
            ready_workspaces: status.ready_workspace_count,
        })
    }

    pub fn repository_summary(
        &self,
        cwd: impl AsRef<Path>,
    ) -> Result<Option<GitRepositorySummary>> {
        let Some(inspection) = tt_git::GitRepository::inspect(cwd)? else {
            return Ok(None);
        };
        Ok(Some(GitRepositorySummary {
            repository_root: inspection.repository_root.display().to_string(),
            current_worktree: inspection
                .current_worktree
                .map(|path| path.display().to_string()),
            current_branch: inspection.current_branch,
            current_head_commit: inspection.current_head_commit,
            dirty: inspection.dirty,
            upstream: inspection.upstream,
            ahead_by: inspection.ahead_by,
            behind_by: inspection.behind_by,
            merge_ready: inspection.merge_readiness == MergeReadiness::Ready,
            worktree_count: inspection.worktrees.len(),
        }))
    }

    pub fn inspect_managed_project(
        &self,
        cwd: impl AsRef<Path>,
    ) -> Result<ManagedProjectInspection> {
        let cwd = cwd.as_ref();
        let Some(repo_root) = managed_project_repo_root(cwd)? else {
            anyhow::bail!("managed project inspect requires a git repository");
        };
        let manifest_path = require_initialized_managed_project(&repo_root)?;
        let manifest = load_managed_project_manifest(&manifest_path)?;
        let bootstrap = self.managed_project_bootstrap_from_manifest(&manifest_path, &manifest)?;
        Ok(ManagedProjectInspection {
            bootstrap,
            repository_summary: self.repository_summary(&repo_root)?,
        })
    }

    pub fn inspect_managed_project_plan(
        &self,
        cwd: impl AsRef<Path>,
    ) -> Result<ManagedProjectInspection> {
        self.inspect_managed_project(cwd)
    }

    pub fn refresh_managed_project_plan(
        &self,
        cwd: impl AsRef<Path>,
    ) -> Result<ManagedProjectInspection> {
        let cwd = cwd.as_ref();
        let Some(repo_root) = managed_project_repo_root(cwd)? else {
            anyhow::bail!("managed project plan refresh requires a git repository");
        };
        let manifest_path = require_initialized_managed_project(&repo_root)?;
        let manifest = load_managed_project_manifest(&manifest_path)?;
        let bootstrap = self.managed_project_bootstrap_from_manifest(&manifest_path, &manifest)?;
        Ok(ManagedProjectInspection {
            bootstrap,
            repository_summary: self.repository_summary(&repo_root)?,
        })
    }

    pub fn set_managed_project_thread_control(
        &self,
        cwd: impl AsRef<Path>,
        role: ThreadRole,
        mode: ManagedProjectThreadControlMode,
    ) -> Result<ManagedProjectInspection> {
        let cwd = cwd.as_ref();
        let Some(repo_root) = managed_project_repo_root(cwd)? else {
            anyhow::bail!("managed project control requires a git repository");
        };
        let manifest_path = require_initialized_managed_project(&repo_root)?;
        let manifest = load_managed_project_manifest(&manifest_path)?;
        let mut bootstrap =
            self.managed_project_bootstrap_from_manifest(&manifest_path, &manifest)?;
        let role_index = bootstrap
            .roles
            .iter()
            .position(|candidate| candidate.role == role)
            .ok_or_else(|| {
                anyhow::anyhow!("managed project role `{}` not found", role_slug(role))
            })?;
        bootstrap.roles[role_index].control_mode = mode;
        self.save_managed_project_bootstrap(&bootstrap)?;
        self.inspect_managed_project(cwd)
    }

    pub fn open_managed_project(
        &self,
        cwd: impl AsRef<Path>,
        title: Option<String>,
        objective: Option<String>,
        base_branch: Option<String>,
        worktree_root: Option<PathBuf>,
        director_model: Option<String>,
        dev_model: Option<String>,
        test_model: Option<String>,
        integration_model: Option<String>,
    ) -> Result<ManagedProjectBootstrap> {
        let cwd = cwd.as_ref();
        let Some(repo_root) = managed_project_repo_root(cwd)? else {
            anyhow::bail!("managed project open requires a git repository");
        };
        let repository = GitRepository::discover(&repo_root)?
            .ok_or_else(|| anyhow::anyhow!("managed project open requires a git repository"))?;
        let inspection = repository.inspect_repository()?;
        let repo_name = repo_root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("tt-project");
        let title = title.unwrap_or_else(|| repo_name.replace('-', " "));
        let slug = sanitize_project_slug(&title);
        let base_branch = base_branch
            .or(inspection.current_branch)
            .unwrap_or_else(|| "main".to_string());
        let objective = objective
            .unwrap_or_else(|| format!("Coordinate dev, test, and integration work for {title}"));
        let now = Utc::now();
        if self.get_project(&slug)?.is_some() {
            self.delete_project(&slug)?;
        }
        let project_id = uuid::Uuid::now_v7().to_string();
        let project = Project {
            id: project_id,
            slug: slug.clone(),
            title: title.clone(),
            objective: objective.clone(),
            status: ProjectStatus::Active,
            created_at: now,
            updated_at: now,
        };
        self.upsert_project(&project)?;

        let worktree_root = worktree_root
            .map(|path| resolve_path(cwd, path))
            .unwrap_or_else(|| default_worktree_root(&repo_root, &slug));
        fs::create_dir_all(&worktree_root)?;

        let contract_path = repo_root.join(".tt").join(TT_CONTRACT_FILE_NAME);
        let project_config_path = repo_root.join(".tt").join("project.toml");
        let plan_path = repo_root.join(".tt").join("plan.toml");
        let settings_env_path = repo_root.join(".tt").join("settings.env");
        let project_config = load_or_seed_managed_project_config(
            &project_config_path,
            &repo_root,
            title.as_str(),
            objective.as_str(),
            &base_branch,
        )?;
        let initial_plan = default_managed_project_plan(&project, &project_config, &[]);
        let codex_config_path = ensure_layered_codex_config(&repo_root)?;
        write_or_replace_managed_file(
            &contract_path,
            &render_worker_contract(&repo_root, &slug, &base_branch, &project_config),
        )?;
        if !settings_env_path.exists() {
            fs::write(&settings_env_path, render_default_settings_env())?;
        }

        let director_role = self.build_role_bootstrap(
            &repository,
            &project,
            ThreadRole::Director,
            &base_branch,
            director_model,
            &worktree_root,
            false,
            &project_config,
            &initial_plan,
        )?;
        let dev_role = self.build_role_bootstrap(
            &repository,
            &project,
            ThreadRole::Develop,
            &base_branch,
            dev_model,
            &worktree_root,
            true,
            &project_config,
            &initial_plan,
        )?;
        let test_role = self.build_role_bootstrap(
            &repository,
            &project,
            ThreadRole::Test,
            &base_branch,
            test_model,
            &worktree_root,
            true,
            &project_config,
            &initial_plan,
        )?;
        let integration_role = self.build_role_bootstrap(
            &repository,
            &project,
            ThreadRole::Integrate,
            &base_branch,
            integration_model,
            &worktree_root,
            true,
            &project_config,
            &initial_plan,
        )?;
        let plan = load_or_seed_managed_project_plan(
            &plan_path,
            &project_config,
            &project,
            &[&director_role, &dev_role, &test_role, &integration_role],
        )?;

        let mut bootstrap = ManagedProjectBootstrap {
            project,
            repo_root: repo_root.clone(),
            base_branch: base_branch.clone(),
            worktree_root,
            manifest_path: repo_root.join(".tt").join("state.toml"),
            project_config_path,
            plan_path,
            contract_path,
            codex_config_path,
            project_config,
            plan,
            startup: default_managed_project_startup_state(),
            scenario: None,
            roles: vec![director_role, dev_role, test_role, integration_role],
        };
        for role in default_managed_project_roles() {
            self.spawn_managed_project_role(&repository, &mut bootstrap, role)?;
        }
        bootstrap.startup.phase = ManagedProjectStartupPhase::ThreadsStarted;
        bootstrap.startup.updated_at = Utc::now();

        let manifest = build_managed_project_manifest(
            &bootstrap.project,
            &bootstrap.repo_root,
            &bootstrap.base_branch,
            &bootstrap.worktree_root,
            &bootstrap.project_config_path,
            &bootstrap.plan_path,
            &bootstrap.contract_path,
            &bootstrap.codex_config_path,
            &bootstrap.startup,
            None,
            &bootstrap.roles.iter().collect::<Vec<_>>(),
        )?;
        save_managed_project_manifest(&bootstrap.manifest_path, &manifest)?;
        self.launch_managed_project_startup(&bootstrap);

        Ok(bootstrap)
    }

    fn build_role_bootstrap(
        &self,
        repository: &GitRepository,
        project: &Project,
        role: ThreadRole,
        base_branch: &str,
        model: Option<String>,
        worktree_root: &Path,
        create_worktree: bool,
        project_config: &ManagedProjectProjectConfig,
        plan: &ManagedProjectPlan,
    ) -> Result<ManagedProjectRoleBootstrap> {
        let now = Utc::now();
        let role_slug = role_slug(role);
        let model = Some(model.unwrap_or_else(|| role_default_model(role).to_string()));
        let reasoning_effort = Some(role_default_reasoning_effort(role).to_string());
        let work_unit = WorkUnit {
            id: uuid::Uuid::now_v7().to_string(),
            project_id: project.id.clone(),
            slug: Some(role_slug.to_string()),
            title: role_title(role).to_string(),
            task: role_task(role, &project.title),
            status: WorkUnitStatus::Ready,
            created_at: now,
            updated_at: now,
        };
        self.upsert_work_unit(&work_unit)?;

        let agent_path = repository
            .repository_root
            .join(".codex")
            .join("agents")
            .join(format!("{role_slug}.toml"));
        write_or_replace_managed_file(
            &agent_path,
            &render_agent_file(
                role,
                model.as_deref(),
                reasoning_effort.as_deref(),
                &project.title,
                &project.objective,
                project_config,
                plan,
            ),
        )?;

        let (branch_name, worktree_path) = if create_worktree {
            let branch_name = format!("tt/{role_slug}");
            let worktree_path = worktree_root.join(role_slug);
            ensure_role_worktree(repository, &worktree_path, &branch_name, base_branch)?;
            (Some(branch_name), Some(worktree_path))
        } else {
            (None, None)
        };

        Ok(ManagedProjectRoleBootstrap {
            role,
            work_unit,
            agent_path,
            model,
            reasoning_effort,
            control_mode: ManagedProjectThreadControlMode::Director,
            branch_name,
            worktree_path,
            thread_id: None,
            thread_name: None,
            workspace_binding_id: None,
        })
    }

    pub fn handle_request(&self, request: DaemonRequest) -> Result<DaemonResponse> {
        use DaemonRequest::*;
        Ok(match request {
            Doctor { cwd, check_listen } => DaemonResponse::Doctor(self.doctor(cwd, check_listen)),
            DoctorCodex { cwd, check_listen } => DaemonResponse::CodexDoctor(if check_listen {
                self.codex_doctor_with_listen_check(cwd)
            } else {
                self.codex_doctor(cwd)
            }),
            Status { cwd } => DaemonResponse::Status(self.status(cwd)?),
            DashboardSummary => DaemonResponse::DashboardSummary(self.dashboard_summary()?),
            RepositorySummary { cwd } => {
                DaemonResponse::RepositorySummary(self.repository_summary(cwd)?)
            }
            InspectManagedProject { cwd } => {
                DaemonResponse::ManagedProjectInspection(self.inspect_managed_project(cwd)?)
            }
            InspectManagedProjectPlan { cwd } => {
                DaemonResponse::ManagedProjectPlan(self.inspect_managed_project_plan(cwd)?)
            }
            RefreshManagedProjectPlan { cwd } => {
                DaemonResponse::ManagedProjectPlan(self.refresh_managed_project_plan(cwd)?)
            }
            CleanManagedProject { cwd, force } => {
                DaemonResponse::Count(self.clean_managed_project(cwd, force)?)
            }
            SetManagedProjectThreadControl { cwd, role, mode } => {
                DaemonResponse::ManagedProjectInspection(
                    self.set_managed_project_thread_control(cwd, role, mode)?,
                )
            }
            ListProjects => DaemonResponse::Projects(self.list_projects()?),
            GetProject { id_or_slug } => DaemonResponse::Project(self.get_project(&id_or_slug)?),
            UpsertProject { project } => {
                self.upsert_project(&project)?;
                DaemonResponse::Unit
            }
            SetProjectStatus { id_or_slug, status } => {
                DaemonResponse::Count(self.set_project_status(&id_or_slug, status)?)
            }
            DeleteProject { id_or_slug } => {
                DaemonResponse::Count(self.delete_project(&id_or_slug)?)
            }
            ListWorkUnits { project_id } => {
                DaemonResponse::WorkUnits(self.list_work_units(project_id.as_deref())?)
            }
            GetWorkUnit { id_or_slug } => {
                DaemonResponse::WorkUnit(self.get_work_unit(&id_or_slug)?)
            }
            UpsertWorkUnit { work_unit } => {
                self.upsert_work_unit(&work_unit)?;
                DaemonResponse::Unit
            }
            SetWorkUnitStatus { id_or_slug, status } => {
                DaemonResponse::Count(self.set_work_unit_status(&id_or_slug, status)?)
            }
            DeleteWorkUnit { id_or_slug } => {
                DaemonResponse::Count(self.delete_work_unit(&id_or_slug)?)
            }
            ListThreadBindings => DaemonResponse::ThreadBindings(self.list_thread_bindings()?),
            GetThreadBinding { codex_thread_id } => {
                DaemonResponse::ThreadBinding(self.get_thread_binding(&codex_thread_id)?)
            }
            UpsertThreadBinding { binding } => {
                self.upsert_thread_binding(&binding)?;
                DaemonResponse::Unit
            }
            SetThreadBindingStatus {
                codex_thread_id,
                status,
            } => DaemonResponse::Count(self.set_thread_binding_status(&codex_thread_id, status)?),
            DeleteThreadBinding { codex_thread_id } => {
                DaemonResponse::Count(self.delete_thread_binding(&codex_thread_id)?)
            }
            ListThreadBindingsForWorkUnit { work_unit_id } => DaemonResponse::ThreadBindings(
                self.list_thread_bindings_for_work_unit(&work_unit_id)?,
            ),
            ListWorkspaceBindings => {
                DaemonResponse::WorkspaceBindings(self.list_workspace_bindings()?)
            }
            GetWorkspaceBinding { id } => {
                DaemonResponse::WorkspaceBinding(self.get_workspace_binding(&id)?)
            }
            UpsertWorkspaceBinding { binding } => {
                self.upsert_workspace_binding(&binding)?;
                DaemonResponse::Unit
            }
            SetWorkspaceBindingStatus { id, status } => {
                DaemonResponse::Count(self.set_workspace_binding_status(&id, status)?)
            }
            DeleteWorkspaceBinding { id } => {
                DaemonResponse::Count(self.delete_workspace_binding(&id)?)
            }
            ListWorkspaceBindingsForThread { codex_thread_id } => {
                DaemonResponse::WorkspaceBindings(
                    self.list_workspace_bindings_for_thread(&codex_thread_id)?,
                )
            }
            RefreshWorkspaceBinding { id } => {
                DaemonResponse::WorkspaceBinding(self.refresh_workspace_binding(&id)?)
            }
            PrepareWorkspaceBinding { id } => {
                DaemonResponse::WorkspaceBinding(self.prepare_workspace_binding(&id)?)
            }
            MergePrepWorkspaceBinding { id } => {
                DaemonResponse::MergeRun(self.merge_prep_workspace_binding(&id)?)
            }
            AuthorizeMergeWorkspaceBinding { id } => {
                DaemonResponse::MergeRun(self.authorize_merge_workspace_binding(&id)?)
            }
            ExecuteLandingWorkspaceBinding { id } => {
                DaemonResponse::MergeRun(self.execute_landing_workspace_binding(&id)?)
            }
            PruneWorkspaceBinding { id, force } => {
                DaemonResponse::WorkspaceBinding(self.prune_workspace_binding(&id, force)?)
            }
            CloseWorkspace {
                cwd,
                selector,
                force,
            } => DaemonResponse::WorkspaceBinding(self.close_workspace(
                cwd,
                selector.as_deref(),
                force,
            )?),
            ParkWorkspace {
                cwd,
                selector,
                note,
            } => DaemonResponse::WorkspaceBinding(self.park_workspace(
                cwd,
                selector.as_deref(),
                note,
            )?),
            SplitWorkspace {
                cwd,
                role,
                model,
                ephemeral,
            } => {
                DaemonResponse::WorkspaceBinding(self.split_workspace(cwd, role, model, ephemeral)?)
            }
            ListMergeRuns => DaemonResponse::MergeRuns(self.list_merge_runs()?),
            GetMergeRun { id } => DaemonResponse::MergeRun(self.get_merge_run(&id)?),
            UpsertMergeRun { run } => {
                self.upsert_merge_run(&run)?;
                DaemonResponse::Unit
            }
            RefreshMergeRun {
                workspace_binding_id,
            } => DaemonResponse::MergeRun(self.refresh_merge_run(&workspace_binding_id)?),
            SetMergeRunStatus {
                id,
                readiness,
                authorization,
                execution,
                head_commit,
            } => DaemonResponse::Count(self.set_merge_run_status(
                &id,
                readiness,
                authorization,
                execution,
                head_commit,
            )?),
            DeleteMergeRun { id } => DaemonResponse::Count(self.delete_merge_run(&id)?),
            ListCodexThreads { cwd, limit } => {
                DaemonResponse::CodexThreads(self.codex_threads(cwd, limit)?)
            }
            GetCodexThread { cwd, selector } => {
                DaemonResponse::CodexThread(self.codex_thread(cwd, &selector)?)
            }
            ReadCodexThread {
                cwd,
                selector,
                include_turns,
            } => DaemonResponse::CodexThreadDetail(self.read_codex_thread(
                cwd,
                &selector,
                include_turns,
            )?),
            InspectCodexAppServers { cwd } => {
                DaemonResponse::CodexAppServers(self.inspect_codex_app_servers(cwd)?)
            }
            StartCodexThread {
                cwd,
                model,
                ephemeral,
            } => DaemonResponse::CodexThreadDetail(Some(
                self.start_codex_thread(cwd, model, ephemeral)?,
            )),
            ResumeCodexThread {
                cwd,
                selector,
                model,
            } => {
                DaemonResponse::CodexThreadDetail(self.resume_codex_thread(cwd, &selector, model)?)
            }
            OpenManagedProject {
                cwd,
                title,
                objective,
                base_branch,
                worktree_root,
                director_model,
                dev_model,
                test_model,
                integration_model,
            } => DaemonResponse::ManagedProject(self.open_managed_project(
                cwd,
                title,
                objective,
                base_branch,
                worktree_root,
                director_model,
                dev_model,
                test_model,
                integration_model,
            )?),
            InitManagedProject {
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
            } => DaemonResponse::ManagedProject(self.init_managed_project(
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
            )?),
            DirectManagedProject {
                cwd,
                title,
                objective,
                base_branch,
                worktree_root,
                director_model,
                dev_model,
                test_model,
                integration_model,
                roles,
                bindings,
                scenario,
                seed_file,
            } => DaemonResponse::ManagedProject(self.direct_managed_project(
                cwd,
                title,
                objective,
                base_branch,
                worktree_root,
                director_model,
                dev_model,
                test_model,
                integration_model,
                roles,
                bindings,
                scenario,
                seed_file,
            )?),
            SpawnManagedProject { cwd, roles } => {
                DaemonResponse::ManagedProject(self.spawn_managed_project(cwd, roles)?)
            }
            AttachManagedProject { cwd, bindings } => {
                DaemonResponse::ManagedProject(self.attach_managed_project(cwd, bindings)?)
            }
        })
    }

    pub fn codex_home_root(&self) -> Option<&Path> {
        self.codex_home.as_ref().map(|home| home.root())
    }

    fn codex_runtime_client(&self, cwd: &Path) -> Result<CodexRuntimeClient> {
        CodexRuntimeClient::open(cwd)
    }

    fn enrich_codex_snapshot(
        &self,
        snapshot: CodexThreadRuntimeSnapshot,
    ) -> Result<CodexThreadDetail> {
        let bound_work_unit_id = self
            .get_thread_binding(&snapshot.thread_id)?
            .and_then(|binding| binding.work_unit_id);
        let workspace_binding_count = self
            .list_workspace_bindings_for_thread(&snapshot.thread_id)?
            .len();
        Ok(CodexThreadDetail {
            thread_id: snapshot.thread_id,
            thread_name: snapshot.thread_name,
            preview: snapshot.preview,
            status: snapshot.status,
            cwd: snapshot.cwd,
            model_provider: snapshot.model_provider,
            ephemeral: snapshot.ephemeral,
            updated_at: snapshot.updated_at,
            turn_count: snapshot.turn_count,
            latest_turn_id: snapshot.latest_turn_id,
            latest_turn_status: None,
            latest_turn_error: None,
            latest_turn_summary: None,
            bound_work_unit_id,
            workspace_binding_count,
        })
    }

    fn enrich_codex_thread(&self, thread: protocol::Thread) -> Result<CodexThreadDetail> {
        let bound_work_unit_id = self
            .get_thread_binding(&thread.id)?
            .and_then(|binding| binding.work_unit_id);
        let workspace_binding_count = self.list_workspace_bindings_for_thread(&thread.id)?.len();
        let latest_turn = thread.turns.last();
        let latest_turn_id = latest_turn.map(|turn| turn.id.clone());
        let latest_turn_status = latest_turn.map(|turn| format!("{:?}", turn.status));
        let latest_turn_error = latest_turn.and_then(|turn| {
            turn.error.as_ref().map(|error| {
                let details = error.additional_details.as_deref().unwrap_or("").trim();
                if details.is_empty() {
                    error.message.clone()
                } else {
                    format!("{}\n{}", error.message, details)
                }
            })
        });
        let latest_turn_summary = latest_turn.map(|turn| summarize_turn_items(&turn.items));
        Ok(CodexThreadDetail {
            thread_id: thread.id,
            thread_name: thread.name.or(thread.agent_nickname),
            preview: thread.preview,
            status: format!("{:?}", thread.status),
            cwd: thread.cwd.display().to_string(),
            model_provider: thread.model_provider.to_string(),
            ephemeral: thread.ephemeral,
            updated_at: thread.updated_at,
            turn_count: thread.turns.len(),
            latest_turn_id,
            latest_turn_status,
            latest_turn_error,
            latest_turn_summary,
            bound_work_unit_id,
            workspace_binding_count,
        })
    }
}

fn role_slug(role: ThreadRole) -> &'static str {
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

fn role_title(role: ThreadRole) -> &'static str {
    match role {
        ThreadRole::Director => "Director",
        ThreadRole::Develop => "Developer",
        ThreadRole::Test => "Test",
        ThreadRole::Integrate => "Integration",
        ThreadRole::Review => "Reviewer",
        ThreadRole::Todo => "TODO",
        ThreadRole::Chat => "Chat",
        ThreadRole::Learn => "Research",
        ThreadRole::Handoff => "Handoff",
        ThreadRole::Custom => "Custom",
    }
}

fn role_task(role: ThreadRole, project_title: &str) -> String {
    match role {
        ThreadRole::Director => {
            format!(
                "Coordinate the operator, workers, branch strategy, and handoffs for {project_title}"
            )
        }
        ThreadRole::Develop => {
            format!("Implement the assigned feature slice for {project_title}")
        }
        ThreadRole::Test => format!("Validate the assigned work for {project_title}"),
        ThreadRole::Integrate => {
            format!("Prepare landing and merge readiness for {project_title}")
        }
        ThreadRole::Review => format!("Review the assigned change set for {project_title}"),
        ThreadRole::Todo => format!("Capture and organize TODOs for {project_title}"),
        ThreadRole::Chat => format!("Discuss the current project for {project_title}"),
        ThreadRole::Learn => format!("Research gaps and unknowns for {project_title}"),
        ThreadRole::Handoff => format!("Prepare the handoff package for {project_title}"),
        ThreadRole::Custom => format!("Handle the assigned custom role for {project_title}"),
    }
}

fn role_description(role: ThreadRole) -> &'static str {
    match role {
        ThreadRole::Director => {
            "Coordinates the operator, branch strategy, worker assignments, and handoffs."
        }
        ThreadRole::Develop => "Implementation worker focused on the assigned code slice.",
        ThreadRole::Test => "Validation worker that reports exact failures and test results.",
        ThreadRole::Integrate => "Landing worker that prepares merge readiness and cleanup.",
        ThreadRole::Review => "Review worker focused on correctness and test coverage.",
        ThreadRole::Todo => "Organizer that converts notes into scoped work.",
        ThreadRole::Chat => "Discussion worker for design and planning conversations.",
        ThreadRole::Learn => "Research worker for gaps, unknowns, and evidence gathering.",
        ThreadRole::Handoff => "Package handoff context for the next worker or maintainer.",
        ThreadRole::Custom => "Custom worker role.",
    }
}

fn role_sandbox_mode(role: ThreadRole) -> &'static str {
    match role {
        ThreadRole::Director | ThreadRole::Review | ThreadRole::Learn => "read-only",
        ThreadRole::Test => "danger-full-access",
        ThreadRole::Develop | ThreadRole::Integrate | ThreadRole::Handoff => "danger-full-access",
        ThreadRole::Todo | ThreadRole::Chat | ThreadRole::Custom => "danger-full-access",
    }
}

fn role_default_model(role: ThreadRole) -> &'static str {
    match role {
        ThreadRole::Director => "gpt-5.4",
        ThreadRole::Develop | ThreadRole::Test | ThreadRole::Integrate => "gpt-5.4-mini",
        _ => "gpt-5.4-mini",
    }
}

fn role_default_reasoning_effort(_role: ThreadRole) -> &'static str {
    "medium"
}

fn render_codex_config_defaults(max_threads: usize, max_depth: usize) -> String {
    format!("[agents]\nmax_threads = {max_threads}\nmax_depth = {max_depth}\n")
}

fn render_empty_codex_config_local() -> String {
    "# Machine-local Codex overrides.\n".to_string()
}

fn load_toml_table_or_default(path: &Path) -> Result<toml::Table> {
    if !path.exists() {
        return Ok(toml::Table::new());
    }
    let contents =
        fs::read_to_string(path).with_context(|| format!("read TOML file {}", path.display()))?;
    if contents.trim().is_empty() {
        return Ok(toml::Table::new());
    }
    toml::from_str::<toml::Table>(&contents)
        .with_context(|| format!("parse TOML file {}", path.display()))
}

fn write_toml_table(path: &Path, table: &toml::Table) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(table)
        .with_context(|| format!("serialize TOML file {}", path.display()))?;
    fs::write(path, contents).with_context(|| format!("write TOML file {}", path.display()))?;
    Ok(())
}

fn merge_toml_tables(base: &toml::Table, overlay: &toml::Table) -> toml::Table {
    let mut merged = base.clone();
    for (key, value) in overlay {
        match (merged.get_mut(key), value) {
            (Some(toml::Value::Table(base_table)), toml::Value::Table(overlay_table)) => {
                let nested = merge_toml_tables(base_table, overlay_table);
                *base_table = nested;
            }
            _ => {
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    merged
}

fn layered_codex_config_paths(repo_root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let codex_root = repo_root.join(".codex");
    (
        codex_root.join(CODEX_CONFIG_DEFAULTS_FILE_NAME),
        codex_root.join(CODEX_CONFIG_LOCAL_FILE_NAME),
        codex_root.join(CODEX_CONFIG_FILE_NAME),
    )
}

fn split_legacy_codex_config(table: toml::Table) -> (toml::Table, toml::Table) {
    let mut defaults = toml::Table::new();
    let mut local = toml::Table::new();
    for (key, value) in table {
        match key.as_str() {
            "projects" | "plugins" => {
                local.insert(key, value);
            }
            _ => {
                defaults.insert(key, value);
            }
        }
    }
    (defaults, local)
}

fn ensure_layered_codex_config(repo_root: &Path) -> Result<PathBuf> {
    let (defaults_path, local_path, generated_path) = layered_codex_config_paths(repo_root);
    if let Some(parent) = generated_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if !defaults_path.exists() {
        let legacy_table = load_toml_table_or_default(&generated_path)?;
        let (defaults, local) = if legacy_table.is_empty() {
            (
                load_toml_table_from_string(&render_codex_config_defaults(
                    DEFAULT_AGENT_CONFIG_MAX_THREADS,
                    DEFAULT_AGENT_CONFIG_MAX_DEPTH,
                ))?,
                toml::Table::new(),
            )
        } else {
            split_legacy_codex_config(legacy_table)
        };
        write_toml_table(&defaults_path, &defaults)?;
        if !local_path.exists() {
            if local.is_empty() {
                fs::write(&local_path, render_empty_codex_config_local()).with_context(|| {
                    format!("write local Codex config {}", local_path.display())
                })?;
            } else {
                write_toml_table(&local_path, &local)?;
            }
        }
    }

    if !local_path.exists() {
        fs::write(&local_path, render_empty_codex_config_local())
            .with_context(|| format!("write local Codex config {}", local_path.display()))?;
    }

    reconcile_generated_codex_local_overrides(repo_root)?;
    regenerate_codex_effective_config(repo_root)?;
    Ok(generated_path)
}

fn load_toml_table_from_string(contents: &str) -> Result<toml::Table> {
    toml::from_str::<toml::Table>(contents).context("parse generated TOML contents")
}

fn reconcile_generated_codex_local_overrides(repo_root: &Path) -> Result<()> {
    let (_defaults_path, local_path, generated_path) = layered_codex_config_paths(repo_root);
    let generated = load_toml_table_or_default(&generated_path)?;
    if generated.is_empty() {
        return Ok(());
    }
    let (_defaults, imported_local) = split_legacy_codex_config(generated);
    if imported_local.is_empty() {
        return Ok(());
    }
    let local = load_toml_table_or_default(&local_path)?;
    let merged_local = merge_toml_tables(&local, &imported_local);
    write_toml_table(&local_path, &merged_local)?;
    Ok(())
}

fn regenerate_codex_effective_config(repo_root: &Path) -> Result<()> {
    let (defaults_path, local_path, generated_path) = layered_codex_config_paths(repo_root);
    let defaults = load_toml_table_or_default(&defaults_path)?;
    let local = load_toml_table_or_default(&local_path)?;
    let merged = merge_toml_tables(&defaults, &local);
    write_toml_table(&generated_path, &merged)?;
    Ok(())
}

fn render_default_settings_env() -> String {
    [
        "# Repo-local TT and Codex overrides.",
        "# TT_CODEX_BIN=~/.local/bin/codex",
        "# TT_CODEX_APP_SERVER_BIN=~/.local/bin/codex-app-server",
        "# TT_CODEX_LOGIN_MODE=auto",
        "",
    ]
    .join("\n")
}

fn render_agent_file(
    role: ThreadRole,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    project_title: &str,
    project_objective: &str,
    project_config: &ManagedProjectProjectConfig,
    plan: &ManagedProjectPlan,
) -> String {
    let role_roster = managed_project_role_roster();
    let model = model.unwrap_or_else(|| role_default_model(role));
    let reasoning_effort = reasoning_effort.unwrap_or_else(|| role_default_reasoning_effort(role));
    let mut output = String::new();
    output.push_str(&format!("name = {:?}\n", role_slug(role)));
    output.push_str(&format!("description = {:?}\n", role_description(role)));
    output.push_str(&format!("model = {:?}\n", model));
    output.push_str(&format!(
        "model_reasoning_effort = {:?}\n",
        reasoning_effort
    ));
    output.push_str(&format!("sandbox_mode = {:?}\n", role_sandbox_mode(role)));
    output.push_str("developer_instructions = \"\"\"\n");
    output.push_str(&format!(
        "You are the {} agent for {project_title}.\n",
        role_slug(role)
    ));
    output.push_str(&format!("Project objective: {project_objective}\n"));
    output.push_str(&format!(
        "Project config: plan_first={} commit_policy={} checkpoint_triggers={:?}\n",
        project_config.plan_first, project_config.commit_policy, project_config.checkpoint_triggers
    ));
    output.push_str(&format!(
        "Liveness policy: expected_long_build={} progress_updates_required={} soft_silence_seconds={} hard_ceiling_seconds={}\n",
        project_config.expected_long_build,
        project_config.require_progress_updates,
        project_config.soft_silence_seconds,
        project_config.hard_ceiling_seconds
    ));
    output.push_str(&format!("Plan status: {}\n", plan.status));
    output.push_str("Project protocol:\n");
    output.push_str("- The operator talks to the director.\n");
    output.push_str("- The director is the only coordinator and speaks to the operator on behalf of the project.\n");
    output.push_str(
        "- Workers do not coordinate directly with each other; all assignments and escalations go through the director.\n",
    );
    output.push_str(
        "- Use `.tt/contract.md`, `.tt/project.toml`, and `.tt/plan.toml` as the source of truth.\n",
    );
    output.push_str("Role roster:\n");
    for line in role_roster.lines() {
        output.push_str("- ");
        output.push_str(line);
        output.push('\n');
    }
    output.push_str(
        "Liveness expectations:\n\
- Prefer short progress updates before and after long-running builds, tests, or waits.\n\
- If the repository is known to have slow builds, say so explicitly and mention what signal will show forward motion.\n\
- Keep commits and handoffs frequent enough that quiet periods are meaningful, not accidental.\n\
- If progress is quiet but plausible, the director should classify it as quiet or suspect rather than failing immediately.\n",
    );
    output.push_str(
        "Thread control:\n\
- The operator may temporarily take over the next turn for this thread in Codex TUI.\n\
- If control is marked manual_next_turn or manual, pause automatic dispatch and preserve the live thread for manual continuation.\n\
- When control returns to director, resume the project from the saved round state instead of restarting it.\n",
    );
    output.push_str(
        "Planning expectations:\n\
- Start with the plan before dispatching workers.\n\
- Keep checkpoint commits aligned with the plan's checkpoint triggers.\n\
- Prefer concise milestone updates over broad status prose.\n",
    );
    output.push_str(
        "Startup handshake:\n\
- TT may start this thread headlessly before the operator opens the project.\n\
- When TT sends a startup readiness prompt, treat it as mandatory bootstrap protocol.\n\
- Workers must return a concise readiness report for the director.\n\
- The director must validate the full worker roster and emit the operator-facing readiness acknowledgement.\n",
    );
    output.push_str(managed_project_progress_guidance());
    match role {
        ThreadRole::Director => {
            output.push_str("Your job is to turn operator intent into a plan, todo list, and dispatch decisions.\n");
            output.push_str(
                "Own branch strategy, worker assignment, phase transitions, and readiness.\n",
            );
            output.push_str("Keep the operator informed, request approval for merges or destructive cleanup, summarize outcomes after each phase, and watch for quiet or suspect progress periods.\n");
            output.push_str(
                "If a role is marked manual_next_turn or manual, pause auto-dispatch for that role and let the operator continue the live thread in Codex TUI before resuming director control later.\n",
            );
            output.push_str(
                "Review `.tt/project.toml` and `.tt/plan.toml` before dispatching any worker.\n",
            );
            output.push_str(
                "During startup, wait for `dev`, `test`, and `integration` readiness reports, validate them, and acknowledge when the project is ready for operator handoff.\n",
            );
            output.push_str("Do not implement product code unless explicitly instructed.\n");
        }
        ThreadRole::Develop => {
            output.push_str("Implement only the assigned slice in the provided worktree.\n");
            output.push_str("You report to the director, not to other workers or the operator.\n");
            output.push_str("Treat test as the validator and integration as the landing worker.\n");
            output.push_str("Report changed files, tests run, blockers, next step, and whether you are actively building, testing, or waiting on I/O.\n");
            output.push_str("If TT sends a startup readiness prompt, do not change code; return a precise readiness report for the director.\n");
            output.push_str("If the operator takes over the thread for the next turn, keep the live thread open and wait for director control to be restored before the next autonomous turn.\n");
            output.push_str("Honor checkpoint commits required by the project plan.\n");
        }
        ThreadRole::Test => {
            output.push_str("Validate the assigned changes and report exact failures.\n");
            output.push_str("You report to the director, not to other workers or the operator.\n");
            output.push_str("Assume dev produced the change and integration will handle landing if tests pass.\n");
            output.push_str("Do not widen scope or rewrite implementation code.\n");
            output.push_str("If TT sends a startup readiness prompt, do not change code; return a precise readiness report for the director.\n");
            output.push_str("Include progress checkpoints if tests are long-running or flaky so the director can distinguish quiet from stalled.\n");
            output.push_str("If the operator pauses the thread for manual takeover, stop automatic dispatch and resume only when the director regains control.\n");
            output.push_str("Honor checkpoint commits required by the project plan.\n");
        }
        ThreadRole::Integrate => {
            output
                .push_str("Own merge prep, landing checks, and cleanup for the managed project.\n");
            output.push_str("You report to the director, not to other workers or the operator.\n");
            output.push_str(
                "Assume dev implemented the slice and test validated it before landing.\n",
            );
            output.push_str("Keep the landing path narrow, evidence-driven, and punctuated with short status updates if merge prep takes a while.\n");
            output.push_str("If TT sends a startup readiness prompt, do not change code; return a precise readiness report for the director.\n");
            output.push_str("If a manual takeover is requested for the next turn, pause automatic landing steps until the director is restored.\n");
            output.push_str("Honor checkpoint commits required by the project plan.\n");
        }
        ThreadRole::Review => {
            output.push_str("Review correctness, regressions, and missing tests.\n");
        }
        ThreadRole::Todo => {
            output.push_str("Convert notes into scoped work items and preserve traceability.\n");
        }
        ThreadRole::Chat => {
            output.push_str("Use the conversation to clarify intent, not to change code.\n");
        }
        ThreadRole::Learn => {
            output.push_str("Gather evidence and fill gaps before implementation starts.\n");
        }
        ThreadRole::Handoff => {
            output.push_str("Summarize the state for the next worker or maintainer.\n");
        }
        ThreadRole::Custom => {
            output.push_str("Follow the project contract and stay within the assigned scope.\n");
        }
    }
    output.push_str("\"\"\"\n");
    output
}

fn render_worker_contract(
    repo_root: &Path,
    project_slug: &str,
    base_branch: &str,
    project_config: &ManagedProjectProjectConfig,
) -> String {
    let role_roster = managed_project_role_roster();
    format!(
        "# TT Managed Project Contract\n\n\
Project: `{project_slug}`\n\
Repository root: `{}`\n\
Base branch: `{base_branch}`\n\n\
## Coordination Model\n\
- The operator talks to the director.\n\
- The director plans, dispatches, and arbitrates for the project.\n\
- Workers only communicate with the director.\n\
- Peer-to-peer worker coordination is out of scope.\n\n\
## Startup Handshake\n\
- TT may start role threads before the operator opens the project.\n\
- Workers must answer TT startup readiness prompts with a concise report for the director.\n\
- The director must validate `dev`, `test`, and `integration` before acknowledging operator handoff.\n\
- `tt open` should only attach once the director has acknowledged startup readiness.\n\n\
## Roles\n\
{role_roster}\n\n\
## Project Policy\n\
- Plan-first: `{plan_first}`\n\
- Commit policy: `{commit_policy}`\n\
- Require operator merge approval: `{require_operator_merge_approval}`\n\
- Checkpoint triggers: `{checkpoint_triggers:#?}`\n\
\n\
## Phase Vocabulary\n\
- `plan`: turn operator intent into scope and milestones.\n\
- `todo`: capture actionable items and traceability.\n\
- `dispatch`: assign work to a role and a worktree.\n\
- `develop`: implement the assigned slice.\n\
- `test`: validate the change set.\n\
- `integrate`: prepare merge readiness and landing.\n\
- `docs`: update project documentation and handoff notes.\n\
- `merge`: request approval and land the project.\n\n\
## Handoff Format\n\
- `status`: `blocked`, `needs-review`, or `complete`\n\
- `changed_files`: list of paths\n\
- `tests_run`: list of commands\n\
- `blockers`: list of blockers or `[]`\n\
- `next_step`: the next concrete action\n\n\
## Escalation Rules\n\
- Workers escalate questions and blockers to the director.\n\
- The director escalates merge/landing approval to the operator when needed.\n\
- Workers do not change branch strategy or project topology on their own.\n\n\
## Thread Control\n\
- The operator may temporarily take over a thread for the next turn in Codex TUI.\n\
- `manual_next_turn` pauses automatic dispatch before the next role turn.\n\
- `manual` keeps the thread live but under operator control until the director is restored.\n\
- `director_paused` means the director is not dispatching that thread yet.\n\n\
## Liveness Policy\n\
- Expected long builds: `{expected_long_build}`\n\
- Progress updates required: `{require_progress_updates}`\n\
- Soft silence threshold: `{soft_silence_seconds}` seconds\n\
- Hard ceiling: `{hard_ceiling_seconds}` seconds\n\
\n\
## Rules\n\
- Stay inside the assigned worktree and scope.\n\
- Do not widen scope without director approval.\n\
- Keep evidence in the handoff, not in prose alone.\n",
        repo_root.display(),
        role_roster = role_roster,
        plan_first = project_config.plan_first,
        commit_policy = project_config.commit_policy,
        require_operator_merge_approval = project_config.require_operator_merge_approval,
        checkpoint_triggers = project_config.checkpoint_triggers,
        expected_long_build = project_config.expected_long_build,
        require_progress_updates = project_config.require_progress_updates,
        soft_silence_seconds = project_config.soft_silence_seconds,
        hard_ceiling_seconds = project_config.hard_ceiling_seconds
    )
}

fn build_managed_project_manifest(
    project: &Project,
    repo_root: &Path,
    base_branch: &str,
    worktree_root: &Path,
    project_config_path: &Path,
    plan_path: &Path,
    contract_path: &Path,
    codex_config_path: &Path,
    startup: &ManagedProjectStartupState,
    scenario: Option<ManagedProjectScenarioState>,
    roles: &[&ManagedProjectRoleBootstrap],
) -> Result<ManagedProjectManifest> {
    let mut role_map = BTreeMap::new();
    for role in roles {
        role_map.insert(
            role_slug(role.role).to_string(),
            ManagedProjectManifestRole {
                work_unit_id: role.work_unit.id.clone(),
                agent_path: role.agent_path.display().to_string(),
                model: role.model.clone(),
                reasoning_effort: role.reasoning_effort.clone(),
                control_mode: role.control_mode,
                branch_name: role.branch_name.clone(),
                worktree_path: role
                    .worktree_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                thread_id: role.thread_id.clone(),
                thread_name: role.thread_name.clone(),
                workspace_binding_id: role.workspace_binding_id.clone(),
            },
        );
    }

    Ok(ManagedProjectManifest {
        schema: "tt-managed-project-v1".to_string(),
        project_id: project.id.clone(),
        slug: project.slug.clone(),
        title: project.title.clone(),
        objective: project.objective.clone(),
        repo_root: repo_root.display().to_string(),
        base_branch: base_branch.to_string(),
        worktree_root: worktree_root.display().to_string(),
        project_config_path: project_config_path.display().to_string(),
        plan_path: plan_path.display().to_string(),
        project_config_sha256: file_sha256_hex(project_config_path)?,
        plan_sha256: file_sha256_hex(plan_path)?,
        contract_path: contract_path.display().to_string(),
        codex_config_path: codex_config_path.display().to_string(),
        startup: startup.clone(),
        scenario,
        roles: role_map,
    })
}

fn file_sha256_hex(path: &Path) -> Result<String> {
    let contents =
        fs::read(path).with_context(|| format!("read {} for checksum", path.display()))?;
    let digest = Sha256::digest(contents);
    Ok(format!("{digest:x}"))
}

fn save_managed_project_manifest(path: &Path, manifest: &ManagedProjectManifest) -> Result<()> {
    let contents = toml::to_string_pretty(manifest)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::read_to_string(path) {
        Ok(existing) if existing == contents => Ok(()),
        _ => {
            fs::write(path, contents)?;
            Ok(())
        }
    }
}

fn load_managed_project_manifest(path: &Path) -> Result<ManagedProjectManifest> {
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parse {}", path.display()))
}

fn load_managed_project_seed(path: Option<&Path>) -> Result<ManagedProjectScenarioSeed> {
    if let Some(path) = path {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("read scenario seed {}", path.display()))?;
        return toml::from_str(&contents)
            .with_context(|| format!("parse scenario seed {}", path.display()));
    }
    Ok(ManagedProjectScenarioSeed {
        operator_seed: default_taskflow_operator_seed(),
        landing_approval: Some(
            "Approved. Proceed with final integration and landing if validation is green."
                .to_string(),
        ),
    })
}

fn default_taskflow_operator_seed() -> String {
    "Build a Rust CLI called taskflow.\n\nRequirements:\n- Read a YAML workflow file describing tasks with ids, commands, dependencies, and retry counts.\n- Validate dependency graphs and reject cycles or missing dependencies.\n- Execute tasks in topological order.\n- Retry failed tasks up to the configured retry count.\n- Write a JSON report summarizing task results, retries, execution order, and overall outcome.\n- Expose:\n  - taskflow validate <workflow.yml>\n  - taskflow run <workflow.yml> --report <report.json>\n- Include unit tests, integration tests, example workflows, and a README.\n".to_string()
}

fn default_managed_project_startup_state() -> ManagedProjectStartupState {
    ManagedProjectStartupState {
        phase: ManagedProjectStartupPhase::Scaffolded,
        updated_at: Utc::now(),
        worker_reports: default_managed_project_worker_report_state(),
        director_ack: None,
    }
}

fn default_managed_project_worker_report_state() -> BTreeMap<String, ManagedProjectStartupRoleState>
{
    [ThreadRole::Develop, ThreadRole::Test, ThreadRole::Integrate]
        .into_iter()
        .map(|role| {
            (
                role_slug(role).to_string(),
                ManagedProjectStartupRoleState {
                    status: ManagedProjectStartupRoleStatus::NotStarted,
                    updated_at: Utc::now(),
                    turn_id: None,
                    summary: None,
                },
            )
        })
        .collect()
}

fn managed_project_roles_in_order(
    mut roles: Vec<ManagedProjectRoleBootstrap>,
) -> Vec<ManagedProjectRoleBootstrap> {
    roles.sort_by_key(|role| role_order_index(role.role));
    roles
}

fn role_order_index(role: ThreadRole) -> usize {
    match role {
        ThreadRole::Director => 0,
        ThreadRole::Develop => 1,
        ThreadRole::Test => 2,
        ThreadRole::Integrate => 3,
        ThreadRole::Review => 4,
        ThreadRole::Todo => 5,
        ThreadRole::Chat => 6,
        ThreadRole::Learn => 7,
        ThreadRole::Handoff => 8,
        ThreadRole::Custom => 9,
    }
}

fn default_managed_project_roles() -> Vec<ThreadRole> {
    vec![
        ThreadRole::Director,
        ThreadRole::Develop,
        ThreadRole::Test,
        ThreadRole::Integrate,
    ]
}

fn managed_project_role_roster() -> String {
    [
        "director: coordinates the operator, plans the project, dispatches work, and owns handoffs.",
        "dev: implements the assigned code slice only and reports concrete changes.",
        "test: validates the assigned changes and reports exact failures.",
        "integration: prepares landing, merge readiness, and cleanup.",
    ]
    .join("\n")
}

fn managed_project_progress_guidance() -> &'static str {
    "Progress guidance:\n\
- Report progress before and after long-running build, test, or I/O-heavy operations.\n\
- If a task may take a while, mention what is still running and what signal will show completion.\n\
- Prefer concise status updates at phase boundaries so the director can distinguish progress from silence.\n\
- Keep stable milestone commits frequent enough that the director can distinguish live work from a stall.\n\
- The director should summarize quiet periods, classify them as healthy/quiet/suspect, and only escalate when the watchdog reaches its hard ceiling.\n"
}

fn managed_project_liveness_policy_from_env() -> ManagedProjectLivenessPolicy {
    fn parse_bool_env(key: &str, default: bool) -> bool {
        repo_env_var(key)
            .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                _ => None,
            })
            .unwrap_or(default)
    }

    fn parse_u64_env(key: &str, default: u64) -> u64 {
        repo_env_var(key)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(default)
    }

    ManagedProjectLivenessPolicy {
        expected_long_build: parse_bool_env("TT_MANAGED_PROJECT_EXPECTED_LONG_BUILD", false),
        require_progress_updates: parse_bool_env(
            "TT_MANAGED_PROJECT_REQUIRES_PROGRESS_UPDATES",
            true,
        ),
        soft_silence_seconds: parse_u64_env("TT_MANAGED_PROJECT_SOFT_SILENCE_SECONDS", 900),
        hard_ceiling_seconds: parse_u64_env("TT_MANAGED_PROJECT_HARD_CEILING_SECONDS", 7_200),
    }
}

fn default_managed_project_project_config(
    repo_root: &Path,
    project_title: &str,
    project_objective: &str,
    base_branch: &str,
) -> ManagedProjectProjectConfig {
    let liveness = managed_project_liveness_policy_from_env();
    let tt_runtime_bin = repo_env_var("TT_RUNTIME_BIN").or_else(|| {
        repo_root
            .join("target")
            .join("debug")
            .join("tt-cli")
            .exists()
            .then_some("./target/debug/tt-cli".to_string())
    });
    ManagedProjectProjectConfig {
        schema: "tt-managed-project-config-v1".to_string(),
        title: project_title.to_string(),
        objective: project_objective.to_string(),
        base_branch: base_branch.to_string(),
        branch_prefix: "tt".to_string(),
        tt_runtime_bin,
        plan_first: true,
        commit_policy: "checkpoint-enforced".to_string(),
        require_operator_merge_approval: true,
        expected_long_build: liveness.expected_long_build,
        require_progress_updates: liveness.require_progress_updates,
        soft_silence_seconds: liveness.soft_silence_seconds,
        hard_ceiling_seconds: liveness.hard_ceiling_seconds,
        default_validation_commands: vec!["cargo test".to_string()],
        smoke_validation_commands: vec!["cargo check".to_string()],
        checkpoint_triggers: vec![
            "after_plan".to_string(),
            "after_develop".to_string(),
            "after_test".to_string(),
            "before_merge".to_string(),
        ],
        pitfalls: Vec::new(),
        hints: Vec::new(),
        exceptions: Vec::new(),
    }
}

fn load_managed_project_project_config(path: &Path) -> Result<ManagedProjectProjectConfig> {
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parse {}", path.display()))
}

fn save_managed_project_project_config(
    path: &Path,
    config: &ManagedProjectProjectConfig,
) -> Result<()> {
    let contents = toml::to_string_pretty(config)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::read_to_string(path) {
        Ok(existing) if existing == contents => Ok(()),
        _ => {
            fs::write(path, contents)?;
            Ok(())
        }
    }
}

fn load_or_seed_managed_project_config(
    path: &Path,
    repo_root: &Path,
    title: &str,
    objective: &str,
    base_branch: &str,
) -> Result<ManagedProjectProjectConfig> {
    tt_codex::load_repo_settings_env(repo_root)?;
    if path.exists() {
        return load_managed_project_project_config(path);
    }
    let config = default_managed_project_project_config(repo_root, title, objective, base_branch);
    save_managed_project_project_config(path, &config)?;
    Ok(config)
}

fn default_managed_project_plan(
    project: &Project,
    config: &ManagedProjectProjectConfig,
    roles: &[&ManagedProjectRoleBootstrap],
) -> ManagedProjectPlan {
    let milestone_titles = [
        "Plan and scope the work",
        "Implement and validate the change set",
        "Prepare merge readiness",
    ];
    let milestones = milestone_titles
        .iter()
        .enumerate()
        .map(|(index, title)| ManagedProjectPlanMilestone {
            id: format!("milestone-{}", index + 1),
            title: (*title).to_string(),
            success_criteria: vec![
                "The director has an explicit plan".to_string(),
                "Workers have clear ownership".to_string(),
            ],
            evidence: Vec::new(),
        })
        .collect();
    let work_items = roles
        .iter()
        .map(|role| ManagedProjectPlanWorkItem {
            id: format!("{}-{}", project.slug, role_slug(role.role)),
            title: role_title(role.role).to_string(),
            owner_role: role_slug(role.role).to_string(),
            phase: match role.role {
                ThreadRole::Director => "plan",
                ThreadRole::Develop => "develop",
                ThreadRole::Test => "test",
                ThreadRole::Integrate => "integrate",
                _ => "plan",
            }
            .to_string(),
            depends_on: match role.role {
                ThreadRole::Director => Vec::new(),
                ThreadRole::Develop => vec![format!("{}-director", project.slug)],
                ThreadRole::Test => vec![format!("{}-dev", project.slug)],
                ThreadRole::Integrate => vec![format!("{}-test", project.slug)],
                _ => Vec::new(),
            },
            acceptance_criteria: vec![
                "The handoff is explicit".to_string(),
                "Validation evidence is recorded".to_string(),
            ],
            validation_commands: config.default_validation_commands.clone(),
            commit_required: !matches!(role.role, ThreadRole::Director),
            status: "planned".to_string(),
        })
        .collect();
    let mut open_questions = vec![
        format!(
            "What exact scope and non-goals should the director enforce for `{}`?",
            project.slug
        ),
        "Which validation commands are required before the director can mark the plan ready to dispatch?"
            .to_string(),
        "What repo-specific pitfalls or exceptions should the director carry forward during execution?"
            .to_string(),
    ];
    if config.require_operator_merge_approval {
        open_questions.push(
            "What operator approval or landing gate must be satisfied before merge?".to_string(),
        );
    }
    ManagedProjectPlan {
        schema: "tt-managed-project-plan-v1".to_string(),
        status: "draft".to_string(),
        objective: project.objective.clone(),
        updated_at: Utc::now().to_rfc3339(),
        milestones,
        work_items,
        notes: ManagedProjectPlanNotes {
            risks: Vec::new(),
            pitfalls: config.pitfalls.clone(),
            open_questions,
            operator_constraints: config.exceptions.clone(),
        },
    }
}

fn load_managed_project_plan(path: &Path) -> Result<ManagedProjectPlan> {
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parse {}", path.display()))
}

fn save_managed_project_plan(path: &Path, plan: &ManagedProjectPlan) -> Result<()> {
    let contents = toml::to_string_pretty(plan)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::read_to_string(path) {
        Ok(existing) if existing == contents => Ok(()),
        _ => {
            fs::write(path, contents)?;
            Ok(())
        }
    }
}

fn load_or_seed_managed_project_plan(
    path: &Path,
    config: &ManagedProjectProjectConfig,
    project: &Project,
    roles: &[&ManagedProjectRoleBootstrap],
) -> Result<ManagedProjectPlan> {
    if path.exists() {
        return load_managed_project_plan(path);
    }
    let plan = default_managed_project_plan(project, config, roles);
    save_managed_project_plan(path, &plan)?;
    Ok(plan)
}

fn managed_project_thread_binding_id(project_slug: &str, role: ThreadRole) -> String {
    format!("{project_slug}:{}", role_slug(role))
}

fn managed_project_workspace_strategy(role: ThreadRole) -> WorkspaceStrategy {
    match role {
        ThreadRole::Director
        | ThreadRole::Review
        | ThreadRole::Todo
        | ThreadRole::Chat
        | ThreadRole::Learn
        | ThreadRole::Handoff
        | ThreadRole::Custom => WorkspaceStrategy::Shared,
        ThreadRole::Develop | ThreadRole::Test | ThreadRole::Integrate => {
            WorkspaceStrategy::DedicatedWorktree
        }
    }
}

fn managed_project_workspace_sync_policy(role: ThreadRole) -> WorkspaceSyncPolicy {
    match role {
        ThreadRole::Director
        | ThreadRole::Review
        | ThreadRole::Todo
        | ThreadRole::Chat
        | ThreadRole::Learn
        | ThreadRole::Handoff
        | ThreadRole::Custom => WorkspaceSyncPolicy::Manual,
        ThreadRole::Develop => WorkspaceSyncPolicy::RebaseBeforeReview,
        ThreadRole::Test | ThreadRole::Integrate => WorkspaceSyncPolicy::RebaseBeforeLanding,
    }
}

fn managed_project_workspace_cleanup_policy(role: ThreadRole) -> WorkspaceCleanupPolicy {
    match role {
        ThreadRole::Director
        | ThreadRole::Review
        | ThreadRole::Todo
        | ThreadRole::Chat
        | ThreadRole::Learn
        | ThreadRole::Handoff
        | ThreadRole::Custom => WorkspaceCleanupPolicy::KeepForAudit,
        ThreadRole::Develop | ThreadRole::Test | ThreadRole::Integrate => {
            WorkspaceCleanupPolicy::PruneAfterLanding
        }
    }
}

fn parse_managed_project_sandbox_mode(raw: &str) -> Result<protocol::SandboxMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "read-only" | "readonly" => Ok(protocol::SandboxMode::ReadOnly),
        "workspace-write" | "workspace_write" | "workspacewrite" => {
            Ok(protocol::SandboxMode::WorkspaceWrite)
        }
        "danger-full-access" | "danger_full_access" | "dangerfullaccess" => {
            Ok(protocol::SandboxMode::DangerFullAccess)
        }
        other => anyhow::bail!("unknown sandbox mode `{other}`"),
    }
}

fn parse_managed_project_reasoning_effort(raw: &str) -> Result<ReasoningEffort> {
    raw.trim()
        .parse::<ReasoningEffort>()
        .map_err(|error| anyhow::anyhow!("unknown reasoning effort `{}`: {}", raw.trim(), error))
}

fn load_managed_agent_file(path: &Path) -> Result<ManagedAgentFile> {
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parse {}", path.display()))
}

fn ordered_managed_project_roles(
    roles: impl IntoIterator<Item = ManagedProjectRoleBootstrap>,
) -> Vec<ManagedProjectRoleBootstrap> {
    managed_project_roles_in_order(roles.into_iter().collect())
}

impl DaemonService {
    pub fn init_managed_project(
        &self,
        path: impl AsRef<Path>,
        title: Option<String>,
        objective: Option<String>,
        template: Option<String>,
        base_branch: Option<String>,
        worktree_root: Option<PathBuf>,
        director_model: Option<String>,
        dev_model: Option<String>,
        test_model: Option<String>,
        integration_model: Option<String>,
    ) -> Result<ManagedProjectBootstrap> {
        let path = path.as_ref();
        fs::create_dir_all(path)?;
        initialize_git_repository(path, base_branch.as_deref())?;
        if template.is_some() || repo_root_has_no_non_git_entries(path)? {
            scaffold_managed_project_template(path, template.as_deref())?;
        }
        self.open_managed_project(
            path,
            title,
            objective,
            base_branch,
            worktree_root,
            director_model,
            dev_model,
            test_model,
            integration_model,
        )
    }

    fn managed_project_bootstrap_from_manifest(
        &self,
        manifest_path: &Path,
        manifest: &ManagedProjectManifest,
    ) -> Result<ManagedProjectBootstrap> {
        let project = self
            .get_project(&manifest.project_id)?
            .ok_or_else(|| anyhow::anyhow!("managed project {} not found", manifest.project_id))?;
        let mut roles = Vec::with_capacity(manifest.roles.len());
        for (role_name, role_manifest) in &manifest.roles {
            let role = ThreadRole::from_str(role_name)
                .map_err(|error| anyhow::anyhow!(error))
                .with_context(|| format!("parse managed project role `{role_name}`"))?;
            roles.push(ManagedProjectRoleBootstrap {
                role,
                work_unit: self
                    .get_work_unit(&role_manifest.work_unit_id)?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "managed project work unit {} not found",
                            role_manifest.work_unit_id
                        )
                    })?,
                agent_path: PathBuf::from(&role_manifest.agent_path),
                model: Some(
                    role_manifest
                        .model
                        .clone()
                        .unwrap_or_else(|| role_default_model(role).to_string()),
                ),
                reasoning_effort: Some(
                    role_manifest
                        .reasoning_effort
                        .clone()
                        .unwrap_or_else(|| role_default_reasoning_effort(role).to_string()),
                ),
                branch_name: role_manifest.branch_name.clone(),
                worktree_path: role_manifest.worktree_path.as_ref().map(PathBuf::from),
                thread_id: role_manifest.thread_id.clone(),
                thread_name: role_manifest.thread_name.clone(),
                workspace_binding_id: role_manifest.workspace_binding_id.clone(),
                control_mode: role_manifest.control_mode,
            });
        }

        let project_config_path = if manifest.project_config_path.is_empty() {
            PathBuf::from(&manifest.repo_root)
                .join(".tt")
                .join("project.toml")
        } else {
            PathBuf::from(&manifest.project_config_path)
        };
        let plan_path = if manifest.plan_path.is_empty() {
            PathBuf::from(&manifest.repo_root)
                .join(".tt")
                .join("plan.toml")
        } else {
            PathBuf::from(&manifest.plan_path)
        };
        let project_config = load_or_seed_managed_project_config(
            &project_config_path,
            Path::new(&manifest.repo_root),
            &project.title,
            &project.objective,
            &manifest.base_branch,
        )?;
        let role_refs: Vec<_> = roles.iter().collect();
        let plan =
            load_or_seed_managed_project_plan(&plan_path, &project_config, &project, &role_refs)?;
        let codex_config_path = ensure_layered_codex_config(Path::new(&manifest.repo_root))?;

        Ok(ManagedProjectBootstrap {
            project: project.clone(),
            repo_root: PathBuf::from(&manifest.repo_root),
            base_branch: manifest.base_branch.clone(),
            worktree_root: PathBuf::from(&manifest.worktree_root),
            manifest_path: manifest_path.to_path_buf(),
            project_config_path,
            plan_path,
            contract_path: PathBuf::from(&manifest.contract_path),
            codex_config_path,
            project_config,
            plan,
            startup: manifest.startup.clone(),
            scenario: manifest.scenario.clone(),
            roles: ordered_managed_project_roles(roles),
        })
    }

    fn spawn_managed_project(
        &self,
        cwd: impl AsRef<Path>,
        roles: Option<Vec<ThreadRole>>,
    ) -> Result<ManagedProjectBootstrap> {
        let cwd = cwd.as_ref();
        let Some(repo_root) = managed_project_repo_root(cwd)? else {
            anyhow::bail!("managed project spawn requires a git repository");
        };
        let repository = GitRepository::discover(&repo_root)?
            .ok_or_else(|| anyhow::anyhow!("managed project spawn requires a git repository"))?;
        let manifest_path = require_initialized_managed_project(&repo_root)?;
        let manifest = load_managed_project_manifest(&manifest_path)?;
        let mut bootstrap =
            self.managed_project_bootstrap_from_manifest(&manifest_path, &manifest)?;
        let selected_roles = roles.unwrap_or_else(default_managed_project_roles);

        for role in selected_roles {
            self.spawn_managed_project_role(&repository, &mut bootstrap, role)?;
        }

        let role_refs: Vec<_> = bootstrap.roles.iter().collect();
        let manifest = build_managed_project_manifest(
            &bootstrap.project,
            &bootstrap.repo_root,
            &bootstrap.base_branch,
            &bootstrap.worktree_root,
            &bootstrap.project_config_path,
            &bootstrap.plan_path,
            &bootstrap.contract_path,
            &bootstrap.codex_config_path,
            &bootstrap.startup,
            bootstrap.scenario.clone(),
            &role_refs,
        )?;
        save_managed_project_manifest(&bootstrap.manifest_path, &manifest)?;
        Ok(bootstrap)
    }

    fn direct_managed_project(
        &self,
        cwd: impl AsRef<Path>,
        _title: Option<String>,
        _objective: Option<String>,
        _base_branch: Option<String>,
        _worktree_root: Option<PathBuf>,
        _director_model: Option<String>,
        _dev_model: Option<String>,
        _test_model: Option<String>,
        _integration_model: Option<String>,
        roles: Option<Vec<ThreadRole>>,
        bindings: Vec<ManagedProjectThreadAttachment>,
        scenario: Option<String>,
        seed_file: Option<PathBuf>,
    ) -> Result<ManagedProjectBootstrap> {
        let cwd = cwd.as_ref();
        let Some(repo_root) = managed_project_repo_root(cwd)? else {
            anyhow::bail!("managed project director requires a git repository");
        };
        let repository = GitRepository::discover(&repo_root)?
            .ok_or_else(|| anyhow::anyhow!("managed project director requires a git repository"))?;
        let manifest_path = require_initialized_managed_project(&repo_root)?;
        let manifest = load_managed_project_manifest(&manifest_path)?;
        let mut bootstrap =
            self.managed_project_bootstrap_from_manifest(&manifest_path, &manifest)?;
        self.save_managed_project_bootstrap(&bootstrap)?;
        if !bindings.is_empty() || roles.is_some() {
            let selected_roles = roles.unwrap_or_else(default_managed_project_roles);
            let mut binding_map = BTreeMap::new();
            for binding in bindings {
                let role_key = role_slug(binding.role).to_string();
                if binding_map
                    .insert(role_key.clone(), binding.thread_id.clone())
                    .is_some()
                {
                    anyhow::bail!(
                        "managed project role `{}` was specified more than once",
                        role_key
                    );
                }
            }

            for role in selected_roles {
                let role_index = bootstrap
                    .roles
                    .iter()
                    .position(|candidate| candidate.role == role)
                    .ok_or_else(|| {
                        anyhow::anyhow!("managed project role `{}` not found", role_slug(role))
                    })?;
                let existing_thread_id = bootstrap.roles[role_index].thread_id.clone();
                if let Some(existing_thread_id) = existing_thread_id.as_ref() {
                    if let Some(requested_thread_id) = binding_map.get(role_slug(role))
                        && requested_thread_id != existing_thread_id
                    {
                        anyhow::bail!(
                            "managed project role `{}` is already bound to thread `{}`",
                            role_slug(role),
                            existing_thread_id
                        );
                    }
                    continue;
                }

                if let Some(thread_id) = binding_map.get(role_slug(role)).cloned() {
                    self.attach_managed_project_role(
                        &repository,
                        &mut bootstrap,
                        ManagedProjectThreadAttachment {
                            role,
                            thread_id: thread_id.clone(),
                        },
                    )?;
                } else {
                    self.spawn_managed_project_role(&repository, &mut bootstrap, role)?;
                }
            }

            self.save_managed_project_bootstrap(&bootstrap)?;
        }
        if bootstrap.startup.phase != ManagedProjectStartupPhase::Ready {
            anyhow::bail!(
                "managed project startup is not ready yet (phase={:?}); run `tt status` and wait for director=Ready",
                bootstrap.startup.phase
            );
        }
        if let Some(scenario_kind) = scenario.as_deref() {
            let seed = load_managed_project_seed(seed_file.as_deref())?;
            let scenario_state =
                self.run_managed_project_scenario(&mut bootstrap, scenario_kind, &seed)?;
            bootstrap.scenario = Some(scenario_state);
        }
        self.save_managed_project_bootstrap(&bootstrap)?;
        Ok(bootstrap)
    }

    fn attach_managed_project(
        &self,
        cwd: impl AsRef<Path>,
        bindings: Vec<ManagedProjectThreadAttachment>,
    ) -> Result<ManagedProjectBootstrap> {
        let cwd = cwd.as_ref();
        let Some(repo_root) = managed_project_repo_root(cwd)? else {
            anyhow::bail!("managed project attach requires a git repository");
        };
        let repository = GitRepository::discover(&repo_root)?
            .ok_or_else(|| anyhow::anyhow!("managed project attach requires a git repository"))?;
        let manifest_path = require_initialized_managed_project(&repo_root)?;
        let manifest = load_managed_project_manifest(&manifest_path)?;
        let mut bootstrap =
            self.managed_project_bootstrap_from_manifest(&manifest_path, &manifest)?;

        for attachment in bindings {
            self.attach_managed_project_role(&repository, &mut bootstrap, attachment)?;
        }

        let role_refs: Vec<_> = bootstrap.roles.iter().collect();
        let manifest = build_managed_project_manifest(
            &bootstrap.project,
            &bootstrap.repo_root,
            &bootstrap.base_branch,
            &bootstrap.worktree_root,
            &bootstrap.project_config_path,
            &bootstrap.plan_path,
            &bootstrap.contract_path,
            &bootstrap.codex_config_path,
            &bootstrap.startup,
            bootstrap.scenario.clone(),
            &role_refs,
        )?;
        save_managed_project_manifest(&bootstrap.manifest_path, &manifest)?;
        Ok(bootstrap)
    }

    fn run_managed_project_scenario(
        &self,
        bootstrap: &mut ManagedProjectBootstrap,
        scenario_kind: &str,
        seed: &ManagedProjectScenarioSeed,
    ) -> Result<ManagedProjectScenarioState> {
        if !matches!(
            scenario_kind,
            "rust-taskflow-four-round" | "rust-taskflow-integration-pressure"
        ) {
            anyhow::bail!("unsupported managed project scenario `{scenario_kind}`");
        }

        let director = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Director)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("managed project director role missing"))?;
        let director_thread_id = director
            .thread_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("managed project director thread is not attached"))?;

        let dev = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Develop)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("managed project dev role missing"))?;
        let test = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Test)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("managed project test role missing"))?;
        let integration = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Integrate)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("managed project integration role missing"))?;

        let resumed_scenario = bootstrap
            .scenario
            .clone()
            .filter(|scenario| scenario.scenario_kind == scenario_kind && !scenario.completed);
        let resumed = resumed_scenario.is_some();
        let mut state = resumed_scenario.unwrap_or_else(|| ManagedProjectScenarioState {
            scenario_id: uuid::Uuid::now_v7().to_string(),
            scenario_kind: scenario_kind.to_string(),
            operator_seed: seed.operator_seed.clone(),
            current_round: 0,
            current_phase: "plan".to_string(),
            liveness_policy: managed_project_liveness_policy_from_env(),
            watchdog: Some(ManagedProjectWatchdogState {
                state: "healthy".to_string(),
                last_signal: Some("scenario initialized".to_string()),
                last_observed_at: Some(Utc::now()),
                last_progress_at: Some(Utc::now()),
                role: None,
                round: None,
                turn_id: None,
                elapsed_seconds: 0,
                silence_seconds: 0,
                turn_status: None,
                turn_items: 0,
                app_server_log_modified_at: None,
                app_server_log_size: None,
                note: Some("waiting for the first director turn".to_string()),
            }),
            pending_approval: None,
            rounds: Vec::new(),
            completed: false,
        });
        let scenario_event = if resumed {
            "scenario-resume"
        } else {
            "scenario-start"
        };
        write_scenario_progress_event(
            bootstrap,
            &state.scenario_id,
            &ManagedProjectProgressEvent {
                event: scenario_event.to_string(),
                scenario_id: state.scenario_id.clone(),
                scenario_kind: scenario_kind.to_string(),
                phase: state.current_phase.clone(),
                round: state.current_round,
                role: Some("director".to_string()),
                thread_id: Some(director_thread_id.to_string()),
                turn_id: None,
                state: state
                    .watchdog
                    .as_ref()
                    .map(|watchdog| watchdog.state.clone()),
                signal: state
                    .watchdog
                    .as_ref()
                    .and_then(|watchdog| watchdog.last_signal.clone()),
                message: format!(
                    "director thread {director_thread_id} {} managed project with roles director/dev/test/integration",
                    if resumed { "resuming" } else { "starting" }
                ),
                timestamp: Utc::now(),
            },
        )?;

        let round_specs = taskflow_round_specs(scenario_kind);
        let mut worker_context = String::new();

        for spec in &round_specs {
            state.current_round = spec.round_number;
            state.current_phase = spec.phase.to_string();
            eprintln!(
                "tt director scenario {} round {} phase {} starting",
                scenario_kind, spec.round_number, spec.phase
            );
            write_scenario_progress_event(
                bootstrap,
                &state.scenario_id,
                &ManagedProjectProgressEvent {
                    event: "round-start".to_string(),
                    scenario_id: state.scenario_id.clone(),
                    scenario_kind: scenario_kind.to_string(),
                    phase: spec.phase.to_string(),
                    round: spec.round_number,
                    role: Some("director".to_string()),
                    thread_id: Some(director_thread_id.to_string()),
                    turn_id: None,
                    state: state
                        .watchdog
                        .as_ref()
                        .map(|watchdog| watchdog.state.clone()),
                    signal: state
                        .watchdog
                        .as_ref()
                        .and_then(|watchdog| watchdog.last_signal.clone()),
                    message: format!(
                        "director round {} phase {} planning and dispatch",
                        spec.round_number, spec.phase
                    ),
                    timestamp: Utc::now(),
                },
            )?;

            let round_position = state
                .rounds
                .iter()
                .position(|round| round.round_number == spec.round_number);
            let mut round = round_position
                .and_then(|index| state.rounds.get(index).cloned())
                .unwrap_or_else(|| ManagedProjectRoundState {
                    round_number: spec.round_number,
                    phase: spec.phase.to_string(),
                    director_turn_id: None,
                    director_summary: None,
                    role_handoffs: BTreeMap::new(),
                });

            let director_turn = if round.director_turn_id.is_some() {
                ManagedProjectTurnOutcome {
                    turn_id: round.director_turn_id.clone().unwrap(),
                    summary: round.director_summary.clone().unwrap_or_default(),
                    extraction: WorkerHandoffExtraction {
                        handoff: None,
                        raw_text: None,
                        parse_error: None,
                        source: WorkerHandoffSource::SeededFallback,
                    },
                    attempts: Vec::new(),
                    watchdog: state.watchdog.clone(),
                }
            } else {
                let director_prompt = build_director_round_prompt(
                    bootstrap,
                    spec,
                    &state.operator_seed,
                    &worker_context,
                    state.pending_approval.as_ref(),
                );
                write_scenario_artifact(
                    bootstrap,
                    &state.scenario_id,
                    spec.round_number,
                    "director-prompt.txt",
                    &director_prompt,
                )?;
                let director_turn = self.run_role_prompt(
                    bootstrap,
                    &state.scenario_id,
                    spec.round_number,
                    &director,
                    director_thread_id,
                    &director_prompt,
                )?;
                eprintln!(
                    "tt director scenario {} round {} director completed turn {}",
                    scenario_kind, spec.round_number, director_turn.turn_id
                );
                write_scenario_progress_event(
                    bootstrap,
                    &state.scenario_id,
                    &ManagedProjectProgressEvent {
                        event: "director-turn-complete".to_string(),
                        scenario_id: state.scenario_id.clone(),
                        scenario_kind: scenario_kind.to_string(),
                        phase: spec.phase.to_string(),
                        round: spec.round_number,
                        role: Some("director".to_string()),
                        thread_id: Some(director_thread_id.to_string()),
                        turn_id: Some(director_turn.turn_id.clone()),
                        state: state
                            .watchdog
                            .as_ref()
                            .map(|watchdog| watchdog.state.clone()),
                        signal: state
                            .watchdog
                            .as_ref()
                            .and_then(|watchdog| watchdog.last_signal.clone()),
                        message: format!(
                            "director completed turn {} with summary {}",
                            director_turn.turn_id,
                            director_turn.summary.lines().next().unwrap_or("<empty>")
                        ),
                        timestamp: Utc::now(),
                    },
                )?;
                round.director_turn_id = Some(director_turn.turn_id.clone());
                round.director_summary = Some(director_turn.summary.clone());
                state.watchdog = director_turn.watchdog.clone();
                director_turn
            };

            if spec.requires_landing_approval {
                state.pending_approval = Some(ManagedProjectApprovalState {
                    approval_kind: "landing".to_string(),
                    requested_by_role: "director".to_string(),
                    prompt: director_turn.summary.clone(),
                    approved: true,
                    response: seed.landing_approval.clone(),
                });
            }

            for role in [&dev, &test, &integration] {
                let role_name = role_slug(role.role).to_string();
                if round.role_handoffs.contains_key(&role_name) {
                    continue;
                }
                let role_index = bootstrap
                    .roles
                    .iter()
                    .position(|candidate| candidate.role == role.role)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "managed project role `{}` missing during scenario run",
                            role_slug(role.role)
                        )
                    })?;
                let control_mode = bootstrap.roles[role_index].control_mode;
                match control_mode {
                    ManagedProjectThreadControlMode::Director => {}
                    ManagedProjectThreadControlMode::ManualNextTurn => {
                        bootstrap.roles[role_index].control_mode =
                            ManagedProjectThreadControlMode::Manual;
                        state.current_phase = format!("manual-override-{}", role_name);
                        state.watchdog = Some(ManagedProjectWatchdogState {
                            state: "quiet".to_string(),
                            last_signal: Some(format!(
                                "manual takeover requested before {} turn",
                                role_name
                            )),
                            last_observed_at: Some(Utc::now()),
                            last_progress_at: Some(Utc::now()),
                            role: Some(role_name.clone()),
                            round: Some(spec.round_number),
                            turn_id: None,
                            elapsed_seconds: 0,
                            silence_seconds: 0,
                            turn_status: None,
                            turn_items: 0,
                            app_server_log_modified_at: None,
                            app_server_log_size: None,
                            note: Some(format!(
                                "manual takeover requested before {} turn; resume director control later",
                                role_name
                            )),
                        });
                        round.director_summary = Some(format!(
                            "paused before {} turn for manual takeover",
                            role_name
                        ));
                        write_scenario_artifact(
                            bootstrap,
                            &state.scenario_id,
                            spec.round_number,
                            &format!("{}-control.txt", role_name),
                            &format!(
                                "mode: {}\nthread: {}\nstatus: paused before next turn for manual takeover\n",
                                control_mode,
                                role.thread_id.as_deref().unwrap_or("<none>")
                            ),
                        )?;
                        write_scenario_progress_event(
                            bootstrap,
                            &state.scenario_id,
                            &ManagedProjectProgressEvent {
                                event: "manual-takeover-pending".to_string(),
                                scenario_id: state.scenario_id.clone(),
                                scenario_kind: scenario_kind.to_string(),
                                phase: spec.phase.to_string(),
                                round: spec.round_number,
                                role: Some(role_name.clone()),
                                thread_id: role.thread_id.clone(),
                                turn_id: None,
                                state: state
                                    .watchdog
                                    .as_ref()
                                    .map(|watchdog| watchdog.state.clone()),
                                signal: state
                                    .watchdog
                                    .as_ref()
                                    .and_then(|watchdog| watchdog.last_signal.clone()),
                                message: format!(
                                    "{} control mode set to manual_next_turn; pause before turn",
                                    role_name
                                ),
                                timestamp: Utc::now(),
                            },
                        )?;
                        state
                            .rounds
                            .retain(|existing| existing.round_number != round.round_number);
                        state.rounds.push(round.clone());
                        bootstrap.scenario = Some(state.clone());
                        self.save_managed_project_bootstrap(bootstrap)?;
                        return Ok(state);
                    }
                    ManagedProjectThreadControlMode::Manual
                    | ManagedProjectThreadControlMode::DirectorPaused => {
                        state.current_phase = format!("paused-{}", role_name);
                        state.watchdog = Some(ManagedProjectWatchdogState {
                            state: "quiet".to_string(),
                            last_signal: Some(format!("director paused before {} turn", role_name)),
                            last_observed_at: Some(Utc::now()),
                            last_progress_at: Some(Utc::now()),
                            role: Some(role_name.clone()),
                            round: Some(spec.round_number),
                            turn_id: None,
                            elapsed_seconds: 0,
                            silence_seconds: 0,
                            turn_status: None,
                            turn_items: 0,
                            app_server_log_modified_at: None,
                            app_server_log_size: None,
                            note: Some(format!("manual control remains active for {}", role_name)),
                        });
                        round.director_summary = Some(format!(
                            "paused before {} turn because control mode is {}",
                            role_name, control_mode
                        ));
                        write_scenario_artifact(
                            bootstrap,
                            &state.scenario_id,
                            spec.round_number,
                            &format!("{}-control.txt", role_name),
                            &format!(
                                "mode: {}\nthread: {}\nstatus: paused before next turn\n",
                                control_mode,
                                role.thread_id.as_deref().unwrap_or("<none>")
                            ),
                        )?;
                        write_scenario_progress_event(
                            bootstrap,
                            &state.scenario_id,
                            &ManagedProjectProgressEvent {
                                event: "director-paused".to_string(),
                                scenario_id: state.scenario_id.clone(),
                                scenario_kind: scenario_kind.to_string(),
                                phase: spec.phase.to_string(),
                                round: spec.round_number,
                                role: Some(role_name.clone()),
                                thread_id: role.thread_id.clone(),
                                turn_id: None,
                                state: state
                                    .watchdog
                                    .as_ref()
                                    .map(|watchdog| watchdog.state.clone()),
                                signal: state
                                    .watchdog
                                    .as_ref()
                                    .and_then(|watchdog| watchdog.last_signal.clone()),
                                message: format!(
                                    "{} control mode {} paused director dispatch",
                                    role_name, control_mode
                                ),
                                timestamp: Utc::now(),
                            },
                        )?;
                        state
                            .rounds
                            .retain(|existing| existing.round_number != round.round_number);
                        state.rounds.push(round.clone());
                        bootstrap.scenario = Some(state.clone());
                        self.save_managed_project_bootstrap(bootstrap)?;
                        return Ok(state);
                    }
                }
                let thread_id = role.thread_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "managed project role `{}` is not attached",
                        role_slug(role.role)
                    )
                })?;
                eprintln!(
                    "tt director scenario {} round {} dispatching {} on thread {}",
                    scenario_kind,
                    spec.round_number,
                    role_slug(role.role),
                    thread_id
                );
                let worker_prompt = build_worker_round_prompt(
                    bootstrap,
                    spec,
                    role,
                    &director_turn.summary,
                    state.pending_approval.as_ref(),
                );
                let assigned_goal = worker_prompt
                    .lines()
                    .find_map(|line| line.strip_prefix("Assigned goal: "))
                    .unwrap_or("<empty>");
                write_scenario_progress_event(
                    bootstrap,
                    &state.scenario_id,
                    &ManagedProjectProgressEvent {
                        event: "worker-dispatch".to_string(),
                        scenario_id: state.scenario_id.clone(),
                        scenario_kind: scenario_kind.to_string(),
                        phase: spec.phase.to_string(),
                        round: spec.round_number,
                        role: Some(role_slug(role.role).to_string()),
                        thread_id: Some(thread_id.to_string()),
                        turn_id: None,
                        state: state
                            .watchdog
                            .as_ref()
                            .map(|watchdog| watchdog.state.clone()),
                        signal: state
                            .watchdog
                            .as_ref()
                            .and_then(|watchdog| watchdog.last_signal.clone()),
                        message: format!(
                            "director dispatching {} with goal {}",
                            role_slug(role.role),
                            assigned_goal
                        ),
                        timestamp: Utc::now(),
                    },
                )?;
                write_scenario_artifact(
                    bootstrap,
                    &state.scenario_id,
                    spec.round_number,
                    &format!("{}-prompt.txt", role_slug(role.role)),
                    &worker_prompt,
                )?;
                let handoff = self.run_role_prompt(
                    bootstrap,
                    &state.scenario_id,
                    spec.round_number,
                    role,
                    thread_id,
                    &worker_prompt,
                )?;
                state.watchdog = handoff.watchdog.clone();
                let extraction = handoff.extraction;
                let (structured_handoff, handoff_source, handoff_parse_error, raw_handoff_text) =
                    match extraction.handoff {
                        Some(parsed) => (
                            parsed,
                            extraction.source,
                            extraction.parse_error,
                            extraction.raw_text,
                        ),
                        None => (
                            seeded_worker_handoff(scenario_kind, spec, role.role),
                            WorkerHandoffSource::SeededFallback,
                            extraction.parse_error,
                            extraction.raw_text,
                        ),
                    };
                let handoff_summary = serde_json::to_string_pretty(&structured_handoff)?;
                write_scenario_artifact(
                    bootstrap,
                    &state.scenario_id,
                    spec.round_number,
                    &format!("{}-handoff.txt", role_slug(role.role)),
                    &handoff_summary,
                )?;
                write_scenario_artifact(
                    bootstrap,
                    &state.scenario_id,
                    spec.round_number,
                    &format!("{}-handoff-source.txt", role_slug(role.role)),
                    match handoff_source {
                        WorkerHandoffSource::Extracted => "extracted\n",
                        WorkerHandoffSource::SeededFallback => "seeded_fallback\n",
                    },
                )?;
                if let Some(raw_handoff_text) = raw_handoff_text.as_ref() {
                    write_scenario_artifact(
                        bootstrap,
                        &state.scenario_id,
                        spec.round_number,
                        &format!("{}-handoff-raw.txt", role_slug(role.role)),
                        raw_handoff_text,
                    )?;
                }
                if let Some(handoff_parse_error) = handoff_parse_error.as_ref() {
                    write_scenario_artifact(
                        bootstrap,
                        &state.scenario_id,
                        spec.round_number,
                        &format!("{}-handoff-parse-error.txt", role_slug(role.role)),
                        handoff_parse_error,
                    )?;
                }
                eprintln!(
                    "tt director scenario {} round {} {} completed turn {}",
                    scenario_kind,
                    spec.round_number,
                    role_slug(role.role),
                    handoff.turn_id
                );
                write_scenario_progress_event(
                    bootstrap,
                    &state.scenario_id,
                    &ManagedProjectProgressEvent {
                        event: "worker-turn-complete".to_string(),
                        scenario_id: state.scenario_id.clone(),
                        scenario_kind: scenario_kind.to_string(),
                        phase: spec.phase.to_string(),
                        round: spec.round_number,
                        role: Some(role_slug(role.role).to_string()),
                        thread_id: Some(thread_id.to_string()),
                        turn_id: Some(handoff.turn_id.clone()),
                        state: state
                            .watchdog
                            .as_ref()
                            .map(|watchdog| watchdog.state.clone()),
                        signal: state
                            .watchdog
                            .as_ref()
                            .and_then(|watchdog| watchdog.last_signal.clone()),
                        message: format!(
                            "{} completed turn {} source={} status={}",
                            role_slug(role.role),
                            handoff.turn_id,
                            handoff_source_string(&handoff_source),
                            structured_handoff.status
                        ),
                        timestamp: Utc::now(),
                    },
                )?;
                round.role_handoffs.insert(
                    role_slug(role.role).to_string(),
                    ManagedProjectRoleHandoff {
                        role: role_slug(role.role).to_string(),
                        thread_id: thread_id.to_string(),
                        turn_id: Some(handoff.turn_id),
                        prompt_summary: worker_prompt.lines().next().unwrap_or("").to_string(),
                        handoff_summary: Some(handoff_summary),
                        status: Some(structured_handoff.status.clone()),
                        changed_files: structured_handoff.changed_files.clone(),
                        tests_run: structured_handoff.tests_run.clone(),
                        blockers: structured_handoff.blockers.clone(),
                        next_step: Some(structured_handoff.next_step.clone()),
                        handoff_source: match handoff_source {
                            WorkerHandoffSource::Extracted => "extracted".to_string(),
                            WorkerHandoffSource::SeededFallback => "seeded_fallback".to_string(),
                        },
                        handoff_parse_error,
                        raw_handoff_text,
                        completed: true,
                    },
                );
            }

            worker_context = render_round_handoffs(&round);
            write_scenario_artifact(
                bootstrap,
                &state.scenario_id,
                spec.round_number,
                "round-summary.md",
                &worker_context,
            )?;
            write_scenario_progress_event(
                bootstrap,
                &state.scenario_id,
                &ManagedProjectProgressEvent {
                    event: "round-summary".to_string(),
                    scenario_id: state.scenario_id.clone(),
                    scenario_kind: scenario_kind.to_string(),
                    phase: spec.phase.to_string(),
                    round: spec.round_number,
                    role: Some("director".to_string()),
                    thread_id: Some(director_thread_id.to_string()),
                    turn_id: round.director_turn_id.clone(),
                    state: state
                        .watchdog
                        .as_ref()
                        .map(|watchdog| watchdog.state.clone()),
                    signal: state
                        .watchdog
                        .as_ref()
                        .and_then(|watchdog| watchdog.last_signal.clone()),
                    message: format!(
                        "round {} phase {} summary recorded with {} role handoffs",
                        spec.round_number,
                        spec.phase,
                        round.role_handoffs.len()
                    ),
                    timestamp: Utc::now(),
                },
            )?;
            if let Some(index) = round_position {
                state.rounds[index] = round;
            } else {
                state.rounds.push(round);
            }
            bootstrap.scenario = Some(state.clone());
            self.save_managed_project_bootstrap(bootstrap)?;
            eprintln!(
                "tt director scenario {} round {} phase {} recorded",
                scenario_kind, spec.round_number, spec.phase
            );
        }

        state.current_phase = "completed".to_string();
        state.completed = true;
        bootstrap.scenario = Some(state.clone());
        self.save_managed_project_bootstrap(bootstrap)?;
        write_scenario_progress_event(
            bootstrap,
            &state.scenario_id,
            &ManagedProjectProgressEvent {
                event: "scenario-complete".to_string(),
                scenario_id: state.scenario_id.clone(),
                scenario_kind: scenario_kind.to_string(),
                phase: state.current_phase.clone(),
                round: state.current_round,
                role: Some("director".to_string()),
                thread_id: Some(director_thread_id.to_string()),
                turn_id: None,
                state: state
                    .watchdog
                    .as_ref()
                    .map(|watchdog| watchdog.state.clone()),
                signal: state
                    .watchdog
                    .as_ref()
                    .and_then(|watchdog| watchdog.last_signal.clone()),
                message: "managed project scenario completed".to_string(),
                timestamp: Utc::now(),
            },
        )?;
        eprintln!("tt director scenario {} completed", scenario_kind);
        Ok(state)
    }

    fn run_role_prompt(
        &self,
        bootstrap: &ManagedProjectBootstrap,
        scenario_id: &str,
        round_number: usize,
        role_bootstrap: &ManagedProjectRoleBootstrap,
        thread_id: &str,
        prompt: &str,
    ) -> Result<ManagedProjectTurnOutcome> {
        let cwd = role_bootstrap
            .worktree_path
            .as_deref()
            .unwrap_or(&bootstrap.repo_root);
        let model = role_bootstrap
            .model
            .clone()
            .unwrap_or_else(|| role_default_model(role_bootstrap.role).to_string());
        let reasoning_effort = role_bootstrap
            .reasoning_effort
            .clone()
            .unwrap_or_else(|| role_default_reasoning_effort(role_bootstrap.role).to_string());
        write_scenario_artifact(
            bootstrap,
            scenario_id,
            round_number,
            &format!("{}-runtime.txt", role_slug(role_bootstrap.role)),
            &format!(
                "role: {}\nmodel: {}\nmodel_reasoning_effort: {}\nthread_id: {}\ncwd: {}\n",
                role_slug(role_bootstrap.role),
                model,
                reasoning_effort,
                thread_id,
                cwd.display()
            ),
        )?;
        eprintln!(
            "tt director prompting role {} thread {} cwd {}",
            role_slug(role_bootstrap.role),
            thread_id,
            cwd.display()
        );
        let _ = append_managed_project_event(
            &bootstrap.repo_root,
            &ManagedProjectEvent {
                ts: Utc::now(),
                project_id: bootstrap.project.id.clone(),
                phase: bootstrap
                    .scenario
                    .as_ref()
                    .map(|scenario| scenario.current_phase.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                kind: ManagedProjectEventKind::PromptSent,
                role: Some("director".to_string()),
                counterparty_role: Some(role_slug(role_bootstrap.role).to_string()),
                thread_id: Some(thread_id.to_string()),
                turn_id: None,
                text: prompt.to_string(),
                status: Some(format!("round-{round_number}")),
                error: None,
            },
        );
        let client = self.codex_runtime_client(cwd)?;
        let output_schema = matches!(
            role_bootstrap.role,
            ThreadRole::Develop | ThreadRole::Test | ThreadRole::Integrate
        )
        .then(worker_handoff_output_schema);
        let mut attempts = Vec::new();
        let mut latest_watchdog = Some(ManagedProjectWatchdogState {
            state: "healthy".to_string(),
            last_signal: Some("turn prompt dispatched".to_string()),
            last_observed_at: Some(Utc::now()),
            last_progress_at: Some(Utc::now()),
            role: Some(role_slug(role_bootstrap.role).to_string()),
            round: Some(round_number),
            turn_id: None,
            elapsed_seconds: 0,
            silence_seconds: 0,
            turn_status: None,
            turn_items: 0,
            app_server_log_modified_at: None,
            app_server_log_size: None,
            note: Some("waiting for the first turn observation".to_string()),
        });
        let scenario_kind_name = bootstrap
            .scenario
            .as_ref()
            .map(|scenario| scenario.scenario_kind.clone())
            .unwrap_or_else(|| "managed-project".to_string());
        let phase_name = bootstrap
            .scenario
            .as_ref()
            .map(|scenario| scenario.current_phase.clone())
            .unwrap_or_else(|| "unknown".to_string());

        for attempt_number in 1..=LIVE_TURN_MAX_ATTEMPTS {
            let attempt_result: Result<ManagedProjectTurnOutcome> = (|| {
                let turn = client.start_turn(
                    thread_id,
                    prompt,
                    Some(cwd),
                    Some(model.clone()),
                    Some(parse_managed_project_reasoning_effort(&reasoning_effort)?),
                    output_schema.clone(),
                )?;
                eprintln!(
                    "tt director role {} started turn {} attempt {}",
                    role_slug(role_bootstrap.role),
                    turn.id,
                    attempt_number
                );
                let watchdog_config = tt_codex::TurnWatchdogConfig {
                    soft_silence: std::time::Duration::from_secs(
                        managed_project_liveness_policy_from_env().soft_silence_seconds,
                    ),
                    hard_ceiling: std::time::Duration::from_secs(
                        managed_project_liveness_policy_from_env().hard_ceiling_seconds,
                    ),
                };
                let completed = client.wait_for_turn_completion_with_watchdog(
                    thread_id,
                    &turn.id,
                    watchdog_config,
                    |observation| {
                        let watchdog_state = ManagedProjectWatchdogState {
                            state: format!("{:?}", observation.state).to_lowercase(),
                            last_signal: observation.progress_signal.clone(),
                            last_observed_at: Some(Utc::now()),
                            last_progress_at: observation
                                .progress_signal
                                .as_ref()
                                .map(|_| Utc::now())
                                .or_else(|| {
                                    latest_watchdog
                                        .as_ref()
                                        .and_then(|state| state.last_progress_at)
                                }),
                            role: Some(role_slug(role_bootstrap.role).to_string()),
                            round: Some(round_number),
                            turn_id: Some(turn.id.clone()),
                            elapsed_seconds: observation.elapsed_seconds,
                            silence_seconds: observation.silent_seconds,
                            turn_status: observation.turn_status.clone(),
                            turn_items: observation.turn_items,
                            app_server_log_modified_at: observation.app_server_log_modified_at,
                            app_server_log_size: observation.app_server_log_size,
                            note: Some(format!(
                                "thread_updated_at={} turn_count={} status={} items={} app_server_log_mtime={:?}",
                                observation.thread_updated_at,
                                observation.turn_count,
                                observation.turn_status.as_deref().unwrap_or("<unknown>"),
                                observation.turn_items,
                                observation.app_server_log_modified_at
                            )),
                        };
                        latest_watchdog = Some(watchdog_state);
                        if let Some(watchdog_state) = latest_watchdog.as_ref() {
                            let _ = write_scenario_progress_event(
                                bootstrap,
                                scenario_id,
                                &ManagedProjectProgressEvent {
                                    event: "watchdog-progress".to_string(),
                                    scenario_id: scenario_id.to_string(),
                                    scenario_kind: scenario_kind_name.clone(),
                                    phase: phase_name.clone(),
                                    round: round_number,
                                    role: Some(role_slug(role_bootstrap.role).to_string()),
                                    thread_id: Some(thread_id.to_string()),
                                    turn_id: Some(turn.id.clone()),
                                    state: Some(watchdog_state.state.clone()),
                                    signal: watchdog_state.last_signal.clone(),
                                    message: watchdog_state
                                        .note
                                        .clone()
                                        .unwrap_or_else(|| "watchdog observation".to_string()),
                                    timestamp: Utc::now(),
                                },
                            );
                            eprintln!(
                                "tt watchdog role={} round={} state={} elapsed={}s silence={}s status={} items={} signal={} log_size={}",
                                role_slug(role_bootstrap.role),
                                round_number,
                                watchdog_state.state,
                                watchdog_state.elapsed_seconds,
                                watchdog_state.silence_seconds,
                                watchdog_state
                                    .turn_status
                                    .as_deref()
                                    .unwrap_or("<unknown>"),
                                watchdog_state.turn_items,
                                watchdog_state
                                    .last_signal
                                    .as_deref()
                                    .unwrap_or("<none>"),
                                watchdog_state
                                    .app_server_log_size
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "<none>".to_string()),
                            );
                            let _ = write_scenario_artifact(
                                bootstrap,
                                scenario_id,
                                round_number,
                                &format!("{}-watchdog.txt", role_slug(role_bootstrap.role)),
                                &render_watchdog_state(watchdog_state),
                            );
                        }
                    },
                )?;
                eprintln!(
                    "tt director role {} observed turn {} status {:?} attempt {}",
                    role_slug(role_bootstrap.role),
                    completed.id,
                    completed.status,
                    attempt_number
                );
                let finished_turn = client
                    .load_completed_turn_with_history(
                        thread_id,
                        &completed.id,
                        Some(cwd),
                        Some(model.clone()),
                    )?
                    .ok_or_else(|| {
                        anyhow::anyhow!("turn `{}` not found after completion", completed.id)
                    })?;
                match finished_turn.status {
                    protocol::TurnStatus::Completed => {
                        let extraction = extract_worker_handoff(&finished_turn.items);
                        let summary = summarize_turn_items(&finished_turn.items);
                        let _ = append_managed_project_event(
                            &bootstrap.repo_root,
                            &ManagedProjectEvent {
                                ts: Utc::now(),
                                project_id: bootstrap.project.id.clone(),
                                phase: phase_name.clone(),
                                kind: ManagedProjectEventKind::ResponseReceived,
                                role: Some(role_slug(role_bootstrap.role).to_string()),
                                counterparty_role: Some("director".to_string()),
                                thread_id: Some(thread_id.to_string()),
                                turn_id: Some(finished_turn.id.clone()),
                                text: summary.clone(),
                                status: Some("completed".to_string()),
                                error: None,
                            },
                        );
                        Ok(ManagedProjectTurnOutcome {
                            turn_id: finished_turn.id,
                            summary,
                            extraction,
                            attempts: Vec::new(),
                            watchdog: latest_watchdog.clone(),
                        })
                    }
                    protocol::TurnStatus::Failed => anyhow::bail!(
                        "{}",
                        render_turn_failure(
                            &finished_turn,
                            role_slug(role_bootstrap.role),
                            &model,
                            &reasoning_effort
                        )
                    ),
                    protocol::TurnStatus::Interrupted => anyhow::bail!(
                        "turn `{}` for role `{}` was interrupted (model={}, reasoning_effort={})",
                        finished_turn.id,
                        role_slug(role_bootstrap.role),
                        model,
                        reasoning_effort
                    ),
                    protocol::TurnStatus::InProgress => anyhow::bail!(
                        "turn `{}` for role `{}` did not complete (model={}, reasoning_effort={})",
                        finished_turn.id,
                        role_slug(role_bootstrap.role),
                        model,
                        reasoning_effort
                    ),
                }
            })();

            match attempt_result {
                Ok(mut outcome) => {
                    attempts.push(ManagedProjectTurnAttempt {
                        attempt_number,
                        model: model.clone(),
                        reasoning_effort: reasoning_effort.clone(),
                        thread_id: thread_id.to_string(),
                        turn_id: Some(outcome.turn_id.clone()),
                        status: Some("completed".to_string()),
                        failure_summary: None,
                    });
                    write_role_attempt_artifact(
                        bootstrap,
                        scenario_id,
                        round_number,
                        role_bootstrap.role,
                        attempts.last().expect("attempt"),
                    )?;
                    write_scenario_artifact(
                        bootstrap,
                        scenario_id,
                        round_number,
                        &format!("{}-attempts.txt", role_slug(role_bootstrap.role)),
                        &render_attempt_log(&attempts),
                    )?;
                    outcome.attempts = attempts;
                    return Ok(outcome);
                }
                Err(error) => {
                    let _ = append_managed_project_event(
                        &bootstrap.repo_root,
                        &ManagedProjectEvent {
                            ts: Utc::now(),
                            project_id: bootstrap.project.id.clone(),
                            phase: phase_name.clone(),
                            kind: ManagedProjectEventKind::TurnFailed,
                            role: Some(role_slug(role_bootstrap.role).to_string()),
                            counterparty_role: Some("director".to_string()),
                            thread_id: Some(thread_id.to_string()),
                            turn_id: None,
                            text: format!(
                                "turn failed for `{}` on attempt {}",
                                role_slug(role_bootstrap.role),
                                attempt_number
                            ),
                            status: Some("failed".to_string()),
                            error: Some(format_error_chain(&error)),
                        },
                    );
                    let failure_summary = format_error_chain(&error);
                    attempts.push(ManagedProjectTurnAttempt {
                        attempt_number,
                        model: model.clone(),
                        reasoning_effort: reasoning_effort.clone(),
                        thread_id: thread_id.to_string(),
                        turn_id: extract_turn_id_from_error_message(&failure_summary),
                        status: Some("failed".to_string()),
                        failure_summary: Some(failure_summary.clone()),
                    });
                    write_role_attempt_artifact(
                        bootstrap,
                        scenario_id,
                        round_number,
                        role_bootstrap.role,
                        attempts.last().expect("attempt"),
                    )?;
                    let is_retryable = is_retryable_live_turn_failure(&failure_summary);
                    if is_retryable && attempt_number < LIVE_TURN_MAX_ATTEMPTS {
                        eprintln!(
                            "tt director role {} retrying transient upstream failure on attempt {}: {}",
                            role_slug(role_bootstrap.role),
                            attempt_number,
                            failure_summary
                        );
                        thread::sleep(live_turn_backoff_delay(attempt_number));
                        continue;
                    }

                    let attempt_log = render_attempt_log(&attempts);
                    write_scenario_artifact(
                        bootstrap,
                        scenario_id,
                        round_number,
                        &format!("{}-attempts.txt", role_slug(role_bootstrap.role)),
                        &attempt_log,
                    )?;
                    return Err(error.context(format!(
                        "role `{}` failed after {} attempt(s); model={}, reasoning_effort={}",
                        role_slug(role_bootstrap.role),
                        attempts.len(),
                        model,
                        reasoning_effort
                    )));
                }
            }
        }

        anyhow::bail!(
            "role `{}` exhausted live turn attempts without a result",
            role_slug(role_bootstrap.role)
        )
    }

    fn save_managed_project_bootstrap(&self, bootstrap: &ManagedProjectBootstrap) -> Result<()> {
        save_managed_project_project_config(
            &bootstrap.project_config_path,
            &bootstrap.project_config,
        )?;
        save_managed_project_plan(&bootstrap.plan_path, &bootstrap.plan)?;
        let role_refs: Vec<_> = bootstrap.roles.iter().collect();
        let manifest = build_managed_project_manifest(
            &bootstrap.project,
            &bootstrap.repo_root,
            &bootstrap.base_branch,
            &bootstrap.worktree_root,
            &bootstrap.project_config_path,
            &bootstrap.plan_path,
            &bootstrap.contract_path,
            &bootstrap.codex_config_path,
            &bootstrap.startup,
            bootstrap.scenario.clone(),
            &role_refs,
        )?;
        save_managed_project_manifest(&bootstrap.manifest_path, &manifest)
    }

    fn spawn_managed_project_role(
        &self,
        repository: &GitRepository,
        bootstrap: &mut ManagedProjectBootstrap,
        role: ThreadRole,
    ) -> Result<()> {
        let role_index = bootstrap
            .roles
            .iter()
            .position(|candidate| candidate.role == role)
            .ok_or_else(|| {
                anyhow::anyhow!("managed project role `{}` not found", role_slug(role))
            })?;
        if bootstrap.roles[role_index].thread_id.is_some() {
            anyhow::bail!(
                "managed project role `{}` is already attached",
                role_slug(role)
            );
        }

        let role_bootstrap = bootstrap.roles[role_index].clone();
        let agent_file = load_managed_agent_file(&role_bootstrap.agent_path)?;
        let cwd = role_bootstrap
            .worktree_path
            .as_deref()
            .unwrap_or(bootstrap.repo_root.as_path());
        let thread =
            self.start_managed_project_thread(cwd, &bootstrap.project, role, &agent_file)?;
        self.verify_thread_workspace(&role_bootstrap, &bootstrap.repo_root, &thread)?;
        let updated =
            self.bind_managed_project_role(repository, bootstrap, &role_bootstrap, &thread)?;
        bootstrap.roles[role_index] = updated;
        Ok(())
    }

    fn launch_managed_project_startup(&self, bootstrap: &ManagedProjectBootstrap) {
        let service = self.clone();
        let mut bootstrap = bootstrap.clone();
        thread::spawn(move || {
            if let Err(error) = service.run_managed_project_startup(&mut bootstrap) {
                let _ = service.mark_managed_project_startup_blocked(
                    &bootstrap.repo_root,
                    format!("startup handshake failed: {}", format_error_chain(&error)),
                );
                let _ = append_managed_project_event(
                    &bootstrap.repo_root,
                    &ManagedProjectEvent {
                        ts: Utc::now(),
                        project_id: bootstrap.project.id.clone(),
                        phase: managed_project_startup_phase_slug(
                            ManagedProjectStartupPhase::Blocked,
                        )
                        .to_string(),
                        kind: ManagedProjectEventKind::TurnFailed,
                        role: None,
                        counterparty_role: None,
                        thread_id: None,
                        turn_id: None,
                        text: "startup handshake failed".to_string(),
                        status: Some("blocked".to_string()),
                        error: Some(format_error_chain(&error)),
                    },
                );
                eprintln!("tt managed startup error: {error:?}");
            }
        });
    }

    fn run_managed_project_startup(&self, bootstrap: &mut ManagedProjectBootstrap) -> Result<()> {
        bootstrap.startup.phase = ManagedProjectStartupPhase::WorkerReportsPending;
        bootstrap.startup.updated_at = Utc::now();
        self.save_managed_project_bootstrap(bootstrap)?;
        let _ = append_managed_project_event(
            &bootstrap.repo_root,
            &ManagedProjectEvent {
                ts: Utc::now(),
                project_id: bootstrap.project.id.clone(),
                phase: managed_project_startup_phase_slug(bootstrap.startup.phase).to_string(),
                kind: ManagedProjectEventKind::PhaseChanged,
                role: None,
                counterparty_role: None,
                thread_id: None,
                turn_id: None,
                text: "startup handshake entered worker report collection".to_string(),
                status: Some("worker_reports_pending".to_string()),
                error: None,
            },
        );

        let worker_roles = [ThreadRole::Develop, ThreadRole::Test, ThreadRole::Integrate];
        let mut reports = Vec::new();
        for role in worker_roles {
            let Some(role_bootstrap) = bootstrap
                .roles
                .iter()
                .find(|candidate| candidate.role == role)
                .cloned()
            else {
                continue;
            };
            let role_key = role_slug(role).to_string();
            let startup_prompt = build_worker_startup_prompt(bootstrap, &role_bootstrap);
            if let Some(state) = bootstrap.startup.worker_reports.get_mut(&role_key) {
                state.status = ManagedProjectStartupRoleStatus::Pending;
                state.updated_at = Utc::now();
                state.turn_id = None;
                state.summary = Some("waiting for bootstrap readiness report".to_string());
            }
            bootstrap.startup.updated_at = Utc::now();
            self.save_managed_project_bootstrap(bootstrap)?;
            let _ = append_managed_project_event(
                &bootstrap.repo_root,
                &ManagedProjectEvent {
                    ts: Utc::now(),
                    project_id: bootstrap.project.id.clone(),
                    phase: managed_project_startup_phase_slug(bootstrap.startup.phase).to_string(),
                    kind: ManagedProjectEventKind::PromptSent,
                    role: None,
                    counterparty_role: Some(role_key.clone()),
                    thread_id: role_bootstrap.thread_id.clone(),
                    turn_id: None,
                    text: startup_prompt.clone(),
                    status: Some("startup".to_string()),
                    error: None,
                },
            );

            let outcome =
                self.run_managed_project_startup_turn(bootstrap, &role_bootstrap, &startup_prompt)?;
            let report = match parse_bootstrap_worker_report(
                &outcome.summary,
                &outcome.extraction,
                &role_bootstrap,
                &bootstrap.repo_root,
            ) {
                Ok(report) => report,
                Err(error) => {
                    let raw_text = outcome
                        .extraction
                        .raw_text
                        .clone()
                        .unwrap_or_else(|| outcome.summary.clone());
                    let _ = append_managed_project_event(
                        &bootstrap.repo_root,
                        &ManagedProjectEvent {
                            ts: Utc::now(),
                            project_id: bootstrap.project.id.clone(),
                            phase: managed_project_startup_phase_slug(bootstrap.startup.phase)
                                .to_string(),
                            kind: ManagedProjectEventKind::ParseFailed,
                            role: Some(role_key.clone()),
                            counterparty_role: Some("director".to_string()),
                            thread_id: role_bootstrap.thread_id.clone(),
                            turn_id: Some(outcome.turn_id.clone()),
                            text: raw_text,
                            status: Some("parse_failed".to_string()),
                            error: Some(error.to_string()),
                        },
                    );
                    return Err(error);
                }
            };
            let blocked = report.status.eq_ignore_ascii_case("blocked")
                || report
                    .blocker
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty());
            if let Some(state) = bootstrap.startup.worker_reports.get_mut(&role_key) {
                state.status = if blocked {
                    ManagedProjectStartupRoleStatus::Blocked
                } else {
                    ManagedProjectStartupRoleStatus::Reported
                };
                state.updated_at = Utc::now();
                state.turn_id = Some(outcome.turn_id.clone());
                state.summary = Some(report.summary.clone());
            }
            bootstrap.startup.updated_at = Utc::now();
            reports.push(report);
            let _ = append_managed_project_event(
                &bootstrap.repo_root,
                &ManagedProjectEvent {
                    ts: Utc::now(),
                    project_id: bootstrap.project.id.clone(),
                    phase: managed_project_startup_phase_slug(bootstrap.startup.phase).to_string(),
                    kind: ManagedProjectEventKind::ResponseReceived,
                    role: Some(role_key.clone()),
                    counterparty_role: Some("director".to_string()),
                    thread_id: role_bootstrap.thread_id.clone(),
                    turn_id: Some(outcome.turn_id.clone()),
                    text: outcome.summary.clone(),
                    status: Some(if blocked { "blocked" } else { "reported" }.to_string()),
                    error: None,
                },
            );
            if blocked {
                bootstrap.startup.phase = ManagedProjectStartupPhase::Blocked;
                self.save_managed_project_bootstrap(bootstrap)?;
                let _ = append_managed_project_event(
                    &bootstrap.repo_root,
                    &ManagedProjectEvent {
                        ts: Utc::now(),
                        project_id: bootstrap.project.id.clone(),
                        phase: managed_project_startup_phase_slug(bootstrap.startup.phase)
                            .to_string(),
                        kind: ManagedProjectEventKind::PhaseChanged,
                        role: None,
                        counterparty_role: None,
                        thread_id: role_bootstrap.thread_id.clone(),
                        turn_id: Some(outcome.turn_id.clone()),
                        text: format!("startup blocked by `{}` readiness report", role_key),
                        status: Some("blocked".to_string()),
                        error: None,
                    },
                );
                return Ok(());
            }
            self.save_managed_project_bootstrap(bootstrap)?;
        }

        bootstrap.startup.phase = ManagedProjectStartupPhase::DirectorAckPending;
        bootstrap.startup.updated_at = Utc::now();
        self.save_managed_project_bootstrap(bootstrap)?;
        let _ = append_managed_project_event(
            &bootstrap.repo_root,
            &ManagedProjectEvent {
                ts: Utc::now(),
                project_id: bootstrap.project.id.clone(),
                phase: managed_project_startup_phase_slug(bootstrap.startup.phase).to_string(),
                kind: ManagedProjectEventKind::PhaseChanged,
                role: None,
                counterparty_role: None,
                thread_id: None,
                turn_id: None,
                text: "startup handshake waiting for director acknowledgement".to_string(),
                status: Some("director_ack_pending".to_string()),
                error: None,
            },
        );

        let director = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Director)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("managed project director role missing"))?;
        let director_prompt = build_director_startup_prompt(bootstrap, &reports);
        let _ = append_managed_project_event(
            &bootstrap.repo_root,
            &ManagedProjectEvent {
                ts: Utc::now(),
                project_id: bootstrap.project.id.clone(),
                phase: managed_project_startup_phase_slug(bootstrap.startup.phase).to_string(),
                kind: ManagedProjectEventKind::PromptSent,
                role: None,
                counterparty_role: Some("director".to_string()),
                thread_id: director.thread_id.clone(),
                turn_id: None,
                text: director_prompt.clone(),
                status: Some("startup".to_string()),
                error: None,
            },
        );
        let director_outcome =
            self.run_managed_project_startup_turn(bootstrap, &director, &director_prompt)?;
        let ack = match parse_bootstrap_director_ack(
            &director_outcome.summary,
            &director_outcome.extraction,
            &reports,
        ) {
            Ok(ack) => ack,
            Err(error) => {
                let raw_text = director_outcome
                    .extraction
                    .raw_text
                    .clone()
                    .unwrap_or_else(|| director_outcome.summary.clone());
                let _ = append_managed_project_event(
                    &bootstrap.repo_root,
                    &ManagedProjectEvent {
                        ts: Utc::now(),
                        project_id: bootstrap.project.id.clone(),
                        phase: managed_project_startup_phase_slug(bootstrap.startup.phase)
                            .to_string(),
                        kind: ManagedProjectEventKind::ParseFailed,
                        role: Some("director".to_string()),
                        counterparty_role: Some("operator".to_string()),
                        thread_id: director.thread_id.clone(),
                        turn_id: Some(director_outcome.turn_id.clone()),
                        text: raw_text,
                        status: Some("parse_failed".to_string()),
                        error: Some(error.to_string()),
                    },
                );
                return Err(error);
            }
        };
        let _ = append_managed_project_event(
            &bootstrap.repo_root,
            &ManagedProjectEvent {
                ts: Utc::now(),
                project_id: bootstrap.project.id.clone(),
                phase: managed_project_startup_phase_slug(bootstrap.startup.phase).to_string(),
                kind: ManagedProjectEventKind::ResponseReceived,
                role: Some("director".to_string()),
                counterparty_role: Some("operator".to_string()),
                thread_id: director.thread_id.clone(),
                turn_id: Some(director_outcome.turn_id.clone()),
                text: director_outcome.summary.clone(),
                status: Some(
                    if ack.status.eq_ignore_ascii_case("ready") {
                        "ready"
                    } else {
                        "blocked"
                    }
                    .to_string(),
                ),
                error: None,
            },
        );
        bootstrap.startup.director_ack = Some(ManagedProjectStartupDirectorAck {
            status: if ack.status.eq_ignore_ascii_case("ready") {
                ManagedProjectStartupAckStatus::Ready
            } else {
                ManagedProjectStartupAckStatus::Blocked
            },
            updated_at: Utc::now(),
            turn_id: Some(director_outcome.turn_id),
            summary: ack.summary,
            received_roles: ack.received_roles,
            missing_roles: ack.missing_roles.clone(),
        });
        bootstrap.startup.phase = if bootstrap.startup.director_ack.as_ref().is_some_and(|ack| {
            ack.status == ManagedProjectStartupAckStatus::Ready && ack.missing_roles.is_empty()
        }) {
            ManagedProjectStartupPhase::Ready
        } else {
            ManagedProjectStartupPhase::Blocked
        };
        bootstrap.startup.updated_at = Utc::now();
        self.save_managed_project_bootstrap(bootstrap)?;
        let _ = append_managed_project_event(
            &bootstrap.repo_root,
            &ManagedProjectEvent {
                ts: Utc::now(),
                project_id: bootstrap.project.id.clone(),
                phase: managed_project_startup_phase_slug(bootstrap.startup.phase).to_string(),
                kind: ManagedProjectEventKind::PhaseChanged,
                role: None,
                counterparty_role: None,
                thread_id: director.thread_id.clone(),
                turn_id: bootstrap
                    .startup
                    .director_ack
                    .as_ref()
                    .and_then(|ack| ack.turn_id.clone()),
                text: bootstrap
                    .startup
                    .director_ack
                    .as_ref()
                    .map(|ack| ack.summary.clone())
                    .unwrap_or_else(|| "startup phase updated".to_string()),
                status: Some(
                    managed_project_startup_phase_slug(bootstrap.startup.phase).to_string(),
                ),
                error: None,
            },
        );
        Ok(())
    }

    fn run_managed_project_startup_turn(
        &self,
        bootstrap: &ManagedProjectBootstrap,
        role_bootstrap: &ManagedProjectRoleBootstrap,
        prompt: &str,
    ) -> Result<ManagedProjectTurnOutcome> {
        let thread_id = role_bootstrap.thread_id.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "managed project role `{}` is not attached",
                role_slug(role_bootstrap.role)
            )
        })?;
        let cwd = role_bootstrap
            .worktree_path
            .as_deref()
            .unwrap_or(bootstrap.repo_root.as_path());
        let model = role_bootstrap
            .model
            .clone()
            .unwrap_or_else(|| role_default_model(role_bootstrap.role).to_string());
        let reasoning_effort = role_bootstrap
            .reasoning_effort
            .clone()
            .unwrap_or_else(|| role_default_reasoning_effort(role_bootstrap.role).to_string());
        let client = self.codex_runtime_client(cwd)?;
        let turn = client.start_turn(
            thread_id,
            prompt,
            Some(cwd),
            Some(model.clone()),
            Some(parse_managed_project_reasoning_effort(&reasoning_effort)?),
            None,
        )?;
        let completed = client.wait_for_turn_completion(thread_id, &turn.id)?;
        let finished_turn = client
            .load_completed_turn_with_history(thread_id, &completed.id, Some(cwd), Some(model))?
            .ok_or_else(|| anyhow::anyhow!("turn `{}` not found after completion", completed.id))?;
        match finished_turn.status {
            protocol::TurnStatus::Completed => Ok(ManagedProjectTurnOutcome {
                turn_id: finished_turn.id,
                summary: summarize_turn_items(&finished_turn.items),
                extraction: extract_worker_handoff(&finished_turn.items),
                attempts: Vec::new(),
                watchdog: None,
            }),
            protocol::TurnStatus::Failed => anyhow::bail!(
                "{}",
                render_turn_failure(
                    &finished_turn,
                    role_slug(role_bootstrap.role),
                    &role_bootstrap.model.clone().unwrap_or_default(),
                    &reasoning_effort
                )
            ),
            protocol::TurnStatus::Interrupted => anyhow::bail!(
                "startup turn `{}` for role `{}` was interrupted",
                finished_turn.id,
                role_slug(role_bootstrap.role)
            ),
            protocol::TurnStatus::InProgress => anyhow::bail!(
                "startup turn `{}` for role `{}` did not complete",
                finished_turn.id,
                role_slug(role_bootstrap.role)
            ),
        }
    }

    fn mark_managed_project_startup_blocked(
        &self,
        repo_root: &Path,
        summary: String,
    ) -> Result<()> {
        let manifest_path = require_initialized_managed_project(repo_root)?;
        let manifest = load_managed_project_manifest(&manifest_path)?;
        let mut bootstrap =
            self.managed_project_bootstrap_from_manifest(&manifest_path, &manifest)?;
        bootstrap.startup.phase = ManagedProjectStartupPhase::Blocked;
        bootstrap.startup.updated_at = Utc::now();
        bootstrap.startup.director_ack = Some(ManagedProjectStartupDirectorAck {
            status: ManagedProjectStartupAckStatus::Blocked,
            updated_at: Utc::now(),
            turn_id: None,
            summary,
            received_roles: Vec::new(),
            missing_roles: vec![
                "dev".to_string(),
                "test".to_string(),
                "integration".to_string(),
            ],
        });
        self.save_managed_project_bootstrap(&bootstrap)
    }

    fn attach_managed_project_role(
        &self,
        repository: &GitRepository,
        bootstrap: &mut ManagedProjectBootstrap,
        attachment: ManagedProjectThreadAttachment,
    ) -> Result<()> {
        let role = attachment.role;
        let role_index = bootstrap
            .roles
            .iter()
            .position(|candidate| candidate.role == role)
            .ok_or_else(|| {
                anyhow::anyhow!("managed project role `{}` not found", role_slug(role))
            })?;
        let role_bootstrap = bootstrap.roles[role_index].clone();
        if let Some(existing_thread_id) = role_bootstrap.thread_id.as_ref()
            && existing_thread_id != &attachment.thread_id
        {
            anyhow::bail!(
                "managed project role `{}` is already bound to thread `{}`",
                role_slug(role),
                existing_thread_id
            );
        }
        let client = self.codex_runtime_client(bootstrap.repo_root.as_path())?;
        let snapshot = client
            .read_thread(&attachment.thread_id, false)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "codex thread `{}` not found for managed project role `{}`",
                    attachment.thread_id,
                    role_slug(role)
                )
            })?;
        self.verify_thread_workspace(&role_bootstrap, &bootstrap.repo_root, &snapshot)?;
        let updated =
            self.bind_managed_project_role(repository, bootstrap, &role_bootstrap, &snapshot)?;
        bootstrap.roles[role_index] = updated;
        Ok(())
    }

    fn start_managed_project_thread(
        &self,
        cwd: &Path,
        project: &Project,
        role: ThreadRole,
        agent_file: &ManagedAgentFile,
    ) -> Result<CodexThreadRuntimeSnapshot> {
        let sandbox = parse_managed_project_sandbox_mode(&agent_file.sandbox_mode)?;
        let client = self.codex_runtime_client(cwd)?;
        client.start_thread_with_params(protocol::ThreadStartParams {
            cwd: Some(cwd.display().to_string()),
            model: agent_file.model.clone(),
            sandbox: Some(sandbox),
            service_name: Some("tt-managed-project".to_string()),
            base_instructions: Some(format!(
                "Managed TT project `{}` role `{}`. Follow `.tt/contract.md` and stay inside the assigned scope.",
                project.slug,
                role_slug(role)
            )),
            developer_instructions: Some(agent_file.developer_instructions.clone()),
            ephemeral: Some(false),
            persist_extended_history: true,
            ..protocol::ThreadStartParams::default()
        })
    }

    fn verify_thread_workspace(
        &self,
        role_bootstrap: &ManagedProjectRoleBootstrap,
        repo_root: &Path,
        snapshot: &CodexThreadRuntimeSnapshot,
    ) -> Result<()> {
        let observed = Path::new(&snapshot.cwd);
        let expected = role_bootstrap.worktree_path.as_deref().unwrap_or(repo_root);
        if observed != expected {
            anyhow::bail!(
                "thread `{}` is running in `{}` but role `{}` expects `{}`",
                snapshot.thread_id,
                observed.display(),
                role_slug(role_bootstrap.role),
                expected.display()
            );
        }
        Ok(())
    }

    fn bind_managed_project_role(
        &self,
        repository: &GitRepository,
        bootstrap: &ManagedProjectBootstrap,
        role_bootstrap: &ManagedProjectRoleBootstrap,
        snapshot: &CodexThreadRuntimeSnapshot,
    ) -> Result<ManagedProjectRoleBootstrap> {
        let now = Utc::now();
        let thread_binding = ThreadBinding {
            codex_thread_id: snapshot.thread_id.clone(),
            work_unit_id: Some(role_bootstrap.work_unit.id.clone()),
            role: role_bootstrap.role,
            status: ThreadBindingStatus::Bound,
            notes: Some("managed-project attachment".to_string()),
            created_at: now,
            updated_at: now,
        };
        self.upsert_thread_binding(&thread_binding)?;

        let workspace_binding_id =
            managed_project_thread_binding_id(&bootstrap.project.slug, role_bootstrap.role);
        let workspace_binding = self.build_managed_project_workspace_binding(
            repository,
            bootstrap,
            role_bootstrap,
            snapshot,
            &workspace_binding_id,
        )?;
        self.upsert_workspace_binding(&workspace_binding)?;
        upsert_session_index_entry(
            managed_project_codex_home(&bootstrap.repo_root),
            &snapshot.thread_id,
            snapshot.thread_name.as_deref(),
        )?;

        Ok(ManagedProjectRoleBootstrap {
            thread_id: Some(snapshot.thread_id.clone()),
            thread_name: snapshot.thread_name.clone(),
            workspace_binding_id: Some(workspace_binding_id),
            ..role_bootstrap.clone()
        })
    }

    fn build_managed_project_workspace_binding(
        &self,
        repository: &GitRepository,
        bootstrap: &ManagedProjectBootstrap,
        role_bootstrap: &ManagedProjectRoleBootstrap,
        snapshot: &CodexThreadRuntimeSnapshot,
        id: &str,
    ) -> Result<WorkspaceBinding> {
        let inspect_repository = if let Some(worktree_path) = role_bootstrap.worktree_path.as_ref()
        {
            GitRepository::discover(worktree_path)?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "managed project role worktree {} is not a git repository",
                        worktree_path.display()
                    )
                })?
                .inspect_repository()?
        } else {
            repository.inspect_repository()?
        };
        let base_commit = inspect_repository.current_head_commit.clone();
        let status = workspace_status_from_git(&inspect_repository);
        Ok(WorkspaceBinding {
            id: id.to_string(),
            codex_thread_id: snapshot.thread_id.clone(),
            repo_root: bootstrap.repo_root.display().to_string(),
            worktree_path: role_bootstrap
                .worktree_path
                .as_ref()
                .map(|path| path.display().to_string()),
            branch_name: role_bootstrap.branch_name.clone(),
            base_ref: Some(bootstrap.base_branch.clone()),
            base_commit,
            landing_target: Some(bootstrap.base_branch.clone()),
            strategy: managed_project_workspace_strategy(role_bootstrap.role),
            sync_policy: managed_project_workspace_sync_policy(role_bootstrap.role),
            cleanup_policy: managed_project_workspace_cleanup_policy(role_bootstrap.role),
            status,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }
}

fn ensure_role_worktree(
    repository: &GitRepository,
    worktree_path: &Path,
    branch_name: &str,
    base_branch: &str,
) -> Result<()> {
    if repository
        .list_worktrees()?
        .iter()
        .any(|worktree| worktree.worktree_path == worktree_path)
    {
        return Ok(());
    }
    if let Some(parent) = worktree_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if repository.create_worktree(worktree_path, branch_name, Some(base_branch))? {
        Ok(())
    } else {
        anyhow::bail!(
            "failed to create worktree `{}` for branch `{}`",
            worktree_path.display(),
            branch_name
        )
    }
}

fn write_managed_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::read_to_string(path) {
        Ok(existing) if existing == contents => Ok(()),
        Ok(_) => anyhow::bail!(
            "managed file already exists and differs: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::write(path, contents)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn write_or_replace_managed_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::read_to_string(path) {
        Ok(existing) if existing == contents => Ok(()),
        Ok(_) | Err(_) => {
            fs::write(path, contents)?;
            Ok(())
        }
    }
}

fn prune_repo_codex_runtime_artifacts(codex_root: &Path) -> Result<usize> {
    let mut removed = 0usize;
    removed += remove_if_exists(codex_root.join("session_index.jsonl"))?;
    removed += remove_if_exists(codex_root.join("sessions"))?;
    removed += remove_if_exists(codex_root.join("archived_sessions"))?;
    removed += remove_if_exists(codex_root.join("logs"))?;

    if let Ok(entries) = fs::read_dir(codex_root) {
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("sqlite") {
                removed += remove_if_exists(path)?;
            }
        }
    }

    Ok(removed)
}

fn initialize_git_repository(path: &Path, base_branch: Option<&str>) -> Result<()> {
    if path.join(".git").exists() {
        return Ok(());
    }
    let branch = base_branch.unwrap_or("main");
    run_command(path, "git", ["init", "-b", branch])?;
    Ok(())
}

fn scaffold_managed_project_template(path: &Path, template: Option<&str>) -> Result<()> {
    match template.unwrap_or("rust-taskflow") {
        "rust-taskflow" => {
            if !path.join("src").exists() {
                fs::create_dir_all(path.join("src"))?;
            }
            write_managed_file(
                &path.join("Cargo.toml"),
                "[package]\nname = \"taskflow\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nserde_json = \"1\"\nserde_yaml = \"0.9\"\n",
            )?;
            write_managed_file(
                &path.join("src").join("main.rs"),
                "fn main() {\n    println!(\"taskflow\");\n}\n",
            )?;
            write_managed_file(
                &path.join("src").join("lib.rs"),
                "pub fn project_name() -> &'static str {\n    \"taskflow\"\n}\n",
            )?;
            write_managed_file(
                &path.join(".gitignore"),
                "/target\n/.tt\n/.codex/config.toml\n/.codex/config.local.toml\n/.codex/session_index.jsonl\n/.codex/sessions/\n/.codex/archived_sessions/\n/.codex/*.sqlite\n/.codex/logs/\n*.log\n",
            )?;
            if !path.join("tests").exists() {
                fs::create_dir_all(path.join("tests"))?;
            }
            write_managed_file(
                &path.join("README.md"),
                "# taskflow\n\nA Rust workflow runner managed by TT.\n",
            )?;
            ensure_initial_repository_commit(path)?;
            Ok(())
        }
        other => anyhow::bail!("unsupported managed project template `{other}`"),
    }
}

fn repo_root_has_no_non_git_entries(path: &Path) -> Result<bool> {
    for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", path.display()))?;
        if entry.file_name() != ".git" {
            return Ok(false);
        }
    }
    Ok(true)
}

fn run_command<I, S>(cwd: &Path, program: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new(program);
    command.current_dir(cwd).args(args);
    apply_repo_settings_env(&mut command);
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("command `{program}` failed in {}", cwd.display())
    }
}

fn ensure_initial_repository_commit(path: &Path) -> Result<()> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()?;
    if output.status.success() {
        return Ok(());
    }

    run_command(path, "git", ["config", "user.email", "tt@example.invalid"])?;
    run_command(path, "git", ["config", "user.name", "TT Scenario"])?;
    run_command(path, "git", ["add", "."])?;
    run_command(path, "git", ["commit", "-m", "Initial scaffold"])?;
    Ok(())
}

fn resolve_path(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn managed_project_repo_root(cwd: &Path) -> Result<Option<PathBuf>> {
    Ok(GitRepository::discover(cwd)?.map(|repository| repository.repository_root))
}

fn require_initialized_managed_project(repo_root: &Path) -> Result<PathBuf> {
    let tt_root = repo_root.join(".tt");
    if !tt_root.is_dir() {
        anyhow::bail!(
            "no project initialized in {}; .tt/ not found",
            repo_root.display()
        );
    }

    let manifest_path = tt_root.join("state.toml");
    if !manifest_path.exists() {
        anyhow::bail!(
            "no project initialized in {}; managed project state not found at {}",
            repo_root.display(),
            manifest_path.display()
        );
    }

    Ok(manifest_path)
}

fn managed_project_status_for_repo(repo_root: &Path) -> Result<(bool, Option<String>)> {
    let tt_root = repo_root.join(".tt");
    if !tt_root.is_dir() {
        return Ok((false, None));
    }

    let manifest_path = tt_root.join("state.toml");
    if !manifest_path.exists() {
        return Ok((false, None));
    }

    let manifest = load_managed_project_manifest(&manifest_path)?;
    let total_roles = manifest.roles.len();
    let attached_roles = manifest
        .roles
        .values()
        .filter(|role| role.thread_id.is_some())
        .count();
    let state = if attached_roles == 0 {
        format!("scaffolded ({attached_roles}/{total_roles})")
    } else if attached_roles == total_roles {
        format!("attached ({attached_roles}/{total_roles})")
    } else {
        format!("partial ({attached_roles}/{total_roles})")
    };
    Ok((true, Some(state)))
}

fn managed_project_director_state_for_repo(
    _service: &DaemonService,
    repo_root: &Path,
) -> Result<ManagedProjectDirectorState> {
    let tt_root = repo_root.join(".tt");
    if !tt_root.is_dir() {
        return Ok(ManagedProjectDirectorState::Missing);
    }

    let manifest_path = tt_root.join("state.toml");
    if !manifest_path.exists() {
        return Ok(ManagedProjectDirectorState::Missing);
    }

    let manifest = load_managed_project_manifest(&manifest_path)?;
    let Some(director_thread_id) = manifest
        .roles
        .get("director")
        .and_then(|role| role.thread_id.as_deref())
    else {
        return Ok(ManagedProjectDirectorState::Missing);
    };
    let _ = director_thread_id;
    match manifest.startup.phase {
        ManagedProjectStartupPhase::Ready => Ok(ManagedProjectDirectorState::Ready),
        ManagedProjectStartupPhase::Blocked => Ok(ManagedProjectDirectorState::Blocked),
        ManagedProjectStartupPhase::Scaffolded
        | ManagedProjectStartupPhase::ThreadsStarted
        | ManagedProjectStartupPhase::WorkerReportsPending
        | ManagedProjectStartupPhase::DirectorAckPending => {
            Ok(ManagedProjectDirectorState::Starting)
        }
    }
}

fn default_worktree_root(repo_root: &Path, _project_slug: &str) -> PathBuf {
    repo_root.join(".tt").join("worktrees")
}

fn sanitize_project_slug(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            output.push(ch);
        } else {
            output.push('-');
        }
    }
    while output.starts_with('-') {
        output.remove(0);
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "tt-project".to_string()
    } else {
        output
    }
}

fn taskflow_round_specs(scenario_kind: &str) -> [ManagedProjectRoundSpec; 4] {
    [
        ManagedProjectRoundSpec {
            round_number: 1,
            phase: "plan",
            director_goal: "Create the first architecture and work plan for the taskflow CLI. Convert the operator seed into a concrete plan, todo list, and role assignments.",
            dev_goal: "Create the initial Rust crate structure for taskflow and implement the workflow/task model plus YAML parsing and validation scaffolding.",
            test_goal: "Design the test matrix and create fixture workflows covering valid graphs, missing dependencies, and cycles.",
            integration_goal: "Define the CLI shape, example workflow expectations, README outline, and the merge constraints for later rounds.",
            requires_landing_approval: false,
        },
        ManagedProjectRoundSpec {
            round_number: 2,
            phase: "develop",
            director_goal: "Review the first worker handoffs and dispatch the second round focused on execution order, dependency validation, and CLI cohesion.",
            dev_goal: "Implement dependency validation and the executor foundation for topological task execution.",
            test_goal: "Add automated tests for validation failures, invalid graphs, and the first execution-path fixtures.",
            integration_goal: "Wire the CLI commands so validate and run are coherent with the implementation direction and update docs/examples to match.",
            requires_landing_approval: false,
        },
        ManagedProjectRoundSpec {
            round_number: 3,
            phase: "integrate",
            director_goal: "Use the round-two reports to dispatch retry behavior, JSON reporting, and end-to-end usage alignment.",
            dev_goal: "Implement retry behavior and JSON report generation for taskflow runs.",
            test_goal: "Add retry, failure, and report-output assertions to the test suite.",
            integration_goal: "Add sample workflows, ensure CLI/report path behavior is usable, and prepare a merge-readiness summary.",
            requires_landing_approval: false,
        },
        ManagedProjectRoundSpec {
            round_number: 4,
            phase: "merge",
            director_goal: "Review the final worker reports, request landing approval, and coordinate final stabilization and landing preparation.",
            dev_goal: "Fix remaining defects surfaced by validation and keep the implementation scope narrow.",
            test_goal: "Run the full cargo test pass and produce a final validation summary with any remaining blockers.",
            integration_goal: if scenario_kind == "rust-taskflow-integration-pressure" {
                "Resolve the final integration blocker, verify merge readiness, and prepare the repo for landing after approval."
            } else {
                "Finalize README and examples, verify merge readiness, and prepare the repo for landing after approval."
            },
            requires_landing_approval: true,
        },
    ]
}

fn build_director_round_prompt(
    bootstrap: &ManagedProjectBootstrap,
    spec: &ManagedProjectRoundSpec,
    operator_seed: &str,
    prior_handoffs: &str,
    approval: Option<&ManagedProjectApprovalState>,
) -> String {
    let approval_text = approval
        .map(|approval| {
            format!(
                "Approval state: kind={} approved={} response={}\n",
                approval.approval_kind,
                approval.approved,
                approval.response.as_deref().unwrap_or("<none>")
            )
        })
        .unwrap_or_default();
    let planning_agenda = if spec.phase == "plan" {
        let open_questions = if bootstrap.plan.notes.open_questions.is_empty() {
            "  - none recorded yet".to_string()
        } else {
            bootstrap
                .plan
                .notes
                .open_questions
                .iter()
                .map(|question| format!("  - {}", question))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let validation_commands = if bootstrap
            .project_config
            .default_validation_commands
            .is_empty()
        {
            "  - none configured".to_string()
        } else {
            bootstrap
                .project_config
                .default_validation_commands
                .iter()
                .map(|command| format!("  - {}", command))
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "Planning agenda:\n- Confirm scope and explicit non-goals.\n- Resolve validation and merge criteria.\n- Capture repo-specific risks, pitfalls, and operator constraints.\n- Update the plan artifact before dispatch.\nCurrent open questions:\n{}\nValidation commands:\n{}\n",
            open_questions, validation_commands
        )
    } else {
        String::new()
    };
    format!(
        "Managed project round {} phase `{}` for project `{}`.\n{}{}\nOperator seed:\n{}\n\nPrior worker handoffs:\n{}\n\nWrite a concrete project update for this round. If this is a planning round, resolve the agenda first and record the resulting plan decisions before dispatch. Include: plan, todo, dispatch decisions, blockers, and next step.",
        spec.round_number,
        spec.phase,
        bootstrap.project.slug,
        approval_text,
        planning_agenda,
        operator_seed,
        if prior_handoffs.trim().is_empty() {
            "<none yet>"
        } else {
            prior_handoffs
        }
    )
}

fn build_worker_round_prompt(
    bootstrap: &ManagedProjectBootstrap,
    spec: &ManagedProjectRoundSpec,
    role: &ManagedProjectRoleBootstrap,
    director_summary: &str,
    approval: Option<&ManagedProjectApprovalState>,
) -> String {
    let role_goal = match role.role {
        ThreadRole::Develop => spec.dev_goal,
        ThreadRole::Test => spec.test_goal,
        ThreadRole::Integrate => spec.integration_goal,
        _ => spec.director_goal,
    };
    let approval_text = approval
        .map(|value| {
            format!(
                "\nApproval state: kind={} approved={} response={}",
                value.approval_kind,
                value.approved,
                value.response.as_deref().unwrap_or("<none>")
            )
        })
        .unwrap_or_default();
    format!(
        "Director dispatch for project `{}` round {} phase `{}`.\nRole: {}\nAssigned goal: {}\nDirector summary:\n{}\n{}\n\nStay in your assigned worktree.\nReturn exactly one JSON object with this shape:\n{{\n  \"status\": \"blocked|needs-review|complete\",\n  \"changed_files\": [\"path\"],\n  \"tests_run\": [\"command\"],\n  \"blockers\": [\"description\"],\n  \"next_step\": \"one concrete next action\"\n}}",
        bootstrap.project.slug,
        spec.round_number,
        spec.phase,
        role_slug(role.role),
        role_goal,
        director_summary,
        approval_text
    )
}

fn build_worker_startup_prompt(
    bootstrap: &ManagedProjectBootstrap,
    role: &ManagedProjectRoleBootstrap,
) -> String {
    let cwd = role
        .worktree_path
        .as_deref()
        .unwrap_or(bootstrap.repo_root.as_path())
        .display()
        .to_string();
    format!(
        "TT managed project startup handshake for project `{}`.\n\
Role: {}\n\
Repository root: {}\n\
Assigned cwd: {}\n\
Assigned worktree: {}\n\
Assigned branch: {}\n\
\n\
Do not change code. This turn is only a startup readiness report to the director.\n\
Confirm that you loaded `.tt/contract.md` and `.tt/plan.toml`, that you are in the correct cwd/worktree/branch, and whether you are ready or blocked.\n\
Prefer exactly one JSON object with this shape. If you make a typo or cannot format JSON cleanly, a short plain-text readiness note is acceptable and TT will infer the status.\n\
Return exactly one JSON object with this shape:\n\
{{\n\
  \"role\": \"{}\",\n\
  \"cwd\": \"{}\",\n\
  \"worktree\": \"{}\",\n\
  \"branch\": \"{}\",\n\
  \"contract_loaded\": true,\n\
  \"plan_loaded\": true,\n\
  \"status\": \"ready|blocked\",\n\
  \"blocker\": null,\n\
  \"summary\": \"one concise readiness summary for the director\"\n\
}}",
        bootstrap.project.slug,
        role_slug(role.role),
        bootstrap.repo_root.display(),
        cwd,
        role.worktree_path
            .as_deref()
            .map(Path::display)
            .map(|value| value.to_string())
            .unwrap_or_else(|| cwd.clone()),
        role.branch_name.as_deref().unwrap_or("<none>"),
        role_slug(role.role),
        cwd,
        role.worktree_path
            .as_deref()
            .map(Path::display)
            .map(|value| value.to_string())
            .unwrap_or_else(|| cwd.clone()),
        role.branch_name.as_deref().unwrap_or("<none>")
    )
}

fn build_director_startup_prompt(
    bootstrap: &ManagedProjectBootstrap,
    reports: &[ManagedProjectBootstrapWorkerReport],
) -> String {
    let mut report_lines = Vec::new();
    for report in reports {
        report_lines.push(format!(
            "- role={} cwd={} worktree={} branch={} status={} blocker={} summary={}",
            report.role,
            report.cwd,
            report.worktree,
            report.branch,
            report.status,
            report.blocker.as_deref().unwrap_or("<none>"),
            report.summary
        ));
    }
    format!(
        "TT managed project startup handshake for project `{}`.\n\
All worker startup reports have been collected by TT.\n\
Validate the roster for `dev`, `test`, and `integration`, confirm whether the project is ready for operator handoff, and report any missing or blocked workers.\n\
Prefer exactly one JSON object with the fields below. If you make a typo or cannot format JSON cleanly, a short plain-text ack is acceptable and TT will infer the status.\n\
\n\
Worker reports:\n{}\n\
\n\
Return exactly one JSON object with this shape:\n\
{{\n\
  \"status\": \"ready|blocked\",\n\
  \"received_roles\": [\"dev\", \"test\", \"integration\"],\n\
  \"missing_roles\": [],\n\
  \"summary\": \"one concise operator-facing startup acknowledgement\"\n\
}}",
        bootstrap.project.slug,
        if report_lines.is_empty() {
            "<none>".to_string()
        } else {
            report_lines.join("\n")
        }
    )
}

fn parse_bootstrap_worker_report(
    summary: &str,
    extraction: &WorkerHandoffExtraction,
    role_bootstrap: &ManagedProjectRoleBootstrap,
    repo_root: &Path,
) -> Result<ManagedProjectBootstrapWorkerReport> {
    if let Some(candidate) = extraction
        .raw_text
        .as_deref()
        .and_then(normalize_worker_handoff_json_candidate)
        .or_else(|| normalize_worker_handoff_json_candidate(summary))
    {
        if let Ok(report) = serde_json::from_str::<ManagedProjectBootstrapWorkerReport>(&candidate)
        {
            return Ok(report);
        }
    }
    Ok(parse_plaintext_bootstrap_worker_report(
        summary,
        extraction.raw_text.as_deref().unwrap_or(summary),
        role_bootstrap,
        repo_root,
    ))
}

fn parse_bootstrap_director_ack(
    summary: &str,
    extraction: &WorkerHandoffExtraction,
    reports: &[ManagedProjectBootstrapWorkerReport],
) -> Result<ManagedProjectBootstrapDirectorAckPayload> {
    if let Some(candidate) = extraction
        .raw_text
        .as_deref()
        .and_then(normalize_worker_handoff_json_candidate)
        .or_else(|| normalize_worker_handoff_json_candidate(summary))
    {
        if let Ok(ack) = serde_json::from_str::<ManagedProjectBootstrapDirectorAckPayload>(&candidate)
        {
            return Ok(ack);
        }
    }
    Ok(parse_plaintext_bootstrap_director_ack(
        summary,
        extraction.raw_text.as_deref().unwrap_or(summary),
        reports,
    ))
}

fn parse_plaintext_bootstrap_worker_report(
    summary: &str,
    raw_text: &str,
    role_bootstrap: &ManagedProjectRoleBootstrap,
    repo_root: &Path,
) -> ManagedProjectBootstrapWorkerReport {
    let text = if raw_text.trim().is_empty() {
        summary.trim()
    } else {
        raw_text.trim()
    };
    let status = if is_plaintext_blocked_report(text) {
        "blocked"
    } else {
        "ready"
    };
    ManagedProjectBootstrapWorkerReport {
        role: role_slug(role_bootstrap.role).to_string(),
        cwd: role_bootstrap
            .worktree_path
            .as_deref()
            .unwrap_or(repo_root)
            .display()
            .to_string(),
        worktree: role_bootstrap
            .worktree_path
            .as_deref()
            .unwrap_or(repo_root)
            .display()
            .to_string(),
        branch: role_bootstrap
            .branch_name
            .clone()
            .unwrap_or_else(|| role_slug(role_bootstrap.role).to_string()),
        contract_loaded: text.contains("contract") || !text.to_ascii_lowercase().contains("missing"),
        plan_loaded: text.contains("plan") || !text.to_ascii_lowercase().contains("missing"),
        status: status.to_string(),
        blocker: if status == "blocked" {
            Some(extract_plaintext_blocker_summary(text))
        } else {
            None
        },
        summary: summarize_plaintext_startup_response(text),
    }
}

fn parse_plaintext_bootstrap_director_ack(
    summary: &str,
    raw_text: &str,
    reports: &[ManagedProjectBootstrapWorkerReport],
) -> ManagedProjectBootstrapDirectorAckPayload {
    let text = if raw_text.trim().is_empty() {
        summary.trim()
    } else {
        raw_text.trim()
    };
    let ready = !is_plaintext_blocked_report(text);
    let received_roles = reports.iter().map(|report| report.role.clone()).collect();
    let missing_roles = if ready {
        Vec::new()
    } else {
        reports
            .iter()
            .filter(|report| report.status != "ready")
            .map(|report| report.role.clone())
            .collect()
    };
    ManagedProjectBootstrapDirectorAckPayload {
        status: if ready { "ready".to_string() } else { "blocked".to_string() },
        received_roles,
        missing_roles,
        summary: summarize_plaintext_startup_response(text),
    }
}

fn summarize_plaintext_startup_response(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "ready".to_string();
    }
    trimmed
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

fn is_plaintext_blocked_report(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("blocked")
        || normalized.contains("not ready")
        || normalized.contains("cannot")
        || normalized.contains("can't")
        || normalized.contains("error")
        || normalized.contains("issue")
        || normalized.contains("missing")
        || normalized.contains("fail")
}

fn extract_plaintext_blocker_summary(text: &str) -> String {
    let trimmed = text.trim();
    for marker in ["blocker:", "because", "due to", "error:"] {
        if let Some(index) = trimmed.to_ascii_lowercase().find(marker) {
            let value = trimmed[index + marker.len()..].trim();
            if !value.is_empty() {
                return value.lines().next().unwrap_or(value).trim().to_string();
            }
        }
    }
    summarize_plaintext_startup_response(trimmed)
}

fn summarize_turn_items(items: &[protocol::ThreadItem]) -> String {
    let mut chunks = Vec::new();
    for item in items {
        match item {
            protocol::ThreadItem::AgentMessage { text, .. }
            | protocol::ThreadItem::Plan { text, .. } => {
                if !text.trim().is_empty() {
                    chunks.push(text.trim().to_string());
                }
            }
            _ => {}
        }
    }
    if chunks.is_empty() {
        "<no agent summary captured>".to_string()
    } else {
        chunks.join("\n\n")
    }
}

fn worker_handoff_text_candidates(items: &[protocol::ThreadItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match item {
            protocol::ThreadItem::AgentMessage { text, .. } => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            _ => None,
        })
        .collect()
}

fn normalize_worker_handoff_json_candidate(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with("```") {
        let mut lines = trimmed.lines();
        let header = lines.next()?;
        if header.starts_with("```") {
            let mut body = Vec::new();
            for line in lines {
                if line.trim_start().starts_with("```") {
                    break;
                }
                body.push(line);
            }
            let fenced = body.join("\n").trim().to_string();
            if !fenced.is_empty() {
                return Some(fenced);
            }
        }
    }

    extract_first_json_object(trimmed).or_else(|| Some(trimmed.to_string()))
}

fn extract_first_json_object(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().enumerate() {
        let ch = *byte as char;
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if start.is_none() {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    if let Some(start_index) = start {
                        return Some(text[start_index..=index].to_string());
                    }
                }
            }
            _ => {}
        }
    }

    None
}

fn extract_worker_handoff(items: &[protocol::ThreadItem]) -> WorkerHandoffExtraction {
    let raw_candidates = worker_handoff_text_candidates(items);
    if raw_candidates.is_empty() {
        return WorkerHandoffExtraction {
            handoff: None,
            raw_text: None,
            source: WorkerHandoffSource::SeededFallback,
            parse_error: Some("no agent message found in completed turn".to_string()),
        };
    }

    let mut last_raw = None;
    let mut last_error = None;
    for raw in raw_candidates.iter().rev() {
        let Some(candidate) = normalize_worker_handoff_json_candidate(raw) else {
            continue;
        };
        last_raw = Some(raw.clone());
        match serde_json::from_str::<StructuredWorkerHandoff>(&candidate) {
            Ok(handoff) => match validate_structured_worker_handoff(&handoff) {
                Ok(()) => {
                    return WorkerHandoffExtraction {
                        handoff: Some(handoff),
                        raw_text: Some(raw.clone()),
                        source: WorkerHandoffSource::Extracted,
                        parse_error: None,
                    };
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                }
            },
            Err(error) => {
                last_error = Some(format!("parse structured worker handoff JSON: {error}"));
            }
        }
    }

    WorkerHandoffExtraction {
        handoff: None,
        raw_text: last_raw,
        source: WorkerHandoffSource::SeededFallback,
        parse_error: last_error
            .or_else(|| Some("no parseable worker handoff JSON found".to_string())),
    }
}

fn validate_structured_worker_handoff(handoff: &StructuredWorkerHandoff) -> Result<()> {
    match handoff.status.as_str() {
        "blocked" | "needs-review" | "complete" => {}
        other => anyhow::bail!("invalid worker handoff status `{other}`"),
    }
    if handoff.next_step.trim().is_empty() {
        anyhow::bail!("worker handoff next_step must not be empty");
    }
    Ok(())
}

fn worker_handoff_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["status", "changed_files", "tests_run", "blockers", "next_step"],
        "properties": {
            "status": {
                "type": "string",
                "enum": ["blocked", "needs-review", "complete"]
            },
            "changed_files": {
                "type": "array",
                "items": {"type": "string"}
            },
            "tests_run": {
                "type": "array",
                "items": {"type": "string"}
            },
            "blockers": {
                "type": "array",
                "items": {"type": "string"}
            },
            "next_step": {"type": "string"}
        }
    })
}

fn seeded_worker_handoff(
    scenario_kind: &str,
    spec: &ManagedProjectRoundSpec,
    role: ThreadRole,
) -> StructuredWorkerHandoff {
    match (spec.round_number, role) {
        (1, ThreadRole::Develop) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["Cargo.toml", "src/lib.rs", "src/main.rs"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo fmt --check"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Implement dependency validation and executor behavior in the next round."
                .to_string(),
        },
        (1, ThreadRole::Test) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["tests/fixtures/valid-workflow.yml", "tests/taskflow_matrix.rs"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo test --test taskflow_matrix -- --list"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Add validation fixtures for cycles and missing dependencies."
                .to_string(),
        },
        (1, ThreadRole::Integrate) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["README.md", "examples/valid-workflow.yml"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo check"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Wire CLI commands to the workflow parser and align docs with the role plan."
                .to_string(),
        },
        (2, ThreadRole::Develop) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["src/lib.rs", "src/executor.rs"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo test parser::tests executor::tests"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Add retry semantics and JSON reporting."
                .to_string(),
        },
        (2, ThreadRole::Test) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["tests/fixtures/cycle.yml", "tests/fixtures/missing-dependency.yml"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo test validation::tests"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Validate retry and reporting behavior once executor changes land."
                .to_string(),
        },
        (2, ThreadRole::Integrate) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["src/main.rs", "README.md"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo check --bin taskflow"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Prepare sample workflows and align the report flag with executor output."
                .to_string(),
        },
        (3, ThreadRole::Develop) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["src/report.rs", "src/executor.rs"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo test report::tests retry::tests"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Address any defects found by the expanded integration tests."
                .to_string(),
        },
        (3, ThreadRole::Test) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["tests/taskflow_run.rs", "tests/taskflow_report.rs"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo test --test taskflow_run --test taskflow_report"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Run a final full-suite pass before landing."
                .to_string(),
        },
        (3, ThreadRole::Integrate) if scenario_kind == "rust-taskflow-integration-pressure" => {
            StructuredWorkerHandoff {
                status: "blocked".to_string(),
                changed_files: vec!["README.md", "examples/retry-workflow.yml", "src/main.rs"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                tests_run: vec!["cargo run -- validate examples/retry-workflow.yml"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                blockers: vec![
                    "merge-readiness is blocked until the report output path and retry example stay aligned across docs and CLI."
                        .to_string(),
                ],
                next_step:
                    "Resolve the integration mismatch, then return a merge-ready landing summary."
                        .to_string(),
            }
        }
        (3, ThreadRole::Integrate) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["README.md", "examples/retry-workflow.yml", "src/main.rs"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo run -- validate examples/retry-workflow.yml"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Prepare final landing notes and merge-readiness summary."
                .to_string(),
        },
        (4, ThreadRole::Develop) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["src/lib.rs", "src/main.rs", "src/report.rs"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo test"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Wait for final integration landing."
                .to_string(),
        },
        (4, ThreadRole::Test) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["tests/taskflow_run.rs", "tests/taskflow_matrix.rs"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo test", "cargo test --test taskflow_run"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Report green validation to the director for landing approval."
                .to_string(),
        },
        (4, ThreadRole::Integrate) => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: vec!["README.md", "examples/valid-workflow.yml", "examples/retry-workflow.yml"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            tests_run: vec!["cargo test", "cargo run -- run examples/valid-workflow.yml --report target/report.json"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            blockers: Vec::new(),
            next_step: "Land the branch set after operator approval."
                .to_string(),
        },
        _ => StructuredWorkerHandoff {
            status: "complete".to_string(),
            changed_files: Vec::new(),
            tests_run: Vec::new(),
            blockers: Vec::new(),
            next_step: format!(
                "Director should review the {} role output for round {}.",
                role_slug(role),
                spec.round_number
            ),
        },
    }
}

fn render_round_handoffs(round: &ManagedProjectRoundState) -> String {
    let mut output = format!("Round {} phase `{}`\n", round.round_number, round.phase);
    if let Some(summary) = round.director_summary.as_ref() {
        output.push_str("Director summary:\n");
        output.push_str(summary);
        output.push_str("\n");
    }
    for (role, handoff) in &round.role_handoffs {
        output.push_str(&format!(
            "\n{} handoff:\n{}\n",
            role,
            handoff
                .handoff_summary
                .as_deref()
                .unwrap_or("<missing handoff>")
        ));
    }
    output
}

fn write_scenario_artifact(
    bootstrap: &ManagedProjectBootstrap,
    scenario_id: &str,
    round_number: usize,
    filename: &str,
    contents: &str,
) -> Result<()> {
    let path = bootstrap
        .repo_root
        .join(".tt")
        .join("scenarios")
        .join(scenario_id)
        .join(format!("round-{:02}", round_number))
        .join(filename);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

fn write_scenario_progress_event(
    bootstrap: &ManagedProjectBootstrap,
    scenario_id: &str,
    event: &ManagedProjectProgressEvent,
) -> Result<()> {
    let path = bootstrap
        .repo_root
        .join(".tt")
        .join("scenarios")
        .join(scenario_id)
        .join("progress.jsonl");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(event)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    use std::io::Write;
    writeln!(file, "{}", line)?;
    Ok(())
}

pub fn managed_project_events_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".tt").join(TT_EVENTS_FILE_NAME)
}

fn append_managed_project_event(repo_root: &Path, event: &ManagedProjectEvent) -> Result<()> {
    let path = managed_project_events_path(repo_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(event)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

pub fn load_managed_project_events(
    repo_root: &Path,
    limit: Option<usize>,
) -> Result<Vec<ManagedProjectEvent>> {
    let path = managed_project_events_path(repo_root);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut events = Vec::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        events.push(
            serde_json::from_str(trimmed)
                .with_context(|| format!("parse managed-project event from {}", path.display()))?,
        );
    }
    if let Some(limit) = limit
        && events.len() > limit
    {
        let start = events.len() - limit;
        Ok(events.split_off(start))
    } else {
        Ok(events)
    }
}

fn write_role_attempt_artifact(
    bootstrap: &ManagedProjectBootstrap,
    scenario_id: &str,
    round_number: usize,
    role: ThreadRole,
    attempt: &ManagedProjectTurnAttempt,
) -> Result<()> {
    write_scenario_artifact(
        bootstrap,
        scenario_id,
        round_number,
        &format!(
            "{}-attempt-{:02}.txt",
            role_slug(role),
            attempt.attempt_number
        ),
        &render_single_attempt(attempt),
    )
}

fn render_single_attempt(attempt: &ManagedProjectTurnAttempt) -> String {
    format!(
        "attempt: {}\nmodel: {}\nmodel_reasoning_effort: {}\nthread_id: {}\nturn_id: {}\nstatus: {}\nfailure_summary: {}\n",
        attempt.attempt_number,
        attempt.model,
        attempt.reasoning_effort,
        attempt.thread_id,
        attempt.turn_id.as_deref().unwrap_or("<none>"),
        attempt.status.as_deref().unwrap_or("<unknown>"),
        attempt.failure_summary.as_deref().unwrap_or("<none>")
    )
}

fn render_attempt_log(attempts: &[ManagedProjectTurnAttempt]) -> String {
    attempts
        .iter()
        .map(render_single_attempt)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_watchdog_state(state: &ManagedProjectWatchdogState) -> String {
    format!(
        "state: {}\nelapsed_seconds: {}\nlast_signal: {}\nlast_observed_at: {}\nlast_progress_at: {}\nrole: {}\nround: {}\nturn_id: {}\nsilence_seconds: {}\nturn_status: {}\nturn_items: {}\napp_server_log_modified_at: {}\napp_server_log_size: {}\nnote: {}\n",
        state.state,
        state.elapsed_seconds,
        state.last_signal.as_deref().unwrap_or("<none>"),
        state
            .last_observed_at
            .map(|value| value.to_rfc3339())
            .unwrap_or_else(|| "<none>".to_string()),
        state
            .last_progress_at
            .map(|value| value.to_rfc3339())
            .unwrap_or_else(|| "<none>".to_string()),
        state.role.as_deref().unwrap_or("<none>"),
        state
            .round
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        state.turn_id.as_deref().unwrap_or("<none>"),
        state.silence_seconds,
        state.turn_status.as_deref().unwrap_or("<none>"),
        state.turn_items,
        state
            .app_server_log_modified_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        state
            .app_server_log_size
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        state.note.as_deref().unwrap_or("<none>")
    )
}

fn managed_project_startup_phase_slug(phase: ManagedProjectStartupPhase) -> &'static str {
    match phase {
        ManagedProjectStartupPhase::Scaffolded => "scaffolded",
        ManagedProjectStartupPhase::ThreadsStarted => "threads_started",
        ManagedProjectStartupPhase::WorkerReportsPending => "worker_reports_pending",
        ManagedProjectStartupPhase::DirectorAckPending => "director_ack_pending",
        ManagedProjectStartupPhase::Ready => "ready",
        ManagedProjectStartupPhase::Blocked => "blocked",
    }
}

fn format_error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\ncaused by: ")
}

fn format_turn_error(error: &protocol::TurnError) -> String {
    let details = error.additional_details.as_deref().unwrap_or("").trim();
    if details.is_empty() {
        error.message.clone()
    } else {
        format!("{}\n{}", error.message, details)
    }
}

fn render_turn_failure(
    turn: &protocol::Turn,
    role_name: &str,
    model: &str,
    reasoning_effort: &str,
) -> String {
    let error = turn
        .error
        .as_ref()
        .map(format_turn_error)
        .unwrap_or_else(|| "turn failed without an error payload".to_string());
    format!(
        "turn `{}` for role `{}` failed (model={}, reasoning_effort={}): {}",
        turn.id, role_name, model, reasoning_effort, error
    )
}

fn is_retryable_live_turn_failure(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("responses_websocket")
        && normalized.contains("500 internal server error")
        && normalized.contains("wss://api.openai.com/v1/responses")
        || normalized.contains("failed to load rollout")
            && normalized.contains("empty session file")
}

fn live_turn_backoff_delay(attempt_number: usize) -> std::time::Duration {
    match attempt_number {
        1 => std::time::Duration::from_secs(1),
        _ => std::time::Duration::from_secs(2),
    }
}

fn extract_turn_id_from_error_message(message: &str) -> Option<String> {
    let marker = "turn `";
    let start = message.find(marker)? + marker.len();
    let rest = &message[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

fn workspace_status_from_git(inspection: &tt_git::GitRepositoryInspection) -> WorkspaceStatus {
    if inspection.dirty {
        WorkspaceStatus::Dirty
    } else if inspection.ahead_by.unwrap_or(0) > 0 && inspection.behind_by.unwrap_or(0) > 0 {
        WorkspaceStatus::Conflicted
    } else if inspection.behind_by.unwrap_or(0) > 0 {
        WorkspaceStatus::Behind
    } else if inspection.ahead_by.unwrap_or(0) > 0 {
        WorkspaceStatus::Ahead
    } else {
        WorkspaceStatus::Ready
    }
}

fn sanitize_branch_component(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            output.push(ch);
        } else {
            output.push('-');
        }
    }
    while output.starts_with('-') {
        output.remove(0);
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "tt".to_string()
    } else {
        output
    }
}

fn remove_if_exists(path: PathBuf) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let metadata = fs::metadata(&path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(&path)?;
    } else {
        fs::remove_file(&path)?;
    }
    Ok(1)
}

pub fn socket_path_for(cwd: impl AsRef<Path>) -> PathBuf {
    cwd.as_ref().join(".tt").join(TT_DAEMON_SOCKET_NAME)
}

pub fn request_for_cwd(cwd: impl AsRef<Path>, request: DaemonRequest) -> Result<DaemonResponse> {
    let cwd = cwd.as_ref();
    if let DaemonRequest::Doctor { cwd, check_listen } = request.clone() {
        let socket_path = socket_path_for(cwd.as_path());
        if let Ok(client) = DaemonClient::connect(&socket_path)
            && let Ok(response) = client.request(DaemonRequest::Doctor {
                cwd: cwd.clone(),
                check_listen,
            })
        {
            return Ok(response);
        }
        return Ok(DaemonResponse::Doctor(doctor_for_cwd(cwd, check_listen)));
    }
    if let DaemonRequest::DoctorCodex { cwd, check_listen } = request.clone() {
        let socket_path = socket_path_for(cwd.as_path());
        if let Ok(client) = DaemonClient::connect(&socket_path)
            && let Ok(response) = client.request(DaemonRequest::DoctorCodex {
                cwd: cwd.clone(),
                check_listen,
            })
        {
            return Ok(response);
        }
        return Ok(DaemonResponse::CodexDoctor(codex_doctor_for_cwd(
            cwd,
            check_listen,
        )));
    }
    let socket_path = socket_path_for(cwd);
    if let Ok(client) = DaemonClient::connect(&socket_path) {
        if let Ok(response) = client.request(request.clone()) {
            return Ok(response);
        }
    }
    if should_auto_spawn_daemon_for_request(&request) {
        if let Ok(client) = spawn_repo_daemon_and_connect(cwd) {
            if let Ok(response) = client.request(request.clone()) {
                return Ok(response);
            }
        }
    }
    DaemonRuntime::open(cwd)?.request(request)
}

fn should_auto_spawn_daemon_for_request(request: &DaemonRequest) -> bool {
    !matches!(
        request,
        DaemonRequest::Doctor { .. } | DaemonRequest::DoctorCodex { .. }
    )
}

fn spawn_repo_daemon_and_connect(cwd: &Path) -> Result<DaemonClient> {
    let socket_path = socket_path_for(cwd);
    if let Ok(client) = DaemonClient::connect(&socket_path) {
        return Ok(client);
    }
    let daemon_bin = resolve_daemon_binary()?;
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }
    Command::new(&daemon_bin)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn TT daemon `{}`", daemon_bin.display()))?;

    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(DAEMON_SPAWN_WAIT_MS);
    loop {
        if let Ok(client) = DaemonClient::connect(&socket_path) {
            return Ok(client);
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for TT daemon socket {} after spawning {}",
                socket_path.display(),
                daemon_bin.display()
            );
        }
        thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn resolve_daemon_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("TT_DAEMON_BIN") {
        return Ok(PathBuf::from(path));
    }
    let current_exe =
        std::env::current_exe().context("resolve current executable for TT daemon spawn")?;
    if let Some(file_name) = current_exe.file_name().and_then(|value| value.to_str()) {
        if file_name.contains("tt-cli") {
            return Ok(current_exe.with_file_name(file_name.replace("tt-cli", "tt-daemon")));
        }
        if file_name == "tt" {
            return Ok(current_exe.with_file_name("tt-daemon"));
        }
    }
    Ok(current_exe.with_file_name("tt-daemon"))
}

fn send_request(stream: &mut UnixStream, request: &DaemonRequest) -> Result<()> {
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()?;
    Ok(())
}

fn recv_response(stream: &mut UnixStream) -> Result<DaemonResponse> {
    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    let bytes = reader.read_line(&mut response_line)?;
    if bytes == 0 {
        anyhow::bail!("daemon closed the socket before sending a response");
    }
    let response: Result<DaemonResponse, String> = serde_json::from_str(&response_line)?;
    match response {
        Ok(value) => Ok(value),
        Err(message) => anyhow::bail!(message),
    }
}

fn handle_connection(runtime: &DaemonRuntime, mut stream: UnixStream) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    let bytes = reader.read_line(&mut request_line)?;
    if bytes == 0 {
        anyhow::bail!("client closed the socket before sending a request");
    }
    let request: DaemonRequest = serde_json::from_str(&request_line)?;
    let response = match runtime.request(request) {
        Ok(response) => Ok(response),
        Err(error) => Err(error.to_string()),
    };
    let mut line = serde_json::to_vec(&response)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()?;
    Ok(())
}

fn doctor_for_cwd(cwd: impl AsRef<Path>, check_listen: bool) -> DoctorReport {
    let cwd = cwd.as_ref().to_path_buf();
    let codex_doctor = codex_doctor_for_cwd(&cwd, check_listen);
    DoctorReport {
        cwd: cwd.clone(),
        tt_cli_generation: "v2".to_string(),
        daemon_api_version: TT_DAEMON_API_VERSION.to_string(),
        tt_project_root: cwd.join(".tt").is_dir().then_some(cwd.clone()),
        codex_project_root: cwd.join(".codex").is_dir().then_some(cwd.clone()),
        daemon_socket_path: socket_path_for(&cwd),
        codex_auth_json: Some(managed_project_auth_json_path(&cwd)),
        codex_auth_present: Some(managed_project_auth_is_present(&cwd)),
        codex_contract_ok: codex_doctor.contract_ok,
        codex_error: codex_doctor.error.clone(),
        codex_listen_url: codex_doctor.configured_listen_url.clone(),
        codex_listen_reachable: codex_doctor.listen_reachable,
        codex_listen_error: codex_doctor.listen_error.clone(),
    }
}

fn codex_app_server_summary_for_cwd(cwd: impl AsRef<Path>) -> CodexAppServerSummary {
    let cwd = cwd.as_ref().to_path_buf();
    let _ = tt_codex::load_repo_settings_env(&cwd);
    let daemon_socket_path = socket_path_for(&cwd);
    let daemon_socket_exists = daemon_socket_path.exists();
    let daemon_socket_reachable = DaemonClient::connect(&daemon_socket_path).is_ok();
    let configured_listen_url = configured_app_server_listen_url();
    let (listen_reachable, listen_error) = match check_listen_reachability(&configured_listen_url) {
        Ok(reachable) => (reachable, None),
        Err(error) => (false, Some(format!("{error:#}"))),
    };
    let source = if repo_env_var_os("CODEX_APP_SERVER_LISTEN_URL").is_some() {
        "CODEX_APP_SERVER_LISTEN_URL"
    } else if repo_env_var_os("TT_APP_SERVER_LISTEN_URL").is_some() {
        "TT_APP_SERVER_LISTEN_URL"
    } else {
        "default"
    }
    .to_string();
    let note = if source == "default" {
        Some(
            "listen URL came from the default runtime fallback and is not repo-owned metadata"
                .to_string(),
        )
    } else {
        None
    };
    CodexAppServerSummary {
        repo_root: cwd,
        daemon_socket_path,
        daemon_socket_exists,
        daemon_socket_reachable,
        configured_listen_url,
        listen_reachable,
        listen_error,
        source,
        note,
    }
}

fn codex_doctor_for_cwd(cwd: impl AsRef<Path>, check_listen: bool) -> CodexDoctorReport {
    let _ = tt_codex::load_repo_settings_env(cwd.as_ref());
    let configured_listen_url = configured_app_server_listen_url();
    let (listen_reachable, listen_error) = if check_listen {
        match check_listen_reachability(&configured_listen_url) {
            Ok(reachable) => (Some(reachable), None),
            Err(error) => (Some(false), Some(format!("{error:#}"))),
        }
    } else {
        (None, None)
    };
    match validate_runtime_contract(cwd.as_ref()) {
        Ok(contract) => CodexDoctorReport {
            contract_ok: true,
            codex_version: read_binary_version(contract.codex_bin()),
            app_server_version: read_binary_version(contract.app_server_bin()),
            codex_bin: Some(contract.codex_bin().to_path_buf()),
            app_server_bin: Some(contract.app_server_bin().to_path_buf()),
            auth_json: Some(contract.auth_json().to_path_buf()),
            auth_present: Some(managed_project_auth_is_present(cwd.as_ref())),
            codex_home: CodexHome::discover_in(cwd)
                .ok()
                .map(|home| home.root().to_path_buf()),
            configured_listen_url,
            listen_reachable,
            listen_error,
            error: None,
        },
        Err(error) => CodexDoctorReport {
            contract_ok: false,
            codex_bin: None,
            app_server_bin: None,
            auth_json: Some(managed_project_auth_json_path(cwd.as_ref())),
            auth_present: Some(managed_project_auth_is_present(cwd.as_ref())),
            codex_version: None,
            app_server_version: None,
            codex_home: None,
            configured_listen_url,
            listen_reachable,
            listen_error,
            error: Some(format!("{error:#}")),
        },
    }
}

fn check_listen_reachability(listen_url: &str) -> Result<bool> {
    let url = Url::parse(listen_url)
        .with_context(|| format!("invalid configured listen URL `{listen_url}`"))?;
    let host = url
        .host_str()
        .with_context(|| format!("listen URL `{listen_url}` has no host"))?;
    let port = url
        .port_or_known_default()
        .with_context(|| format!("listen URL `{listen_url}` has no port"))?;
    let timeout = std::time::Duration::from_millis(DOCTOR_LISTEN_TIMEOUT_MS);
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("resolve `{host}:{port}` for `{listen_url}`"))?
        .collect();
    if addrs.is_empty() {
        anyhow::bail!("no socket addresses resolved for `{listen_url}`");
    }
    for addr in addrs {
        if TcpStream::connect_timeout(&addr, timeout).is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_binary_version(path: &Path) -> Option<String> {
    let output = Command::new(path).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::net::TcpListener;
    use std::process::Command;
    use std::time::Duration;
    use tempfile::tempdir;
    use tt_domain::{
        ProjectStatus, ThreadBindingStatus, ThreadRole, WorkUnitStatus, WorkspaceCleanupPolicy,
        WorkspaceStatus, WorkspaceStrategy, WorkspaceSyncPolicy,
    };

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 8, 12, 0, 0).unwrap()
    }

    #[test]
    fn check_listen_reachability_reports_true_for_open_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("ws://127.0.0.1:{}", addr.port());
        assert!(check_listen_reachability(&url).expect("reachability"));
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {:?} failed: {status}", args);
    }

    fn setup_repo() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "tt-daemon-v2-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let repo = root.join("repo");
        let worktree = root.join("worktree");
        std::fs::create_dir_all(&repo).expect("create repo");
        run_git(&repo, &["init", "-b", "main"]);
        run_git(&repo, &["config", "user.name", "TT Test"]);
        run_git(&repo, &["config", "user.email", "tt@example.com"]);
        std::fs::write(repo.join("README.md"), "tt\n").expect("write file");
        std::fs::create_dir_all(repo.join(".tt")).expect("create tt dir");
        std::fs::write(
            repo.join(".tt/settings.env"),
            "TT_CODEX_BIN=~/.local/bin/codex\nTT_CODEX_APP_SERVER_BIN=~/openai/codex/codex-rs/target/debug/codex-app-server\n",
        )
        .expect("write settings env");
        run_git(&repo, &["add", "README.md"]);
        run_git(&repo, &["commit", "-m", "initial"]);
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "tt/tt-1",
                worktree.to_str().expect("worktree"),
                "HEAD",
            ],
        );
        (repo, worktree)
    }

    #[test]
    fn status_reflects_store_counts() {
        let dir = tempdir().expect("tempdir");
        let store = OverlayStore::open_in_dir(dir.path()).expect("open store");
        let service = DaemonService::new(store);

        service
            .store()
            .upsert_project(&Project {
                id: "p1".into(),
                slug: "alpha".into(),
                title: "Alpha".into(),
                objective: "Ship".into(),
                status: ProjectStatus::Active,
                created_at: ts(),
                updated_at: ts(),
            })
            .expect("upsert project");
        service
            .store()
            .upsert_work_unit(&WorkUnit {
                id: "wu1".into(),
                project_id: "p1".into(),
                slug: Some("chunk".into()),
                title: "Chunk".into(),
                task: "Do the work".into(),
                status: WorkUnitStatus::Ready,
                created_at: ts(),
                updated_at: ts(),
            })
            .expect("upsert work unit");
        service
            .store()
            .upsert_thread_binding(&ThreadBinding {
                codex_thread_id: "thread-1".into(),
                work_unit_id: Some("wu1".into()),
                role: ThreadRole::Develop,
                status: ThreadBindingStatus::Bound,
                notes: None,
                created_at: ts(),
                updated_at: ts(),
            })
            .expect("upsert binding");
        service
            .store()
            .upsert_workspace_binding(&WorkspaceBinding {
                id: "ws1".into(),
                codex_thread_id: "thread-1".into(),
                repo_root: "/repo".into(),
                worktree_path: None,
                branch_name: None,
                base_ref: None,
                base_commit: None,
                landing_target: None,
                strategy: WorkspaceStrategy::DedicatedWorktree,
                sync_policy: WorkspaceSyncPolicy::RebaseBeforeLanding,
                cleanup_policy: WorkspaceCleanupPolicy::PruneAfterLanding,
                status: WorkspaceStatus::Ready,
                created_at: ts(),
                updated_at: ts(),
            })
            .expect("upsert workspace");

        let status = service.status(dir.path()).expect("status");
        let summary = service.dashboard_summary().expect("summary");

        assert_eq!(status.project_count, 1);
        assert_eq!(status.work_unit_count, 1);
        assert_eq!(status.bound_thread_count, 1);
        assert_eq!(status.ready_workspace_count, 1);
        assert_eq!(summary.bound_threads, 1);
    }

    #[test]
    fn request_and_response_round_trip() {
        let request = DaemonRequest::DeleteProject {
            id_or_slug: "alpha".to_string(),
        };
        let encoded = serde_json::to_string(&request).expect("serialize request");
        let decoded: DaemonRequest = serde_json::from_str(&encoded).expect("deserialize request");
        assert_eq!(request, decoded);
    }

    #[test]
    fn director_request_round_trips() {
        let request = DaemonRequest::DirectManagedProject {
            cwd: PathBuf::from("/repo"),
            title: Some("Alpha".into()),
            objective: Some("Ship".into()),
            base_branch: Some("main".into()),
            worktree_root: None,
            director_model: Some("director-model".into()),
            dev_model: Some("dev-model".into()),
            test_model: Some("test-model".into()),
            integration_model: Some("integration-model".into()),
            roles: Some(vec![ThreadRole::Director, ThreadRole::Develop]),
            bindings: vec![ManagedProjectThreadAttachment {
                role: ThreadRole::Director,
                thread_id: "thread-1".into(),
            }],
            scenario: Some("rust-taskflow-four-round".into()),
            seed_file: Some(PathBuf::from("/repo/seed.toml")),
        };
        let encoded = serde_json::to_string(&request).expect("serialize request");
        let decoded: DaemonRequest = serde_json::from_str(&encoded).expect("deserialize request");
        assert_eq!(request, decoded);
    }

    #[test]
    fn inspection_request_round_trips() {
        let request = DaemonRequest::InspectManagedProject {
            cwd: PathBuf::from("/repo"),
        };
        let encoded = serde_json::to_string(&request).expect("serialize request");
        let decoded: DaemonRequest = serde_json::from_str(&encoded).expect("deserialize request");
        assert_eq!(request, decoded);
    }

    #[test]
    fn control_request_round_trips() {
        let request = DaemonRequest::SetManagedProjectThreadControl {
            cwd: PathBuf::from("/repo"),
            role: ThreadRole::Develop,
            mode: ManagedProjectThreadControlMode::ManualNextTurn,
        };
        let encoded = serde_json::to_string(&request).expect("serialize request");
        let decoded: DaemonRequest = serde_json::from_str(&encoded).expect("deserialize request");
        assert_eq!(request, decoded);
    }

    #[test]
    fn managed_project_events_round_trip_with_limit() {
        let dir = tempdir().expect("tempdir");
        let repo_root = dir.path().join("repo");
        std::fs::create_dir_all(repo_root.join(".tt")).expect("create tt dir");
        append_managed_project_event(
            &repo_root,
            &ManagedProjectEvent {
                ts: ts(),
                project_id: "p1".into(),
                phase: "startup".into(),
                kind: ManagedProjectEventKind::PromptSent,
                role: Some("director".into()),
                counterparty_role: Some("dev".into()),
                thread_id: Some("thread-1".into()),
                turn_id: Some("turn-1".into()),
                text: "prompt one".into(),
                status: Some("ok".into()),
                error: None,
            },
        )
        .expect("append first event");
        append_managed_project_event(
            &repo_root,
            &ManagedProjectEvent {
                ts: ts(),
                project_id: "p1".into(),
                phase: "startup".into(),
                kind: ManagedProjectEventKind::ResponseReceived,
                role: Some("dev".into()),
                counterparty_role: Some("director".into()),
                thread_id: Some("thread-1".into()),
                turn_id: Some("turn-2".into()),
                text: "response two".into(),
                status: Some("reported".into()),
                error: None,
            },
        )
        .expect("append second event");

        let events = load_managed_project_events(&repo_root, Some(1)).expect("load events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].text, "response two");
    }

    #[test]
    fn managed_project_thread_control_updates_manifest_and_inspection() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));
        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");

        let inspection = service
            .set_managed_project_thread_control(
                &repo,
                ThreadRole::Develop,
                ManagedProjectThreadControlMode::ManualNextTurn,
            )
            .expect("set thread control");
        let updated_role = inspection
            .bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Develop)
            .expect("dev role");
        assert_eq!(
            updated_role.control_mode,
            ManagedProjectThreadControlMode::ManualNextTurn
        );

        let manifest = load_managed_project_manifest(&bootstrap.manifest_path).expect("manifest");
        let manifest_role = manifest.roles.get("dev").expect("dev role");
        assert_eq!(
            manifest_role.control_mode,
            ManagedProjectThreadControlMode::ManualNextTurn
        );
    }

    #[test]
    fn director_requires_initialized_project_state() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));

        let error = service
            .direct_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
                None,
                Vec::new(),
                None,
                None,
            )
            .expect_err("director should require initialized project state");
        let message = error.to_string();
        assert!(message.contains("no project initialized in"));
    }

    #[test]
    fn plan_request_round_trips() {
        let request = DaemonRequest::InspectManagedProjectPlan {
            cwd: PathBuf::from("/repo"),
        };
        let encoded = serde_json::to_string(&request).expect("serialize request");
        let decoded: DaemonRequest = serde_json::from_str(&encoded).expect("deserialize request");
        assert_eq!(request, decoded);
    }

    #[test]
    fn codex_app_servers_request_round_trips() {
        let request = DaemonRequest::InspectCodexAppServers {
            cwd: PathBuf::from("/repo"),
        };
        let encoded = serde_json::to_string(&request).expect("serialize request");
        let decoded: DaemonRequest = serde_json::from_str(&encoded).expect("deserialize request");
        assert_eq!(request, decoded);
    }

    #[test]
    fn clean_request_round_trips() {
        let request = DaemonRequest::CleanManagedProject {
            cwd: PathBuf::from("/repo"),
            force: true,
        };
        let encoded = serde_json::to_string(&request).expect("serialize request");
        let decoded: DaemonRequest = serde_json::from_str(&encoded).expect("deserialize request");
        assert_eq!(request, decoded);
    }

    #[test]
    fn codex_app_server_summary_reports_repo_scoped_defaults() {
        let dir = tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".tt")).expect("create tt dir");
        let summary = codex_app_server_summary_for_cwd(&repo);
        assert_eq!(summary.repo_root, repo);
        assert_eq!(
            summary.daemon_socket_path,
            summary.repo_root.join(".tt/tt-daemon.sock")
        );
        assert!(!summary.daemon_socket_exists);
        assert!(!summary.daemon_socket_reachable);
        assert_eq!(summary.source, "default");
        assert!(summary.note.is_some());
    }

    #[test]
    fn refresh_managed_project_plan_is_read_only() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));
        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");
        let plan_before = std::fs::read_to_string(&bootstrap.plan_path).expect("read plan");
        let refreshed = service
            .refresh_managed_project_plan(&repo)
            .expect("refresh managed project plan");
        let plan_after = std::fs::read_to_string(&bootstrap.plan_path).expect("read plan");
        assert_eq!(plan_before, plan_after);
        assert_eq!(refreshed.bootstrap.plan, bootstrap.plan);
    }

    #[test]
    fn clean_managed_project_tears_down_runtime_state() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));
        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");

        assert!(bootstrap.manifest_path.exists());
        assert!(bootstrap.contract_path.exists());
        assert!(repo.join(".tt/settings.env").exists());
        assert!(!service.list_projects().expect("list projects").is_empty());
        let dev_worktree = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Develop)
            .and_then(|role| role.worktree_path.clone())
            .expect("dev worktree");
        assert!(dev_worktree.exists());

        let removed = service
            .clean_managed_project(&repo, false)
            .expect("clean managed project");
        assert!(removed > 0);
        assert!(!bootstrap.manifest_path.exists());
        assert!(bootstrap.project_config_path.exists());
        assert!(bootstrap.plan_path.exists());
        assert!(bootstrap.contract_path.exists());
        assert!(repo.join(".tt/settings.env").exists());
        assert!(!dev_worktree.exists());
        assert!(service.list_projects().expect("list projects").is_empty());
        assert!(
            service
                .list_workspace_bindings()
                .expect("list workspace bindings")
                .is_empty()
        );
    }

    #[test]
    fn clean_managed_project_all_prunes_repo_local_codex_runtime_artifacts() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));
        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");

        let codex_root = repo.join(".codex");
        std::fs::write(codex_root.join("auth.json"), "{}").expect("write auth");
        std::fs::write(codex_root.join("session_index.jsonl"), "").expect("write session index");
        std::fs::create_dir_all(codex_root.join("sessions")).expect("create sessions");
        std::fs::create_dir_all(codex_root.join("archived_sessions")).expect("create archived");
        std::fs::create_dir_all(codex_root.join("logs")).expect("create logs");
        std::fs::write(codex_root.join("state_5.sqlite"), "").expect("write sqlite");
        std::fs::write(codex_root.join("curated-note.md"), "keep").expect("write curated file");

        let removed = service
            .clean_managed_project(&repo, true)
            .expect("clean managed project");
        assert!(removed > 0);
        assert!(codex_root.join("auth.json").exists());
        assert!(!codex_root.join("session_index.jsonl").exists());
        assert!(!codex_root.join("sessions").exists());
        assert!(!codex_root.join("archived_sessions").exists());
        assert!(!codex_root.join("logs").exists());
        assert!(!codex_root.join("state_5.sqlite").exists());
        assert!(bootstrap.codex_config_path.exists());
        assert!(codex_root.join("config.defaults.toml").exists());
        assert!(codex_root.join("config.local.toml").exists());
        assert!(codex_root.join("agents/director.toml").exists());
        assert!(codex_root.join("curated-note.md").exists());
    }

    #[test]
    fn clean_managed_project_skips_missing_worktree_directories() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));
        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");

        let dev_worktree = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Develop)
            .and_then(|role| role.worktree_path.clone())
            .expect("dev worktree");
        std::fs::remove_dir_all(&dev_worktree).expect("remove dev worktree");

        let removed = service
            .clean_managed_project(&repo, false)
            .expect("clean managed project");
        assert!(removed > 0);
        assert!(!bootstrap.manifest_path.exists());
        assert!(!dev_worktree.exists());
    }

    #[test]
    fn clean_managed_project_removes_orphaned_worktree_directories() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));
        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");

        let dev_worktree = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Develop)
            .and_then(|role| role.worktree_path.clone())
            .expect("dev worktree");
        let repository = GitRepository::discover(&repo)
            .expect("discover repo")
            .expect("repo");
        repository
            .prune_worktree(&dev_worktree)
            .expect("remove registered worktree");
        fs::create_dir_all(&dev_worktree).expect("recreate orphaned worktree dir");
        fs::write(dev_worktree.join("sentinel.txt"), "orphan").expect("write sentinel");

        let removed = service
            .clean_managed_project(&repo, false)
            .expect("clean managed project");
        assert!(removed > 0);
        assert!(!bootstrap.manifest_path.exists());
        assert!(!dev_worktree.exists());
    }

    #[test]
    fn runtime_supports_request_api_and_repo_summary() {
        let (repo, worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let runtime = DaemonRuntime::open(dir.path()).expect("open runtime");

        let response = runtime
            .request(DaemonRequest::RepositorySummary {
                cwd: worktree.clone(),
            })
            .expect("repository summary");
        match response {
            DaemonResponse::RepositorySummary(Some(summary)) => {
                assert_eq!(summary.repository_root, worktree.display().to_string());
                assert_eq!(summary.current_branch.as_deref(), Some("tt/tt-1"));
            }
            other => panic!("unexpected response: {other:?}"),
        }

        let upsert = DaemonRequest::UpsertProject {
            project: Project {
                id: "p2".into(),
                slug: "beta".into(),
                title: "Beta".into(),
                objective: "Ship".into(),
                status: ProjectStatus::Active,
                created_at: ts(),
                updated_at: ts(),
            },
        };
        assert!(matches!(
            runtime.request(upsert).expect("upsert"),
            DaemonResponse::Unit
        ));

        let list = runtime
            .request(DaemonRequest::ListProjects)
            .expect("list projects");
        match list {
            DaemonResponse::Projects(projects) => assert_eq!(projects.len(), 1),
            other => panic!("unexpected response: {other:?}"),
        }

        let deleted = runtime
            .request(DaemonRequest::DeleteProject {
                id_or_slug: "beta".into(),
            })
            .expect("delete");
        assert!(matches!(deleted, DaemonResponse::Count(1)));

        let _ = repo;
    }

    #[test]
    fn open_managed_project_bootstraps_roles_and_files() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));

        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");

        assert_eq!(bootstrap.project.slug, "alpha");
        assert!(bootstrap.contract_path.exists());
        assert!(bootstrap.codex_config_path.exists());
        assert!(bootstrap.project_config_path.exists());
        assert!(bootstrap.plan_path.exists());
        assert!(bootstrap.manifest_path.exists());
        assert!(bootstrap.project_config.plan_first);
        assert_eq!(
            bootstrap.project_config.commit_policy,
            "checkpoint-enforced"
        );
        assert_eq!(bootstrap.plan.status, "draft");
        assert_eq!(bootstrap.roles.len(), 4);
        assert!(bootstrap.roles.iter().all(|role| role.agent_path.exists()));
        assert!(bootstrap.roles.iter().all(|role| role.thread_id.is_some()));
        assert_ne!(
            bootstrap.startup.phase,
            ManagedProjectStartupPhase::Scaffolded
        );

        let director_role = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Director)
            .expect("director role");
        assert_eq!(director_role.model.as_deref(), Some("director-model"));
        assert_eq!(director_role.reasoning_effort.as_deref(), Some("medium"));
        assert!(director_role.branch_name.is_none());
        assert!(director_role.worktree_path.is_none());
        assert!(director_role.workspace_binding_id.is_some());

        let dev_role = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Develop)
            .expect("dev role");
        assert_eq!(dev_role.model.as_deref(), Some("dev-model"));
        assert_eq!(dev_role.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(dev_role.branch_name.as_deref(), Some("tt/dev"));
        assert!(
            dev_role
                .worktree_path
                .as_ref()
                .expect("dev worktree")
                .exists()
        );

        let test_role = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Test)
            .expect("test role");
        assert_eq!(test_role.branch_name.as_deref(), Some("tt/test"));

        let integration_role = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Integrate)
            .expect("integration role");
        assert_eq!(
            integration_role.branch_name.as_deref(),
            Some("tt/integration")
        );

        let inspection = GitRepository::discover(&repo)
            .expect("discover repo")
            .expect("repo")
            .inspect_repository()
            .expect("inspect repo");
        assert!(inspection.worktrees.len() >= 4);
    }

    #[test]
    fn status_reports_director_state_from_codex_session_catalog() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));

        let missing = service.status(&repo).expect("status");
        assert!(matches!(
            missing.director_state,
            ManagedProjectDirectorState::Missing
        ));

        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");
        let starting = service.status(&repo).expect("status");
        assert!(matches!(
            starting.director_state,
            ManagedProjectDirectorState::Starting | ManagedProjectDirectorState::Ready
        ));

        let mut manifest =
            load_managed_project_manifest(&bootstrap.manifest_path).expect("load manifest");
        manifest.startup.phase = ManagedProjectStartupPhase::Blocked;
        save_managed_project_manifest(&bootstrap.manifest_path, &manifest).expect("save manifest");
        let blocked = service.status(&repo).expect("status");
        assert!(matches!(
            blocked.director_state,
            ManagedProjectDirectorState::Blocked
        ));

        assert!(bootstrap.manifest_path.exists());
    }

    #[test]
    fn direct_managed_project_fails_fast_until_startup_is_ready() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));
        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");

        let mut manifest =
            load_managed_project_manifest(&bootstrap.manifest_path).expect("load manifest");
        manifest.startup.phase = ManagedProjectStartupPhase::WorkerReportsPending;
        save_managed_project_manifest(&bootstrap.manifest_path, &manifest).expect("save manifest");

        let error = service
            .direct_managed_project(
                &repo,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
            )
            .expect_err("open should fail fast before startup is ready");
        assert!(
            error
                .to_string()
                .contains("managed project startup is not ready yet")
        );
    }

    #[test]
    fn open_managed_project_reuses_existing_project_slug() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));
        let existing = Project {
            id: "019dffff-0000-7000-8000-000000000001".to_string(),
            slug: "alpha".to_string(),
            title: "Alpha".to_string(),
            objective: "Ship the alpha slice".to_string(),
            status: ProjectStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        service.upsert_project(&existing).expect("seed project");

        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");

        assert_ne!(bootstrap.project.id, existing.id);
        assert_eq!(bootstrap.project.slug, existing.slug);
        let projects = service.list_projects().expect("list projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, bootstrap.project.id);
    }

    #[test]
    fn open_managed_project_migrates_existing_codex_config_to_layered_files() {
        let (repo, _worktree) = setup_repo();
        let codex_config_path = repo.join(".codex/config.toml");
        std::fs::create_dir_all(codex_config_path.parent().expect("codex parent"))
            .expect("create codex dir");
        std::fs::write(
            &codex_config_path,
            "[agents]\nmax_threads = 6\nmax_depth = 1\n\n[projects.\"/tmp/repo\"]\ntrust_level = \"trusted\"\n\n[plugins.\"github@openai-curated\"]\nenabled = true\n",
        )
        .expect("seed config");

        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));

        let bootstrap = service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");

        let defaults_path = repo.join(".codex/config.defaults.toml");
        let local_path = repo.join(".codex/config.local.toml");
        let defaults = std::fs::read_to_string(&defaults_path).expect("read defaults");
        let local = std::fs::read_to_string(&local_path).expect("read local");
        let generated = std::fs::read_to_string(&codex_config_path).expect("read generated");
        assert!(defaults.contains("[agents]"));
        assert!(!defaults.contains("[projects."));
        assert!(!defaults.contains("[plugins."));
        assert!(local.contains("[projects."));
        assert!(local.contains("trust_level = \"trusted\""));
        assert!(local.contains("[plugins.\"github@openai-curated\"]"));
        assert!(generated.contains("[agents]"));
        assert!(generated.contains("[projects.\"/tmp/repo\"]"));
        assert!(generated.contains("[plugins.\"github@openai-curated\"]"));
        assert_eq!(bootstrap.codex_config_path, codex_config_path);
    }

    #[test]
    fn open_managed_project_imports_codex_written_local_overrides_into_local_layer() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));

        service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("open managed project");

        let generated_path = repo.join(".codex/config.toml");
        std::fs::write(
            &generated_path,
            "[agents]\nmax_threads = 6\nmax_depth = 1\n\n[projects.\"/tmp/repo\"]\ntrust_level = \"trusted\"\n",
        )
        .expect("codex writes generated config");

        service
            .open_managed_project(
                &repo,
                Some("Alpha".to_string()),
                Some("Ship the alpha slice".to_string()),
                None,
                None,
                Some("director-model".to_string()),
                Some("dev-model".to_string()),
                Some("test-model".to_string()),
                Some("integration-model".to_string()),
            )
            .expect("reopen managed project");

        let local = std::fs::read_to_string(repo.join(".codex/config.local.toml"))
            .expect("read local config");
        let regenerated = std::fs::read_to_string(&generated_path).expect("read generated config");
        assert!(local.contains("[projects.\"/tmp/repo\"]"));
        assert!(regenerated.contains("[projects.\"/tmp/repo\"]"));
    }

    #[test]
    fn default_managed_project_plan_seeds_planning_questions() {
        let project = Project {
            id: "p-plan".into(),
            slug: "alpha".into(),
            title: "Alpha".into(),
            objective: "Ship".into(),
            status: ProjectStatus::Active,
            created_at: ts(),
            updated_at: ts(),
        };
        let config = ManagedProjectProjectConfig {
            schema: "tt-managed-project-config-v1".into(),
            title: "Alpha".into(),
            objective: "Ship".into(),
            base_branch: "main".into(),
            branch_prefix: "tt".into(),
            tt_runtime_bin: None,
            plan_first: true,
            commit_policy: "checkpoint-enforced".into(),
            require_operator_merge_approval: true,
            expected_long_build: false,
            require_progress_updates: true,
            soft_silence_seconds: 900,
            hard_ceiling_seconds: 7200,
            default_validation_commands: vec!["cargo test".into()],
            smoke_validation_commands: vec!["cargo check".into()],
            checkpoint_triggers: vec!["after_plan".into(), "before_merge".into()],
            pitfalls: vec!["slow clean builds".into()],
            hints: vec!["run cargo test".into()],
            exceptions: vec!["merge approval required".into()],
        };
        let plan = default_managed_project_plan(&project, &config, &[]);
        assert_eq!(plan.status, "draft");
        assert!(
            plan.notes
                .open_questions
                .iter()
                .any(|question| question.contains("scope and non-goals"))
        );
        assert!(
            plan.notes
                .open_questions
                .iter()
                .any(|question| question.contains("validation commands"))
        );
        assert!(
            plan.notes
                .open_questions
                .iter()
                .any(|question| question.contains("merge"))
        );
    }

    #[test]
    fn default_managed_project_project_config_detects_repo_local_tt_runtime_bin() {
        let dir = tempdir().expect("tempdir");
        let tt_bin = dir.path().join("target").join("debug").join("tt-cli");
        std::fs::create_dir_all(tt_bin.parent().expect("parent")).expect("create target dir");
        std::fs::write(&tt_bin, "").expect("write tt-cli placeholder");
        let config = default_managed_project_project_config(dir.path(), "Alpha", "Ship", "main");
        assert_eq!(
            config.tt_runtime_bin.as_deref(),
            Some("./target/debug/tt-cli")
        );
    }

    #[test]
    fn default_managed_project_project_config_uses_repo_settings_env_overrides() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".tt")).expect("create tt dir");
        std::fs::write(
            dir.path().join(".tt/settings.env"),
            "TT_MANAGED_PROJECT_EXPECTED_LONG_BUILD=true\nTT_MANAGED_PROJECT_REQUIRES_PROGRESS_UPDATES=false\nTT_MANAGED_PROJECT_SOFT_SILENCE_SECONDS=123\nTT_MANAGED_PROJECT_HARD_CEILING_SECONDS=456\n",
        )
        .expect("write settings env");
        let tt_bin = dir.path().join("target").join("debug").join("tt-cli");
        std::fs::create_dir_all(tt_bin.parent().expect("parent")).expect("create target dir");
        std::fs::write(&tt_bin, "").expect("write tt-cli placeholder");
        tt_codex::load_repo_settings_env(dir.path()).expect("load settings env");

        let config = default_managed_project_project_config(dir.path(), "Alpha", "Ship", "main");
        assert_eq!(
            config.tt_runtime_bin.as_deref(),
            Some("./target/debug/tt-cli")
        );
        assert!(config.expected_long_build);
        assert!(!config.require_progress_updates);
        assert_eq!(config.soft_silence_seconds, 123);
        assert_eq!(config.hard_ceiling_seconds, 456);
    }

    #[test]
    fn managed_project_manifest_round_trips_thread_bindings() {
        let dir = tempdir().expect("tempdir");
        let project = Project {
            id: "p-manifest".into(),
            slug: "alpha".into(),
            title: "Alpha".into(),
            objective: "Ship".into(),
            status: ProjectStatus::Active,
            created_at: ts(),
            updated_at: ts(),
        };
        let role = ManagedProjectRoleBootstrap {
            role: ThreadRole::Develop,
            work_unit: WorkUnit {
                id: "wu-manifest".into(),
                project_id: project.id.clone(),
                slug: Some("dev".into()),
                title: "Developer".into(),
                task: "Implement".into(),
                status: WorkUnitStatus::Ready,
                created_at: ts(),
                updated_at: ts(),
            },
            agent_path: dir.path().join(".codex/agents/dev.toml"),
            model: Some("gpt-5.4".into()),
            reasoning_effort: Some("medium".into()),
            control_mode: ManagedProjectThreadControlMode::Director,
            branch_name: Some("tt/dev".into()),
            worktree_path: Some(dir.path().join("worktree")),
            thread_id: Some("thread-1".into()),
            thread_name: Some("alpha-dev".into()),
            workspace_binding_id: Some("alpha:dev".into()),
        };
        let project_config = ManagedProjectProjectConfig {
            schema: "tt-managed-project-config-v1".into(),
            title: "Alpha".into(),
            objective: "Ship".into(),
            base_branch: "main".into(),
            branch_prefix: "tt".into(),
            tt_runtime_bin: None,
            plan_first: true,
            commit_policy: "checkpoint-enforced".into(),
            require_operator_merge_approval: true,
            expected_long_build: false,
            require_progress_updates: true,
            soft_silence_seconds: 900,
            hard_ceiling_seconds: 7200,
            default_validation_commands: vec!["cargo test".into()],
            smoke_validation_commands: vec!["cargo check".into()],
            checkpoint_triggers: vec!["after_plan".into(), "before_merge".into()],
            pitfalls: vec!["pitfall".into()],
            hints: vec!["hint".into()],
            exceptions: vec!["exception".into()],
        };
        let plan = ManagedProjectPlan {
            schema: "tt-managed-project-plan-v1".into(),
            status: "draft".into(),
            objective: "Ship".into(),
            updated_at: ts().to_rfc3339(),
            milestones: vec![],
            work_items: vec![],
            notes: ManagedProjectPlanNotes::default(),
        };
        let project_config_path = dir.path().join(".tt/project.toml");
        let plan_path = dir.path().join(".tt/plan.toml");
        save_managed_project_project_config(&project_config_path, &project_config)
            .expect("save project config");
        save_managed_project_plan(&plan_path, &plan).expect("save plan");
        let manifest = build_managed_project_manifest(
            &project,
            dir.path(),
            "main",
            &dir.path().join(".tt/worktrees"),
            &project_config_path,
            &plan_path,
            &dir.path().join(".tt/contract.md"),
            &dir.path().join(".codex/config.toml"),
            &default_managed_project_startup_state(),
            None,
            &[&role],
        )
        .expect("build manifest");
        let path = dir.path().join("state.toml");
        save_managed_project_manifest(&path, &manifest).expect("save manifest");
        let loaded = load_managed_project_manifest(&path).expect("load manifest");
        let loaded_role = loaded.roles.get("dev").expect("dev role");
        assert_eq!(loaded_role.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(loaded_role.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(loaded_role.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(loaded_role.thread_name.as_deref(), Some("alpha-dev"));
        assert_eq!(
            loaded_role.workspace_binding_id.as_deref(),
            Some("alpha:dev")
        );
        assert_eq!(
            loaded.project_config_sha256,
            file_sha256_hex(&project_config_path).expect("project config checksum")
        );
        assert_eq!(
            loaded.plan_sha256,
            file_sha256_hex(&plan_path).expect("plan checksum")
        );
    }

    #[test]
    fn director_round_prompt_includes_planning_agenda() {
        let bootstrap = ManagedProjectBootstrap {
            project: Project {
                id: "p-prompt".into(),
                slug: "alpha".into(),
                title: "Alpha".into(),
                objective: "Ship".into(),
                status: ProjectStatus::Active,
                created_at: ts(),
                updated_at: ts(),
            },
            repo_root: PathBuf::from("/repo"),
            base_branch: "main".into(),
            worktree_root: PathBuf::from("/repo/.tt/worktrees"),
            manifest_path: PathBuf::from("/repo/.tt/state.toml"),
            project_config_path: PathBuf::from("/repo/.tt/project.toml"),
            plan_path: PathBuf::from("/repo/.tt/plan.toml"),
            contract_path: PathBuf::from("/repo/.tt/contract.md"),
            codex_config_path: PathBuf::from("/repo/.codex/config.toml"),
            project_config: ManagedProjectProjectConfig {
                schema: "tt-managed-project-config-v1".into(),
                title: "Alpha".into(),
                objective: "Ship".into(),
                base_branch: "main".into(),
                branch_prefix: "tt".into(),
                tt_runtime_bin: None,
                plan_first: true,
                commit_policy: "checkpoint-enforced".into(),
                require_operator_merge_approval: true,
                expected_long_build: false,
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
            plan: ManagedProjectPlan {
                schema: "tt-managed-project-plan-v1".into(),
                status: "draft".into(),
                objective: "Ship".into(),
                updated_at: ts().to_rfc3339(),
                milestones: vec![],
                work_items: vec![],
                notes: ManagedProjectPlanNotes {
                    risks: vec!["slow CI".into()],
                    pitfalls: vec!["clean builds are expensive".into()],
                    open_questions: vec!["what is out of scope?".into()],
                    operator_constraints: vec!["wait for approval".into()],
                },
            },
            startup: default_managed_project_startup_state(),
            scenario: None,
            roles: vec![],
        };
        let spec = ManagedProjectRoundSpec {
            round_number: 1,
            phase: "plan",
            director_goal: "Plan the work.",
            dev_goal: "Implement the work.",
            test_goal: "Validate the work.",
            integration_goal: "Prepare landing.",
            requires_landing_approval: false,
        };
        let prompt = build_director_round_prompt(&bootstrap, &spec, "operator seed", "", None);
        assert!(prompt.contains("Planning agenda:"));
        assert!(prompt.contains("Current open questions:"));
        assert!(prompt.contains("what is out of scope?"));
        assert!(prompt.contains("Validation commands:"));
        assert!(prompt.contains("cargo test"));
        assert!(prompt.contains("resolve the agenda first"));
    }

    #[test]
    fn managed_project_contract_mentions_director_and_workers() {
        let contract = render_worker_contract(
            Path::new("/repo"),
            "alpha",
            "main",
            &ManagedProjectProjectConfig {
                schema: "tt-managed-project-config-v1".into(),
                title: "Alpha".into(),
                objective: "Ship".into(),
                base_branch: "main".into(),
                branch_prefix: "tt".into(),
                tt_runtime_bin: None,
                plan_first: true,
                commit_policy: "checkpoint-enforced".into(),
                require_operator_merge_approval: true,
                expected_long_build: false,
                require_progress_updates: true,
                soft_silence_seconds: 900,
                hard_ceiling_seconds: 7200,
                default_validation_commands: vec!["cargo test".into()],
                smoke_validation_commands: vec!["cargo check".into()],
                checkpoint_triggers: vec!["after_plan".into(), "before_merge".into()],
                pitfalls: vec![],
                hints: vec![],
                exceptions: vec![],
            },
        );
        assert!(contract.contains("TT Managed Project Contract"));
        assert!(contract.contains("Coordination Model"));
        assert!(contract.contains("The operator talks to the director."));
        assert!(contract.contains("## Roles"));
        assert!(contract.contains("plan"));
        assert!(contract.contains("merge"));
    }

    #[test]
    fn extract_worker_handoff_accepts_exact_json_agent_message() {
        let extraction = extract_worker_handoff(&[protocol::ThreadItem::AgentMessage {
            id: "item-1".into(),
            text: r#"{"status":"complete","changed_files":["src/main.rs"],"tests_run":["cargo test"],"blockers":[],"next_step":"Hand off to integration."}"#.into(),
            phase: None,
            memory_citation: None,
        }]);

        assert_eq!(extraction.source, WorkerHandoffSource::Extracted);
        let handoff = extraction.handoff.expect("handoff");
        assert_eq!(handoff.status, "complete");
        assert_eq!(handoff.changed_files, vec!["src/main.rs"]);
    }

    #[test]
    fn extract_worker_handoff_accepts_fenced_json_from_last_agent_message() {
        let extraction = extract_worker_handoff(&[
            protocol::ThreadItem::AgentMessage {
                id: "item-1".into(),
                text: "Planning complete.".into(),
                phase: None,
                memory_citation: None,
            },
            protocol::ThreadItem::AgentMessage {
                id: "item-2".into(),
                text: "```json\n{\"status\":\"needs-review\",\"changed_files\":[\"src/lib.rs\"],\"tests_run\":[\"cargo test\"],\"blockers\":[],\"next_step\":\"Request review.\"}\n```".into(),
                phase: None,
                memory_citation: None,
            },
        ]);

        assert_eq!(extraction.source, WorkerHandoffSource::Extracted);
        let handoff = extraction.handoff.expect("handoff");
        assert_eq!(handoff.status, "needs-review");
        assert_eq!(handoff.next_step, "Request review.");
    }

    #[test]
    fn extract_worker_handoff_reports_missing_agent_messages() {
        let extraction = extract_worker_handoff(&[protocol::ThreadItem::Plan {
            id: "item-1".into(),
            text: "work plan".into(),
        }]);

        assert_eq!(extraction.source, WorkerHandoffSource::SeededFallback);
        assert!(extraction.handoff.is_none());
        assert_eq!(
            extraction.parse_error.as_deref(),
            Some("no agent message found in completed turn")
        );
    }

    #[test]
    fn parse_bootstrap_worker_report_accepts_plain_text() {
        let role_bootstrap = ManagedProjectRoleBootstrap {
            role: ThreadRole::Develop,
            work_unit: WorkUnit {
                id: "wu".into(),
                project_id: "p".into(),
                slug: None,
                title: "Dev".into(),
                task: "Task".into(),
                status: WorkUnitStatus::Ready,
                created_at: ts(),
                updated_at: ts(),
            },
            agent_path: PathBuf::from("/repo/.codex/agents/dev.toml"),
            model: None,
            reasoning_effort: None,
            control_mode: ManagedProjectThreadControlMode::Director,
            branch_name: Some("tt/dev".into()),
            worktree_path: Some(PathBuf::from("/repo/.tt/worktrees/dev")),
            thread_id: Some("thread-1".into()),
            thread_name: None,
            workspace_binding_id: None,
        };
        let extraction = WorkerHandoffExtraction {
            handoff: None,
            raw_text: Some("ready. contract loaded and plan loaded.".into()),
            source: WorkerHandoffSource::SeededFallback,
            parse_error: Some("not json".into()),
        };
        let report = parse_bootstrap_worker_report("ready", &extraction, &role_bootstrap, Path::new("/repo"))
            .expect("worker report");
        assert_eq!(report.status, "ready");
        assert!(report.contract_loaded);
        assert!(report.plan_loaded);
        assert_eq!(report.branch, "tt/dev");
        assert_eq!(report.cwd, "/repo/.tt/worktrees/dev");
    }

    #[test]
    fn parse_bootstrap_director_ack_accepts_plain_text() {
        let reports = vec![
            ManagedProjectBootstrapWorkerReport {
                role: "dev".into(),
                cwd: "/repo/.tt/worktrees/dev".into(),
                worktree: "/repo/.tt/worktrees/dev".into(),
                branch: "tt/dev".into(),
                contract_loaded: true,
                plan_loaded: true,
                status: "ready".into(),
                blocker: None,
                summary: "ready".into(),
            },
            ManagedProjectBootstrapWorkerReport {
                role: "test".into(),
                cwd: "/repo/.tt/worktrees/test".into(),
                worktree: "/repo/.tt/worktrees/test".into(),
                branch: "tt/test".into(),
                contract_loaded: true,
                plan_loaded: true,
                status: "ready".into(),
                blocker: None,
                summary: "ready".into(),
            },
        ];
        let extraction = WorkerHandoffExtraction {
            handoff: None,
            raw_text: Some("ready for operator handoff".into()),
            source: WorkerHandoffSource::SeededFallback,
            parse_error: Some("not json".into()),
        };
        let ack = parse_bootstrap_director_ack("ready", &extraction, &reports).expect("ack");
        assert_eq!(ack.status, "ready");
        assert_eq!(ack.received_roles, vec!["dev", "test"]);
        assert!(ack.missing_roles.is_empty());
        assert_eq!(ack.summary, "ready for operator handoff");
    }

    #[test]
    fn retryable_live_turn_failure_matches_exact_upstream_websocket_500() {
        let error = "turn `turn-1` for role `director` failed (model=gpt-5.4, reasoning_effort=medium): responses_websocket failed to connect to websocket: HTTP error: 500 Internal Server Error, url: wss://api.openai.com/v1/responses";
        assert!(is_retryable_live_turn_failure(error));
    }

    #[test]
    fn retryable_live_turn_failure_does_not_match_model_rejections() {
        let error = "turn `turn-1` for role `director` failed (model=gpt-5.4, reasoning_effort=medium): invalid model `gpt-5.4` for current provider";
        assert!(!is_retryable_live_turn_failure(error));
    }

    #[test]
    fn retryable_live_turn_failure_matches_empty_session_rollout() {
        let error = "Codex app-server `ws://127.0.0.1:4500` request failed: failed to load rollout `/home/me/.codex/sessions/2026/04/09/rollout.jsonl` for thread 123: empty session file";
        assert!(is_retryable_live_turn_failure(error));
    }

    #[test]
    fn director_agent_instructions_include_operator_and_roster() {
        let project_config = ManagedProjectProjectConfig {
            schema: "tt-managed-project-config-v1".into(),
            title: "Alpha".into(),
            objective: "Ship the alpha slice".into(),
            base_branch: "main".into(),
            branch_prefix: "tt".into(),
            tt_runtime_bin: None,
            plan_first: true,
            commit_policy: "checkpoint-enforced".into(),
            require_operator_merge_approval: true,
            expected_long_build: true,
            require_progress_updates: true,
            soft_silence_seconds: 900,
            hard_ceiling_seconds: 7200,
            default_validation_commands: vec!["cargo test".into()],
            smoke_validation_commands: vec!["cargo check".into()],
            checkpoint_triggers: vec!["after_plan".into(), "after_test".into()],
            pitfalls: vec![],
            hints: vec![],
            exceptions: vec![],
        };
        let plan = ManagedProjectPlan {
            schema: "tt-managed-project-plan-v1".into(),
            status: "draft".into(),
            objective: "Ship the alpha slice".into(),
            updated_at: ts().to_rfc3339(),
            milestones: vec![],
            work_items: vec![],
            notes: ManagedProjectPlanNotes::default(),
        };
        let instructions = render_agent_file(
            ThreadRole::Director,
            Some("gpt-5.4"),
            Some("medium"),
            "Alpha",
            "Ship the alpha slice",
            &project_config,
            &plan,
        );
        assert!(instructions.contains("model = \"gpt-5.4\""));
        assert!(instructions.contains("model_reasoning_effort = \"medium\""));
        assert!(instructions.contains("You are the director agent for Alpha."));
        assert!(instructions.contains("The operator talks to the director."));
        assert!(instructions.contains(
            "Workers do not coordinate directly with each other; all assignments and escalations go through the director."
        ));
        assert!(instructions.contains("Role roster:"));
        assert!(
            instructions
                .contains("turn operator intent into a plan, todo list, and dispatch decisions")
        );
        assert!(instructions.contains("request approval for merges or destructive cleanup"));
    }

    #[test]
    fn runtime_supports_status_mutations() {
        let dir = tempdir().expect("tempdir");
        let runtime = DaemonRuntime::open(dir.path()).expect("open runtime");
        runtime
            .request(DaemonRequest::UpsertProject {
                project: Project {
                    id: "p3".into(),
                    slug: "gamma".into(),
                    title: "Gamma".into(),
                    objective: "Ship".into(),
                    status: ProjectStatus::Active,
                    created_at: ts(),
                    updated_at: ts(),
                },
            })
            .expect("upsert project");

        let updated = runtime
            .request(DaemonRequest::SetProjectStatus {
                id_or_slug: "gamma".into(),
                status: ProjectStatus::Blocked,
            })
            .expect("set project status");
        assert!(matches!(updated, DaemonResponse::Count(1)));

        match runtime
            .request(DaemonRequest::GetProject {
                id_or_slug: "gamma".into(),
            })
            .expect("get project")
        {
            DaemonResponse::Project(Some(project)) => {
                assert_eq!(project.status, ProjectStatus::Blocked)
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn runtime_supports_managed_project_inspection() {
        let (repo, _worktree) = setup_repo();
        let dir = tempdir().expect("tempdir");
        let runtime = DaemonRuntime::open(dir.path()).expect("open runtime");

        runtime
            .request(DaemonRequest::OpenManagedProject {
                cwd: repo.clone(),
                title: Some("Alpha".into()),
                objective: Some("Ship the alpha slice".into()),
                base_branch: None,
                worktree_root: None,
                director_model: Some("director-model".into()),
                dev_model: Some("dev-model".into()),
                test_model: Some("test-model".into()),
                integration_model: Some("integration-model".into()),
            })
            .expect("open managed project");

        let response = runtime
            .request(DaemonRequest::InspectManagedProject { cwd: repo.clone() })
            .expect("inspect managed project");
        match response {
            DaemonResponse::ManagedProjectInspection(inspection) => {
                assert_eq!(inspection.bootstrap.project.slug, "alpha");
                assert_eq!(inspection.bootstrap.roles.len(), 4);
                assert!(inspection.repository_summary.is_some());
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn socket_client_round_trips_requests() {
        let dir = tempdir().expect("tempdir");
        let runtime = DaemonRuntime::open(dir.path()).expect("open runtime");
        let server = DaemonServer::new(runtime.clone());
        let socket_path = server.socket_path().to_path_buf();
        let handle = thread::spawn(move || server.serve().expect("serve"));
        for _ in 0..20 {
            if socket_path.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        let client = DaemonClient::connect(&socket_path).expect("client connect");
        let response = client
            .request(DaemonRequest::Status {
                cwd: dir.path().to_path_buf(),
            })
            .expect("status");
        assert!(matches!(response, DaemonResponse::Status(_)));

        drop(handle);
    }

    #[test]
    fn init_managed_project_scaffold_is_workspace_isolated() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("taskflow");
        fs::create_dir_all(root.join(".tt")).expect("create tt dir");
        fs::write(
            root.join(".tt/settings.env"),
            "TT_CODEX_BIN=~/.local/bin/codex\nTT_CODEX_APP_SERVER_BIN=~/openai/codex/codex-rs/target/debug/codex-app-server\n",
        )
        .expect("seed settings env");
        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));
        let bootstrap = service
            .init_managed_project(
                &root,
                Some("Taskflow".into()),
                Some("Ship".into()),
                Some("rust-taskflow".into()),
                Some("main".into()),
                None,
                None,
                None,
                None,
                None,
            )
            .expect("init managed project");
        let cargo_toml =
            fs::read_to_string(bootstrap.repo_root.join("Cargo.toml")).expect("read Cargo.toml");
        assert!(cargo_toml.contains("[workspace]"));
        assert!(cargo_toml.contains("name = \"taskflow\""));
    }

    #[test]
    fn init_managed_project_preserves_existing_settings_env() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("taskflow");
        fs::create_dir_all(root.join(".tt")).expect("create tt dir");
        fs::write(
            root.join(".tt/settings.env"),
            "TT_CODEX_BIN=~/.local/bin/codex\nTT_CODEX_APP_SERVER_BIN=~/openai/codex/codex-rs/target/debug/codex-app-server\n",
        )
        .expect("seed settings env");

        let service = DaemonService::new(OverlayStore::open_in_dir(dir.path()).expect("store"));
        let bootstrap = service
            .init_managed_project(
                &root,
                Some("Taskflow".into()),
                Some("Ship".into()),
                Some("rust-taskflow".into()),
                Some("main".into()),
                None,
                None,
                None,
                None,
                None,
            )
            .expect("init managed project");

        let settings_env = fs::read_to_string(bootstrap.repo_root.join(".tt/settings.env"))
            .expect("read settings env");
        assert!(settings_env.contains("TT_CODEX_BIN=~/.local/bin/codex"));
        assert!(settings_env.contains(
            "TT_CODEX_APP_SERVER_BIN=~/openai/codex/codex-rs/target/debug/codex-app-server"
        ));
    }
}
