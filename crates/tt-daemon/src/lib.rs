//! Local orchestration for TT v2.
//!
//! The daemon coordinates TT overlay state, Codex runtime state, and git state.
//! It owns the local request/response API used by the TUI and CLI.

use std::collections::BTreeMap;
use std::process::Command;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    thread,
};

use anyhow::{Context, Result};
use chrono::Utc;
use clap as _;
use codex_app_server_protocol as protocol;
use serde::{Deserialize, Serialize};
use tt_codex::{CodexHome, CodexRuntimeClient, CodexThreadRuntimeSnapshot};
use tt_domain::{
    MergeAuthorizationStatus, MergeExecutionStatus, MergeReadiness, MergeRun, Project,
    ProjectStatus, ThreadBinding, ThreadBindingStatus, ThreadRole, WorkUnit, WorkUnitStatus,
    WorkspaceBinding, WorkspaceCleanupPolicy, WorkspaceStatus, WorkspaceStrategy,
    WorkspaceSyncPolicy,
};
use tt_git::GitRepository;
use tt_store::OverlayStore;
use tt_ui_core::{CodexThreadDetail, CodexThreadSummary, DashboardSummary, GitRepositorySummary};

pub const TT_DAEMON_API_VERSION: &str = "v2";
pub const TT_DAEMON_SOCKET_NAME: &str = "tt-daemon.sock";
const DEFAULT_AGENT_CONFIG_MAX_THREADS: usize = 6;
const DEFAULT_AGENT_CONFIG_MAX_DEPTH: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub codex_home: Option<PathBuf>,
    pub codex_state_db: Option<PathBuf>,
    pub codex_session_index: Option<PathBuf>,
    pub project_count: usize,
    pub work_unit_count: usize,
    pub bound_thread_count: usize,
    pub ready_workspace_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum DaemonRequest {
    Status,
    DashboardSummary,
    RepositorySummary {
        cwd: PathBuf,
    },
    InspectManagedProject {
        cwd: PathBuf,
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
    Status(DaemonStatus),
    DashboardSummary(DashboardSummary),
    RepositorySummary(Option<GitRepositorySummary>),
    ManagedProjectInspection(ManagedProjectInspection),
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
    ManagedProject(ManagedProjectBootstrap),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjectRoleBootstrap {
    pub role: ThreadRole,
    pub work_unit: WorkUnit,
    pub agent_path: PathBuf,
    pub model: Option<String>,
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
    pub contract_path: PathBuf,
    pub codex_config_path: PathBuf,
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
pub struct ManagedProjectScenarioState {
    pub scenario_id: String,
    pub scenario_kind: String,
    pub operator_seed: String,
    pub current_round: usize,
    pub current_phase: String,
    pub pending_approval: Option<ManagedProjectApprovalState>,
    pub rounds: Vec<ManagedProjectRoundState>,
    pub completed: bool,
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
    contract_path: String,
    codex_config_path: String,
    scenario: Option<ManagedProjectScenarioState>,
    roles: BTreeMap<String, ManagedProjectManifestRole>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ManagedProjectManifestRole {
    work_unit_id: String,
    agent_path: String,
    model: Option<String>,
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
    handoff: Option<StructuredWorkerHandoff>,
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
        let codex_home = CodexHome::discover_in(&cwd).ok();
        let service = match codex_home.clone() {
            Some(home) => DaemonService::with_codex_home(store, home),
            None => DaemonService::new(store),
        };
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

    pub fn status(&self) -> Result<DaemonStatus> {
        let codex_home = self.codex_home.as_ref();
        Ok(DaemonStatus {
            codex_home: codex_home.map(|home| home.root().to_path_buf()),
            codex_state_db: codex_home.map(|home| home.state_db_path()),
            codex_session_index: codex_home.map(|home| home.session_index_path()),
            project_count: self.store.count_projects()?,
            work_unit_count: self.store.count_work_units()?,
            bound_thread_count: self.store.count_bound_threads()?,
            ready_workspace_count: self.store.count_ready_workspaces()?,
        })
    }

    pub fn dashboard_summary(&self) -> Result<DashboardSummary> {
        let status = self.status()?;
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
        let manifest_path = repo_root.join(".tt").join("managed-project.toml");
        if !manifest_path.exists() {
            anyhow::bail!(
                "managed project manifest not found at {}",
                manifest_path.display()
            );
        }
        let manifest = load_managed_project_manifest(&manifest_path)?;
        let bootstrap = self.managed_project_bootstrap_from_manifest(&manifest_path, &manifest)?;
        Ok(ManagedProjectInspection {
            bootstrap,
            repository_summary: self.repository_summary(&repo_root)?,
        })
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
        let project = Project {
            id: uuid::Uuid::now_v7().to_string(),
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

        let contract_path = repo_root
            .join(".tt")
            .join("contracts")
            .join("worker-contract.md");
        let codex_config_path = repo_root.join(".codex").join("config.toml");
        write_managed_file(
            &codex_config_path,
            &render_codex_config(
                DEFAULT_AGENT_CONFIG_MAX_THREADS,
                DEFAULT_AGENT_CONFIG_MAX_DEPTH,
            ),
        )?;
        write_managed_file(
            &contract_path,
            &render_worker_contract(&repo_root, &slug, &base_branch),
        )?;

        let director_role = self.build_role_bootstrap(
            &repository,
            &project,
            ThreadRole::Director,
            &base_branch,
            director_model,
            &worktree_root,
            false,
        )?;
        let dev_role = self.build_role_bootstrap(
            &repository,
            &project,
            ThreadRole::Develop,
            &base_branch,
            dev_model,
            &worktree_root,
            true,
        )?;
        let test_role = self.build_role_bootstrap(
            &repository,
            &project,
            ThreadRole::Test,
            &base_branch,
            test_model,
            &worktree_root,
            true,
        )?;
        let integration_role = self.build_role_bootstrap(
            &repository,
            &project,
            ThreadRole::Integrate,
            &base_branch,
            integration_model,
            &worktree_root,
            true,
        )?;

        let manifest_path = repo_root.join(".tt").join("managed-project.toml");
        let manifest = build_managed_project_manifest(
            &project,
            &repo_root,
            &base_branch,
            &worktree_root,
            &contract_path,
            &codex_config_path,
            None,
            &[&director_role, &dev_role, &test_role, &integration_role],
        );
        save_managed_project_manifest(&manifest_path, &manifest)?;

        Ok(ManagedProjectBootstrap {
            project,
            repo_root,
            base_branch,
            worktree_root,
            manifest_path,
            contract_path,
            codex_config_path,
            scenario: None,
            roles: vec![director_role, dev_role, test_role, integration_role],
        })
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
    ) -> Result<ManagedProjectRoleBootstrap> {
        let now = Utc::now();
        let role_slug = role_slug(role);
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
        write_managed_file(
            &agent_path,
            &render_agent_file(role, model.as_deref(), &project.title, &project.objective),
        )?;

        let (branch_name, worktree_path) = if create_worktree {
            let branch_name = format!("tt/{}/{}", project.slug, role_slug);
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
            Status => DaemonResponse::Status(self.status()?),
            DashboardSummary => DaemonResponse::DashboardSummary(self.dashboard_summary()?),
            RepositorySummary { cwd } => {
                DaemonResponse::RepositorySummary(self.repository_summary(cwd)?)
            }
            InspectManagedProject { cwd } => {
                DaemonResponse::ManagedProjectInspection(self.inspect_managed_project(cwd)?)
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
        ThreadRole::Director | ThreadRole::Test | ThreadRole::Review | ThreadRole::Learn => {
            "read-only"
        }
        ThreadRole::Develop | ThreadRole::Integrate | ThreadRole::Handoff => "workspace-write",
        ThreadRole::Todo | ThreadRole::Chat | ThreadRole::Custom => "workspace-write",
    }
}

fn render_codex_config(max_threads: usize, max_depth: usize) -> String {
    format!("[agents]\nmax_threads = {max_threads}\nmax_depth = {max_depth}\n")
}

fn render_agent_file(
    role: ThreadRole,
    model: Option<&str>,
    project_title: &str,
    project_objective: &str,
) -> String {
    let role_roster = managed_project_role_roster();
    let mut output = String::new();
    output.push_str(&format!("name = {:?}\n", role_slug(role)));
    output.push_str(&format!("description = {:?}\n", role_description(role)));
    if let Some(model) = model {
        output.push_str(&format!("model = {:?}\n", model));
    }
    output.push_str(&format!("sandbox_mode = {:?}\n", role_sandbox_mode(role)));
    output.push_str("developer_instructions = \"\"\"\n");
    output.push_str(&format!(
        "You are the {} agent for {project_title}.\n",
        role_slug(role)
    ));
    output.push_str(&format!("Project objective: {project_objective}\n"));
    output.push_str("Project protocol:\n");
    output.push_str("- The operator talks to the director.\n");
    output.push_str("- The director is the only coordinator and speaks to the operator on behalf of the project.\n");
    output.push_str(
        "- Workers do not coordinate directly with each other; all assignments and escalations go through the director.\n",
    );
    output.push_str(
        "- Use the shared contract in .tt/contracts/worker-contract.md as the source of truth.\n",
    );
    output.push_str("Role roster:\n");
    for line in role_roster.lines() {
        output.push_str("- ");
        output.push_str(line);
        output.push('\n');
    }
    match role {
        ThreadRole::Director => {
            output.push_str("Your job is to turn operator intent into a plan, todo list, and dispatch decisions.\n");
            output.push_str(
                "Own branch strategy, worker assignment, phase transitions, and readiness.\n",
            );
            output.push_str("Keep the operator informed, request approval for merges or destructive cleanup, and summarize outcomes after each phase.\n");
            output.push_str("Do not implement product code unless explicitly instructed.\n");
        }
        ThreadRole::Develop => {
            output.push_str("Implement only the assigned slice in the provided worktree.\n");
            output.push_str("You report to the director, not to other workers or the operator.\n");
            output.push_str("Treat test as the validator and integration as the landing worker.\n");
            output.push_str("Report changed files, tests run, blockers, and next step.\n");
        }
        ThreadRole::Test => {
            output.push_str("Validate the assigned changes and report exact failures.\n");
            output.push_str("You report to the director, not to other workers or the operator.\n");
            output.push_str("Assume dev produced the change and integration will handle landing if tests pass.\n");
            output.push_str("Do not widen scope or rewrite implementation code.\n");
        }
        ThreadRole::Integrate => {
            output
                .push_str("Own merge prep, landing checks, and cleanup for the managed project.\n");
            output.push_str("You report to the director, not to other workers or the operator.\n");
            output.push_str(
                "Assume dev implemented the slice and test validated it before landing.\n",
            );
            output.push_str("Keep the landing path narrow and evidence-driven.\n");
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

fn render_worker_contract(repo_root: &Path, project_slug: &str, base_branch: &str) -> String {
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
## Roles\n\
{role_roster}\n\n\
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
## Rules\n\
- Stay inside the assigned worktree and scope.\n\
- Do not widen scope without director approval.\n\
- Keep evidence in the handoff, not in prose alone.\n",
        repo_root.display(),
        role_roster = role_roster
    )
}

fn build_managed_project_manifest(
    project: &Project,
    repo_root: &Path,
    base_branch: &str,
    worktree_root: &Path,
    contract_path: &Path,
    codex_config_path: &Path,
    scenario: Option<ManagedProjectScenarioState>,
    roles: &[&ManagedProjectRoleBootstrap],
) -> ManagedProjectManifest {
    let mut role_map = BTreeMap::new();
    for role in roles {
        role_map.insert(
            role_slug(role.role).to_string(),
            ManagedProjectManifestRole {
                work_unit_id: role.work_unit.id.clone(),
                agent_path: role.agent_path.display().to_string(),
                model: role.model.clone(),
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

    ManagedProjectManifest {
        schema: "tt-managed-project-v1".to_string(),
        project_id: project.id.clone(),
        slug: project.slug.clone(),
        title: project.title.clone(),
        objective: project.objective.clone(),
        repo_root: repo_root.display().to_string(),
        base_branch: base_branch.to_string(),
        worktree_root: worktree_root.display().to_string(),
        contract_path: contract_path.display().to_string(),
        codex_config_path: codex_config_path.display().to_string(),
        scenario,
        roles: role_map,
    }
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
        let contents =
            fs::read_to_string(path).with_context(|| format!("read scenario seed {}", path.display()))?;
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
        scaffold_managed_project_template(path, template.as_deref())?;
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
                model: role_manifest.model.clone(),
                branch_name: role_manifest.branch_name.clone(),
                worktree_path: role_manifest.worktree_path.as_ref().map(PathBuf::from),
                thread_id: role_manifest.thread_id.clone(),
                thread_name: role_manifest.thread_name.clone(),
                workspace_binding_id: role_manifest.workspace_binding_id.clone(),
            });
        }

        Ok(ManagedProjectBootstrap {
            project,
            repo_root: PathBuf::from(&manifest.repo_root),
            base_branch: manifest.base_branch.clone(),
            worktree_root: PathBuf::from(&manifest.worktree_root),
            manifest_path: manifest_path.to_path_buf(),
            contract_path: PathBuf::from(&manifest.contract_path),
            codex_config_path: PathBuf::from(&manifest.codex_config_path),
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
        let manifest_path = repo_root.join(".tt").join("managed-project.toml");
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
            &bootstrap.contract_path,
            &bootstrap.codex_config_path,
            bootstrap.scenario.clone(),
            &role_refs,
        );
        save_managed_project_manifest(&bootstrap.manifest_path, &manifest)?;
        Ok(bootstrap)
    }

    fn direct_managed_project(
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
        let manifest_path = repo_root.join(".tt").join("managed-project.toml");
        let mut bootstrap = if manifest_path.exists() {
            let manifest = load_managed_project_manifest(&manifest_path)?;
            self.managed_project_bootstrap_from_manifest(&manifest_path, &manifest)?
        } else {
            self.open_managed_project(
                cwd,
                title,
                objective,
                base_branch,
                worktree_root,
                director_model,
                dev_model,
                test_model,
                integration_model,
            )?
        };

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
        if let Some(scenario_kind) = scenario.as_deref() {
            let seed = load_managed_project_seed(seed_file.as_deref())?;
            let scenario_state = self.run_managed_project_scenario(
                &mut bootstrap,
                scenario_kind,
                &seed,
            )?;
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
        let manifest_path = repo_root.join(".tt").join("managed-project.toml");
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
            &bootstrap.contract_path,
            &bootstrap.codex_config_path,
            bootstrap.scenario.clone(),
            &role_refs,
        );
        save_managed_project_manifest(&bootstrap.manifest_path, &manifest)?;
        Ok(bootstrap)
    }

    fn run_managed_project_scenario(
        &self,
        bootstrap: &mut ManagedProjectBootstrap,
        scenario_kind: &str,
        seed: &ManagedProjectScenarioSeed,
    ) -> Result<ManagedProjectScenarioState> {
        if scenario_kind != "rust-taskflow-four-round" {
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

        let mut state = ManagedProjectScenarioState {
            scenario_id: uuid::Uuid::now_v7().to_string(),
            scenario_kind: scenario_kind.to_string(),
            operator_seed: seed.operator_seed.clone(),
            current_round: 0,
            current_phase: "plan".to_string(),
            pending_approval: None,
            rounds: Vec::new(),
            completed: false,
        };

        let round_specs = taskflow_round_specs();
        let mut worker_context = String::new();

        for spec in &round_specs {
            state.current_round = spec.round_number;
            state.current_phase = spec.phase.to_string();
            eprintln!(
                "tt director scenario {} round {} phase {} starting",
                scenario_kind, spec.round_number, spec.phase
            );

            let mut round = ManagedProjectRoundState {
                round_number: spec.round_number,
                phase: spec.phase.to_string(),
                director_turn_id: None,
                director_summary: None,
                role_handoffs: BTreeMap::new(),
            };

            let director_prompt = build_director_round_prompt(
                bootstrap,
                spec,
                &seed.operator_seed,
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
                &bootstrap.repo_root,
                &director,
                director_thread_id,
                &director_prompt,
            )?;
            eprintln!(
                "tt director scenario {} round {} director completed turn {}",
                scenario_kind, spec.round_number, director_turn.turn_id
            );
            round.director_turn_id = Some(director_turn.turn_id.clone());
            round.director_summary = Some(director_turn.summary.clone());

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
                let thread_id = role.thread_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("managed project role `{}` is not attached", role_slug(role.role))
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
                write_scenario_artifact(
                    bootstrap,
                    &state.scenario_id,
                    spec.round_number,
                    &format!("{}-prompt.txt", role_slug(role.role)),
                    &worker_prompt,
                )?;
                let handoff =
                    self.run_role_prompt(&bootstrap.repo_root, role, thread_id, &worker_prompt)?;
                let structured_handoff = handoff
                    .handoff
                    .clone()
                    .unwrap_or_else(|| seeded_worker_handoff(spec, role.role));
                let handoff_summary = serde_json::to_string_pretty(&structured_handoff)?;
                write_scenario_artifact(
                    bootstrap,
                    &state.scenario_id,
                    spec.round_number,
                    &format!("{}-handoff.txt", role_slug(role.role)),
                    &handoff_summary,
                )?;
                eprintln!(
                    "tt director scenario {} round {} {} completed turn {}",
                    scenario_kind,
                    spec.round_number,
                    role_slug(role.role),
                    handoff.turn_id
                );
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
            state.rounds.push(round);
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
        eprintln!("tt director scenario {} completed", scenario_kind);
        Ok(state)
    }

    fn run_role_prompt(
        &self,
        repo_root: &Path,
        role_bootstrap: &ManagedProjectRoleBootstrap,
        thread_id: &str,
        prompt: &str,
    ) -> Result<ManagedProjectTurnOutcome> {
        let cwd = role_bootstrap
            .worktree_path
            .as_deref()
            .unwrap_or(repo_root);
        eprintln!(
            "tt director prompting role {} thread {} cwd {}",
            role_slug(role_bootstrap.role),
            thread_id,
            cwd.display()
        );
        let client = self.codex_runtime_client(cwd)?;
        let output_schema = matches!(
            role_bootstrap.role,
            ThreadRole::Develop | ThreadRole::Test | ThreadRole::Integrate
        )
        .then(worker_handoff_output_schema);
        let turn = client.start_turn(
            thread_id,
            prompt,
            Some(cwd),
            role_bootstrap.model.clone(),
            output_schema,
        )?;
        eprintln!(
            "tt director role {} started turn {}",
            role_slug(role_bootstrap.role),
            turn.id
        );
        let completed = client.wait_for_turn_completion(thread_id, &turn.id)?;
        eprintln!(
            "tt director role {} observed turn {} status {:?}",
            role_slug(role_bootstrap.role),
            completed.id,
            completed.status
        );
        let thread = client
            .resume_thread_full(thread_id, Some(cwd), role_bootstrap.model.clone())?
            .ok_or_else(|| anyhow::anyhow!("thread `{thread_id}` not found after turn"))?;
        let finished_turn = thread
            .turns
            .into_iter()
            .find(|candidate| candidate.id == completed.id)
            .ok_or_else(|| anyhow::anyhow!("turn `{}` not found after completion", completed.id))?;
        match finished_turn.status {
            protocol::TurnStatus::Completed => Ok(ManagedProjectTurnOutcome {
                turn_id: finished_turn.id,
                summary: summarize_turn_items(&finished_turn.items),
                handoff: parse_structured_worker_handoff(&finished_turn.items)?,
            }),
            protocol::TurnStatus::Failed => anyhow::bail!(
                "turn `{}` for role `{}` failed",
                finished_turn.id,
                role_slug(role_bootstrap.role)
            ),
            protocol::TurnStatus::Interrupted => anyhow::bail!(
                "turn `{}` for role `{}` was interrupted",
                finished_turn.id,
                role_slug(role_bootstrap.role)
            ),
            protocol::TurnStatus::InProgress => anyhow::bail!(
                "turn `{}` for role `{}` did not complete",
                finished_turn.id,
                role_slug(role_bootstrap.role)
            ),
        }
    }

    fn save_managed_project_bootstrap(&self, bootstrap: &ManagedProjectBootstrap) -> Result<()> {
        let role_refs: Vec<_> = bootstrap.roles.iter().collect();
        let manifest = build_managed_project_manifest(
            &bootstrap.project,
            &bootstrap.repo_root,
            &bootstrap.base_branch,
            &bootstrap.worktree_root,
            &bootstrap.contract_path,
            &bootstrap.codex_config_path,
            bootstrap.scenario.clone(),
            &role_refs,
        );
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
                "Managed TT project `{}` role `{}`. Follow `.tt/contracts/worker-contract.md` and stay inside the assigned scope.",
                project.slug,
                role_slug(role)
            )),
            developer_instructions: Some(agent_file.developer_instructions.clone()),
            ephemeral: Some(false),
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
                "/target\n/.tt\n/.codex\n*.log\n",
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

fn run_command<I, S>(cwd: &Path, program: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let status = Command::new(program).current_dir(cwd).args(args).status()?;
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

fn default_worktree_root(repo_root: &Path, project_slug: &str) -> PathBuf {
    let base = repo_root.parent().unwrap_or(repo_root);
    base.join(".tt-worktrees").join(project_slug)
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

fn taskflow_round_specs() -> [ManagedProjectRoundSpec; 4] {
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
            integration_goal: "Finalize README and examples, verify merge readiness, and prepare the repo for landing after approval.",
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
    format!(
        "Managed project round {} phase `{}` for project `{}`.\n{}\nOperator seed:\n{}\n\nPrior worker handoffs:\n{}\n\nWrite a concrete project update for this round. Include: plan, todo, dispatch decisions, blockers, and next step.",
        spec.round_number,
        spec.phase,
        bootstrap.project.slug,
        approval_text,
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

fn summarize_turn_items(items: &[protocol::ThreadItem]) -> String {
    let mut chunks = Vec::new();
    for item in items {
        match item {
            protocol::ThreadItem::AgentMessage { text, .. } | protocol::ThreadItem::Plan { text, .. } => {
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

fn parse_structured_worker_handoff(
    items: &[protocol::ThreadItem],
) -> Result<Option<StructuredWorkerHandoff>> {
    let text = items.iter().find_map(|item| match item {
        protocol::ThreadItem::AgentMessage { text, .. } => Some(text.trim()),
        _ => None,
    });
    let Some(text) = text else {
        return Ok(None);
    };
    if text.is_empty() {
        return Ok(None);
    }
    let handoff: StructuredWorkerHandoff = serde_json::from_str(text)
        .with_context(|| "parse structured worker handoff JSON".to_string())?;
    validate_structured_worker_handoff(&handoff)?;
    Ok(Some(handoff))
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

pub fn socket_path_for(cwd: impl AsRef<Path>) -> PathBuf {
    cwd.as_ref()
        .join(".tt")
        .join("runtime")
        .join(TT_DAEMON_SOCKET_NAME)
}

pub fn request_for_cwd(cwd: impl AsRef<Path>, request: DaemonRequest) -> Result<DaemonResponse> {
    let cwd = cwd.as_ref();
    let socket_path = socket_path_for(cwd);
    if let Ok(client) = DaemonClient::connect(&socket_path) {
        if let Ok(response) = client.request(request.clone()) {
            return Ok(response);
        }
    }
    DaemonRuntime::open(cwd)?.request(request)
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
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

        let status = service.status().expect("status");
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
        assert!(bootstrap.manifest_path.exists());
        assert_eq!(bootstrap.roles.len(), 4);
        assert!(bootstrap.roles.iter().all(|role| role.agent_path.exists()));

        let director_role = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Director)
            .expect("director role");
        assert!(director_role.branch_name.is_none());
        assert!(director_role.worktree_path.is_none());

        let dev_role = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Develop)
            .expect("dev role");
        assert_eq!(dev_role.branch_name.as_deref(), Some("tt/alpha/dev"));
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
        assert_eq!(test_role.branch_name.as_deref(), Some("tt/alpha/test"));

        let integration_role = bootstrap
            .roles
            .iter()
            .find(|role| role.role == ThreadRole::Integrate)
            .expect("integration role");
        assert_eq!(
            integration_role.branch_name.as_deref(),
            Some("tt/alpha/integration")
        );

        let inspection = GitRepository::discover(&repo)
            .expect("discover repo")
            .expect("repo")
            .inspect_repository()
            .expect("inspect repo");
        assert!(inspection.worktrees.len() >= 4);
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
            branch_name: Some("tt/alpha/dev".into()),
            worktree_path: Some(dir.path().join("worktree")),
            thread_id: Some("thread-1".into()),
            thread_name: Some("alpha-dev".into()),
            workspace_binding_id: Some("alpha:dev".into()),
        };
        let manifest = build_managed_project_manifest(
            &project,
            dir.path(),
            "main",
            &dir.path().join(".tt-worktrees/alpha"),
            &dir.path().join(".tt/contracts/worker-contract.md"),
            &dir.path().join(".codex/config.toml"),
            None,
            &[&role],
        );
        let path = dir.path().join("managed-project.toml");
        save_managed_project_manifest(&path, &manifest).expect("save manifest");
        let loaded = load_managed_project_manifest(&path).expect("load manifest");
        let loaded_role = loaded.roles.get("dev").expect("dev role");
        assert_eq!(loaded_role.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(loaded_role.thread_name.as_deref(), Some("alpha-dev"));
        assert_eq!(
            loaded_role.workspace_binding_id.as_deref(),
            Some("alpha:dev")
        );
    }

    #[test]
    fn managed_project_contract_mentions_director_and_workers() {
        let contract = render_worker_contract(Path::new("/repo"), "alpha", "main");
        assert!(contract.contains("TT Managed Project Contract"));
        assert!(contract.contains("Coordination Model"));
        assert!(contract.contains("The operator talks to the director."));
        assert!(contract.contains("## Roles"));
        assert!(contract.contains("plan"));
        assert!(contract.contains("merge"));
    }

    #[test]
    fn director_agent_instructions_include_operator_and_roster() {
        let instructions = render_agent_file(
            ThreadRole::Director,
            Some("gpt-5.4"),
            "Alpha",
            "Ship the alpha slice",
        );
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
        let response = client.request(DaemonRequest::Status).expect("status");
        assert!(matches!(response, DaemonResponse::Status(_)));

        drop(handle);
    }

    #[test]
    fn init_managed_project_scaffold_is_workspace_isolated() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("taskflow");
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
}
