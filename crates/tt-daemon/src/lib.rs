//! Local orchestration for TT v2.
//!
//! The daemon coordinates TT overlay state, Codex runtime state, and git state.
//! It owns the local request/response API used by the TUI and CLI.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    thread,
};

use anyhow::Result;
use chrono::Utc;
use clap as _;
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum DaemonResponse {
    Unit,
    Count(usize),
    Status(DaemonStatus),
    DashboardSummary(DashboardSummary),
    RepositorySummary(Option<GitRepositorySummary>),
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
    pub roles: Vec<ManagedProjectRoleBootstrap>,
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
        let Some(snapshot) = client.read_thread(selector, include_turns)? else {
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
        let Some(repository) = GitRepository::discover(cwd)? else {
            anyhow::bail!("managed project open requires a git repository");
        };
        let inspection = repository.inspect_repository()?;
        let repo_root = repository.repository_root.clone();
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
        write_managed_file(
            &manifest_path,
            &render_project_manifest(
                &project,
                &repo_root,
                &base_branch,
                &worktree_root,
                &contract_path,
                &codex_config_path,
                &[&director_role, &dev_role, &test_role, &integration_role],
            ),
        )?;

        Ok(ManagedProjectBootstrap {
            project,
            repo_root,
            base_branch,
            worktree_root,
            manifest_path,
            contract_path,
            codex_config_path,
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
            format!("Coordinate workers, branch strategy, and handoffs for {project_title}")
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
        ThreadRole::Director => "Coordinates TT-managed workers, branch strategy, and handoffs.",
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
    output
        .push_str("Follow .tt/contracts/worker-contract.md and stay inside your assigned scope.\n");
    match role {
        ThreadRole::Director => {
            output.push_str("Own branch strategy, worker assignment, handoffs, and readiness.\n");
            output.push_str("Do not implement product code unless explicitly instructed.\n");
        }
        ThreadRole::Develop => {
            output.push_str("Implement only the assigned slice in the provided worktree.\n");
            output.push_str("Report changed files, tests run, blockers, and next step.\n");
        }
        ThreadRole::Test => {
            output.push_str("Validate the assigned changes and report exact failures.\n");
            output.push_str("Do not widen scope or rewrite implementation code.\n");
        }
        ThreadRole::Integrate => {
            output
                .push_str("Own merge prep, landing checks, and cleanup for the managed project.\n");
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
    format!(
        "# TT Worker Contract\n\n\
Project: `{project_slug}`\n\
Repository root: `{}`\n\
Base branch: `{base_branch}`\n\n\
## Roles\n\
- `director`: assigns work, manages branching, and approves handoffs.\n\
- `dev`: implements the assigned slice only.\n\
- `test`: validates the branch and reports failures exactly.\n\
- `integration`: prepares landing and merge readiness.\n\n\
## Handoff Format\n\
- `status`: `blocked`, `needs-review`, or `complete`\n\
- `changed_files`: list of paths\n\
- `tests_run`: list of commands\n\
- `blockers`: list of blockers or `[]`\n\
- `next_step`: the next concrete action\n\n\
## Rules\n\
- Stay inside the assigned worktree and scope.\n\
- Do not widen scope without director approval.\n\
- Keep evidence in the handoff, not in prose alone.\n",
        repo_root.display()
    )
}

fn render_project_manifest(
    project: &Project,
    repo_root: &Path,
    base_branch: &str,
    worktree_root: &Path,
    contract_path: &Path,
    codex_config_path: &Path,
    roles: &[&ManagedProjectRoleBootstrap],
) -> String {
    let mut output = String::new();
    output.push_str("schema = \"tt-managed-project-v1\"\n");
    output.push_str(&format!("project_id = {:?}\n", project.id));
    output.push_str(&format!("slug = {:?}\n", project.slug));
    output.push_str(&format!("title = {:?}\n", project.title));
    output.push_str(&format!("objective = {:?}\n", project.objective));
    output.push_str(&format!(
        "repo_root = {:?}\n",
        repo_root.display().to_string()
    ));
    output.push_str(&format!("base_branch = {:?}\n", base_branch));
    output.push_str(&format!(
        "worktree_root = {:?}\n",
        worktree_root.display().to_string()
    ));
    output.push_str(&format!(
        "contract_path = {:?}\n",
        contract_path.display().to_string()
    ));
    output.push_str(&format!(
        "codex_config_path = {:?}\n\n",
        codex_config_path.display().to_string()
    ));
    output.push_str("[roles]\n");
    for role in roles {
        output.push_str(&format!("[roles.{}]\n", role_slug(role.role)));
        output.push_str(&format!("work_unit_id = {:?}\n", role.work_unit.id));
        output.push_str(&format!(
            "agent_path = {:?}\n",
            role.agent_path.display().to_string()
        ));
        if let Some(model) = role.model.as_ref() {
            output.push_str(&format!("model = {:?}\n", model));
        }
        if let Some(branch_name) = role.branch_name.as_ref() {
            output.push_str(&format!("branch_name = {:?}\n", branch_name));
        }
        if let Some(worktree_path) = role.worktree_path.as_ref() {
            output.push_str(&format!(
                "worktree_path = {:?}\n",
                worktree_path.display().to_string()
            ));
        }
        output.push('\n');
    }
    output
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

fn resolve_path(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
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
}
