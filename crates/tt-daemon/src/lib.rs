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
use clap as _;
use serde::{Deserialize, Serialize};
use tt_codex::{CodexHome, CodexRuntimeClient, CodexThreadRuntimeSnapshot};
use tt_domain::{
    MergeAuthorizationStatus, MergeExecutionStatus, MergeReadiness, MergeRun, Project,
    ProjectStatus, ThreadBinding, ThreadBindingStatus, WorkUnit, WorkUnitStatus, WorkspaceBinding,
    WorkspaceStatus,
};
use tt_store::OverlayStore;
use tt_ui_core::{CodexThreadDetail, CodexThreadSummary, DashboardSummary, GitRepositorySummary};

pub const TT_DAEMON_API_VERSION: &str = "v2";
pub const TT_DAEMON_SOCKET_NAME: &str = "ttd.sock";

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
    ListMergeRuns,
    GetMergeRun {
        id: String,
    },
    UpsertMergeRun {
        run: MergeRun,
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
        let Some(catalog) = CodexHome::discover_in(cwd)?.session_catalog().ok()
        else {
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
        let Some(catalog) = CodexHome::discover_in(cwd)?.session_catalog().ok()
        else {
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
            ListMergeRuns => DaemonResponse::MergeRuns(self.list_merge_runs()?),
            GetMergeRun { id } => DaemonResponse::MergeRun(self.get_merge_run(&id)?),
            UpsertMergeRun { run } => {
                self.upsert_merge_run(&run)?;
                DaemonResponse::Unit
            }
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
            } => DaemonResponse::CodexThreadDetail(Some(self.start_codex_thread(
                cwd,
                model,
                ephemeral,
            )?)),
            ResumeCodexThread {
                cwd,
                selector,
                model,
            } => DaemonResponse::CodexThreadDetail(self.resume_codex_thread(
                cwd,
                &selector,
                model,
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
