use std::collections::{BTreeSet, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use chrono::{TimeZone, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, Notify, RwLock, broadcast, mpsc};
use tokio::time::{Duration, sleep};
use tracing::{debug, info, warn};
use uuid::Uuid;

use orcas_codex::types;
use orcas_codex::{
    CodexClient, CodexDaemonManager, DaemonLaunch as CodexDaemonLaunch, LocalCodexDaemonManager,
    RejectingApprovalRouter, WebSocketTransport,
};
use orcas_core::authority::{AuthorityCommand, AuthorityQueryStore};
use orcas_core::ipc;
use orcas_core::jsonrpc::{
    JsonRpcError, JsonRpcErrorObject, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse,
};
use orcas_core::{
    AppConfig, AppPaths, Assignment, AssignmentCommunicationPacket, AssignmentCommunicationRecord,
    AssignmentCommunicationSeed, AssignmentModeSpec, AssignmentStatus, CodexConnectionMode,
    CodexThreadAssignment, CodexThreadAssignmentStatus, CodexThreadBootstrapState,
    CollaborationState, ConnectionState, Decision, DecisionType, DraftAssignment, EventEnvelope,
    ImplementModeSpec, JsonSessionStore, OrcasError, OrcasEvent, OrcasResult, OrcasSessionStore,
    Report, SupervisorContextPack, SupervisorProposal, SupervisorProposalFailure,
    SupervisorProposalFailureStage, SupervisorProposalRecord, SupervisorProposalStatus,
    SupervisorProposalTriggerKind, SupervisorReasonerUsage, SupervisorTurnDecision,
    SupervisorTurnDecisionKind, SupervisorTurnDecisionStatus, SupervisorTurnProposalKind,
    ThreadMetadata, WorkUnit, WorkUnitStatus, Worker, WorkerSession, WorkerSessionAttachability,
    WorkerSessionRuntimeStatus, WorkerStatus, Workstream, WorkstreamStatus,
};

use crate::assignment_comm::parse::parse_worker_report_for_turn;
use crate::assignment_comm::policy::validate_assignment_packet;
use crate::assignment_comm::render::build_assignment_communication_record;
use crate::assignment_comm::stable_fingerprint;
use crate::authority_store::{AuthorityMutationResult, AuthoritySqliteStore};
use crate::process::{OrcasDaemonProcessManager, OrcasRuntimeOverrides, apply_runtime_overrides};
use crate::supervisor::{
    ResponsesApiReasoner, SupervisorReasoner, apply_edits, build_context_pack,
    compile_assignment_instructions, proposal_freshness_error, state_anchor_freshness_error,
    validate_proposal,
};

const RECENT_EVENT_LIMIT: usize = 200;
const CLIENT_WRITE_QUEUE: usize = 256;

#[derive(Debug)]
struct DaemonState {
    upstream: ConnectionState,
    session: ipc::SessionState,
    threads: HashMap<String, ipc::ThreadView>,
    turns: HashMap<TurnKey, ipc::TurnStateView>,
    recent_thread_id: Option<String>,
    collaboration: CollaborationState,
}

#[derive(Debug, Default)]
struct BridgeSnapshotMetadata {
    workstream_bridge_ids: BTreeSet<String>,
    work_unit_bridge_ids: BTreeSet<String>,
    hidden_workstream_ids: BTreeSet<String>,
    hidden_work_unit_ids: BTreeSet<String>,
}

impl BridgeSnapshotMetadata {
    fn workstream_source_kind(&self, workstream_id: &str) -> ipc::PlanningSummarySourceKind {
        if self.workstream_bridge_ids.contains(workstream_id) {
            ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
        } else {
            ipc::PlanningSummarySourceKind::Collaboration
        }
    }

    fn work_unit_source_kind(&self, work_unit_id: &str) -> ipc::PlanningSummarySourceKind {
        if self.work_unit_bridge_ids.contains(work_unit_id) {
            ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
        } else {
            ipc::PlanningSummarySourceKind::Collaboration
        }
    }
}

impl Default for DaemonState {
    fn default() -> Self {
        Self {
            upstream: ConnectionState {
                endpoint: String::new(),
                status: "disconnected".to_string(),
                detail: None,
            },
            session: ipc::SessionState::default(),
            threads: HashMap::new(),
            turns: HashMap::new(),
            recent_thread_id: None,
            collaboration: CollaborationState::default(),
        }
    }
}

#[derive(Debug, Clone)]
struct PreparedAssignment {
    assignment: Assignment,
    created_new: bool,
}

#[derive(Debug, Clone)]
struct ProposalGenerationRequest {
    work_unit_id: String,
    source_report_id: Option<String>,
    requested_by: String,
    note: Option<String>,
    trigger_kind: SupervisorProposalTriggerKind,
}

#[derive(Debug, Clone, Copy)]
enum ProposalDuplicatePolicy {
    Manual { supersede_open: bool },
    Auto,
}

#[derive(Debug, Clone)]
struct PreparedProposalGeneration {
    collaboration: CollaborationState,
    source_report_id: String,
}

#[derive(Debug, Clone)]
enum PreparedProposalGenerationOutcome {
    Ready(PreparedProposalGeneration),
    Suppressed { reason: String },
}

#[derive(Debug, Clone)]
enum ProposalGenerationOutcome {
    Created(SupervisorProposalRecord),
    Suppressed { reason: String },
}

#[derive(Debug, Clone, Eq)]
struct TurnKey {
    thread_id: String,
    turn_id: String,
}

impl TurnKey {
    fn new(thread_id: &str, turn_id: &str) -> Self {
        Self {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
        }
    }
}

impl PartialEq for TurnKey {
    fn eq(&self, other: &Self) -> bool {
        self.thread_id == other.thread_id && self.turn_id == other.turn_id
    }
}

impl Hash for TurnKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.thread_id.hash(state);
        self.turn_id.hash(state);
    }
}

pub struct OrcasDaemonService {
    paths: AppPaths,
    config: AppConfig,
    runtime: ipc::DaemonRuntimeMetadata,
    store: Arc<JsonSessionStore>,
    authority_store: Arc<AuthoritySqliteStore>,
    codex_daemon: Arc<dyn CodexDaemonManager>,
    codex_client: Arc<CodexClient>,
    state: RwLock<DaemonState>,
    recent_events: Mutex<VecDeque<ipc::EventSummary>>,
    connect_gate: Mutex<()>,
    event_tx: broadcast::Sender<ipc::DaemonEventEnvelope>,
    client_count: AtomicUsize,
    shutdown: Notify,
    supervisor_reasoner: Arc<dyn SupervisorReasoner>,
}

impl OrcasDaemonService {
    pub async fn load_from_env() -> OrcasResult<Arc<Self>> {
        debug!("loading daemon service from environment");
        Self::load_with_runtime_overrides(Self::overrides_from_env()).await
    }

    pub async fn load_with_runtime_overrides(
        runtime_overrides: OrcasRuntimeOverrides,
    ) -> OrcasResult<Arc<Self>> {
        let paths = AppPaths::discover()?;
        paths.ensure().await?;
        let mut config = AppConfig::write_default_if_missing(&paths).await?;
        apply_runtime_overrides(&mut config, &runtime_overrides);
        let runtime =
            OrcasDaemonProcessManager::runtime_metadata_for_current_process(&paths).await?;

        let store = Arc::new(JsonSessionStore::new(paths.clone(), config.clone()));
        let authority_store = Arc::new(AuthoritySqliteStore::open(paths.clone())?);
        let codex_daemon: Arc<dyn CodexDaemonManager> = Arc::new(LocalCodexDaemonManager::new(
            config.codex.clone(),
            &paths,
            config.defaults.cwd.clone(),
        ));
        let codex_client = CodexClient::new(
            Arc::new(WebSocketTransport::new(config.codex.listen_url.clone())),
            config.codex.reconnect.clone(),
            Arc::new(RejectingApprovalRouter),
        );
        let (event_tx, _) = broadcast::channel(512);

        let service = Arc::new(Self {
            paths,
            supervisor_reasoner: Arc::new(ResponsesApiReasoner::new(config.clone())),
            config,
            runtime,
            store,
            authority_store,
            codex_daemon,
            codex_client,
            state: RwLock::new(DaemonState::default()),
            recent_events: Mutex::new(VecDeque::with_capacity(RECENT_EVENT_LIMIT)),
            connect_gate: Mutex::new(()),
            event_tx,
            client_count: AtomicUsize::new(0),
            shutdown: Notify::new(),
        });

        service.initialize_state().await?;
        service.spawn_codex_event_bridge();
        debug!(pid = std::process::id(), "daemon service initialized");

        Ok(service)
    }

    pub async fn run(self: Arc<Self>) -> OrcasResult<()> {
        debug!(
            socket = %self.paths.socket_file.display(),
            pid = std::process::id(),
            "starting orcasd service loop"
        );
        let listener = self.bind_listener().await?;
        OrcasDaemonProcessManager::write_runtime_metadata(&self.paths, &self.runtime).await?;
        let _socket_guard = SocketGuard::new(
            self.paths.socket_file.clone(),
            self.paths.daemon_metadata_file.clone(),
        );

        if let Err(error) = self.connect_upstream().await {
            warn!(%error, "initial Codex connect failed");
            self.emit(ipc::DaemonEvent::Warning {
                message: format!("initial Codex connect failed: {error}"),
            })
            .await;
        }

        info!(socket = %self.paths.socket_file.display(), "orcasd listening");

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    let (stream, _) = accept_result?;
                    let service = Arc::clone(&self);
                    tokio::spawn(async move {
                        service.handle_client(stream).await;
                    });
                }
                _ = Self::wait_for_shutdown_signal() => break,
                _ = self.shutdown.notified() => {
                    break;
                }
            }
        }

        Ok(())
    }

    async fn wait_for_shutdown_signal() {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};

            let mut sigterm = match signal(SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(error) => {
                    warn!(%error, "failed to listen for SIGTERM");
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };

            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    if let Err(error) = signal {
                        warn!(%error, "failed to listen for ctrl-c");
                    }
                }
                _ = sigterm.recv() => {}
            }
        }

        #[cfg(not(unix))]
        {
            if let Err(error) = tokio::signal::ctrl_c().await {
                warn!(%error, "failed to listen for shutdown signal");
            }
        }
    }

    async fn bind_listener(&self) -> OrcasResult<UnixListener> {
        debug!(socket = %self.paths.socket_file.display(), "binding unix socket");
        self.paths.ensure().await?;
        if tokio::fs::try_exists(&self.paths.socket_file).await? {
            if crate::process::OrcasDaemonProcessManager::socket_responsive(&self.paths.socket_file)
                .await
            {
                return Err(OrcasError::Transport(format!(
                    "Orcas daemon socket already active at {}",
                    self.paths.socket_file.display()
                )));
            }
            tokio::fs::remove_file(&self.paths.socket_file).await?;
        }
        UnixListener::bind(&self.paths.socket_file).map_err(Into::into)
    }

    async fn initialize_state(&self) -> OrcasResult<()> {
        debug!("initializing daemon state from persisted store");
        let stored = self.store.load().await.unwrap_or_default();
        let mut state = self.state.write().await;
        state.upstream = ConnectionState {
            endpoint: self.config.codex.listen_url.clone(),
            status: "disconnected".to_string(),
            detail: None,
        };
        state.threads = if stored.thread_views.is_empty() {
            stored
                .registry
                .threads
                .values()
                .map(|metadata| {
                    let view = Self::thread_view_from_metadata(metadata);
                    (view.summary.id.clone(), view)
                })
                .collect()
        } else {
            stored.thread_views.into_iter().collect()
        };
        state.turns = stored
            .turn_states
            .into_values()
            .map(|turn| (TurnKey::new(&turn.thread_id, &turn.turn_id), turn))
            .collect();
        state.recent_thread_id = state
            .threads
            .values()
            .max_by_key(|thread| thread.summary.updated_at)
            .map(|thread| thread.summary.id.clone());
        state.collaboration = stored.collaboration;
        Ok(())
    }

    fn spawn_codex_event_bridge(self: &Arc<Self>) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let mut subscription = service.codex_client.subscribe();
            loop {
                match subscription.recv().await {
                    Ok(event) => {
                        service.apply_codex_event(event).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        service
                            .emit(ipc::DaemonEvent::Warning {
                                message: format!(
                                    "Codex event bridge lagged; skipped {skipped} events"
                                ),
                            })
                            .await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    async fn handle_client(self: Arc<Self>, stream: UnixStream) {
        self.client_count.fetch_add(1, Ordering::SeqCst);
        let _client_guard = ClientGuard::new(Arc::clone(&self));

        let (read_half, mut write_half) = stream.into_split();
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<String>(CLIENT_WRITE_QUEUE);
        let writer = tokio::spawn(async move {
            while let Some(raw) = outbound_rx.recv().await {
                if write_half.write_all(raw.as_bytes()).await.is_err() {
                    break;
                }
                if write_half.write_all(b"\n").await.is_err() {
                    break;
                }
            }
        });

        let mut lines = BufReader::new(read_half).lines();
        let mut subscription_task: Option<tokio::task::JoinHandle<()>> = None;

        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if let Err(error) = self
                        .handle_client_message(&line, outbound_tx.clone(), &mut subscription_task)
                        .await
                    {
                        warn!(%error, "ipc client message failed");
                        let _ =
                            Self::send_error(&outbound_tx, None, -32000, &error.to_string(), None)
                                .await;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    warn!(%error, "ipc client read failed");
                    break;
                }
            }
        }

        if let Some(task) = subscription_task.take() {
            task.abort();
        }
        drop(outbound_tx);
        let _ = writer.await;
    }

    async fn handle_client_message(
        self: &Arc<Self>,
        raw: &str,
        outbound: mpsc::Sender<String>,
        subscription_task: &mut Option<tokio::task::JoinHandle<()>>,
    ) -> OrcasResult<()> {
        let message: JsonRpcMessage = serde_json::from_str(raw)?;
        match message {
            JsonRpcMessage::Request(request) => {
                let start = std::time::Instant::now();
                debug!(
                    request_id = ?request.id,
                    method = request.method.as_str(),
                    "ipc request received"
                );
                match self
                    .handle_request(request.clone(), outbound.clone(), subscription_task)
                    .await
                {
                    Ok(()) => {
                        debug!(
                            request_id = ?request.id,
                            method = request.method.as_str(),
                            duration_ms = start.elapsed().as_millis() as u64,
                            "ipc request completed"
                        );
                    }
                    Err(error) => {
                        warn!(
                            request_id = ?request.id,
                            method = request.method.as_str(),
                            duration_ms = start.elapsed().as_millis() as u64,
                            error = %error,
                            "ipc request failed"
                        );
                        let _ = Self::send_error(
                            &outbound,
                            Some(request.id),
                            -32000,
                            &error.to_string(),
                            None,
                        )
                        .await;
                    }
                }
            }
            JsonRpcMessage::Notification(_) => {}
            JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_) => {}
        }
        Ok(())
    }

    async fn handle_request(
        self: &Arc<Self>,
        request: JsonRpcRequest,
        outbound: mpsc::Sender<String>,
        subscription_task: &mut Option<tokio::task::JoinHandle<()>>,
    ) -> OrcasResult<()> {
        debug!(
            request_id = ?request.id,
            method = request.method.as_str(),
            "processing ipc request"
        );
        if request.method == ipc::methods::DAEMON_STOP {
            let response = serde_json::to_value(self.daemon_stop().await?)?;
            Self::send_response(&outbound, request.id, response).await?;
            let service = Arc::clone(self);
            tokio::spawn(async move {
                sleep(Duration::from_millis(100)).await;
                service.shutdown.notify_waiters();
            });
            return Ok(());
        }

        let result = match request.method.as_str() {
            ipc::methods::DAEMON_STATUS => serde_json::to_value(self.daemon_status().await?)?,
            ipc::methods::DAEMON_CONNECT => serde_json::to_value(self.daemon_connect().await?)?,
            ipc::methods::STATE_GET => {
                let _: ipc::StateGetRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.state_get().await?)?
            }
            ipc::methods::SESSION_GET_ACTIVE => {
                let _: ipc::SessionGetActiveRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.session_get_active().await?)?
            }
            ipc::methods::MODELS_LIST => serde_json::to_value(self.models_list().await?)?,
            ipc::methods::THREADS_LIST => {
                let _: ipc::ThreadsListRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.threads_list().await?)?
            }
            ipc::methods::THREADS_LIST_SCOPED => {
                let _: ipc::ThreadsListScopedRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.threads_list_scoped().await?)?
            }
            ipc::methods::THREADS_LIST_LOADED => {
                let _: ipc::ThreadsListLoadedRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.threads_list_loaded().await?)?
            }
            ipc::methods::THREAD_START => {
                let params: ipc::ThreadStartRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.thread_start(params).await?)?
            }
            ipc::methods::THREAD_READ => {
                let params: ipc::ThreadReadRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.thread_read(params).await?)?
            }
            ipc::methods::THREAD_READ_HISTORY => {
                let params: ipc::ThreadReadHistoryRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.thread_read_history(params).await?)?
            }
            ipc::methods::THREAD_GET => {
                let params: ipc::ThreadGetRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.thread_get(params).await?)?
            }
            ipc::methods::THREAD_ATTACH => {
                let params: ipc::ThreadAttachRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.thread_attach(params).await?)?
            }
            ipc::methods::THREAD_DETACH => {
                let params: ipc::ThreadDetachRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.thread_detach(params).await?)?
            }
            ipc::methods::THREAD_RESUME => {
                let params: ipc::ThreadResumeRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.thread_resume(params).await?)?
            }
            ipc::methods::TURNS_LIST_ACTIVE => {
                let _: ipc::TurnsListActiveRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.turns_list_active().await?)?
            }
            ipc::methods::TURNS_RECENT => {
                let params: ipc::TurnsRecentRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.turns_recent(params).await?)?
            }
            ipc::methods::TURN_GET => {
                let params: ipc::TurnGetRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.turn_get(params).await?)?
            }
            ipc::methods::TURN_ATTACH => {
                let params: ipc::TurnAttachRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.turn_attach(params).await?)?
            }
            ipc::methods::TURN_START => {
                let params: ipc::TurnStartRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.turn_start(params).await?)?
            }
            ipc::methods::TURN_STEER => {
                let params: ipc::TurnSteerRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.turn_steer(params).await?)?
            }
            ipc::methods::TURN_INTERRUPT => {
                let params: ipc::TurnInterruptRequest =
                    Self::decode_params(request.params.clone())?;
                self.turn_interrupt(params).await?;
                serde_json::to_value(ipc::Empty::default())?
            }
            ipc::methods::WORKSTREAM_CREATE => {
                let params: ipc::WorkstreamCreateRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.workstream_create(params).await?)?
            }
            ipc::methods::WORKSTREAM_LIST => {
                let _: ipc::WorkstreamListRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.workstream_list().await?)?
            }
            ipc::methods::WORKSTREAM_GET => {
                let params: ipc::WorkstreamGetRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.workstream_get(params).await?)?
            }
            ipc::methods::WORKUNIT_CREATE => {
                let params: ipc::WorkunitCreateRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.workunit_create(params).await?)?
            }
            ipc::methods::WORKUNIT_LIST => {
                let params: ipc::WorkunitListRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.workunit_list(params).await?)?
            }
            ipc::methods::WORKUNIT_GET => {
                let params: ipc::WorkunitGetRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.workunit_get(params).await?)?
            }
            ipc::methods::AUTHORITY_HIERARCHY_GET => {
                let params: ipc::AuthorityHierarchyGetRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_hierarchy_get(params).await?)?
            }
            ipc::methods::AUTHORITY_DELETE_PLAN => {
                let params: ipc::AuthorityDeletePlanRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_delete_plan(params).await?)?
            }
            ipc::methods::AUTHORITY_WORKSTREAM_CREATE => {
                let params: ipc::AuthorityWorkstreamCreateRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_workstream_create(params).await?)?
            }
            ipc::methods::AUTHORITY_WORKSTREAM_EDIT => {
                let params: ipc::AuthorityWorkstreamEditRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_workstream_edit(params).await?)?
            }
            ipc::methods::AUTHORITY_WORKSTREAM_DELETE => {
                let params: ipc::AuthorityWorkstreamDeleteRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_workstream_delete(params).await?)?
            }
            ipc::methods::AUTHORITY_WORKSTREAM_LIST => {
                let params: ipc::AuthorityWorkstreamListRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_workstream_list(params).await?)?
            }
            ipc::methods::AUTHORITY_WORKSTREAM_GET => {
                let params: ipc::AuthorityWorkstreamGetRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_workstream_get(params).await?)?
            }
            ipc::methods::AUTHORITY_WORKUNIT_CREATE => {
                let params: ipc::AuthorityWorkunitCreateRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_workunit_create(params).await?)?
            }
            ipc::methods::AUTHORITY_WORKUNIT_EDIT => {
                let params: ipc::AuthorityWorkunitEditRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_workunit_edit(params).await?)?
            }
            ipc::methods::AUTHORITY_WORKUNIT_DELETE => {
                let params: ipc::AuthorityWorkunitDeleteRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_workunit_delete(params).await?)?
            }
            ipc::methods::AUTHORITY_WORKUNIT_LIST => {
                let params: ipc::AuthorityWorkunitListRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_workunit_list(params).await?)?
            }
            ipc::methods::AUTHORITY_WORKUNIT_GET => {
                let params: ipc::AuthorityWorkunitGetRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_workunit_get(params).await?)?
            }
            ipc::methods::AUTHORITY_TRACKED_THREAD_CREATE => {
                let params: ipc::AuthorityTrackedThreadCreateRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_tracked_thread_create(params).await?)?
            }
            ipc::methods::AUTHORITY_TRACKED_THREAD_EDIT => {
                let params: ipc::AuthorityTrackedThreadEditRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_tracked_thread_edit(params).await?)?
            }
            ipc::methods::AUTHORITY_TRACKED_THREAD_DELETE => {
                let params: ipc::AuthorityTrackedThreadDeleteRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_tracked_thread_delete(params).await?)?
            }
            ipc::methods::AUTHORITY_TRACKED_THREAD_LIST => {
                let params: ipc::AuthorityTrackedThreadListRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_tracked_thread_list(params).await?)?
            }
            ipc::methods::AUTHORITY_TRACKED_THREAD_GET => {
                let params: ipc::AuthorityTrackedThreadGetRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.authority_tracked_thread_get(params).await?)?
            }
            ipc::methods::ASSIGNMENT_START => {
                let params: ipc::AssignmentStartRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.assignment_start(params).await?)?
            }
            ipc::methods::ASSIGNMENT_GET => {
                let params: ipc::AssignmentGetRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.assignment_get(params).await?)?
            }
            ipc::methods::CODEX_ASSIGNMENT_CREATE => {
                let params: ipc::CodexAssignmentCreateRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.codex_assignment_create(params).await?)?
            }
            ipc::methods::CODEX_ASSIGNMENT_GET => {
                let params: ipc::CodexAssignmentGetRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.codex_assignment_get(params).await?)?
            }
            ipc::methods::CODEX_ASSIGNMENT_LIST => {
                let params: ipc::CodexAssignmentListRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.codex_assignment_list(params).await?)?
            }
            ipc::methods::CODEX_ASSIGNMENT_PAUSE => {
                let params: ipc::CodexAssignmentPauseRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.codex_assignment_pause(params).await?)?
            }
            ipc::methods::CODEX_ASSIGNMENT_RESUME => {
                let params: ipc::CodexAssignmentResumeRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.codex_assignment_resume(params).await?)?
            }
            ipc::methods::CODEX_ASSIGNMENT_RELEASE => {
                let params: ipc::CodexAssignmentReleaseRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.codex_assignment_release(params).await?)?
            }
            ipc::methods::SUPERVISOR_DECISION_LIST => {
                let params: ipc::SupervisorDecisionListRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.supervisor_decision_list(params).await?)?
            }
            ipc::methods::SUPERVISOR_DECISION_GET => {
                let params: ipc::SupervisorDecisionGetRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.supervisor_decision_get(params).await?)?
            }
            ipc::methods::SUPERVISOR_DECISION_PROPOSE_STEER => {
                let params: ipc::SupervisorDecisionProposeSteerRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.supervisor_decision_propose_steer(params).await?)?
            }
            ipc::methods::SUPERVISOR_DECISION_REPLACE_PENDING_STEER => {
                let params: ipc::SupervisorDecisionReplacePendingSteerRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(
                    self.supervisor_decision_replace_pending_steer(params)
                        .await?,
                )?
            }
            ipc::methods::SUPERVISOR_DECISION_PROPOSE_INTERRUPT => {
                let params: ipc::SupervisorDecisionProposeInterruptRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.supervisor_decision_propose_interrupt(params).await?)?
            }
            ipc::methods::SUPERVISOR_DECISION_RECORD_NO_ACTION => {
                let params: ipc::SupervisorDecisionRecordNoActionRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.supervisor_decision_record_no_action(params).await?)?
            }
            ipc::methods::SUPERVISOR_DECISION_MANUAL_REFRESH => {
                let params: ipc::SupervisorDecisionManualRefreshRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.supervisor_decision_manual_refresh(params).await?)?
            }
            ipc::methods::SUPERVISOR_DECISION_APPROVE_AND_SEND => {
                let params: ipc::SupervisorDecisionApproveAndSendRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.supervisor_decision_approve_and_send(params).await?)?
            }
            ipc::methods::SUPERVISOR_DECISION_REJECT => {
                let params: ipc::SupervisorDecisionRejectRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.supervisor_decision_reject(params).await?)?
            }
            ipc::methods::ASSIGNMENT_COMMUNICATION_GET => {
                let params: ipc::AssignmentCommunicationGetRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.assignment_communication_get(params).await?)?
            }
            ipc::methods::REPORT_GET => {
                let params: ipc::ReportGetRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.report_get(params).await?)?
            }
            ipc::methods::REPORT_LIST_FOR_WORKUNIT => {
                let params: ipc::ReportListForWorkunitRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.report_list_for_workunit(params).await?)?
            }
            ipc::methods::DECISION_APPLY => {
                let params: ipc::DecisionApplyRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.decision_apply(params).await?)?
            }
            ipc::methods::PROPOSAL_CREATE => {
                let params: ipc::ProposalCreateRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.proposal_create(params).await?)?
            }
            ipc::methods::PROPOSAL_GET => {
                let params: ipc::ProposalGetRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.proposal_get(params).await?)?
            }
            ipc::methods::PROPOSAL_LIST_FOR_WORKUNIT => {
                let params: ipc::ProposalListForWorkunitRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.proposal_list_for_workunit(params).await?)?
            }
            ipc::methods::PROPOSAL_APPROVE => {
                let params: ipc::ProposalApproveRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.proposal_approve(params).await?)?
            }
            ipc::methods::PROPOSAL_REJECT => {
                let params: ipc::ProposalRejectRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.proposal_reject(params).await?)?
            }
            ipc::methods::EVENTS_SUBSCRIBE => {
                let params: ipc::EventsSubscribeRequest =
                    Self::decode_params(request.params.clone())?;
                serde_json::to_value(
                    self.events_subscribe(params, outbound.clone(), subscription_task)
                        .await?,
                )?
            }
            other => {
                Self::send_error(
                    &outbound,
                    Some(request.id),
                    -32601,
                    &format!("unknown method `{other}`"),
                    None,
                )
                .await?;
                return Ok(());
            }
        };

        Self::send_response(&outbound, request.id, result).await
    }

    async fn daemon_status(&self) -> OrcasResult<ipc::DaemonStatusResponse> {
        debug!("building daemon status response");
        let upstream = self.state.read().await.upstream.clone();
        Ok(ipc::DaemonStatusResponse {
            socket_path: self.paths.socket_file.display().to_string(),
            metadata_path: self.paths.daemon_metadata_file.display().to_string(),
            codex_endpoint: self.config.codex.listen_url.clone(),
            codex_binary_path: self.config.codex.binary_path.display().to_string(),
            upstream,
            client_count: self.client_count.load(Ordering::SeqCst),
            known_threads: self.known_thread_summaries().await.len(),
            runtime: self.runtime.clone(),
        })
    }

    async fn daemon_connect(&self) -> OrcasResult<ipc::DaemonConnectResponse> {
        debug!("handling daemon connect request");
        self.connect_upstream().await?;
        Ok(ipc::DaemonConnectResponse {
            status: self.daemon_status().await?,
        })
    }

    async fn daemon_stop(&self) -> OrcasResult<ipc::DaemonStopResponse> {
        debug!("handling daemon stop request");
        Ok(ipc::DaemonStopResponse { stopping: true })
    }

    async fn state_get(&self) -> OrcasResult<ipc::StateGetResponse> {
        debug!("handling state_get request");
        Ok(ipc::StateGetResponse {
            snapshot: self.snapshot().await?,
        })
    }

    async fn session_get_active(&self) -> OrcasResult<ipc::SessionGetActiveResponse> {
        debug!("handling session_get_active request");
        Ok(ipc::SessionGetActiveResponse {
            session: self.state.read().await.session.clone(),
        })
    }

    async fn models_list(&self) -> OrcasResult<ipc::ModelsListResponse> {
        debug!(listen_url = %self.config.codex.listen_url, "handling models_list request");
        self.connect_upstream().await?;
        let response = self
            .codex_client
            .model_list(types::ModelListParams::default())
            .await?;
        Ok(ipc::ModelsListResponse {
            data: response
                .data
                .into_iter()
                .map(|model| ipc::ModelSummary {
                    id: model.model,
                    display_name: model.display_name,
                    hidden: model.hidden,
                    is_default: model.is_default,
                })
                .collect(),
        })
    }

    async fn threads_list(&self) -> OrcasResult<ipc::ThreadsListResponse> {
        self.connect_upstream().await?;
        let response = self
            .codex_client
            .thread_list(types::ThreadListParams::default())
            .await?;
        self.sync_threads(&response.data, None, Some("upstream_discovered"))
            .await?;
        Ok(ipc::ThreadsListResponse {
            data: self.known_thread_summaries().await,
        })
    }

    async fn threads_list_scoped(&self) -> OrcasResult<ipc::ThreadsListResponse> {
        Ok(ipc::ThreadsListResponse {
            data: self.scoped_known_thread_summaries().await,
        })
    }

    async fn threads_list_loaded(&self) -> OrcasResult<ipc::ThreadsListResponse> {
        let mut data = self
            .known_thread_summaries()
            .await
            .into_iter()
            .filter(|thread| thread.loaded_status != ipc::ThreadLoadedStatus::NotLoaded)
            .collect::<Vec<_>>();
        data.sort_by(|left, right| {
            right
                .last_sync_at
                .cmp(&left.last_sync_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(ipc::ThreadsListResponse { data })
    }

    async fn thread_start(
        &self,
        params: ipc::ThreadStartRequest,
    ) -> OrcasResult<ipc::ThreadStartResponse> {
        self.connect_upstream().await?;
        let response = self
            .codex_client
            .thread_start(types::ThreadStartParams {
                cwd: params.cwd.or_else(|| {
                    self.config
                        .defaults
                        .cwd
                        .clone()
                        .map(|path| path.display().to_string())
                }),
                model: params.model.or_else(|| self.config.defaults.model.clone()),
                ephemeral: Some(params.ephemeral),
                service_name: Some("orcasd".to_string()),
                ..types::ThreadStartParams::default()
            })
            .await?;
        let view = self
            .sync_thread(
                &response.thread,
                Some(response.model.clone()),
                Some("orcas_managed"),
            )
            .await?;
        self.set_thread_monitor_state(&view.summary.id, ipc::ThreadMonitorState::Attached)
            .await?;
        self.set_active_thread(&view.summary.id).await;
        Ok(ipc::ThreadStartResponse {
            thread: view.summary,
        })
    }

    async fn thread_read(
        &self,
        params: ipc::ThreadReadRequest,
    ) -> OrcasResult<ipc::ThreadReadResponse> {
        self.connect_upstream().await?;
        let response = self
            .codex_client
            .thread_read(types::ThreadReadParams {
                thread_id: params.thread_id,
                include_turns: params.include_turns,
            })
            .await?;
        let view = self
            .sync_thread(&response.thread, None, Some("live_observed"))
            .await?;
        Ok(ipc::ThreadReadResponse { thread: view })
    }

    async fn thread_read_history(
        &self,
        params: ipc::ThreadReadHistoryRequest,
    ) -> OrcasResult<ipc::ThreadReadHistoryResponse> {
        let response = self
            .thread_read(ipc::ThreadReadRequest {
                thread_id: params.thread_id,
                include_turns: true,
            })
            .await?;
        Ok(ipc::ThreadReadHistoryResponse {
            thread: response.thread,
        })
    }

    async fn thread_get(
        &self,
        params: ipc::ThreadGetRequest,
    ) -> OrcasResult<ipc::ThreadGetResponse> {
        if let Some(thread) = self.thread_from_state(&params.thread_id).await {
            if !thread.turns.is_empty() {
                return Ok(ipc::ThreadGetResponse { thread });
            }
        }

        self.connect_upstream().await?;
        let response = self
            .codex_client
            .thread_read(types::ThreadReadParams {
                thread_id: params.thread_id,
                include_turns: true,
            })
            .await?;
        let view = self
            .sync_thread(&response.thread, None, Some("live_observed"))
            .await?;
        Ok(ipc::ThreadGetResponse { thread: view })
    }

    async fn thread_resume(
        &self,
        params: ipc::ThreadResumeRequest,
    ) -> OrcasResult<ipc::ThreadResumeResponse> {
        self.connect_upstream().await?;
        let response = self
            .codex_client
            .thread_resume(types::ThreadResumeParams {
                thread_id: params.thread_id,
                cwd: params.cwd.or_else(|| {
                    self.config
                        .defaults
                        .cwd
                        .clone()
                        .map(|path| path.display().to_string())
                }),
                model: params.model.or_else(|| self.config.defaults.model.clone()),
                approval_policy: Some(types::AskForApproval::default()),
                approvals_reviewer: None,
                sandbox: None,
                config: None,
                base_instructions: None,
                developer_instructions: None,
                persist_extended_history: true,
            })
            .await?;
        let view = self
            .sync_thread(
                &response.thread,
                Some(response.model.clone()),
                Some("orcas_managed"),
            )
            .await?;
        self.set_thread_monitor_state(&view.summary.id, ipc::ThreadMonitorState::Attached)
            .await?;
        self.set_active_thread(&view.summary.id).await;
        Ok(ipc::ThreadResumeResponse {
            thread: view.summary,
        })
    }

    async fn thread_attach(
        &self,
        params: ipc::ThreadAttachRequest,
    ) -> OrcasResult<ipc::ThreadAttachResponse> {
        if let Some(thread) = self.thread_from_state(&params.thread_id).await
            && thread.summary.monitor_state == ipc::ThreadMonitorState::Attached
        {
            return Ok(ipc::ThreadAttachResponse {
                thread: Some(thread),
                attached: true,
                reason: None,
            });
        }

        self.set_thread_monitor_state(&params.thread_id, ipc::ThreadMonitorState::Attaching)
            .await?;
        self.connect_upstream().await?;

        let response = self
            .codex_client
            .thread_resume(types::ThreadResumeParams {
                thread_id: params.thread_id.clone(),
                cwd: params.cwd.or_else(|| {
                    self.config
                        .defaults
                        .cwd
                        .clone()
                        .map(|path| path.display().to_string())
                }),
                model: params.model.or_else(|| self.config.defaults.model.clone()),
                approval_policy: Some(types::AskForApproval::default()),
                approvals_reviewer: None,
                sandbox: None,
                config: None,
                base_instructions: None,
                developer_instructions: None,
                persist_extended_history: true,
            })
            .await;

        match response {
            Ok(response) => {
                let view = self
                    .sync_thread(
                        &response.thread,
                        Some(response.model),
                        Some("live_observed"),
                    )
                    .await?;
                self.set_thread_monitor_state(&view.summary.id, ipc::ThreadMonitorState::Attached)
                    .await?;
                Ok(ipc::ThreadAttachResponse {
                    thread: self.thread_from_state(&view.summary.id).await,
                    attached: true,
                    reason: None,
                })
            }
            Err(error) => {
                self.set_thread_monitor_state(&params.thread_id, ipc::ThreadMonitorState::Errored)
                    .await?;
                Ok(ipc::ThreadAttachResponse {
                    thread: self.thread_from_state(&params.thread_id).await,
                    attached: false,
                    reason: Some(error.to_string()),
                })
            }
        }
    }

    async fn thread_detach(
        &self,
        params: ipc::ThreadDetachRequest,
    ) -> OrcasResult<ipc::ThreadDetachResponse> {
        let Some(thread) = self.thread_from_state(&params.thread_id).await else {
            return Ok(ipc::ThreadDetachResponse {
                thread: None,
                detached: false,
                reason: Some("thread was not found in the Orcas mirror".to_string()),
            });
        };

        self.set_thread_monitor_state(&params.thread_id, ipc::ThreadMonitorState::Detached)
            .await?;
        Ok(ipc::ThreadDetachResponse {
            thread: self.thread_from_state(&thread.summary.id).await,
            detached: true,
            reason: None,
        })
    }

    async fn turns_list_active(&self) -> OrcasResult<ipc::TurnsListActiveResponse> {
        let mut turns = self
            .state
            .read()
            .await
            .turns
            .values()
            .filter(|turn| {
                turn.attachable && matches!(turn.lifecycle, ipc::TurnLifecycleState::Active)
            })
            .cloned()
            .collect::<Vec<_>>();
        turns.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.turn_id.cmp(&right.turn_id))
        });
        Ok(ipc::TurnsListActiveResponse { turns })
    }

    async fn turns_recent(
        &self,
        params: ipc::TurnsRecentRequest,
    ) -> OrcasResult<ipc::TurnsRecentResponse> {
        let thread = self
            .thread_get(ipc::ThreadGetRequest {
                thread_id: params.thread_id.clone(),
            })
            .await?
            .thread;
        let turns = thread
            .turns
            .into_iter()
            .rev()
            .take(params.limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Ok(ipc::TurnsRecentResponse {
            thread_id: params.thread_id,
            turns,
        })
    }

    async fn turn_get(&self, params: ipc::TurnGetRequest) -> OrcasResult<ipc::TurnGetResponse> {
        Ok(ipc::TurnGetResponse {
            turn: self
                .resolve_turn_state(&params.thread_id, &params.turn_id)
                .await?,
        })
    }

    async fn turn_attach(
        &self,
        params: ipc::TurnAttachRequest,
    ) -> OrcasResult<ipc::TurnAttachResponse> {
        let turn = self
            .resolve_turn_state(&params.thread_id, &params.turn_id)
            .await?;
        let (attached, reason) = match turn.as_ref() {
            Some(turn)
                if turn.attachable && matches!(turn.lifecycle, ipc::TurnLifecycleState::Active) =>
            {
                (true, None)
            }
            Some(turn) => (false, Some(Self::turn_attach_failure_reason(turn))),
            None => (
                false,
                Some("turn was not found in the current Orcas daemon state".to_string()),
            ),
        };
        Ok(ipc::TurnAttachResponse {
            turn,
            attached,
            reason,
        })
    }

    async fn turn_start(
        &self,
        params: ipc::TurnStartRequest,
    ) -> OrcasResult<ipc::TurnStartResponse> {
        self.connect_upstream().await?;
        let response = self
            .codex_client
            .turn_start(types::TurnStartParams {
                thread_id: params.thread_id.clone(),
                input: vec![types::UserInput::Text {
                    text: params.text,
                    text_elements: Vec::new(),
                }],
                cwd: params.cwd,
                approval_policy: Some(types::AskForApproval::default()),
                approvals_reviewer: None,
                sandbox_policy: None,
                model: params.model,
            })
            .await?;
        self.record_turn_started(&params.thread_id, &response.turn.id, "submitted")
            .await;
        Ok(ipc::TurnStartResponse {
            turn_id: response.turn.id,
            thread_id: params.thread_id,
        })
    }

    async fn turn_steer(
        &self,
        params: ipc::TurnSteerRequest,
    ) -> OrcasResult<ipc::TurnSteerResponse> {
        if params.text.trim().is_empty() {
            return Err(OrcasError::Protocol(
                "turn/steer requires non-empty text".to_string(),
            ));
        }
        self.connect_upstream().await?;
        let response = self
            .codex_client
            .turn_steer(types::TurnSteerParams {
                thread_id: params.thread_id.clone(),
                input: vec![types::UserInput::Text {
                    text: params.text,
                    text_elements: Vec::new(),
                }],
                expected_turn_id: params.expected_turn_id.clone(),
            })
            .await?;
        Ok(ipc::TurnSteerResponse {
            turn_id: response.turn_id,
            thread_id: params.thread_id,
        })
    }

    async fn turn_interrupt(&self, params: ipc::TurnInterruptRequest) -> OrcasResult<()> {
        self.connect_upstream().await?;
        self.codex_client
            .turn_interrupt(types::TurnInterruptParams {
                thread_id: params.thread_id,
                turn_id: params.turn_id,
            })
            .await?;
        Ok(())
    }

    async fn workstream_create(
        &self,
        params: ipc::WorkstreamCreateRequest,
    ) -> OrcasResult<ipc::WorkstreamCreateResponse> {
        let workstream = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let workstream = Workstream {
                id: Self::new_object_id("ws"),
                title: params.title,
                objective: params.objective,
                status: WorkstreamStatus::Active,
                priority: params.priority.unwrap_or_else(|| "normal".to_string()),
                created_at: now,
                updated_at: now,
            };
            state
                .collaboration
                .workstreams
                .insert(workstream.id.clone(), workstream.clone());
            workstream
        };
        self.persist_collaboration_state().await?;
        self.emit_workstream_lifecycle(ipc::CollaborationLifecycleAction::Created, &workstream)
            .await;
        Ok(ipc::WorkstreamCreateResponse { workstream })
    }

    async fn workstream_list(&self) -> OrcasResult<ipc::WorkstreamListResponse> {
        let mut workstreams = self
            .state
            .read()
            .await
            .collaboration
            .workstreams
            .values()
            .cloned()
            .collect::<Vec<_>>();
        workstreams.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(ipc::WorkstreamListResponse { workstreams })
    }

    async fn workstream_get(
        &self,
        params: ipc::WorkstreamGetRequest,
    ) -> OrcasResult<ipc::WorkstreamGetResponse> {
        let state = self.state.read().await;
        let workstream = state
            .collaboration
            .workstreams
            .get(&params.workstream_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!("unknown workstream `{}`", params.workstream_id))
            })?;
        let mut work_units = state
            .collaboration
            .work_units
            .values()
            .filter(|work_unit| work_unit.workstream_id == params.workstream_id)
            .cloned()
            .collect::<Vec<_>>();
        work_units.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(ipc::WorkstreamGetResponse {
            workstream,
            work_units,
        })
    }

    async fn workunit_create(
        &self,
        params: ipc::WorkunitCreateRequest,
    ) -> OrcasResult<ipc::WorkunitCreateResponse> {
        let (work_unit, workstream) = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            if !state
                .collaboration
                .workstreams
                .contains_key(&params.workstream_id)
            {
                return Err(OrcasError::Protocol(format!(
                    "unknown workstream `{}`",
                    params.workstream_id
                )));
            }
            for dependency in &params.dependencies {
                if !state.collaboration.work_units.contains_key(dependency) {
                    return Err(OrcasError::Protocol(format!(
                        "unknown dependency work unit `{dependency}`"
                    )));
                }
            }

            let status = if Self::dependencies_satisfied(&state.collaboration, &params.dependencies)
            {
                WorkUnitStatus::Ready
            } else {
                WorkUnitStatus::Blocked
            };
            let work_unit = WorkUnit {
                id: Self::new_object_id("wu"),
                workstream_id: params.workstream_id.clone(),
                title: params.title,
                task_statement: params.task_statement,
                status,
                dependencies: params.dependencies,
                latest_report_id: None,
                current_assignment_id: None,
                created_at: now,
                updated_at: now,
            };
            state
                .collaboration
                .work_units
                .insert(work_unit.id.clone(), work_unit.clone());
            if let Some(workstream) = state
                .collaboration
                .workstreams
                .get_mut(&params.workstream_id)
            {
                workstream.updated_at = now;
            }
            Self::refresh_workstream_statuses(&mut state.collaboration);
            let workstream = state
                .collaboration
                .workstreams
                .get(&params.workstream_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown workstream `{}`", params.workstream_id))
                })?;
            (work_unit, workstream)
        };
        self.persist_collaboration_state().await?;
        self.emit_work_unit_lifecycle(ipc::CollaborationLifecycleAction::Created, &work_unit)
            .await;
        self.emit_workstream_lifecycle(ipc::CollaborationLifecycleAction::Updated, &workstream)
            .await;
        Ok(ipc::WorkunitCreateResponse { work_unit })
    }

    async fn workunit_list(
        &self,
        params: ipc::WorkunitListRequest,
    ) -> OrcasResult<ipc::WorkunitListResponse> {
        let state = self.state.read().await;
        let mut work_units = state
            .collaboration
            .work_units
            .values()
            .filter(|work_unit| {
                params
                    .workstream_id
                    .as_ref()
                    .map(|workstream_id| &work_unit.workstream_id == workstream_id)
                    .unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();
        work_units.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(ipc::WorkunitListResponse { work_units })
    }

    async fn workunit_get(
        &self,
        params: ipc::WorkunitGetRequest,
    ) -> OrcasResult<ipc::WorkunitGetResponse> {
        let stale_proposals = {
            let mut state = self.state.write().await;
            Self::refresh_stale_proposals_for_work_unit(
                &mut state.collaboration,
                &params.work_unit_id,
            )
        };
        if !stale_proposals.is_empty() {
            self.persist_collaboration_state().await?;
            for proposal in &stale_proposals {
                self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Stale, proposal)
                    .await;
            }
        }

        let state = self.state.read().await;
        let work_unit = state
            .collaboration
            .work_units
            .get(&params.work_unit_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!("unknown work unit `{}`", params.work_unit_id))
            })?;
        let mut assignments = state
            .collaboration
            .assignments
            .values()
            .filter(|assignment| assignment.work_unit_id == params.work_unit_id)
            .cloned()
            .collect::<Vec<_>>();
        assignments.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut reports = state
            .collaboration
            .reports
            .values()
            .filter(|report| report.work_unit_id == params.work_unit_id)
            .cloned()
            .collect::<Vec<_>>();
        reports.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut decisions = state
            .collaboration
            .decisions
            .values()
            .filter(|decision| decision.work_unit_id == params.work_unit_id)
            .cloned()
            .collect::<Vec<_>>();
        decisions.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut proposals = state
            .collaboration
            .supervisor_proposals
            .values()
            .filter(|proposal| proposal.primary_work_unit_id == params.work_unit_id)
            .cloned()
            .collect::<Vec<_>>();
        proposals.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(ipc::WorkunitGetResponse {
            work_unit,
            assignments,
            reports,
            decisions,
            proposals,
        })
    }

    async fn authority_hierarchy_get(
        &self,
        params: ipc::AuthorityHierarchyGetRequest,
    ) -> OrcasResult<ipc::AuthorityHierarchyGetResponse> {
        Ok(ipc::AuthorityHierarchyGetResponse {
            hierarchy: self
                .authority_store
                .hierarchy_snapshot(params.include_deleted)
                .await?,
        })
    }

    async fn authority_delete_plan(
        &self,
        params: ipc::AuthorityDeletePlanRequest,
    ) -> OrcasResult<ipc::AuthorityDeletePlanResponse> {
        let delete_plan = self
            .authority_store
            .delete_plan(&params.target)
            .await?
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown authority delete target `{}`",
                    params.target.aggregate_key().aggregate_id
                ))
            })?;
        Ok(ipc::AuthorityDeletePlanResponse { delete_plan })
    }

    async fn authority_workstream_create(
        &self,
        params: ipc::AuthorityWorkstreamCreateRequest,
    ) -> OrcasResult<ipc::AuthorityWorkstreamCreateResponse> {
        match self
            .authority_store
            .execute_command(AuthorityCommand::CreateWorkstream(params.command))
            .await?
        {
            AuthorityMutationResult::Workstream(workstream) => {
                self.emit_authority_workstream_lifecycle(
                    ipc::CollaborationLifecycleAction::Created,
                    &workstream,
                )
                .await;
                Ok(ipc::AuthorityWorkstreamCreateResponse { workstream })
            }
            AuthorityMutationResult::WorkUnit(_) | AuthorityMutationResult::TrackedThread(_) => {
                Err(OrcasError::Store(
                    "authority workstream create returned wrong mutation type".to_string(),
                ))
            }
        }
    }

    async fn authority_workstream_edit(
        &self,
        params: ipc::AuthorityWorkstreamEditRequest,
    ) -> OrcasResult<ipc::AuthorityWorkstreamEditResponse> {
        match self
            .authority_store
            .execute_command(AuthorityCommand::EditWorkstream(params.command))
            .await?
        {
            AuthorityMutationResult::Workstream(workstream) => {
                self.emit_authority_workstream_lifecycle(
                    ipc::CollaborationLifecycleAction::Updated,
                    &workstream,
                )
                .await;
                Ok(ipc::AuthorityWorkstreamEditResponse { workstream })
            }
            AuthorityMutationResult::WorkUnit(_) | AuthorityMutationResult::TrackedThread(_) => {
                Err(OrcasError::Store(
                    "authority workstream edit returned wrong mutation type".to_string(),
                ))
            }
        }
    }

    async fn authority_workstream_delete(
        &self,
        params: ipc::AuthorityWorkstreamDeleteRequest,
    ) -> OrcasResult<ipc::AuthorityWorkstreamDeleteResponse> {
        let affected_work_units = self
            .authority_store
            .list_work_units(Some(&params.command.workstream_id), false)
            .await?;
        let mut affected_tracked_thread_ids = Vec::new();
        for work_unit in &affected_work_units {
            let tracked_threads = self
                .authority_store
                .list_tracked_threads(&work_unit.id, false)
                .await?;
            affected_tracked_thread_ids.extend(
                tracked_threads
                    .into_iter()
                    .map(|tracked_thread| tracked_thread.id),
            );
        }
        match self
            .authority_store
            .execute_command(AuthorityCommand::DeleteWorkstream(params.command))
            .await?
        {
            AuthorityMutationResult::Workstream(workstream) => {
                self.emit_authority_workstream_lifecycle(
                    ipc::CollaborationLifecycleAction::Deleted,
                    &workstream,
                )
                .await;
                for work_unit_id in affected_work_units
                    .into_iter()
                    .map(|work_unit| work_unit.id)
                {
                    if let Some(work_unit) =
                        self.authority_store.get_work_unit(&work_unit_id).await?
                    {
                        self.emit_authority_work_unit_lifecycle(
                            ipc::CollaborationLifecycleAction::Deleted,
                            &work_unit,
                        )
                        .await;
                    }
                }
                for tracked_thread_id in affected_tracked_thread_ids {
                    if let Some(tracked_thread) = self
                        .authority_store
                        .get_tracked_thread(&tracked_thread_id)
                        .await?
                    {
                        self.emit_authority_tracked_thread_lifecycle(
                            ipc::CollaborationLifecycleAction::Deleted,
                            &tracked_thread,
                        )
                        .await;
                    }
                }
                Ok(ipc::AuthorityWorkstreamDeleteResponse { workstream })
            }
            AuthorityMutationResult::WorkUnit(_) | AuthorityMutationResult::TrackedThread(_) => {
                Err(OrcasError::Store(
                    "authority workstream delete returned wrong mutation type".to_string(),
                ))
            }
        }
    }

    async fn authority_workstream_list(
        &self,
        params: ipc::AuthorityWorkstreamListRequest,
    ) -> OrcasResult<ipc::AuthorityWorkstreamListResponse> {
        Ok(ipc::AuthorityWorkstreamListResponse {
            workstreams: self
                .authority_store
                .list_workstreams(params.include_deleted)
                .await?,
        })
    }

    async fn authority_workstream_get(
        &self,
        params: ipc::AuthorityWorkstreamGetRequest,
    ) -> OrcasResult<ipc::AuthorityWorkstreamGetResponse> {
        let workstream = self
            .authority_store
            .get_workstream(&params.workstream_id)
            .await?
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown authority workstream `{}`",
                    params.workstream_id
                ))
            })?;
        let work_units = self
            .authority_store
            .list_work_units(Some(&params.workstream_id), false)
            .await?;
        Ok(ipc::AuthorityWorkstreamGetResponse {
            workstream,
            work_units,
        })
    }

    async fn authority_workunit_create(
        &self,
        params: ipc::AuthorityWorkunitCreateRequest,
    ) -> OrcasResult<ipc::AuthorityWorkunitCreateResponse> {
        match self
            .authority_store
            .execute_command(AuthorityCommand::CreateWorkUnit(params.command))
            .await?
        {
            AuthorityMutationResult::WorkUnit(work_unit) => {
                self.emit_authority_work_unit_lifecycle(
                    ipc::CollaborationLifecycleAction::Created,
                    &work_unit,
                )
                .await;
                Ok(ipc::AuthorityWorkunitCreateResponse { work_unit })
            }
            AuthorityMutationResult::Workstream(_) | AuthorityMutationResult::TrackedThread(_) => {
                Err(OrcasError::Store(
                    "authority work unit create returned wrong mutation type".to_string(),
                ))
            }
        }
    }

    async fn authority_workunit_edit(
        &self,
        params: ipc::AuthorityWorkunitEditRequest,
    ) -> OrcasResult<ipc::AuthorityWorkunitEditResponse> {
        match self
            .authority_store
            .execute_command(AuthorityCommand::EditWorkUnit(params.command))
            .await?
        {
            AuthorityMutationResult::WorkUnit(work_unit) => {
                self.emit_authority_work_unit_lifecycle(
                    ipc::CollaborationLifecycleAction::Updated,
                    &work_unit,
                )
                .await;
                Ok(ipc::AuthorityWorkunitEditResponse { work_unit })
            }
            AuthorityMutationResult::Workstream(_) | AuthorityMutationResult::TrackedThread(_) => {
                Err(OrcasError::Store(
                    "authority work unit edit returned wrong mutation type".to_string(),
                ))
            }
        }
    }

    async fn authority_workunit_delete(
        &self,
        params: ipc::AuthorityWorkunitDeleteRequest,
    ) -> OrcasResult<ipc::AuthorityWorkunitDeleteResponse> {
        let affected_tracked_thread_ids = self
            .authority_store
            .list_tracked_threads(&params.command.work_unit_id, false)
            .await?
            .into_iter()
            .map(|tracked_thread| tracked_thread.id)
            .collect::<Vec<_>>();
        match self
            .authority_store
            .execute_command(AuthorityCommand::DeleteWorkUnit(params.command))
            .await?
        {
            AuthorityMutationResult::WorkUnit(work_unit) => {
                self.emit_authority_work_unit_lifecycle(
                    ipc::CollaborationLifecycleAction::Deleted,
                    &work_unit,
                )
                .await;
                for tracked_thread_id in affected_tracked_thread_ids {
                    if let Some(tracked_thread) = self
                        .authority_store
                        .get_tracked_thread(&tracked_thread_id)
                        .await?
                    {
                        self.emit_authority_tracked_thread_lifecycle(
                            ipc::CollaborationLifecycleAction::Deleted,
                            &tracked_thread,
                        )
                        .await;
                    }
                }
                Ok(ipc::AuthorityWorkunitDeleteResponse { work_unit })
            }
            AuthorityMutationResult::Workstream(_) | AuthorityMutationResult::TrackedThread(_) => {
                Err(OrcasError::Store(
                    "authority work unit delete returned wrong mutation type".to_string(),
                ))
            }
        }
    }

    async fn authority_workunit_list(
        &self,
        params: ipc::AuthorityWorkunitListRequest,
    ) -> OrcasResult<ipc::AuthorityWorkunitListResponse> {
        Ok(ipc::AuthorityWorkunitListResponse {
            work_units: self
                .authority_store
                .list_work_units(params.workstream_id.as_ref(), params.include_deleted)
                .await?,
        })
    }

    async fn authority_workunit_get(
        &self,
        params: ipc::AuthorityWorkunitGetRequest,
    ) -> OrcasResult<ipc::AuthorityWorkunitGetResponse> {
        let work_unit = self
            .authority_store
            .get_work_unit(&params.work_unit_id)
            .await?
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown authority work unit `{}`",
                    params.work_unit_id
                ))
            })?;
        let tracked_threads = self
            .authority_store
            .list_tracked_threads(&params.work_unit_id, false)
            .await?;
        Ok(ipc::AuthorityWorkunitGetResponse {
            work_unit,
            tracked_threads,
        })
    }

    async fn authority_tracked_thread_create(
        &self,
        params: ipc::AuthorityTrackedThreadCreateRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadCreateResponse> {
        match self
            .authority_store
            .execute_command(AuthorityCommand::CreateTrackedThread(params.command))
            .await?
        {
            AuthorityMutationResult::TrackedThread(tracked_thread) => {
                self.emit_authority_tracked_thread_lifecycle(
                    ipc::CollaborationLifecycleAction::Created,
                    &tracked_thread,
                )
                .await;
                Ok(ipc::AuthorityTrackedThreadCreateResponse { tracked_thread })
            }
            AuthorityMutationResult::Workstream(_) | AuthorityMutationResult::WorkUnit(_) => {
                Err(OrcasError::Store(
                    "authority tracked thread create returned wrong mutation type".to_string(),
                ))
            }
        }
    }

    async fn authority_tracked_thread_edit(
        &self,
        params: ipc::AuthorityTrackedThreadEditRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadEditResponse> {
        match self
            .authority_store
            .execute_command(AuthorityCommand::EditTrackedThread(params.command))
            .await?
        {
            AuthorityMutationResult::TrackedThread(tracked_thread) => {
                self.emit_authority_tracked_thread_lifecycle(
                    ipc::CollaborationLifecycleAction::Updated,
                    &tracked_thread,
                )
                .await;
                Ok(ipc::AuthorityTrackedThreadEditResponse { tracked_thread })
            }
            AuthorityMutationResult::Workstream(_) | AuthorityMutationResult::WorkUnit(_) => {
                Err(OrcasError::Store(
                    "authority tracked thread edit returned wrong mutation type".to_string(),
                ))
            }
        }
    }

    async fn authority_tracked_thread_delete(
        &self,
        params: ipc::AuthorityTrackedThreadDeleteRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadDeleteResponse> {
        match self
            .authority_store
            .execute_command(AuthorityCommand::DeleteTrackedThread(params.command))
            .await?
        {
            AuthorityMutationResult::TrackedThread(tracked_thread) => {
                self.emit_authority_tracked_thread_lifecycle(
                    ipc::CollaborationLifecycleAction::Deleted,
                    &tracked_thread,
                )
                .await;
                Ok(ipc::AuthorityTrackedThreadDeleteResponse { tracked_thread })
            }
            AuthorityMutationResult::Workstream(_) | AuthorityMutationResult::WorkUnit(_) => {
                Err(OrcasError::Store(
                    "authority tracked thread delete returned wrong mutation type".to_string(),
                ))
            }
        }
    }

    async fn authority_tracked_thread_list(
        &self,
        params: ipc::AuthorityTrackedThreadListRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadListResponse> {
        Ok(ipc::AuthorityTrackedThreadListResponse {
            tracked_threads: self
                .authority_store
                .list_tracked_threads(&params.work_unit_id, params.include_deleted)
                .await?,
        })
    }

    async fn authority_tracked_thread_get(
        &self,
        params: ipc::AuthorityTrackedThreadGetRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadGetResponse> {
        let tracked_thread = self
            .authority_store
            .get_tracked_thread(&params.tracked_thread_id)
            .await?
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown authority tracked thread `{}`",
                    params.tracked_thread_id
                ))
            })?;
        Ok(ipc::AuthorityTrackedThreadGetResponse { tracked_thread })
    }

    async fn assignment_start(
        &self,
        params: ipc::AssignmentStartRequest,
    ) -> OrcasResult<ipc::AssignmentStartResponse> {
        let session_model = params.model.clone();
        let session_cwd = params.cwd.clone();
        let communication_model = session_model
            .clone()
            .or_else(|| self.config.defaults.model.clone());
        let communication_cwd = session_cwd.clone().or_else(|| {
            self.config
                .defaults
                .cwd
                .as_ref()
                .map(|path| path.display().to_string())
        });
        let prepared = self.prepare_assignment(params).await?;
        let assignment_id = prepared.assignment.id.clone();
        let worker_id = prepared.assignment.worker_id.clone();
        let worker_session_id = prepared.assignment.worker_session_id.clone();
        if prepared.created_new {
            self.emit_assignment_lifecycle(
                ipc::AssignmentLifecycleAction::Created,
                &prepared.assignment,
            )
            .await;
        }
        let worker_session = match self
            .ensure_worker_session_thread(&worker_session_id, session_model, session_cwd)
            .await
        {
            Ok(worker_session) => worker_session,
            Err(error) => {
                let _ = self
                    .mark_assignment_start_failed(&assignment_id, &worker_id, &worker_session_id)
                    .await;
                return Err(error);
            }
        };
        let Some(thread_id) = worker_session.thread_id.clone() else {
            let error = OrcasError::Protocol("worker session has no backing thread".to_string());
            let _ = self
                .mark_assignment_start_failed(&assignment_id, &worker_id, &worker_session_id)
                .await;
            return Err(error);
        };

        self.ensure_assignment_communication_record(
            &assignment_id,
            communication_model,
            communication_cwd,
        )
        .await?;

        let prompt = {
            let state = self.state.read().await;
            state
                .collaboration
                .assignment_communications
                .get(&assignment_id)
                .map(|record| record.prompt_render.prompt_text.clone())
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "missing assignment communication record for `{assignment_id}`"
                    ))
                })?
        };
        let turn = match self
            .turn_start(ipc::TurnStartRequest {
                thread_id: thread_id.clone(),
                text: prompt,
                cwd: None,
                model: None,
            })
            .await
        {
            Ok(turn) => turn,
            Err(error) => {
                let _ = self
                    .mark_assignment_start_failed(&assignment_id, &worker_id, &worker_session_id)
                    .await;
                return Err(error);
            }
        };

        let (started_assignment, started_work_unit) = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let work_unit_id = state
                .collaboration
                .assignments
                .get(&assignment_id)
                .map(|assignment| assignment.work_unit_id.clone())
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown assignment `{assignment_id}`"))
                })?;
            if let Some(assignment) = state.collaboration.assignments.get_mut(&assignment_id) {
                assignment.status = AssignmentStatus::Running;
                assignment.updated_at = now;
            }
            if let Some(worker) = state.collaboration.workers.get_mut(&worker_id) {
                worker.status = WorkerStatus::Busy;
                worker.current_assignment_id = Some(assignment_id.clone());
            }
            if let Some(session) = state
                .collaboration
                .worker_sessions
                .get_mut(&worker_session_id)
            {
                session.active_turn_id = Some(turn.turn_id.clone());
                session.runtime_status = WorkerSessionRuntimeStatus::Running;
                session.attachability = WorkerSessionAttachability::Attachable;
                session.updated_at = now;
            }
            if let Some(work_unit) = state.collaboration.work_units.get_mut(&work_unit_id) {
                work_unit.status = WorkUnitStatus::Running;
                work_unit.updated_at = now;
            }
            let started_assignment = state
                .collaboration
                .assignments
                .get(&assignment_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown assignment `{assignment_id}`"))
                })?;
            let started_work_unit = state
                .collaboration
                .work_units
                .get(&work_unit_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown work unit `{work_unit_id}`"))
                })?;
            (started_assignment, started_work_unit)
        };
        self.persist_collaboration_state().await?;
        self.emit_assignment_lifecycle(
            ipc::AssignmentLifecycleAction::Started,
            &started_assignment,
        )
        .await;
        self.emit_work_unit_lifecycle(
            ipc::CollaborationLifecycleAction::Updated,
            &started_work_unit,
        )
        .await;

        let turn_state = match self.wait_for_turn_terminal(&thread_id, &turn.turn_id).await {
            Ok(turn_state) => turn_state,
            Err(error) => {
                let _ = self
                    .mark_assignment_runtime_lost(&assignment_id, &worker_id, &worker_session_id)
                    .await;
                return Err(error);
            }
        };
        let raw_output = self
            .raw_output_for_turn(&thread_id, &turn.turn_id)
            .await
            .unwrap_or_default();
        let (report, _assignment_after_report, _work_unit_after_report) = self
            .ingest_assignment_turn_outcome(
                &assignment_id,
                &worker_id,
                &worker_session_id,
                turn_state,
                raw_output,
            )
            .await?;

        let (assignment, worker, worker_session) = {
            let state = self.state.read().await;
            (
                state
                    .collaboration
                    .assignments
                    .get(&assignment_id)
                    .cloned()
                    .ok_or_else(|| {
                        OrcasError::Protocol(format!("unknown assignment `{assignment_id}`"))
                    })?,
                state
                    .collaboration
                    .workers
                    .get(&worker_id)
                    .cloned()
                    .ok_or_else(|| OrcasError::Protocol(format!("unknown worker `{worker_id}`")))?,
                state
                    .collaboration
                    .worker_sessions
                    .get(&worker_session_id)
                    .cloned()
                    .ok_or_else(|| {
                        OrcasError::Protocol(format!(
                            "unknown worker session `{worker_session_id}`"
                        ))
                    })?,
            )
        };
        Ok(ipc::AssignmentStartResponse {
            assignment,
            worker,
            worker_session,
            report,
        })
    }

    async fn mark_assignment_start_failed(
        &self,
        assignment_id: &str,
        worker_id: &str,
        worker_session_id: &str,
    ) -> OrcasResult<()> {
        let session_anchor_lost = self
            .worker_session_anchor_is_lost(worker_session_id)
            .await
            .unwrap_or(false);
        let (assignment, work_unit) = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let work_unit_id = state
                .collaboration
                .assignments
                .get(assignment_id)
                .map(|assignment| assignment.work_unit_id.clone())
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown assignment `{assignment_id}`"))
                })?;
            if let Some(assignment) = state.collaboration.assignments.get_mut(assignment_id) {
                assignment.status = AssignmentStatus::Failed;
                assignment.updated_at = now;
            }
            if let Some(worker) = state.collaboration.workers.get_mut(worker_id) {
                worker.status = WorkerStatus::Idle;
                worker.current_assignment_id = None;
            }
            if let Some(session) = state
                .collaboration
                .worker_sessions
                .get_mut(worker_session_id)
            {
                session.active_turn_id = None;
                if session_anchor_lost {
                    session.thread_id = None;
                    session.runtime_status = WorkerSessionRuntimeStatus::Lost;
                    session.attachability = WorkerSessionAttachability::NotAttachable;
                } else {
                    session.runtime_status = WorkerSessionRuntimeStatus::Failed;
                    session.attachability = if session.thread_id.is_some() {
                        WorkerSessionAttachability::Unknown
                    } else {
                        WorkerSessionAttachability::NotAttachable
                    };
                }
                session.updated_at = now;
            }
            if let Some(work_unit) = state.collaboration.work_units.get_mut(&work_unit_id) {
                work_unit.status = WorkUnitStatus::AwaitingDecision;
                work_unit.current_assignment_id = Some(assignment_id.to_string());
                work_unit.updated_at = now;
            }
            Self::refresh_workstream_statuses(&mut state.collaboration);
            let assignment = state
                .collaboration
                .assignments
                .get(assignment_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown assignment `{assignment_id}`"))
                })?;
            let work_unit = state
                .collaboration
                .work_units
                .get(&work_unit_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown work unit `{work_unit_id}`"))
                })?;
            (assignment, work_unit)
        };
        self.persist_collaboration_state().await?;
        self.emit_assignment_lifecycle(ipc::AssignmentLifecycleAction::Failed, &assignment)
            .await;
        self.emit_work_unit_lifecycle(ipc::CollaborationLifecycleAction::Updated, &work_unit)
            .await;
        Ok(())
    }

    async fn mark_assignment_runtime_lost(
        &self,
        assignment_id: &str,
        worker_id: &str,
        worker_session_id: &str,
    ) -> OrcasResult<()> {
        let (assignment, work_unit) = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let work_unit_id = state
                .collaboration
                .assignments
                .get(assignment_id)
                .map(|assignment| assignment.work_unit_id.clone())
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown assignment `{assignment_id}`"))
                })?;
            if let Some(assignment) = state.collaboration.assignments.get_mut(assignment_id) {
                assignment.status = AssignmentStatus::Lost;
                assignment.updated_at = now;
            }
            if let Some(worker) = state.collaboration.workers.get_mut(worker_id) {
                worker.status = WorkerStatus::Idle;
                worker.current_assignment_id = None;
            }
            if let Some(session) = state
                .collaboration
                .worker_sessions
                .get_mut(worker_session_id)
            {
                session.active_turn_id = None;
                session.thread_id = None;
                session.runtime_status = WorkerSessionRuntimeStatus::Lost;
                session.attachability = WorkerSessionAttachability::NotAttachable;
                session.updated_at = now;
            }
            if let Some(work_unit) = state.collaboration.work_units.get_mut(&work_unit_id) {
                work_unit.status = WorkUnitStatus::AwaitingDecision;
                work_unit.current_assignment_id = Some(assignment_id.to_string());
                work_unit.updated_at = now;
            }
            Self::refresh_workstream_statuses(&mut state.collaboration);
            let assignment = state
                .collaboration
                .assignments
                .get(assignment_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown assignment `{assignment_id}`"))
                })?;
            let work_unit = state
                .collaboration
                .work_units
                .get(&work_unit_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown work unit `{work_unit_id}`"))
                })?;
            (assignment, work_unit)
        };
        self.persist_collaboration_state().await?;
        self.emit_assignment_lifecycle(ipc::AssignmentLifecycleAction::Failed, &assignment)
            .await;
        self.emit_work_unit_lifecycle(ipc::CollaborationLifecycleAction::Updated, &work_unit)
            .await;
        Ok(())
    }

    async fn record_assignment_turn_outcome(
        &self,
        assignment_id: &str,
        worker_id: &str,
        worker_session_id: &str,
        turn_state: ipc::TurnStateView,
        raw_output: String,
    ) -> OrcasResult<(Report, Assignment, WorkUnit, Vec<SupervisorProposalRecord>)> {
        let started_at = Instant::now();
        info!(
            assignment_id,
            worker_id,
            worker_session_id,
            lifecycle = ?turn_state.lifecycle,
            attachable = turn_state.attachable,
            raw_output_len = raw_output.len(),
            "recording assignment turn outcome"
        );
        self.ensure_assignment_communication_record(assignment_id, None, None)
            .await?;
        let (assignment_for_parse, communication_record) = {
            let state = self.state.read().await;
            let assignment = state
                .collaboration
                .assignments
                .get(assignment_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown assignment `{assignment_id}`"))
                })?;
            let record = state
                .collaboration
                .assignment_communications
                .get(assignment_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "missing assignment communication record for `{assignment_id}`"
                    ))
                })?;
            (assignment, record)
        };
        let parsed_report = parse_worker_report_for_turn(
            &raw_output,
            turn_state.lifecycle,
            &assignment_for_parse,
            &communication_record,
        );
        let raw_output_hash = stable_fingerprint(&raw_output);
        let now = Utc::now();
        let mut state = self.state.write().await;
        let assignment = state
            .collaboration
            .assignments
            .get_mut(assignment_id)
            .ok_or_else(|| OrcasError::Protocol(format!("unknown assignment `{assignment_id}`")))?;
        let work_unit_id = assignment.work_unit_id.clone();
        assignment.status = match turn_state.lifecycle {
            ipc::TurnLifecycleState::Interrupted => AssignmentStatus::Interrupted,
            ipc::TurnLifecycleState::Lost | ipc::TurnLifecycleState::Unknown => {
                AssignmentStatus::Lost
            }
            _ => AssignmentStatus::AwaitingDecision,
        };
        assignment.updated_at = now;

        let report = Report {
            id: Self::new_object_id("report"),
            work_unit_id: work_unit_id.clone(),
            assignment_id: assignment_id.to_string(),
            worker_id: worker_id.to_string(),
            disposition: parsed_report.disposition,
            summary: parsed_report.summary,
            findings: parsed_report.findings,
            blockers: parsed_report.blockers,
            questions: parsed_report.questions,
            recommended_next_actions: parsed_report.recommended_next_actions,
            confidence: parsed_report.confidence,
            raw_output,
            parse_result: parsed_report.validation.parse_result,
            needs_supervisor_review: parsed_report.validation.needs_supervisor_review,
            created_at: now,
        };
        state
            .collaboration
            .reports
            .insert(report.id.clone(), report.clone());
        if let Some(record) = state
            .collaboration
            .assignment_communications
            .get_mut(assignment_id)
        {
            record.response_envelope = parsed_report.envelope.clone();
            record.validation = Some(parsed_report.validation.clone());
            record.raw_output_hash = Some(raw_output_hash);
        }
        if let Some(work_unit) = state.collaboration.work_units.get_mut(&work_unit_id) {
            work_unit.status = WorkUnitStatus::AwaitingDecision;
            work_unit.latest_report_id = Some(report.id.clone());
            work_unit.current_assignment_id = Some(assignment_id.to_string());
            work_unit.updated_at = now;
        }
        if let Some(worker) = state.collaboration.workers.get_mut(worker_id) {
            worker.status = WorkerStatus::Idle;
            worker.current_assignment_id = None;
        }
        if let Some(session) = state
            .collaboration
            .worker_sessions
            .get_mut(worker_session_id)
        {
            session.active_turn_id = None;
            session.runtime_status = match turn_state.lifecycle {
                ipc::TurnLifecycleState::Interrupted => WorkerSessionRuntimeStatus::Interrupted,
                ipc::TurnLifecycleState::Lost | ipc::TurnLifecycleState::Unknown => {
                    WorkerSessionRuntimeStatus::Lost
                }
                _ => WorkerSessionRuntimeStatus::Completed,
            };
            session.attachability = if turn_state.attachable {
                WorkerSessionAttachability::Attachable
            } else {
                WorkerSessionAttachability::NotAttachable
            };
            session.updated_at = now;
        }
        let stale_proposals =
            Self::refresh_stale_proposals_for_work_unit(&mut state.collaboration, &work_unit_id);
        Self::refresh_workstream_statuses(&mut state.collaboration);
        let assignment_after_report = state
            .collaboration
            .assignments
            .get(assignment_id)
            .cloned()
            .ok_or_else(|| OrcasError::Protocol(format!("unknown assignment `{assignment_id}`")))?;
        let work_unit_after_report = state
            .collaboration
            .work_units
            .get(&work_unit_id)
            .cloned()
            .ok_or_else(|| OrcasError::Protocol(format!("unknown work unit `{work_unit_id}`")))?;
        info!(
            assignment_id,
            worker_id,
            worker_session_id,
            work_unit_id = %work_unit_after_report.id,
            report_id = %report.id,
            parse_result = ?report.parse_result,
            needs_supervisor_review = report.needs_supervisor_review,
            disposition = ?report.disposition,
            stale_proposal_count = stale_proposals.len(),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "recorded assignment turn outcome"
        );
        Ok((
            report,
            assignment_after_report,
            work_unit_after_report,
            stale_proposals,
        ))
    }

    async fn ingest_assignment_turn_outcome(
        &self,
        assignment_id: &str,
        worker_id: &str,
        worker_session_id: &str,
        turn_state: ipc::TurnStateView,
        raw_output: String,
    ) -> OrcasResult<(Report, Assignment, WorkUnit)> {
        let (report, assignment_after_report, work_unit_after_report, stale_proposals) = self
            .record_assignment_turn_outcome(
                assignment_id,
                worker_id,
                worker_session_id,
                turn_state,
                raw_output,
            )
            .await?;
        self.persist_collaboration_state().await?;
        self.emit_report_recorded(&report).await;
        let assignment_event_action = match assignment_after_report.status {
            AssignmentStatus::Interrupted => ipc::AssignmentLifecycleAction::Interrupted,
            AssignmentStatus::Failed | AssignmentStatus::Lost => {
                ipc::AssignmentLifecycleAction::Failed
            }
            AssignmentStatus::AwaitingDecision => ipc::AssignmentLifecycleAction::Reported,
            _ => ipc::AssignmentLifecycleAction::Reported,
        };
        self.emit_assignment_lifecycle(assignment_event_action, &assignment_after_report)
            .await;
        self.emit_work_unit_lifecycle(
            ipc::CollaborationLifecycleAction::Updated,
            &work_unit_after_report,
        )
        .await;
        for proposal in &stale_proposals {
            self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Stale, proposal)
                .await;
        }
        self.maybe_auto_create_proposal_for_report(&report).await;
        Ok((report, assignment_after_report, work_unit_after_report))
    }

    async fn assignment_get(
        &self,
        params: ipc::AssignmentGetRequest,
    ) -> OrcasResult<ipc::AssignmentGetResponse> {
        let state = self.state.read().await;
        let assignment = state
            .collaboration
            .assignments
            .get(&params.assignment_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!("unknown assignment `{}`", params.assignment_id))
            })?;
        let worker = state
            .collaboration
            .workers
            .get(&assignment.worker_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!("unknown worker `{}`", assignment.worker_id))
            })?;
        let worker_session = state
            .collaboration
            .worker_sessions
            .get(&assignment.worker_session_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown worker session `{}`",
                    assignment.worker_session_id
                ))
            })?;
        let report = state
            .collaboration
            .reports
            .values()
            .find(|report| report.assignment_id == params.assignment_id)
            .cloned();
        Ok(ipc::AssignmentGetResponse {
            assignment,
            worker,
            worker_session,
            report,
        })
    }

    async fn codex_assignment_create(
        &self,
        params: ipc::CodexAssignmentCreateRequest,
    ) -> OrcasResult<ipc::CodexAssignmentCreateResponse> {
        if params.codex_thread_id.trim().is_empty()
            || params.workstream_id.trim().is_empty()
            || params.work_unit_id.trim().is_empty()
            || params.supervisor_id.trim().is_empty()
            || params.assigned_by.trim().is_empty()
        {
            return Err(OrcasError::Protocol(
                "codex assignment create requires non-empty thread, workstream, work unit, supervisor, and assigned_by fields".to_string(),
            ));
        }

        let thread = self
            .thread_from_state(&params.codex_thread_id)
            .await
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown Codex thread `{}`; discover or load the thread before assigning it",
                    params.codex_thread_id
                ))
            })?;

        let assignment = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let workstream = state
                .collaboration
                .workstreams
                .get(&params.workstream_id)
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown workstream `{}`", params.workstream_id))
                })?;
            let work_unit = state
                .collaboration
                .work_units
                .get(&params.work_unit_id)
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown work unit `{}`", params.work_unit_id))
                })?;
            if work_unit.workstream_id != workstream.id {
                return Err(OrcasError::Protocol(format!(
                    "work unit `{}` does not belong to workstream `{}`",
                    params.work_unit_id, params.workstream_id
                )));
            }
            if let Some(conflict) =
                state
                    .collaboration
                    .codex_thread_assignments
                    .values()
                    .find(|assignment| {
                        assignment.codex_thread_id == params.codex_thread_id
                            && Self::codex_assignment_status_is_active(assignment.status)
                    })
            {
                return Err(OrcasError::Protocol(format!(
                    "Codex thread `{}` already has active assignment `{}`",
                    params.codex_thread_id, conflict.assignment_id
                )));
            }

            let assignment = CodexThreadAssignment {
                assignment_id: Self::new_object_id("cta"),
                codex_thread_id: params.codex_thread_id,
                workstream_id: params.workstream_id,
                work_unit_id: params.work_unit_id,
                supervisor_id: params.supervisor_id,
                assigned_by: params.assigned_by,
                assigned_at: now,
                updated_at: now,
                status: CodexThreadAssignmentStatus::Active,
                send_policy: params.send_policy.unwrap_or_default(),
                bootstrap_state: Self::codex_assignment_bootstrap_state_for_thread(&thread),
                latest_basis_turn_id: thread.summary.last_seen_turn_id.clone(),
                latest_decision_id: None,
                notes: params.notes,
            };
            state
                .collaboration
                .codex_thread_assignments
                .insert(assignment.assignment_id.clone(), assignment.clone());
            assignment
        };
        self.persist_collaboration_state().await?;
        self.emit_codex_assignment_lifecycle(
            ipc::CodexAssignmentLifecycleAction::Created,
            &assignment,
        )
        .await;
        self.refresh_codex_supervisor_state_for_thread(&assignment.codex_thread_id)
            .await?;
        Ok(ipc::CodexAssignmentCreateResponse { assignment })
    }

    async fn codex_assignment_get(
        &self,
        params: ipc::CodexAssignmentGetRequest,
    ) -> OrcasResult<ipc::CodexAssignmentGetResponse> {
        let assignment = self
            .state
            .read()
            .await
            .collaboration
            .codex_thread_assignments
            .get(&params.assignment_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown Codex thread assignment `{}`",
                    params.assignment_id
                ))
            })?;
        Ok(ipc::CodexAssignmentGetResponse { assignment })
    }

    async fn codex_assignment_list(
        &self,
        params: ipc::CodexAssignmentListRequest,
    ) -> OrcasResult<ipc::CodexAssignmentListResponse> {
        let state = self.state.read().await;
        let mut assignments = state
            .collaboration
            .codex_thread_assignments
            .values()
            .filter(|assignment| {
                params
                    .codex_thread_id
                    .as_ref()
                    .map(|thread_id| &assignment.codex_thread_id == thread_id)
                    .unwrap_or(true)
                    && params
                        .workstream_id
                        .as_ref()
                        .map(|workstream_id| &assignment.workstream_id == workstream_id)
                        .unwrap_or(true)
                    && params
                        .work_unit_id
                        .as_ref()
                        .map(|work_unit_id| &assignment.work_unit_id == work_unit_id)
                        .unwrap_or(true)
                    && (params.include_inactive
                        || Self::codex_assignment_status_is_active(assignment.status))
            })
            .map(Self::codex_assignment_summary)
            .collect::<Vec<_>>();
        assignments.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.assignment_id.cmp(&right.assignment_id))
        });
        Ok(ipc::CodexAssignmentListResponse { assignments })
    }

    async fn codex_assignment_pause(
        &self,
        params: ipc::CodexAssignmentPauseRequest,
    ) -> OrcasResult<ipc::CodexAssignmentPauseResponse> {
        let assignment = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let assignment = state
                .collaboration
                .codex_thread_assignments
                .get_mut(&params.assignment_id)
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown Codex thread assignment `{}`",
                        params.assignment_id
                    ))
                })?;
            if !matches!(
                assignment.status,
                CodexThreadAssignmentStatus::Active | CodexThreadAssignmentStatus::Proposed
            ) {
                return Err(OrcasError::Protocol(format!(
                    "Codex thread assignment `{}` can only be paused from active/proposed state",
                    params.assignment_id
                )));
            }
            assignment.status = CodexThreadAssignmentStatus::Paused;
            assignment.updated_at = now;
            Self::merge_assignment_notes(&mut assignment.notes, params.notes);
            assignment.clone()
        };
        self.persist_collaboration_state().await?;
        self.emit_codex_assignment_lifecycle(
            ipc::CodexAssignmentLifecycleAction::Paused,
            &assignment,
        )
        .await;
        self.stale_supervisor_decisions_for_inactive_assignment(&assignment.assignment_id)
            .await?;
        Ok(ipc::CodexAssignmentPauseResponse { assignment })
    }

    async fn codex_assignment_resume(
        &self,
        params: ipc::CodexAssignmentResumeRequest,
    ) -> OrcasResult<ipc::CodexAssignmentResumeResponse> {
        let assignment = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let existing = state
                .collaboration
                .codex_thread_assignments
                .get(&params.assignment_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown Codex thread assignment `{}`",
                        params.assignment_id
                    ))
                })?;
            if existing.status != CodexThreadAssignmentStatus::Paused {
                return Err(OrcasError::Protocol(format!(
                    "Codex thread assignment `{}` can only be resumed from paused state",
                    params.assignment_id
                )));
            }
            if let Some(conflict) =
                state
                    .collaboration
                    .codex_thread_assignments
                    .values()
                    .find(|assignment| {
                        assignment.assignment_id != params.assignment_id
                            && assignment.codex_thread_id == existing.codex_thread_id
                            && Self::codex_assignment_status_is_active(assignment.status)
                    })
            {
                return Err(OrcasError::Protocol(format!(
                    "Codex thread `{}` already has active assignment `{}`",
                    existing.codex_thread_id, conflict.assignment_id
                )));
            }
            let assignment = state
                .collaboration
                .codex_thread_assignments
                .get_mut(&params.assignment_id)
                .expect("assignment exists");
            assignment.status = CodexThreadAssignmentStatus::Active;
            assignment.updated_at = now;
            Self::merge_assignment_notes(&mut assignment.notes, params.notes);
            assignment.clone()
        };
        self.persist_collaboration_state().await?;
        self.emit_codex_assignment_lifecycle(
            ipc::CodexAssignmentLifecycleAction::Resumed,
            &assignment,
        )
        .await;
        self.refresh_codex_supervisor_state_for_thread(&assignment.codex_thread_id)
            .await?;
        Ok(ipc::CodexAssignmentResumeResponse { assignment })
    }

    async fn codex_assignment_release(
        &self,
        params: ipc::CodexAssignmentReleaseRequest,
    ) -> OrcasResult<ipc::CodexAssignmentReleaseResponse> {
        let assignment = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let assignment = state
                .collaboration
                .codex_thread_assignments
                .get_mut(&params.assignment_id)
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown Codex thread assignment `{}`",
                        params.assignment_id
                    ))
                })?;
            if assignment.status == CodexThreadAssignmentStatus::Released {
                return Err(OrcasError::Protocol(format!(
                    "Codex thread assignment `{}` is already released",
                    params.assignment_id
                )));
            }
            assignment.status = CodexThreadAssignmentStatus::Released;
            assignment.updated_at = now;
            Self::merge_assignment_notes(&mut assignment.notes, params.notes);
            assignment.clone()
        };
        self.persist_collaboration_state().await?;
        self.emit_codex_assignment_lifecycle(
            ipc::CodexAssignmentLifecycleAction::Released,
            &assignment,
        )
        .await;
        self.stale_supervisor_decisions_for_inactive_assignment(&assignment.assignment_id)
            .await?;
        Ok(ipc::CodexAssignmentReleaseResponse { assignment })
    }

    async fn supervisor_decision_list(
        &self,
        params: ipc::SupervisorDecisionListRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionListResponse> {
        let state = self.state.read().await;
        let mut decisions = state
            .collaboration
            .supervisor_turn_decisions
            .values()
            .filter(|decision| {
                let assignment = state
                    .collaboration
                    .codex_thread_assignments
                    .get(&decision.assignment_id);
                params
                    .assignment_id
                    .as_ref()
                    .map(|assignment_id| &decision.assignment_id == assignment_id)
                    .unwrap_or(true)
                    && params
                        .codex_thread_id
                        .as_ref()
                        .map(|thread_id| &decision.codex_thread_id == thread_id)
                        .unwrap_or(true)
                    && params
                        .workstream_id
                        .as_ref()
                        .map(|workstream_id| {
                            assignment
                                .map(|assignment| &assignment.workstream_id == workstream_id)
                                .unwrap_or(false)
                        })
                        .unwrap_or(true)
                    && params
                        .work_unit_id
                        .as_ref()
                        .map(|work_unit_id| {
                            assignment
                                .map(|assignment| &assignment.work_unit_id == work_unit_id)
                                .unwrap_or(false)
                        })
                        .unwrap_or(true)
                    && params
                        .supervisor_id
                        .as_ref()
                        .map(|supervisor_id| {
                            assignment
                                .map(|assignment| &assignment.supervisor_id == supervisor_id)
                                .unwrap_or(false)
                        })
                        .unwrap_or(true)
                    && params
                        .status
                        .map(|status| decision.status == status)
                        .unwrap_or(true)
                    && params
                        .kind
                        .map(|kind| decision.kind == kind)
                        .unwrap_or(true)
                    && (!params.actionable_only
                        || decision.status == SupervisorTurnDecisionStatus::ProposedToHuman)
                    && (params.include_closed
                        || params.status.is_some()
                        || params.actionable_only
                        || Self::supervisor_decision_is_open(decision.status))
                    && (params.include_superseded
                        || params.status == Some(SupervisorTurnDecisionStatus::Superseded)
                        || decision.status != SupervisorTurnDecisionStatus::Superseded)
            })
            .map(|decision| Self::supervisor_turn_decision_summary(&state.collaboration, decision))
            .collect::<Vec<_>>();
        decisions.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.decision_id.cmp(&right.decision_id))
        });
        if let Some(limit) = params.limit {
            decisions.truncate(limit);
        }
        Ok(ipc::SupervisorDecisionListResponse { decisions })
    }

    async fn supervisor_decision_get(
        &self,
        params: ipc::SupervisorDecisionGetRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionGetResponse> {
        let decision = self
            .state
            .read()
            .await
            .collaboration
            .supervisor_turn_decisions
            .get(&params.decision_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown supervisor decision `{}`",
                    params.decision_id
                ))
            })?;
        Ok(ipc::SupervisorDecisionGetResponse { decision })
    }

    async fn supervisor_decision_propose_steer(
        &self,
        params: ipc::SupervisorDecisionProposeSteerRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionProposeSteerResponse> {
        let (decision, updated_assignment) = {
            let now = Utc::now();
            let requested_by = params
                .requested_by
                .clone()
                .unwrap_or_else(|| "orcas_operator".to_string());
            let mut state = self.state.write().await;
            let assignment = state
                .collaboration
                .codex_thread_assignments
                .get(&params.assignment_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown Codex thread assignment `{}`",
                        params.assignment_id
                    ))
                })?;
            if !Self::codex_assignment_supports_decisions(assignment.status) {
                return Err(OrcasError::Protocol(format!(
                    "Codex thread assignment `{}` is not active",
                    assignment.assignment_id
                )));
            }
            if let Some(conflict_id) = Self::open_supervisor_decision_id_for_assignment(
                &state.collaboration,
                &assignment.assignment_id,
            ) {
                return Err(OrcasError::Protocol(format!(
                    "assignment `{}` already has open supervisor decision `{}`",
                    assignment.assignment_id, conflict_id
                )));
            }
            let thread = state
                .threads
                .get(&assignment.codex_thread_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "thread `{}` is not loaded in Orcas state",
                        assignment.codex_thread_id
                    ))
                })?;
            let active_turn_id = thread.summary.active_turn_id.clone().ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "thread `{}` has no active turn to steer",
                    assignment.codex_thread_id
                ))
            })?;
            let workstream = state
                .collaboration
                .workstreams
                .get(&assignment.workstream_id);
            let work_unit = state.collaboration.work_units.get(&assignment.work_unit_id);
            let proposed_text = params
                .proposed_text
                .as_ref()
                .map(|text| text.trim())
                .filter(|text| !text.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    OrcasError::Protocol(
                        "steer proposal requires non-empty proposed_text".to_string(),
                    )
                })?;
            let workstream_title = workstream
                .map(|workstream| workstream.title.clone())
                .unwrap_or_else(|| assignment.workstream_id.clone());
            let work_unit_title = work_unit
                .map(|work_unit| work_unit.title.clone())
                .unwrap_or_else(|| assignment.work_unit_id.clone());
            let rationale_summary = params
                .rationale_note
                .as_ref()
                .map(|note| note.trim())
                .filter(|note| !note.is_empty())
                .map(|note| note.to_string())
                .unwrap_or_else(|| {
                    format!(
                        "Operator `{requested_by}` requested review of steering active turn `{active_turn_id}` for assigned workstream `{}` and work unit `{}`.",
                        workstream_title, work_unit_title
                    )
                });
            let decision = SupervisorTurnDecision {
                decision_id: Self::new_object_id("std"),
                assignment_id: assignment.assignment_id.clone(),
                codex_thread_id: assignment.codex_thread_id.clone(),
                basis_turn_id: Some(active_turn_id.clone()),
                kind: SupervisorTurnDecisionKind::SteerActiveTurn,
                proposal_kind: SupervisorTurnProposalKind::OperatorSteer,
                proposed_text: Some(proposed_text),
                rationale_summary,
                status: SupervisorTurnDecisionStatus::ProposedToHuman,
                created_at: now,
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: Some(format!("steer proposal requested by {requested_by}")),
            };
            state
                .collaboration
                .supervisor_turn_decisions
                .insert(decision.decision_id.clone(), decision.clone());
            let updated_assignment = state
                .collaboration
                .codex_thread_assignments
                .get_mut(&assignment.assignment_id)
                .map(|assignment| {
                    assignment.latest_decision_id = Some(decision.decision_id.clone());
                    assignment.latest_basis_turn_id = Some(active_turn_id);
                    assignment.updated_at = now;
                    assignment.clone()
                });
            (decision, updated_assignment)
        };
        self.persist_collaboration_state().await?;
        if let Some(assignment) = updated_assignment.as_ref() {
            self.emit_codex_assignment_lifecycle(
                ipc::CodexAssignmentLifecycleAction::Updated,
                assignment,
            )
            .await;
        }
        self.emit_supervisor_decision_lifecycle(
            ipc::SupervisorDecisionLifecycleAction::Created,
            &decision,
        )
        .await;
        Ok(ipc::SupervisorDecisionProposeSteerResponse { decision })
    }

    async fn supervisor_decision_replace_pending_steer(
        &self,
        params: ipc::SupervisorDecisionReplacePendingSteerRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionReplacePendingSteerResponse> {
        let proposed_text = params.proposed_text.trim();
        if proposed_text.is_empty() {
            return Err(OrcasError::Protocol(
                "pending steer replacement requires non-empty proposed_text".to_string(),
            ));
        }

        let requested_by = params
            .requested_by
            .clone()
            .unwrap_or_else(|| "orcas_operator".to_string());

        enum ReplacePendingSteerOutcome {
            Replaced {
                superseded: SupervisorTurnDecision,
                replacement: SupervisorTurnDecision,
                updated_assignment: Option<CodexThreadAssignment>,
            },
            Stale {
                decision: SupervisorTurnDecision,
                reason: String,
            },
        }

        let outcome = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let existing = state
                .collaboration
                .supervisor_turn_decisions
                .get(&params.decision_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown supervisor decision `{}`",
                        params.decision_id
                    ))
                })?;
            if existing.kind != SupervisorTurnDecisionKind::SteerActiveTurn {
                return Err(OrcasError::Protocol(format!(
                    "supervisor decision `{}` is not a steer decision",
                    existing.decision_id
                )));
            }
            if existing.status != SupervisorTurnDecisionStatus::ProposedToHuman {
                return Err(OrcasError::Protocol(format!(
                    "supervisor decision `{}` is no longer editable",
                    existing.decision_id
                )));
            }

            let assignment = state
                .collaboration
                .codex_thread_assignments
                .get(&existing.assignment_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "assignment `{}` no longer exists",
                        existing.assignment_id
                    ))
                })?;
            let thread = state
                .threads
                .get(&existing.codex_thread_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "thread `{}` is not loaded in Orcas state",
                        existing.codex_thread_id
                    ))
                })?;
            if let Some(reason) = Self::steer_decision_basis_reason(&assignment, &thread, &existing)
            {
                let stale = {
                    let decision = state
                        .collaboration
                        .supervisor_turn_decisions
                        .get_mut(&existing.decision_id)
                        .expect("decision exists");
                    decision.status = SupervisorTurnDecisionStatus::Stale;
                    Self::merge_assignment_notes(
                        &mut decision.notes,
                        Some(format!(
                            "pending steer edit rejected because the decision became stale: {reason}"
                        )),
                    );
                    decision.clone()
                };
                ReplacePendingSteerOutcome::Stale {
                    decision: stale,
                    reason,
                }
            } else {
                let workstream_title = state
                    .collaboration
                    .workstreams
                    .get(&assignment.workstream_id)
                    .map(|workstream| workstream.title.clone())
                    .unwrap_or_else(|| assignment.workstream_id.clone());
                let work_unit_title = state
                    .collaboration
                    .work_units
                    .get(&assignment.work_unit_id)
                    .map(|work_unit| work_unit.title.clone())
                    .unwrap_or_else(|| assignment.work_unit_id.clone());
                let rationale_summary = params
                    .rationale_note
                    .as_ref()
                    .map(|note| note.trim())
                    .filter(|note| !note.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| {
                        format!(
                            "Operator `{requested_by}` revised steer guidance for active turn `{}` on assigned workstream `{}` and work unit `{}`.",
                            existing.basis_turn_id.as_deref().unwrap_or("-"),
                            workstream_title,
                            work_unit_title
                        )
                    });
                let new_decision_id = Self::new_object_id("std");
                let superseded = {
                    let decision = state
                        .collaboration
                        .supervisor_turn_decisions
                        .get_mut(&existing.decision_id)
                        .expect("decision exists");
                    decision.status = SupervisorTurnDecisionStatus::Superseded;
                    decision.superseded_by = Some(new_decision_id.clone());
                    Self::merge_assignment_notes(
                        &mut decision.notes,
                        Some(format!(
                            "superseded by updated steer text from {requested_by}"
                        )),
                    );
                    decision.clone()
                };
                let replacement = SupervisorTurnDecision {
                    decision_id: new_decision_id.clone(),
                    assignment_id: existing.assignment_id.clone(),
                    codex_thread_id: existing.codex_thread_id.clone(),
                    basis_turn_id: existing.basis_turn_id.clone(),
                    kind: SupervisorTurnDecisionKind::SteerActiveTurn,
                    proposal_kind: SupervisorTurnProposalKind::OperatorSteer,
                    proposed_text: Some(proposed_text.to_string()),
                    rationale_summary,
                    status: SupervisorTurnDecisionStatus::ProposedToHuman,
                    created_at: now,
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: Some(format!(
                        "steer text replaced from prior decision {} by {requested_by}",
                        existing.decision_id
                    )),
                };
                state
                    .collaboration
                    .supervisor_turn_decisions
                    .insert(replacement.decision_id.clone(), replacement.clone());
                let updated_assignment = state
                    .collaboration
                    .codex_thread_assignments
                    .get_mut(&existing.assignment_id)
                    .map(|assignment| {
                        assignment.latest_decision_id = Some(replacement.decision_id.clone());
                        assignment.latest_basis_turn_id = replacement.basis_turn_id.clone();
                        assignment.updated_at = now;
                        assignment.clone()
                    });
                ReplacePendingSteerOutcome::Replaced {
                    superseded,
                    replacement,
                    updated_assignment,
                }
            }
        };

        match outcome {
            ReplacePendingSteerOutcome::Replaced {
                superseded,
                replacement,
                updated_assignment,
            } => {
                self.persist_collaboration_state().await?;
                if let Some(assignment) = updated_assignment.as_ref() {
                    self.emit_codex_assignment_lifecycle(
                        ipc::CodexAssignmentLifecycleAction::Updated,
                        assignment,
                    )
                    .await;
                }
                self.emit_supervisor_decision_lifecycle(
                    ipc::SupervisorDecisionLifecycleAction::Superseded,
                    &superseded,
                )
                .await;
                self.emit_supervisor_decision_lifecycle(
                    ipc::SupervisorDecisionLifecycleAction::Created,
                    &replacement,
                )
                .await;
                Ok(ipc::SupervisorDecisionReplacePendingSteerResponse {
                    decision: replacement,
                })
            }
            ReplacePendingSteerOutcome::Stale { decision, reason } => {
                self.persist_collaboration_state().await?;
                self.emit_supervisor_decision_lifecycle(
                    ipc::SupervisorDecisionLifecycleAction::Stale,
                    &decision,
                )
                .await;
                Err(OrcasError::Protocol(format!(
                    "steer decision `{}` became stale: {reason}",
                    decision.decision_id
                )))
            }
        }
    }

    async fn supervisor_decision_propose_interrupt(
        &self,
        params: ipc::SupervisorDecisionProposeInterruptRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionProposeInterruptResponse> {
        let (decision, updated_assignment) = {
            let now = Utc::now();
            let requested_by = params
                .requested_by
                .clone()
                .unwrap_or_else(|| "orcas_operator".to_string());
            let mut state = self.state.write().await;
            let assignment = state
                .collaboration
                .codex_thread_assignments
                .get(&params.assignment_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown Codex thread assignment `{}`",
                        params.assignment_id
                    ))
                })?;
            if !Self::codex_assignment_supports_decisions(assignment.status) {
                return Err(OrcasError::Protocol(format!(
                    "Codex thread assignment `{}` is not active",
                    assignment.assignment_id
                )));
            }
            if let Some(conflict_id) = Self::open_supervisor_decision_id_for_assignment(
                &state.collaboration,
                &assignment.assignment_id,
            ) {
                return Err(OrcasError::Protocol(format!(
                    "assignment `{}` already has open supervisor decision `{}`",
                    assignment.assignment_id, conflict_id
                )));
            }
            let thread = state
                .threads
                .get(&assignment.codex_thread_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "thread `{}` is not loaded in Orcas state",
                        assignment.codex_thread_id
                    ))
                })?;
            let active_turn_id = thread.summary.active_turn_id.clone().ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "thread `{}` has no active turn to interrupt",
                    assignment.codex_thread_id
                ))
            })?;
            let workstream_title = state
                .collaboration
                .workstreams
                .get(&assignment.workstream_id)
                .map(|workstream| workstream.title.clone())
                .unwrap_or_else(|| assignment.workstream_id.clone());
            let work_unit_title = state
                .collaboration
                .work_units
                .get(&assignment.work_unit_id)
                .map(|work_unit| work_unit.title.clone())
                .unwrap_or_else(|| assignment.work_unit_id.clone());
            let rationale_summary = params
                .rationale_note
                .as_ref()
                .map(|note| note.trim())
                .filter(|note| !note.is_empty())
                .map(|note| note.to_string())
                .unwrap_or_else(|| {
                    format!(
                        "Operator `{requested_by}` requested review of interrupting active turn `{active_turn_id}` for assigned workstream `{}` and work unit `{}`.",
                        workstream_title, work_unit_title
                    )
                });
            let decision = SupervisorTurnDecision {
                decision_id: Self::new_object_id("std"),
                assignment_id: assignment.assignment_id.clone(),
                codex_thread_id: assignment.codex_thread_id.clone(),
                basis_turn_id: Some(active_turn_id.clone()),
                kind: SupervisorTurnDecisionKind::InterruptActiveTurn,
                proposal_kind: SupervisorTurnProposalKind::OperatorInterrupt,
                proposed_text: None,
                rationale_summary,
                status: SupervisorTurnDecisionStatus::ProposedToHuman,
                created_at: now,
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: Some(format!("interrupt proposal requested by {requested_by}")),
            };
            state
                .collaboration
                .supervisor_turn_decisions
                .insert(decision.decision_id.clone(), decision.clone());
            let updated_assignment = state
                .collaboration
                .codex_thread_assignments
                .get_mut(&assignment.assignment_id)
                .map(|assignment| {
                    assignment.latest_decision_id = Some(decision.decision_id.clone());
                    assignment.latest_basis_turn_id = Some(active_turn_id);
                    assignment.updated_at = now;
                    assignment.clone()
                });
            (decision, updated_assignment)
        };
        self.persist_collaboration_state().await?;
        if let Some(assignment) = updated_assignment.as_ref() {
            self.emit_codex_assignment_lifecycle(
                ipc::CodexAssignmentLifecycleAction::Updated,
                assignment,
            )
            .await;
        }
        self.emit_supervisor_decision_lifecycle(
            ipc::SupervisorDecisionLifecycleAction::Created,
            &decision,
        )
        .await;
        Ok(ipc::SupervisorDecisionProposeInterruptResponse { decision })
    }

    async fn supervisor_decision_record_no_action(
        &self,
        params: ipc::SupervisorDecisionRecordNoActionRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionRecordNoActionResponse> {
        let started_at = Instant::now();
        info!(
            decision_id = %params.decision_id,
            action = "record_no_action",
            "starting supervisor review action"
        );
        enum RecordNoActionOutcome {
            Recorded {
                superseded: SupervisorTurnDecision,
                recorded: SupervisorTurnDecision,
                updated_assignment: Option<CodexThreadAssignment>,
            },
            Stale {
                decision: SupervisorTurnDecision,
                updated_assignment: Option<CodexThreadAssignment>,
                reason: String,
            },
        }

        let outcome = {
            let now = Utc::now();
            let reviewed_by = params
                .reviewed_by
                .clone()
                .unwrap_or_else(|| "orcas_operator".to_string());
            let mut state = self.state.write().await;
            let existing = state
                .collaboration
                .supervisor_turn_decisions
                .get(&params.decision_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown supervisor decision `{}`",
                        params.decision_id
                    ))
                })?;
            if existing.kind != SupervisorTurnDecisionKind::NextTurn {
                warn!(
                    decision_id = %existing.decision_id,
                    assignment_id = %existing.assignment_id,
                    action = "record_no_action",
                    reason = "decision is not next-turn",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action rejected"
                );
                return Err(OrcasError::Protocol(format!(
                    "supervisor decision `{}` is not a next-turn decision",
                    existing.decision_id
                )));
            }
            if existing.status != SupervisorTurnDecisionStatus::ProposedToHuman {
                warn!(
                    decision_id = %existing.decision_id,
                    assignment_id = %existing.assignment_id,
                    action = "record_no_action",
                    reason = "decision is not pending human review",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action rejected"
                );
                return Err(OrcasError::Protocol(format!(
                    "supervisor decision `{}` is not pending human review",
                    existing.decision_id
                )));
            }

            let assignment = state
                .collaboration
                .codex_thread_assignments
                .get(&existing.assignment_id)
                .cloned()
                .ok_or_else(|| {
                    warn!(
                        decision_id = %existing.decision_id,
                        assignment_id = %existing.assignment_id,
                        action = "record_no_action",
                        reason = "assignment no longer exists",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "supervisor review action failed"
                    );
                    OrcasError::Protocol(format!(
                        "assignment `{}` no longer exists",
                        existing.assignment_id
                    ))
                })?;
            let thread = state
                .threads
                .get(&existing.codex_thread_id)
                .cloned()
                .ok_or_else(|| {
                    warn!(
                        decision_id = %existing.decision_id,
                        assignment_id = %existing.assignment_id,
                        action = "record_no_action",
                        thread_id = %existing.codex_thread_id,
                        reason = "thread is not loaded in Orcas state",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "supervisor review action failed"
                    );
                    OrcasError::Protocol(format!(
                        "thread `{}` is not loaded in Orcas state",
                        existing.codex_thread_id
                    ))
                })?;

            if let Some(reason) =
                Self::next_turn_decision_basis_reason(&assignment, &thread, &existing)
            {
                let stale = {
                    let decision = state
                        .collaboration
                        .supervisor_turn_decisions
                        .get_mut(&existing.decision_id)
                        .expect("decision exists");
                    decision.status = SupervisorTurnDecisionStatus::Stale;
                    Self::merge_assignment_notes(
                        &mut decision.notes,
                        Some(format!(
                            "record no_action rejected because the decision became stale: {reason}"
                        )),
                    );
                    decision.clone()
                };
                let updated_assignment = if existing.proposal_kind
                    == SupervisorTurnProposalKind::Bootstrap
                {
                    state
                        .collaboration
                        .codex_thread_assignments
                        .get_mut(&existing.assignment_id)
                        .and_then(|assignment| {
                            if assignment.bootstrap_state == CodexThreadBootstrapState::Proposed {
                                assignment.bootstrap_state = CodexThreadBootstrapState::Pending;
                                assignment.updated_at = now;
                                Some(assignment.clone())
                            } else {
                                None
                            }
                        })
                } else {
                    None
                };
                RecordNoActionOutcome::Stale {
                    decision: stale,
                    updated_assignment,
                    reason,
                }
            } else {
                let new_decision_id = Self::new_object_id("std");
                let superseded = {
                    let decision = state
                        .collaboration
                        .supervisor_turn_decisions
                        .get_mut(&existing.decision_id)
                        .expect("decision exists");
                    decision.status = SupervisorTurnDecisionStatus::Superseded;
                    decision.superseded_by = Some(new_decision_id.clone());
                    Self::merge_assignment_notes(
                        &mut decision.notes,
                        Some(format!(
                            "superseded by no_action recorded from {reviewed_by}"
                        )),
                    );
                    decision.clone()
                };
                let rationale_summary = params
                    .review_note
                    .as_ref()
                    .map(|note| note.trim())
                    .filter(|note| !note.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| {
                        format!(
                            "Operator `{reviewed_by}` chose to wait on the current idle-thread basis for assignment `{}`.",
                            existing.assignment_id
                        )
                    });
                let recorded = SupervisorTurnDecision {
                    decision_id: new_decision_id,
                    assignment_id: existing.assignment_id.clone(),
                    codex_thread_id: existing.codex_thread_id.clone(),
                    basis_turn_id: existing.basis_turn_id.clone(),
                    kind: SupervisorTurnDecisionKind::NoAction,
                    proposal_kind: existing.proposal_kind,
                    proposed_text: None,
                    rationale_summary,
                    status: SupervisorTurnDecisionStatus::Recorded,
                    created_at: now,
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: Some(format!("no_action recorded by {reviewed_by}")),
                };
                state
                    .collaboration
                    .supervisor_turn_decisions
                    .insert(recorded.decision_id.clone(), recorded.clone());
                let updated_assignment = state
                    .collaboration
                    .codex_thread_assignments
                    .get_mut(&existing.assignment_id)
                    .map(|assignment| {
                        assignment.latest_decision_id = Some(recorded.decision_id.clone());
                        assignment.latest_basis_turn_id = recorded.basis_turn_id.clone();
                        assignment.updated_at = now;
                        if existing.proposal_kind == SupervisorTurnProposalKind::Bootstrap {
                            assignment.bootstrap_state = CodexThreadBootstrapState::NotNeeded;
                        }
                        assignment.clone()
                    });
                RecordNoActionOutcome::Recorded {
                    superseded,
                    recorded,
                    updated_assignment,
                }
            }
        };

        match outcome {
            RecordNoActionOutcome::Recorded {
                superseded,
                recorded,
                updated_assignment,
            } => {
                self.persist_collaboration_state().await?;
                if let Some(assignment) = updated_assignment.as_ref() {
                    self.emit_codex_assignment_lifecycle(
                        ipc::CodexAssignmentLifecycleAction::Updated,
                        assignment,
                    )
                    .await;
                }
                self.emit_supervisor_decision_lifecycle(
                    ipc::SupervisorDecisionLifecycleAction::Superseded,
                    &superseded,
                )
                .await;
                self.emit_supervisor_decision_lifecycle(
                    ipc::SupervisorDecisionLifecycleAction::Created,
                    &recorded,
                )
                .await;
                info!(
                    decision_id = %recorded.decision_id,
                    assignment_id = %recorded.assignment_id,
                    action = "record_no_action",
                    result = "recorded",
                    superseded_decision_id = %superseded.decision_id,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action persisted"
                );
                Ok(ipc::SupervisorDecisionRecordNoActionResponse { decision: recorded })
            }
            RecordNoActionOutcome::Stale {
                decision,
                updated_assignment,
                reason,
            } => {
                self.persist_collaboration_state().await?;
                if let Some(assignment) = updated_assignment.as_ref() {
                    self.emit_codex_assignment_lifecycle(
                        ipc::CodexAssignmentLifecycleAction::Updated,
                        assignment,
                    )
                    .await;
                }
                self.emit_supervisor_decision_lifecycle(
                    ipc::SupervisorDecisionLifecycleAction::Stale,
                    &decision,
                )
                .await;
                warn!(
                    decision_id = %decision.decision_id,
                    assignment_id = %decision.assignment_id,
                    action = "record_no_action",
                    result = "stale",
                    reason = %reason,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action failed"
                );
                Err(OrcasError::Protocol(format!(
                    "supervisor decision `{}` became stale and no_action was not recorded: {reason}",
                    decision.decision_id
                )))
            }
        }
    }

    async fn supervisor_decision_manual_refresh(
        &self,
        params: ipc::SupervisorDecisionManualRefreshRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionManualRefreshResponse> {
        let started_at = Instant::now();
        info!(
            assignment_id = %params.assignment_id,
            action = "manual_refresh",
            "starting supervisor review action"
        );
        let (decision, updated_assignment) = {
            let now = Utc::now();
            let requested_by = params
                .requested_by
                .clone()
                .unwrap_or_else(|| "orcas_operator".to_string());
            let mut state = self.state.write().await;
            let assignment = state
                .collaboration
                .codex_thread_assignments
                .get(&params.assignment_id)
                .cloned()
                .ok_or_else(|| {
                    warn!(
                        assignment_id = %params.assignment_id,
                        action = "manual_refresh",
                        reason = "unknown Codex thread assignment",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "supervisor review action failed"
                    );
                    OrcasError::Protocol(format!(
                        "unknown Codex thread assignment `{}`",
                        params.assignment_id
                    ))
                })?;
            if !Self::codex_assignment_supports_decisions(assignment.status) {
                warn!(
                    assignment_id = %assignment.assignment_id,
                    action = "manual_refresh",
                    reason = "assignment is not active",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action rejected"
                );
                return Err(OrcasError::Protocol(format!(
                    "Codex thread assignment `{}` is not active",
                    assignment.assignment_id
                )));
            }
            if let Some(conflict_id) = Self::open_supervisor_decision_id_for_assignment(
                &state.collaboration,
                &assignment.assignment_id,
            ) {
                warn!(
                    assignment_id = %assignment.assignment_id,
                    action = "manual_refresh",
                    reason = "open supervisor decision already exists",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action rejected"
                );
                return Err(OrcasError::Protocol(format!(
                    "assignment `{}` already has open supervisor decision `{}`",
                    assignment.assignment_id, conflict_id
                )));
            }
            let thread = state
                .threads
                .get(&assignment.codex_thread_id)
                .cloned()
                .ok_or_else(|| {
                    warn!(
                        assignment_id = %assignment.assignment_id,
                        thread_id = %assignment.codex_thread_id,
                        action = "manual_refresh",
                        reason = "thread is not loaded in Orcas state",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "supervisor review action failed"
                    );
                    OrcasError::Protocol(format!(
                        "thread `{}` is not loaded in Orcas state",
                        assignment.codex_thread_id
                    ))
                })?;
            if thread.summary.active_turn_id.is_some() {
                warn!(
                    assignment_id = %assignment.assignment_id,
                    thread_id = %thread.summary.id,
                    action = "manual_refresh",
                    reason = "thread has active turn",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action rejected"
                );
                return Err(OrcasError::Protocol(format!(
                    "thread `{}` has an active turn and cannot manual-refresh a next-turn proposal",
                    thread.summary.id
                )));
            }
            let basis_turn_id = thread.summary.last_seen_turn_id.clone();
            if Self::latest_current_basis_recorded_no_action(
                &state.collaboration,
                &assignment.assignment_id,
                basis_turn_id.as_deref(),
            )
            .is_none()
            {
                warn!(
                    assignment_id = %assignment.assignment_id,
                    action = "manual_refresh",
                    reason = "no recorded no_action for current basis",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action rejected"
                );
                return Err(OrcasError::Protocol(format!(
                    "assignment `{}` has no recorded no_action for the current basis",
                    assignment.assignment_id
                )));
            }

            let rationale_summary = params
                .rationale_note
                .as_ref()
                .map(|note| note.trim())
                .filter(|note| !note.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| {
                    format!(
                        "Operator `{requested_by}` requested a manual refresh of the next-turn proposal for assignment `{}` on the current idle-thread basis.",
                        assignment.assignment_id
                    )
                });
            let decision = SupervisorTurnDecision {
                decision_id: Self::new_object_id("std"),
                assignment_id: assignment.assignment_id.clone(),
                codex_thread_id: assignment.codex_thread_id.clone(),
                basis_turn_id: basis_turn_id.clone(),
                kind: SupervisorTurnDecisionKind::NextTurn,
                proposal_kind: SupervisorTurnProposalKind::ManualRefresh,
                proposed_text: Some(Self::generate_continue_turn_text(
                    &assignment,
                    state
                        .collaboration
                        .workstreams
                        .get(&assignment.workstream_id),
                    state.collaboration.work_units.get(&assignment.work_unit_id),
                    basis_turn_id.as_deref(),
                )),
                rationale_summary,
                status: SupervisorTurnDecisionStatus::ProposedToHuman,
                created_at: now,
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: Some(format!("manual refresh requested by {requested_by}")),
            };
            state
                .collaboration
                .supervisor_turn_decisions
                .insert(decision.decision_id.clone(), decision.clone());
            let updated_assignment = state
                .collaboration
                .codex_thread_assignments
                .get_mut(&assignment.assignment_id)
                .map(|assignment| {
                    assignment.latest_decision_id = Some(decision.decision_id.clone());
                    assignment.latest_basis_turn_id = basis_turn_id;
                    assignment.updated_at = now;
                    assignment.clone()
                });
            (decision, updated_assignment)
        };
        self.persist_collaboration_state().await?;
        if let Some(assignment) = updated_assignment.as_ref() {
            self.emit_codex_assignment_lifecycle(
                ipc::CodexAssignmentLifecycleAction::Updated,
                assignment,
            )
            .await;
        }
        self.emit_supervisor_decision_lifecycle(
            ipc::SupervisorDecisionLifecycleAction::Created,
            &decision,
        )
        .await;
        info!(
            decision_id = %decision.decision_id,
            assignment_id = %decision.assignment_id,
            action = "manual_refresh",
            result = "created",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "supervisor review action persisted"
        );
        Ok(ipc::SupervisorDecisionManualRefreshResponse { decision })
    }

    async fn supervisor_decision_approve_and_send(
        &self,
        params: ipc::SupervisorDecisionApproveAndSendRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionApproveAndSendResponse> {
        let started_at = Instant::now();
        info!(
            decision_id = %params.decision_id,
            action = "approve_and_send",
            "starting supervisor review action"
        );
        let decision = self
            .state
            .read()
            .await
            .collaboration
            .supervisor_turn_decisions
            .get(&params.decision_id)
            .cloned()
            .ok_or_else(|| {
                warn!(
                    decision_id = %params.decision_id,
                    action = "approve_and_send",
                    reason = "unknown supervisor decision",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action failed"
                );
                OrcasError::Protocol(format!(
                    "unknown supervisor decision `{}`",
                    params.decision_id
                ))
            })?;
        if decision.status != SupervisorTurnDecisionStatus::ProposedToHuman {
            warn!(
                decision_id = %decision.decision_id,
                assignment_id = %decision.assignment_id,
                action = "approve_and_send",
                reason = "decision is not pending human review",
                duration_ms = started_at.elapsed().as_millis() as u64,
                "supervisor review action rejected"
            );
            return Err(OrcasError::Protocol(format!(
                "supervisor decision `{}` is not pending human review",
                decision.decision_id
            )));
        }
        if let Err(reason) = self.validate_supervisor_send_basis(&decision).await {
            let stale = self
                .mark_supervisor_decision_stale(&decision.decision_id, &reason)
                .await?;
            warn!(
                decision_id = %stale.decision_id,
                assignment_id = %stale.assignment_id,
                action = "approve_and_send",
                result = "stale",
                reason = %reason,
                duration_ms = started_at.elapsed().as_millis() as u64,
                "supervisor review action failed"
            );
            return Err(OrcasError::Protocol(format!(
                "supervisor decision `{}` became stale and was not sent: {}",
                stale.decision_id, reason
            )));
        }

        {
            let mut state = self.state.write().await;
            let decision_record = state
                .collaboration
                .supervisor_turn_decisions
                .get_mut(&params.decision_id)
                .ok_or_else(|| {
                    warn!(
                        decision_id = %params.decision_id,
                        action = "approve_and_send",
                        reason = "unknown supervisor decision during approval",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "supervisor review action failed"
                    );
                    OrcasError::Protocol(format!(
                        "unknown supervisor decision `{}`",
                        params.decision_id
                    ))
                })?;
            if decision_record.status != SupervisorTurnDecisionStatus::ProposedToHuman {
                warn!(
                    decision_id = %decision_record.decision_id,
                    assignment_id = %decision_record.assignment_id,
                    action = "approve_and_send",
                    reason = "decision is no longer pending human review",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action rejected"
                );
                return Err(OrcasError::Protocol(format!(
                    "supervisor decision `{}` is no longer pending human review",
                    decision_record.decision_id
                )));
            }
            decision_record.status = SupervisorTurnDecisionStatus::Approved;
        }

        let sent_turn_id = match decision.kind {
            SupervisorTurnDecisionKind::NextTurn => {
                let proposed_text = decision.proposed_text.clone().ok_or_else(|| {
                    warn!(
                        decision_id = %decision.decision_id,
                        assignment_id = %decision.assignment_id,
                        action = "approve_and_send",
                        reason = "next-turn decision has no proposed text",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "supervisor review action failed"
                    );
                    OrcasError::Protocol(format!(
                        "supervisor decision `{}` has no proposed text to send",
                        decision.decision_id
                    ))
                })?;
                match self
                    .turn_start(ipc::TurnStartRequest {
                        thread_id: decision.codex_thread_id.clone(),
                        text: proposed_text,
                        cwd: None,
                        model: None,
                    })
                    .await
                {
                    Ok(response) => Some(response),
                    Err(error) => {
                        let mut state = self.state.write().await;
                        if let Some(decision_record) = state
                            .collaboration
                            .supervisor_turn_decisions
                            .get_mut(&params.decision_id)
                            && decision_record.status == SupervisorTurnDecisionStatus::Approved
                            && decision_record.sent_at.is_none()
                        {
                            decision_record.status = SupervisorTurnDecisionStatus::ProposedToHuman;
                        }
                        warn!(
                            decision_id = %decision.decision_id,
                            assignment_id = %decision.assignment_id,
                            action = "approve_and_send",
                            result = "send_failed",
                            duration_ms = started_at.elapsed().as_millis() as u64,
                            error = %error,
                            "supervisor review action failed"
                        );
                        return Err(error);
                    }
                }
                .map(|response| response.turn_id)
            }
            SupervisorTurnDecisionKind::SteerActiveTurn => {
                let basis_turn_id = decision.basis_turn_id.clone().ok_or_else(|| {
                    warn!(
                        decision_id = %decision.decision_id,
                        assignment_id = %decision.assignment_id,
                        action = "approve_and_send",
                        reason = "steer decision missing basis_turn_id",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "supervisor review action failed"
                    );
                    OrcasError::Protocol(format!(
                        "steer decision `{}` is missing basis_turn_id",
                        decision.decision_id
                    ))
                })?;
                let proposed_text = decision
                    .proposed_text
                    .as_ref()
                    .map(|text| text.trim())
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| {
                        warn!(
                            decision_id = %decision.decision_id,
                            assignment_id = %decision.assignment_id,
                            action = "approve_and_send",
                            reason = "steer decision has no proposed text",
                            duration_ms = started_at.elapsed().as_millis() as u64,
                            "supervisor review action failed"
                        );
                        OrcasError::Protocol(format!(
                            "steer decision `{}` has no proposed text to send",
                            decision.decision_id
                        ))
                    })?
                    .to_string();
                match self
                    .turn_steer(ipc::TurnSteerRequest {
                        thread_id: decision.codex_thread_id.clone(),
                        expected_turn_id: basis_turn_id,
                        text: proposed_text,
                    })
                    .await
                {
                    Ok(_response) => None,
                    Err(error) => {
                        let mut state = self.state.write().await;
                        if let Some(decision_record) = state
                            .collaboration
                            .supervisor_turn_decisions
                            .get_mut(&params.decision_id)
                            && decision_record.status == SupervisorTurnDecisionStatus::Approved
                            && decision_record.sent_at.is_none()
                        {
                            decision_record.status = SupervisorTurnDecisionStatus::ProposedToHuman;
                        }
                        warn!(
                            decision_id = %decision.decision_id,
                            assignment_id = %decision.assignment_id,
                            action = "approve_and_send",
                            result = "send_failed",
                            duration_ms = started_at.elapsed().as_millis() as u64,
                            error = %error,
                            "supervisor review action failed"
                        );
                        return Err(error);
                    }
                }
            }
            SupervisorTurnDecisionKind::InterruptActiveTurn => {
                let basis_turn_id = decision.basis_turn_id.clone().ok_or_else(|| {
                    warn!(
                        decision_id = %decision.decision_id,
                        assignment_id = %decision.assignment_id,
                        action = "approve_and_send",
                        reason = "interrupt decision missing basis_turn_id",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "supervisor review action failed"
                    );
                    OrcasError::Protocol(format!(
                        "interrupt decision `{}` is missing basis_turn_id",
                        decision.decision_id
                    ))
                })?;
                match self
                    .turn_interrupt(ipc::TurnInterruptRequest {
                        thread_id: decision.codex_thread_id.clone(),
                        turn_id: basis_turn_id,
                    })
                    .await
                {
                    Ok(()) => None,
                    Err(error) => {
                        let mut state = self.state.write().await;
                        if let Some(decision_record) = state
                            .collaboration
                            .supervisor_turn_decisions
                            .get_mut(&params.decision_id)
                            && decision_record.status == SupervisorTurnDecisionStatus::Approved
                            && decision_record.sent_at.is_none()
                        {
                            decision_record.status = SupervisorTurnDecisionStatus::ProposedToHuman;
                        }
                        warn!(
                            decision_id = %decision.decision_id,
                            assignment_id = %decision.assignment_id,
                            action = "approve_and_send",
                            result = "send_failed",
                            duration_ms = started_at.elapsed().as_millis() as u64,
                            error = %error,
                            "supervisor review action failed"
                        );
                        return Err(error);
                    }
                }
            }
            SupervisorTurnDecisionKind::NoAction => {
                let mut state = self.state.write().await;
                if let Some(decision_record) = state
                    .collaboration
                    .supervisor_turn_decisions
                    .get_mut(&params.decision_id)
                    && decision_record.status == SupervisorTurnDecisionStatus::Approved
                    && decision_record.sent_at.is_none()
                {
                    decision_record.status = SupervisorTurnDecisionStatus::ProposedToHuman;
                }
                warn!(
                    decision_id = %decision.decision_id,
                    assignment_id = %decision.assignment_id,
                    action = "approve_and_send",
                    reason = "no_action decisions are not sendable",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action rejected"
                );
                return Err(OrcasError::Protocol(
                    "no_action decisions are not sendable in this slice".to_string(),
                ));
            }
        };

        let (approved, sent, updated_assignment) = {
            let now = Utc::now();
            let reviewed_by = params
                .reviewed_by
                .clone()
                .unwrap_or_else(|| "orcas_operator".to_string());
            let mut state = self.state.write().await;
            let decision_record = state
                .collaboration
                .supervisor_turn_decisions
                .get_mut(&params.decision_id)
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown supervisor decision `{}`",
                        params.decision_id
                    ))
                })?;
            if decision_record.status != SupervisorTurnDecisionStatus::Approved {
                return Err(OrcasError::Protocol(format!(
                    "supervisor decision `{}` is no longer approved for send",
                    decision_record.decision_id
                )));
            }

            decision_record.approved_at = Some(now);
            let mut approval_note = format!("approved by {reviewed_by}");
            if let Some(review_note) = params.review_note.as_ref().map(|note| note.trim()) {
                if !review_note.is_empty() {
                    approval_note.push_str(": ");
                    approval_note.push_str(review_note);
                }
            }
            Self::merge_assignment_notes(&mut decision_record.notes, Some(approval_note));
            let approved = decision_record.clone();

            decision_record.status = SupervisorTurnDecisionStatus::Sent;
            decision_record.sent_at = Some(now);
            decision_record.sent_turn_id = match decision.kind {
                SupervisorTurnDecisionKind::NextTurn => sent_turn_id,
                SupervisorTurnDecisionKind::SteerActiveTurn
                | SupervisorTurnDecisionKind::InterruptActiveTurn
                | SupervisorTurnDecisionKind::NoAction => None,
            };
            let sent = decision_record.clone();

            let updated_assignment = if let Some(assignment) = state
                .collaboration
                .codex_thread_assignments
                .get_mut(&decision.assignment_id)
            {
                assignment.latest_decision_id = Some(decision.decision_id.clone());
                assignment.latest_basis_turn_id = decision.basis_turn_id.clone();
                assignment.updated_at = now;
                if decision.proposal_kind == SupervisorTurnProposalKind::Bootstrap {
                    assignment.bootstrap_state = CodexThreadBootstrapState::Sent;
                }
                Some(assignment.clone())
            } else {
                None
            };
            (approved, sent, updated_assignment)
        };
        self.persist_collaboration_state().await?;
        if let Some(assignment) = updated_assignment.as_ref() {
            self.emit_codex_assignment_lifecycle(
                ipc::CodexAssignmentLifecycleAction::Updated,
                assignment,
            )
            .await;
        }
        self.emit_supervisor_decision_lifecycle(
            ipc::SupervisorDecisionLifecycleAction::Approved,
            &approved,
        )
        .await;
        self.emit_supervisor_decision_lifecycle(
            ipc::SupervisorDecisionLifecycleAction::Sent,
            &sent,
        )
        .await;
        info!(
            decision_id = %sent.decision_id,
            assignment_id = %sent.assignment_id,
            action = "approve_and_send",
            result = "sent",
            sent_turn_id = sent.sent_turn_id.as_deref().unwrap_or("none"),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "supervisor review action persisted"
        );
        Ok(ipc::SupervisorDecisionApproveAndSendResponse { decision: sent })
    }

    async fn supervisor_decision_reject(
        &self,
        params: ipc::SupervisorDecisionRejectRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionRejectResponse> {
        let started_at = Instant::now();
        info!(
            decision_id = %params.decision_id,
            action = "reject_decision",
            "starting supervisor review action"
        );
        let (decision, updated_assignment) = {
            let now = Utc::now();
            let reviewed_by = params
                .reviewed_by
                .clone()
                .unwrap_or_else(|| "orcas_operator".to_string());
            let mut state = self.state.write().await;
            let existing = state
                .collaboration
                .supervisor_turn_decisions
                .get(&params.decision_id)
                .cloned()
                .ok_or_else(|| {
                    warn!(
                        decision_id = %params.decision_id,
                        action = "reject_decision",
                        reason = "unknown supervisor decision",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "supervisor review action failed"
                    );
                    OrcasError::Protocol(format!(
                        "unknown supervisor decision `{}`",
                        params.decision_id
                    ))
                })?;
            if existing.status != SupervisorTurnDecisionStatus::ProposedToHuman {
                warn!(
                    decision_id = %existing.decision_id,
                    assignment_id = %existing.assignment_id,
                    action = "reject_decision",
                    reason = "decision is not pending human review",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor review action rejected"
                );
                return Err(OrcasError::Protocol(format!(
                    "supervisor decision `{}` is not pending human review",
                    existing.decision_id
                )));
            }
            let decision = state
                .collaboration
                .supervisor_turn_decisions
                .get_mut(&params.decision_id)
                .expect("decision exists");
            decision.status = SupervisorTurnDecisionStatus::Rejected;
            decision.rejected_at = Some(now);
            let mut rejection_note = format!("rejected by {reviewed_by}");
            if let Some(review_note) = params.review_note.as_ref().map(|note| note.trim()) {
                if !review_note.is_empty() {
                    rejection_note.push_str(": ");
                    rejection_note.push_str(review_note);
                }
            }
            Self::merge_assignment_notes(&mut decision.notes, Some(rejection_note));
            let decision = decision.clone();

            let updated_assignment = if let Some(assignment) = state
                .collaboration
                .codex_thread_assignments
                .get_mut(&decision.assignment_id)
            {
                assignment.latest_decision_id = Some(decision.decision_id.clone());
                assignment.updated_at = now;
                if decision.proposal_kind == SupervisorTurnProposalKind::Bootstrap {
                    assignment.bootstrap_state = CodexThreadBootstrapState::NotNeeded;
                }
                Some(assignment.clone())
            } else {
                None
            };
            (decision, updated_assignment)
        };
        self.persist_collaboration_state().await?;
        if let Some(assignment) = updated_assignment.as_ref() {
            self.emit_codex_assignment_lifecycle(
                ipc::CodexAssignmentLifecycleAction::Updated,
                assignment,
            )
            .await;
        }
        self.emit_supervisor_decision_lifecycle(
            ipc::SupervisorDecisionLifecycleAction::Rejected,
            &decision,
        )
        .await;
        info!(
            decision_id = %decision.decision_id,
            assignment_id = %decision.assignment_id,
            action = "reject_decision",
            result = "rejected",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "supervisor review action persisted"
        );
        Ok(ipc::SupervisorDecisionRejectResponse { decision })
    }

    async fn assignment_communication_get(
        &self,
        params: ipc::AssignmentCommunicationGetRequest,
    ) -> OrcasResult<ipc::AssignmentCommunicationGetResponse> {
        if params.assignment_id.trim().is_empty() {
            return Err(OrcasError::Protocol(
                "assignment communication lookup requires a non-empty assignment_id".to_string(),
            ));
        }
        let record = self
            .state
            .read()
            .await
            .collaboration
            .assignment_communications
            .get(&params.assignment_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown assignment communication record for assignment `{}`",
                    params.assignment_id
                ))
            })?;
        Ok(ipc::AssignmentCommunicationGetResponse { record })
    }

    async fn report_get(
        &self,
        params: ipc::ReportGetRequest,
    ) -> OrcasResult<ipc::ReportGetResponse> {
        let report = self
            .state
            .read()
            .await
            .collaboration
            .reports
            .get(&params.report_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!("unknown report `{}`", params.report_id))
            })?;
        Ok(ipc::ReportGetResponse { report })
    }

    async fn report_list_for_workunit(
        &self,
        params: ipc::ReportListForWorkunitRequest,
    ) -> OrcasResult<ipc::ReportListForWorkunitResponse> {
        let mut reports = self
            .state
            .read()
            .await
            .collaboration
            .reports
            .values()
            .filter(|report| report.work_unit_id == params.work_unit_id)
            .cloned()
            .collect::<Vec<_>>();
        reports.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(ipc::ReportListForWorkunitResponse { reports })
    }

    fn auto_proposal_requested_by() -> String {
        "orcasd_auto_report_recorded".to_string()
    }

    fn auto_proposal_note(report_id: &str) -> String {
        format!(
            "Automatically synthesize the next bounded proposal for authoritative report `{report_id}`."
        )
    }

    fn same_source_proposals<'a>(
        collaboration: &'a CollaborationState,
        work_unit_id: &str,
        source_report_id: &str,
    ) -> impl Iterator<Item = &'a SupervisorProposalRecord> {
        collaboration
            .supervisor_proposals
            .values()
            .filter(move |proposal| {
                proposal.primary_work_unit_id == work_unit_id
                    && proposal.source_report_id == source_report_id
            })
    }

    fn same_source_open_proposal_ids(
        collaboration: &CollaborationState,
        work_unit_id: &str,
        source_report_id: &str,
    ) -> Vec<String> {
        Self::same_source_proposals(collaboration, work_unit_id, source_report_id)
            .filter(|proposal| proposal.status == SupervisorProposalStatus::Open)
            .map(|proposal| proposal.id.clone())
            .collect()
    }

    async fn persist_and_emit_stale_proposals(
        &self,
        stale_proposals: Vec<SupervisorProposalRecord>,
    ) -> OrcasResult<()> {
        if stale_proposals.is_empty() {
            return Ok(());
        }
        self.persist_collaboration_state().await?;
        for proposal in &stale_proposals {
            self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Stale, proposal)
                .await;
        }
        Ok(())
    }

    async fn prepare_proposal_generation(
        &self,
        work_unit_id: &str,
        source_report_id: Option<&str>,
        suppress_ineligible: bool,
    ) -> OrcasResult<PreparedProposalGenerationOutcome> {
        let stale_proposals = {
            let mut state = self.state.write().await;
            Self::refresh_stale_proposals_for_work_unit(&mut state.collaboration, work_unit_id)
        };
        self.persist_and_emit_stale_proposals(stale_proposals)
            .await?;

        let state = self.state.read().await;
        let work_unit = state
            .collaboration
            .work_units
            .get(work_unit_id)
            .cloned()
            .ok_or_else(|| OrcasError::Protocol(format!("unknown work unit `{work_unit_id}`")))?;
        if work_unit.status != WorkUnitStatus::AwaitingDecision {
            if suppress_ineligible {
                return Ok(PreparedProposalGenerationOutcome::Suppressed {
                    reason: format!("work unit `{}` is not awaiting_decision", work_unit.id),
                });
            }
            return Err(OrcasError::Protocol(format!(
                "work unit `{}` is not awaiting_decision",
                work_unit.id
            )));
        }
        if Self::work_unit_has_live_attachable_turn(&state, &work_unit.id) {
            if suppress_ineligible {
                return Ok(PreparedProposalGenerationOutcome::Suppressed {
                    reason: "supervisor proposal generation is blocked while the work unit still has a live attachable turn"
                        .to_string(),
                });
            }
            return Err(OrcasError::Protocol(
                "supervisor proposal generation is blocked while the work unit still has a live attachable turn"
                    .to_string(),
            ));
        }
        let resolved_source_report_id = source_report_id
            .map(ToOwned::to_owned)
            .or(work_unit.latest_report_id.clone())
            .ok_or_else(|| {
                if suppress_ineligible {
                    OrcasError::Protocol(format!(
                        "auto proposal generation found no latest report for work unit `{}`",
                        work_unit.id
                    ))
                } else {
                    OrcasError::Protocol(format!(
                        "work unit `{}` has no latest report",
                        work_unit.id
                    ))
                }
            })?;
        if work_unit.latest_report_id.as_deref() != Some(resolved_source_report_id.as_str()) {
            if suppress_ineligible {
                return Ok(PreparedProposalGenerationOutcome::Suppressed {
                    reason: "proposal generation requires the latest report for the work unit"
                        .to_string(),
                });
            }
            return Err(OrcasError::Protocol(
                "proposal generation requires the latest report for the work unit".to_string(),
            ));
        }
        let source_report = state
            .collaboration
            .reports
            .get(&resolved_source_report_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown source report `{resolved_source_report_id}`"
                ))
            })?;
        if work_unit.current_assignment_id.as_deref() != Some(source_report.assignment_id.as_str())
        {
            if suppress_ineligible {
                return Ok(PreparedProposalGenerationOutcome::Suppressed {
                    reason: "proposal generation requires the source report assignment to remain the current assignment"
                        .to_string(),
                });
            }
            return Err(OrcasError::Protocol(
                "proposal generation requires the source report assignment to remain the current assignment"
                    .to_string(),
            ));
        }

        Ok(PreparedProposalGenerationOutcome::Ready(
            PreparedProposalGeneration {
                collaboration: state.collaboration.clone(),
                source_report_id: resolved_source_report_id,
            },
        ))
    }

    async fn generate_proposal(
        &self,
        request: ProposalGenerationRequest,
        duplicate_policy: ProposalDuplicatePolicy,
    ) -> OrcasResult<ProposalGenerationOutcome> {
        let started_at = Instant::now();
        let duplicate_policy_label = match duplicate_policy {
            ProposalDuplicatePolicy::Manual {
                supersede_open: false,
            } => "manual_reject_duplicate",
            ProposalDuplicatePolicy::Manual {
                supersede_open: true,
            } => "manual_supersede_open",
            ProposalDuplicatePolicy::Auto => "auto",
        };
        info!(
            work_unit_id = %request.work_unit_id,
            source_report_id = request.source_report_id.as_deref().unwrap_or("latest"),
            trigger_kind = ?request.trigger_kind,
            duplicate_policy = duplicate_policy_label,
            "starting supervisor proposal workflow"
        );
        let prepared = self
            .prepare_proposal_generation(
                &request.work_unit_id,
                request.source_report_id.as_deref(),
                matches!(duplicate_policy, ProposalDuplicatePolicy::Auto),
            )
            .await?;
        let prepared = match prepared {
            PreparedProposalGenerationOutcome::Ready(prepared) => prepared,
            PreparedProposalGenerationOutcome::Suppressed { reason } => {
                info!(
                    work_unit_id = %request.work_unit_id,
                    source_report_id = request.source_report_id.as_deref().unwrap_or("latest"),
                    trigger_kind = ?request.trigger_kind,
                    result = "suppressed",
                    %reason,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor proposal workflow suppressed before generation"
                );
                return Ok(ProposalGenerationOutcome::Suppressed { reason });
            }
        };

        match duplicate_policy {
            ProposalDuplicatePolicy::Manual {
                supersede_open: false,
            } => {
                let open_same_source = Self::same_source_open_proposal_ids(
                    &prepared.collaboration,
                    &request.work_unit_id,
                    &prepared.source_report_id,
                );
                if !open_same_source.is_empty() {
                    return Err(OrcasError::Protocol(format!(
                        "an open proposal already exists for work unit `{}` and source report `{}`",
                        request.work_unit_id, prepared.source_report_id
                    )));
                }
            }
            ProposalDuplicatePolicy::Manual {
                supersede_open: true,
            } => {}
            ProposalDuplicatePolicy::Auto => {
                if Self::same_source_proposals(
                    &prepared.collaboration,
                    &request.work_unit_id,
                    &prepared.source_report_id,
                )
                .next()
                .is_some()
                {
                    info!(
                        work_unit_id = %request.work_unit_id,
                        source_report_id = %prepared.source_report_id,
                        trigger_kind = ?request.trigger_kind,
                        result = "suppressed",
                        reason = "proposal generation already exists for source report",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "supervisor proposal workflow suppressed"
                    );
                    return Ok(ProposalGenerationOutcome::Suppressed {
                        reason: format!(
                            "proposal generation already exists for work unit `{}` and source report `{}`",
                            request.work_unit_id, prepared.source_report_id
                        ),
                    });
                }
            }
        }

        let context_pack = build_context_pack(
            &prepared.collaboration,
            &request.work_unit_id,
            Some(prepared.source_report_id.as_str()),
            request.requested_by,
            request.note.clone(),
            request.trigger_kind,
        )?;
        let proposal_id = Self::new_object_id("proposal");
        let result = match self.supervisor_reasoner.propose(context_pack.clone()).await {
            Ok(result) => result,
            Err(failure) => {
                warn!(
                    work_unit_id = %request.work_unit_id,
                    source_report_id = %prepared.source_report_id,
                    trigger_kind = ?request.trigger_kind,
                    stage = ?failure.stage,
                    backend_kind = %failure.backend_kind,
                    model = %failure.model,
                    reason = %failure.message,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor proposal generation failed"
                );
                let record = self
                    .persist_failed_proposal_record(
                        proposal_id.clone(),
                        context_pack.clone(),
                        failure.backend_kind.clone(),
                        failure.model.clone(),
                        failure.response_id.clone(),
                        None,
                        failure.output_text.clone(),
                        failure.prompt_render.clone(),
                        failure.response_artifact.clone(),
                        None,
                        SupervisorProposalFailure {
                            stage: failure.stage,
                            message: failure.message.clone(),
                        },
                    )
                    .await?;
                self.emit_proposal_lifecycle(
                    ipc::ProposalLifecycleAction::GenerationFailed,
                    &record,
                )
                .await;
                return Err(Self::proposal_generation_failure_error(&record));
            }
        };
        if let Err(error) =
            validate_proposal(&result.proposal, &context_pack, &prepared.collaboration)
        {
            warn!(
                work_unit_id = %request.work_unit_id,
                source_report_id = %prepared.source_report_id,
                trigger_kind = ?request.trigger_kind,
                stage = "validate_proposal",
                duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "supervisor proposal validation failed"
            );
            let record = self
                .persist_failed_proposal_record(
                    proposal_id.clone(),
                    context_pack.clone(),
                    result.backend_kind.clone(),
                    result.model.clone(),
                    result.response_id.clone(),
                    result.usage.clone(),
                    result.output_text.clone(),
                    Some(result.prompt_render.clone()),
                    Some(result.response_artifact.clone()),
                    Some(result.proposal.clone()),
                    SupervisorProposalFailure {
                        stage: SupervisorProposalFailureStage::ProposalValidation,
                        message: error.to_string(),
                    },
                )
                .await?;
            self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::GenerationFailed, &record)
                .await;
            return Err(Self::proposal_generation_failure_error(&record));
        }

        let mut proposal = SupervisorProposalRecord {
            id: proposal_id,
            workstream_id: context_pack.workstream.id.clone(),
            primary_work_unit_id: context_pack.primary_work_unit.id.clone(),
            source_report_id: context_pack.source_report.id.clone(),
            trigger: context_pack.trigger.clone(),
            status: SupervisorProposalStatus::Open,
            created_at: Utc::now(),
            reasoner_backend: result.backend_kind,
            reasoner_model: result.model,
            reasoner_response_id: result.response_id,
            reasoner_usage: result.usage,
            reasoner_output_text: result.output_text,
            context_pack,
            prompt_render: Some(result.prompt_render),
            response_artifact: Some(result.response_artifact),
            proposal: Some(result.proposal),
            approval_edits: None,
            approved_proposal: None,
            generation_failure: None,
            validated_at: Some(Utc::now()),
            reviewed_at: None,
            reviewed_by: None,
            review_note: None,
            approved_decision_id: None,
            approved_assignment_id: None,
        };

        let mut superseded_proposals = Vec::new();
        {
            let mut state = self.state.write().await;
            if let Some(reason) = state_anchor_freshness_error(
                &proposal.context_pack.state_anchor,
                &state.collaboration,
            ) {
                proposal.status = SupervisorProposalStatus::Stale;
                proposal.review_note = Some(format!(
                    "Proposal became stale before persistence: {reason}"
                ));
            }
            if proposal.status == SupervisorProposalStatus::Open {
                match duplicate_policy {
                    ProposalDuplicatePolicy::Manual {
                        supersede_open: false,
                    } => {
                        let open_same_source = Self::same_source_open_proposal_ids(
                            &state.collaboration,
                            &proposal.primary_work_unit_id,
                            &proposal.source_report_id,
                        );
                        if !open_same_source.is_empty() {
                            return Err(OrcasError::Protocol(format!(
                                "an open proposal already exists for work unit `{}` and source report `{}`",
                                proposal.primary_work_unit_id, proposal.source_report_id
                            )));
                        }
                    }
                    ProposalDuplicatePolicy::Manual {
                        supersede_open: true,
                    } => {
                        for proposal_id in Self::same_source_open_proposal_ids(
                            &state.collaboration,
                            &proposal.primary_work_unit_id,
                            &proposal.source_report_id,
                        ) {
                            if let Some(other) = state
                                .collaboration
                                .supervisor_proposals
                                .get_mut(&proposal_id)
                                && other.status == SupervisorProposalStatus::Open
                            {
                                other.status = SupervisorProposalStatus::Superseded;
                                if other.review_note.is_none() {
                                    other.review_note = Some(
                                        "Superseded by an operator-requested re-synthesis for the same report."
                                            .to_string(),
                                    );
                                }
                                superseded_proposals.push(other.clone());
                            }
                        }
                    }
                    ProposalDuplicatePolicy::Auto => {
                        if Self::same_source_proposals(
                            &state.collaboration,
                            &proposal.primary_work_unit_id,
                            &proposal.source_report_id,
                        )
                        .next()
                        .is_some()
                        {
                            info!(
                                work_unit_id = %proposal.primary_work_unit_id,
                                source_report_id = %proposal.source_report_id,
                                trigger_kind = ?request.trigger_kind,
                                result = "suppressed",
                                reason = "proposal generation raced with existing proposal",
                                duration_ms = started_at.elapsed().as_millis() as u64,
                                "supervisor proposal workflow suppressed"
                            );
                            return Ok(ProposalGenerationOutcome::Suppressed {
                                reason: format!(
                                    "proposal generation raced with an existing proposal for work unit `{}` and source report `{}`",
                                    proposal.primary_work_unit_id, proposal.source_report_id
                                ),
                            });
                        }
                    }
                }
            }
            state
                .collaboration
                .supervisor_proposals
                .insert(proposal.id.clone(), proposal.clone());
        }
        self.persist_collaboration_state().await?;
        for superseded in &superseded_proposals {
            self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Superseded, superseded)
                .await;
        }
        self.emit_proposal_lifecycle(
            match proposal.status {
                SupervisorProposalStatus::GenerationFailed => {
                    ipc::ProposalLifecycleAction::GenerationFailed
                }
                SupervisorProposalStatus::Stale => ipc::ProposalLifecycleAction::Stale,
                _ => ipc::ProposalLifecycleAction::Created,
            },
            &proposal,
        )
        .await;
        info!(
            proposal_id = %proposal.id,
            work_unit_id = %proposal.primary_work_unit_id,
            source_report_id = %proposal.source_report_id,
            trigger_kind = ?request.trigger_kind,
            status = ?proposal.status,
            decision_type = proposal
                .proposal
                .as_ref()
                .map(|proposal| format!("{:?}", proposal.proposed_decision.decision_type))
                .unwrap_or_else(|| "unknown".to_string()),
            superseded_count = superseded_proposals.len(),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "supervisor proposal persisted"
        );

        Ok(ProposalGenerationOutcome::Created(proposal))
    }

    async fn maybe_auto_create_proposal_for_report(&self, report: &Report) {
        if !self
            .config
            .supervisor
            .proposals
            .auto_create_on_report_recorded
        {
            return;
        }

        let result = self
            .generate_proposal(
                ProposalGenerationRequest {
                    work_unit_id: report.work_unit_id.clone(),
                    source_report_id: Some(report.id.clone()),
                    requested_by: Self::auto_proposal_requested_by(),
                    note: Some(Self::auto_proposal_note(&report.id)),
                    trigger_kind: SupervisorProposalTriggerKind::ReportRecorded,
                },
                ProposalDuplicatePolicy::Auto,
            )
            .await;

        match result {
            Ok(ProposalGenerationOutcome::Created(_)) => {}
            Ok(ProposalGenerationOutcome::Suppressed { reason }) => {
                info!(
                    work_unit_id = %report.work_unit_id,
                    report_id = %report.id,
                    %reason,
                    "auto supervisor proposal suppressed"
                );
            }
            Err(error) => {
                warn!(
                    work_unit_id = %report.work_unit_id,
                    report_id = %report.id,
                    %error,
                    "auto supervisor proposal generation failed"
                );
                self.emit(ipc::DaemonEvent::Warning {
                    message: format!(
                        "auto supervisor proposal generation for report `{}` failed: {error}",
                        report.id
                    ),
                })
                .await;
            }
        }
    }

    async fn proposal_create(
        &self,
        params: ipc::ProposalCreateRequest,
    ) -> OrcasResult<ipc::ProposalCreateResponse> {
        let requested_by = params
            .requested_by
            .clone()
            .unwrap_or_else(|| "supervisor_cli".to_string());
        match self
            .generate_proposal(
                ProposalGenerationRequest {
                    work_unit_id: params.work_unit_id,
                    source_report_id: params.source_report_id,
                    requested_by,
                    note: params.note,
                    trigger_kind: SupervisorProposalTriggerKind::HumanRequested,
                },
                ProposalDuplicatePolicy::Manual {
                    supersede_open: params.supersede_open,
                },
            )
            .await?
        {
            ProposalGenerationOutcome::Created(proposal) => {
                Ok(ipc::ProposalCreateResponse { proposal })
            }
            ProposalGenerationOutcome::Suppressed { reason } => Err(OrcasError::Protocol(reason)),
        }
    }

    async fn proposal_get(
        &self,
        params: ipc::ProposalGetRequest,
    ) -> OrcasResult<ipc::ProposalGetResponse> {
        let (proposal, stale_proposals) = {
            let mut state = self.state.write().await;
            let work_unit_id = state
                .collaboration
                .supervisor_proposals
                .get(&params.proposal_id)
                .map(|proposal| proposal.primary_work_unit_id.clone())
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown proposal `{}`", params.proposal_id))
                })?;
            let stale_proposals = Self::refresh_stale_proposals_for_work_unit(
                &mut state.collaboration,
                &work_unit_id,
            );
            let proposal = state
                .collaboration
                .supervisor_proposals
                .get(&params.proposal_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown proposal `{}`", params.proposal_id))
                })?;
            (proposal, stale_proposals)
        };
        if !stale_proposals.is_empty() {
            self.persist_collaboration_state().await?;
            for proposal in &stale_proposals {
                self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Stale, proposal)
                    .await;
            }
        }
        Ok(ipc::ProposalGetResponse { proposal })
    }

    async fn proposal_list_for_workunit(
        &self,
        params: ipc::ProposalListForWorkunitRequest,
    ) -> OrcasResult<ipc::ProposalListForWorkunitResponse> {
        let (proposals, stale_proposals) = {
            let mut state = self.state.write().await;
            let stale_proposals = Self::refresh_stale_proposals_for_work_unit(
                &mut state.collaboration,
                &params.work_unit_id,
            );
            let mut proposals = state
                .collaboration
                .supervisor_proposals
                .values()
                .filter(|proposal| proposal.primary_work_unit_id == params.work_unit_id)
                .map(Self::proposal_summary)
                .collect::<Vec<_>>();
            proposals.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| left.id.cmp(&right.id))
            });
            (proposals, stale_proposals)
        };
        if !stale_proposals.is_empty() {
            self.persist_collaboration_state().await?;
            for proposal in &stale_proposals {
                self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Stale, proposal)
                    .await;
            }
        }
        Ok(ipc::ProposalListForWorkunitResponse { proposals })
    }

    fn implement_mode_spec() -> AssignmentModeSpec {
        AssignmentModeSpec::Implement(ImplementModeSpec {
            expected_verification_commands: Vec::new(),
        })
    }

    fn manual_implement_assignment_seed(
        work_unit: &WorkUnit,
        instructions: Option<&str>,
        source_report_id: Option<String>,
    ) -> AssignmentCommunicationSeed {
        let objective = if !work_unit.task_statement.trim().is_empty() {
            work_unit.task_statement.clone()
        } else if let Some(instructions) = instructions.filter(|value| !value.trim().is_empty()) {
            instructions.trim().to_string()
        } else {
            format!("Complete the bounded work for {}", work_unit.title)
        };

        AssignmentCommunicationSeed {
            source_decision_id: None,
            source_report_id,
            source_proposal_id: None,
            predecessor_assignment_id: None,
            objective,
            instructions: Self::manual_instruction_lines(work_unit, instructions),
            acceptance_criteria: Vec::new(),
            stop_conditions: Vec::new(),
            required_context_refs: Vec::new(),
            expected_report_fields: Vec::new(),
            boundedness_note: None,
            mode_spec: Self::implement_mode_spec(),
        }
    }

    fn manual_instruction_lines(work_unit: &WorkUnit, instructions: Option<&str>) -> Vec<String> {
        let Some(instructions) = instructions.map(str::trim) else {
            return Vec::new();
        };
        if instructions.is_empty() || instructions == work_unit.task_statement {
            return Vec::new();
        }
        vec![instructions.to_string()]
    }

    fn assignment_communication_seed_from_draft(
        draft: &DraftAssignment,
        source_report_id: &str,
        source_proposal_id: &str,
    ) -> AssignmentCommunicationSeed {
        AssignmentCommunicationSeed {
            source_decision_id: None,
            source_report_id: Some(source_report_id.to_string()),
            source_proposal_id: Some(source_proposal_id.to_string()),
            predecessor_assignment_id: Some(draft.predecessor_assignment_id.clone()),
            objective: draft.objective.clone(),
            instructions: draft.instructions.clone(),
            acceptance_criteria: draft.acceptance_criteria.clone(),
            stop_conditions: draft.stop_conditions.clone(),
            required_context_refs: draft.required_context_refs.clone(),
            expected_report_fields: draft.expected_report_fields.clone(),
            boundedness_note: (!draft.boundedness_note.trim().is_empty())
                .then_some(draft.boundedness_note.clone()),
            mode_spec: Self::implement_mode_spec(),
        }
    }

    fn assignment_communication_seed_from_packet(
        packet: &AssignmentCommunicationPacket,
    ) -> AssignmentCommunicationSeed {
        let required_context_refs = packet
            .included_context
            .iter()
            .filter(|block| block.kind == "context_refs" || block.id == "required_context_refs")
            .flat_map(|block| block.lines.iter().cloned())
            .collect::<Vec<_>>();

        AssignmentCommunicationSeed {
            source_decision_id: packet.source_decision_id.clone(),
            source_report_id: packet.source_report_id.clone(),
            source_proposal_id: packet.source_proposal_id.clone(),
            predecessor_assignment_id: packet.predecessor_assignment_id.clone(),
            objective: packet.objective.clone(),
            instructions: packet.instructions.clone(),
            acceptance_criteria: packet
                .acceptance_criteria
                .iter()
                .map(|item| item.text.clone())
                .collect(),
            stop_conditions: packet
                .stop_conditions
                .iter()
                .map(|item| item.text.clone())
                .collect(),
            required_context_refs,
            expected_report_fields: Vec::new(),
            boundedness_note: packet.non_goals.first().cloned(),
            mode_spec: packet.mode_spec.clone(),
        }
    }

    fn next_assignment_communication_seed(
        work_unit: &WorkUnit,
        source_assignment: &Assignment,
        source_record: Option<&AssignmentCommunicationRecord>,
        report_id: Option<String>,
        decision_id: &str,
        override_instructions: Option<&str>,
        explicit_seed: Option<AssignmentCommunicationSeed>,
    ) -> AssignmentCommunicationSeed {
        let has_explicit_seed = explicit_seed.is_some();
        let mut seed = explicit_seed
            .or_else(|| source_assignment.communication_seed.clone())
            .or_else(|| {
                source_record
                    .map(|record| Self::assignment_communication_seed_from_packet(&record.packet))
            })
            .unwrap_or_else(|| {
                Self::manual_implement_assignment_seed(
                    work_unit,
                    Some(source_assignment.instructions.as_str()),
                    report_id.clone(),
                )
            });

        if !has_explicit_seed && override_instructions.is_some() {
            seed.instructions = Self::manual_instruction_lines(work_unit, override_instructions);
        }

        seed.source_decision_id = Some(decision_id.to_string());
        seed.source_report_id = report_id;
        seed.predecessor_assignment_id = Some(source_assignment.id.clone());
        if !has_explicit_seed {
            seed.source_proposal_id = None;
        }
        seed
    }

    async fn proposal_approve(
        &self,
        params: ipc::ProposalApproveRequest,
    ) -> OrcasResult<ipc::ProposalApproveResponse> {
        let started_at = Instant::now();
        let reviewed_by = params
            .reviewed_by
            .clone()
            .unwrap_or_else(|| "supervisor_cli".to_string());
        info!(
            proposal_id = %params.proposal_id,
            action = "approve_proposal",
            reviewed_by = %reviewed_by,
            "starting review action"
        );

        let (proposal, collaboration, stale_proposals) = {
            let mut state = self.state.write().await;
            let work_unit_id = state
                .collaboration
                .supervisor_proposals
                .get(&params.proposal_id)
                .map(|proposal| proposal.primary_work_unit_id.clone())
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown proposal `{}`", params.proposal_id))
                })?;
            let stale_proposals = Self::refresh_stale_proposals_for_work_unit(
                &mut state.collaboration,
                &work_unit_id,
            );
            let proposal = state
                .collaboration
                .supervisor_proposals
                .get(&params.proposal_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown proposal `{}`", params.proposal_id))
                })?;
            let collaboration = state.collaboration.clone();
            (proposal, collaboration, stale_proposals)
        };
        if !stale_proposals.is_empty() {
            self.persist_collaboration_state().await?;
            for proposal in &stale_proposals {
                self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Stale, proposal)
                    .await;
            }
        }

        if proposal.status != SupervisorProposalStatus::Open {
            warn!(
                proposal_id = %proposal.id,
                work_unit_id = %proposal.primary_work_unit_id,
                action = "approve_proposal",
                reason = "proposal is not open",
                duration_ms = started_at.elapsed().as_millis() as u64,
                "review action rejected"
            );
            return Err(OrcasError::Protocol(format!(
                "proposal `{}` is not open and cannot be approved",
                proposal.id
            )));
        }

        let original_proposal = proposal.proposal.as_ref().ok_or_else(|| {
            warn!(
                proposal_id = %proposal.id,
                work_unit_id = %proposal.primary_work_unit_id,
                action = "approve_proposal",
                reason = "proposal payload missing",
                duration_ms = started_at.elapsed().as_millis() as u64,
                "review action failed"
            );
            OrcasError::Protocol(format!(
                "proposal `{}` does not contain a model-generated proposal payload",
                proposal.id
            ))
        })?;
        let approved_proposal = apply_edits(original_proposal, &params.edits);
        if let Err(error) =
            validate_proposal(&approved_proposal, &proposal.context_pack, &collaboration)
        {
            warn!(
                proposal_id = %proposal.id,
                work_unit_id = %proposal.primary_work_unit_id,
                action = "approve_proposal",
                reason = "approved proposal failed validation",
                duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "review action failed"
            );
            return Err(error);
        }

        let (instructions, worker_id, worker_kind, communication_seed) =
            if approved_proposal.proposed_decision.requires_assignment {
                let draft = approved_proposal
                    .draft_next_assignment
                    .as_ref()
                    .ok_or_else(|| {
                        OrcasError::Protocol(
                            "approved proposal requires a draft assignment but none was present"
                                .to_string(),
                        )
                    })?;
                (
                    Some(compile_assignment_instructions(
                        draft,
                        &proposal.source_report_id,
                    )),
                    draft.preferred_worker_id.clone(),
                    draft.worker_kind.clone(),
                    Some(Self::assignment_communication_seed_from_draft(
                        draft,
                        &proposal.source_report_id,
                        &proposal.id,
                    )),
                )
            } else {
                (None, None, None, None)
            };

        let decision_response = self
            .decision_apply_with_seed(
                ipc::DecisionApplyRequest {
                    work_unit_id: proposal.primary_work_unit_id.clone(),
                    report_id: Some(proposal.source_report_id.clone()),
                    decision_type: approved_proposal.proposed_decision.decision_type,
                    rationale: approved_proposal.proposed_decision.rationale.clone(),
                    instructions,
                    worker_id,
                    worker_kind,
                },
                communication_seed,
            )
            .await?;

        let (updated_proposal, sibling_updates) = {
            let mut state = self.state.write().await;
            let mut sibling_updates = Vec::new();
            for other in state.collaboration.supervisor_proposals.values_mut() {
                if other.primary_work_unit_id != proposal.primary_work_unit_id
                    || other.id == proposal.id
                    || other.status != SupervisorProposalStatus::Open
                {
                    continue;
                }
                other.status = if other.source_report_id == proposal.source_report_id {
                    SupervisorProposalStatus::Superseded
                } else {
                    SupervisorProposalStatus::Stale
                };
                if other.review_note.is_none() {
                    other.review_note = Some(format!(
                        "Proposal `{}` was approved for this work unit.",
                        proposal.id
                    ));
                }
                sibling_updates.push(other.clone());
            }

            let proposal_record = state
                .collaboration
                .supervisor_proposals
                .get_mut(&proposal.id)
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown proposal `{}`", proposal.id))
                })?;
            proposal_record.status = SupervisorProposalStatus::Approved;
            proposal_record.approval_edits = Some(params.edits.clone());
            proposal_record.approved_proposal = Some(approved_proposal);
            proposal_record.reviewed_at = Some(Utc::now());
            proposal_record.reviewed_by = Some(reviewed_by);
            proposal_record.review_note = params.review_note.clone();
            proposal_record.approved_decision_id = Some(decision_response.decision.id.clone());
            proposal_record.approved_assignment_id = decision_response
                .next_assignment
                .as_ref()
                .map(|assignment| assignment.id.clone());
            (proposal_record.clone(), sibling_updates)
        };
        self.persist_collaboration_state().await?;
        for sibling in &sibling_updates {
            let action = match sibling.status {
                SupervisorProposalStatus::Superseded => ipc::ProposalLifecycleAction::Superseded,
                SupervisorProposalStatus::Stale => ipc::ProposalLifecycleAction::Stale,
                _ => continue,
            };
            self.emit_proposal_lifecycle(action, sibling).await;
        }
        self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Approved, &updated_proposal)
            .await;
        info!(
            proposal_id = %updated_proposal.id,
            work_unit_id = %updated_proposal.primary_work_unit_id,
            action = "approve_proposal",
            result = "approved",
            decision_id = %decision_response.decision.id,
            next_assignment_id = decision_response
                .next_assignment
                .as_ref()
                .map(|assignment| assignment.id.as_str())
                .unwrap_or("none"),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "review action persisted"
        );

        Ok(ipc::ProposalApproveResponse {
            proposal: updated_proposal,
            decision: decision_response.decision,
            next_assignment: decision_response.next_assignment,
        })
    }

    async fn proposal_reject(
        &self,
        params: ipc::ProposalRejectRequest,
    ) -> OrcasResult<ipc::ProposalRejectResponse> {
        let started_at = Instant::now();
        let reviewed_by = params
            .reviewed_by
            .clone()
            .unwrap_or_else(|| "supervisor_cli".to_string());
        info!(
            proposal_id = %params.proposal_id,
            action = "reject_proposal",
            reviewed_by = %reviewed_by,
            "starting review action"
        );
        let stale_proposals = {
            let mut state = self.state.write().await;
            let work_unit_id = state
                .collaboration
                .supervisor_proposals
                .get(&params.proposal_id)
                .map(|proposal| proposal.primary_work_unit_id.clone())
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown proposal `{}`", params.proposal_id))
                })?;
            Self::refresh_stale_proposals_for_work_unit(&mut state.collaboration, &work_unit_id)
        };
        if !stale_proposals.is_empty() {
            self.persist_collaboration_state().await?;
            for proposal in &stale_proposals {
                self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Stale, proposal)
                    .await;
            }
        }
        let proposal = {
            let mut state = self.state.write().await;
            let proposal_record = state
                .collaboration
                .supervisor_proposals
                .get_mut(&params.proposal_id)
                .ok_or_else(|| {
                    warn!(
                        proposal_id = %params.proposal_id,
                        action = "reject_proposal",
                        reason = "unknown proposal",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        "review action failed"
                    );
                    OrcasError::Protocol(format!("unknown proposal `{}`", params.proposal_id))
                })?;
            if proposal_record.status != SupervisorProposalStatus::Open {
                warn!(
                    proposal_id = %proposal_record.id,
                    work_unit_id = %proposal_record.primary_work_unit_id,
                    action = "reject_proposal",
                    reason = "proposal is not open",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "review action rejected"
                );
                return Err(OrcasError::Protocol(format!(
                    "proposal `{}` is not open and cannot be rejected",
                    proposal_record.id
                )));
            }
            proposal_record.status = SupervisorProposalStatus::Rejected;
            proposal_record.reviewed_at = Some(Utc::now());
            proposal_record.reviewed_by = Some(reviewed_by);
            proposal_record.review_note = params.review_note.clone();
            proposal_record.clone()
        };
        self.persist_collaboration_state().await?;
        self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Rejected, &proposal)
            .await;
        info!(
            proposal_id = %proposal.id,
            work_unit_id = %proposal.primary_work_unit_id,
            action = "reject_proposal",
            result = "rejected",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "review action persisted"
        );
        Ok(ipc::ProposalRejectResponse { proposal })
    }

    async fn decision_apply(
        &self,
        params: ipc::DecisionApplyRequest,
    ) -> OrcasResult<ipc::DecisionApplyResponse> {
        self.decision_apply_with_seed(params, None).await
    }

    async fn decision_apply_with_seed(
        &self,
        params: ipc::DecisionApplyRequest,
        next_assignment_seed: Option<AssignmentCommunicationSeed>,
    ) -> OrcasResult<ipc::DecisionApplyResponse> {
        let started_at = Instant::now();
        info!(
            work_unit_id = %params.work_unit_id,
            report_id = params.report_id.as_deref().unwrap_or("latest"),
            action = "apply_decision",
            decision_type = ?params.decision_type,
            "starting decision apply workflow"
        );
        let (response, closed_assignment, work_unit_event, workstream_event, stale_proposals) = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let prior_work_unit = state
                .collaboration
                .work_units
                .get(&params.work_unit_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown work unit `{}`", params.work_unit_id))
                })?;
            let current_assignment_id = prior_work_unit.current_assignment_id.clone();
            let closed_assignment = if let Some(assignment_id) = current_assignment_id.as_ref()
                && let Some(assignment) = state.collaboration.assignments.get_mut(assignment_id)
            {
                assignment.status = AssignmentStatus::Closed;
                assignment.updated_at = now;
                Some(assignment.clone())
            } else {
                None
            };

            let report_id = params
                .report_id
                .clone()
                .or(prior_work_unit.latest_report_id.clone());
            let decision = Decision {
                id: Self::new_object_id("decision"),
                work_unit_id: params.work_unit_id.clone(),
                report_id,
                decision_type: params.decision_type,
                rationale: params.rationale,
                created_at: now,
            };
            state
                .collaboration
                .decisions
                .insert(decision.id.clone(), decision.clone());

            let next_assignment = match params.decision_type {
                DecisionType::Accept => {
                    if let Some(work_unit) =
                        state.collaboration.work_units.get_mut(&params.work_unit_id)
                    {
                        work_unit.status = WorkUnitStatus::Accepted;
                        work_unit.current_assignment_id = None;
                        work_unit.updated_at = now;
                    }
                    None
                }
                DecisionType::MarkComplete => {
                    if let Some(work_unit) =
                        state.collaboration.work_units.get_mut(&params.work_unit_id)
                    {
                        work_unit.status = WorkUnitStatus::Completed;
                        work_unit.current_assignment_id = None;
                        work_unit.updated_at = now;
                    }
                    None
                }
                DecisionType::EscalateToHuman => {
                    if let Some(work_unit) =
                        state.collaboration.work_units.get_mut(&params.work_unit_id)
                    {
                        work_unit.status = WorkUnitStatus::NeedsHuman;
                        work_unit.current_assignment_id = None;
                        work_unit.updated_at = now;
                    }
                    None
                }
                DecisionType::Continue | DecisionType::Redirect => {
                    let source_assignment = current_assignment_id
                        .as_ref()
                        .and_then(|assignment_id| {
                            state.collaboration.assignments.get(assignment_id)
                        })
                        .cloned()
                        .ok_or_else(|| {
                            OrcasError::Protocol(
                                "continue/redirect requires an existing assignment".to_string(),
                            )
                        })?;
                    let source_record = state
                        .collaboration
                        .assignment_communications
                        .get(&source_assignment.id)
                        .cloned();
                    let worker_id = params
                        .worker_id
                        .clone()
                        .unwrap_or_else(|| source_assignment.worker_id.clone());
                    let worker_session_id = Self::select_worker_session_for_assignment(
                        &mut state.collaboration,
                        &worker_id,
                        params
                            .worker_kind
                            .clone()
                            .unwrap_or_else(|| "codex".to_string()),
                    );
                    let instructions = params
                        .instructions
                        .clone()
                        .unwrap_or_else(|| source_assignment.instructions.clone());
                    let communication_seed = Self::next_assignment_communication_seed(
                        &prior_work_unit,
                        &source_assignment,
                        source_record.as_ref(),
                        decision.report_id.clone(),
                        &decision.id,
                        params.instructions.as_deref(),
                        next_assignment_seed.clone(),
                    );
                    let next_assignment = Assignment {
                        id: Self::new_object_id("assignment"),
                        work_unit_id: params.work_unit_id.clone(),
                        worker_id,
                        worker_session_id,
                        instructions,
                        communication_seed: Some(communication_seed),
                        status: AssignmentStatus::Created,
                        attempt_number: source_assignment.attempt_number + 1,
                        created_at: now,
                        updated_at: now,
                    };
                    state
                        .collaboration
                        .assignments
                        .insert(next_assignment.id.clone(), next_assignment.clone());
                    if let Some(work_unit) =
                        state.collaboration.work_units.get_mut(&params.work_unit_id)
                    {
                        work_unit.status = WorkUnitStatus::Ready;
                        work_unit.current_assignment_id = Some(next_assignment.id.clone());
                        work_unit.updated_at = now;
                    }
                    Some(next_assignment)
                }
            };

            Self::refresh_blocked_work_units(&mut state.collaboration);
            let stale_proposals = Self::refresh_stale_proposals_for_work_unit(
                &mut state.collaboration,
                &params.work_unit_id,
            );
            Self::refresh_workstream_statuses(&mut state.collaboration);
            let work_unit = state
                .collaboration
                .work_units
                .get(&params.work_unit_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown work unit `{}`", params.work_unit_id))
                })?;
            let workstream = state
                .collaboration
                .workstreams
                .get(&work_unit.workstream_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown workstream `{}`",
                        work_unit.workstream_id
                    ))
                })?;
            let work_unit_action = if work_unit.status == WorkUnitStatus::Completed {
                ipc::CollaborationLifecycleAction::Completed
            } else if work_unit.status == WorkUnitStatus::NeedsHuman {
                ipc::CollaborationLifecycleAction::Escalated
            } else {
                ipc::CollaborationLifecycleAction::Updated
            };
            let workstream_action = if workstream.status == WorkstreamStatus::Completed {
                ipc::CollaborationLifecycleAction::Completed
            } else if workstream.status == WorkstreamStatus::Blocked {
                ipc::CollaborationLifecycleAction::Escalated
            } else {
                ipc::CollaborationLifecycleAction::Updated
            };
            (
                ipc::DecisionApplyResponse {
                    decision,
                    work_unit: work_unit.clone(),
                    next_assignment,
                },
                closed_assignment,
                (work_unit_action, work_unit),
                (workstream_action, workstream),
                stale_proposals,
            )
        };
        if let Some(next_assignment) = response.next_assignment.as_ref() {
            self.ensure_assignment_communication_record(&next_assignment.id, None, None)
                .await?;
        }
        self.persist_collaboration_state().await?;
        if let Some(assignment) = closed_assignment.as_ref() {
            self.emit_assignment_lifecycle(ipc::AssignmentLifecycleAction::Closed, assignment)
                .await;
        }
        self.emit_decision_applied(&response.decision).await;
        self.emit_work_unit_lifecycle(work_unit_event.0, &work_unit_event.1)
            .await;
        self.emit_workstream_lifecycle(workstream_event.0, &workstream_event.1)
            .await;
        if let Some(next_assignment) = response.next_assignment.as_ref() {
            self.emit_assignment_lifecycle(
                ipc::AssignmentLifecycleAction::Created,
                next_assignment,
            )
            .await;
        }
        for proposal in &stale_proposals {
            self.emit_proposal_lifecycle(ipc::ProposalLifecycleAction::Stale, proposal)
                .await;
        }
        info!(
            decision_id = %response.decision.id,
            work_unit_id = %response.decision.work_unit_id,
            action = "apply_decision",
            decision_type = ?response.decision.decision_type,
            result = "persisted",
            next_assignment_id = response
                .next_assignment
                .as_ref()
                .map(|assignment| assignment.id.as_str())
                .unwrap_or("none"),
            stale_proposal_count = stale_proposals.len(),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "decision apply workflow persisted"
        );
        Ok(response)
    }

    async fn events_subscribe(
        &self,
        params: ipc::EventsSubscribeRequest,
        outbound: mpsc::Sender<String>,
        subscription_task: &mut Option<tokio::task::JoinHandle<()>>,
    ) -> OrcasResult<ipc::EventsSubscribeResponse> {
        if let Some(task) = subscription_task.take() {
            task.abort();
        }

        let snapshot = if params.include_snapshot {
            Some(self.snapshot().await?)
        } else {
            None
        };
        let mut events = self.event_tx.subscribe();
        *subscription_task = Some(tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        let payload = match serde_json::to_value(ipc::EventsNotification { event })
                        {
                            Ok(value) => value,
                            Err(_) => continue,
                        };
                        let raw = match serde_json::to_string(&JsonRpcNotification::new(
                            ipc::methods::EVENTS_NOTIFICATION,
                            Some(payload),
                        )) {
                            Ok(raw) => raw,
                            Err(_) => continue,
                        };
                        if outbound.try_send(raw).is_err() {
                            continue;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }));

        Ok(ipc::EventsSubscribeResponse {
            subscribed: true,
            snapshot,
        })
    }

    async fn prepare_assignment(
        &self,
        params: ipc::AssignmentStartRequest,
    ) -> OrcasResult<PreparedAssignment> {
        let requested_model = params
            .model
            .clone()
            .or_else(|| self.config.defaults.model.clone());
        let requested_cwd = params.cwd.clone();
        self.ensure_projected_authority_work_unit_available(&params.work_unit_id)
            .await?;
        let outcome = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let work_unit = state
                .collaboration
                .work_units
                .get(&params.work_unit_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown work unit `{}`", params.work_unit_id))
                })?;

            if work_unit.status != WorkUnitStatus::Ready {
                return Err(OrcasError::Protocol(format!(
                    "work unit `{}` is not startable in status `{:?}`",
                    params.work_unit_id, work_unit.status
                )));
            }
            if !Self::dependencies_satisfied(&state.collaboration, &work_unit.dependencies) {
                return Err(OrcasError::Protocol(format!(
                    "work unit `{}` still has incomplete dependencies",
                    params.work_unit_id
                )));
            }

            let pending_assignments = state
                .collaboration
                .assignments
                .values()
                .filter(|assignment| {
                    assignment.work_unit_id == params.work_unit_id
                        && assignment.status == AssignmentStatus::Created
                })
                .cloned()
                .collect::<Vec<_>>();
            if pending_assignments.len() > 1 {
                return Err(OrcasError::Protocol(format!(
                    "work unit `{}` has multiple unexecuted pending assignments",
                    params.work_unit_id
                )));
            }
            if let Some(pending_assignment) = pending_assignments.first() {
                let latest_assignment_id = state
                    .collaboration
                    .assignments
                    .values()
                    .filter(|assignment| assignment.work_unit_id == params.work_unit_id)
                    .max_by(|left, right| {
                        left.attempt_number
                            .cmp(&right.attempt_number)
                            .then_with(|| left.created_at.cmp(&right.created_at))
                    })
                    .map(|assignment| assignment.id.clone());
                if work_unit.current_assignment_id.as_deref()
                    == Some(pending_assignment.id.as_str())
                    && latest_assignment_id.as_deref() == Some(pending_assignment.id.as_str())
                {
                    PreparedAssignment {
                        assignment: pending_assignment.clone(),
                        created_new: false,
                    }
                } else {
                    return Err(OrcasError::Protocol(format!(
                        "work unit `{}` has an unexecuted pending assignment that is not the current latest successor",
                        params.work_unit_id
                    )));
                }
            } else {
                // Reusing a worker session is allowed, but the assignment remains the execution-bearing
                // protocol object. A new execution segment always gets its own explicit assignment id.
                let worker_session_id = Self::select_worker_session_for_assignment(
                    &mut state.collaboration,
                    &params.worker_id,
                    params
                        .worker_kind
                        .clone()
                        .unwrap_or_else(|| "codex".to_string()),
                );
                let attempt_number = state
                    .collaboration
                    .assignments
                    .values()
                    .filter(|assignment| assignment.work_unit_id == params.work_unit_id)
                    .count() as u32
                    + 1;
                let assignment = Assignment {
                    id: Self::new_object_id("assignment"),
                    work_unit_id: params.work_unit_id.clone(),
                    worker_id: params.worker_id.clone(),
                    worker_session_id: worker_session_id.clone(),
                    instructions: params
                        .instructions
                        .clone()
                        .unwrap_or_else(|| work_unit.task_statement.clone()),
                    communication_seed: Some(Self::manual_implement_assignment_seed(
                        &work_unit,
                        params.instructions.as_deref(),
                        work_unit.latest_report_id.clone(),
                    )),
                    status: AssignmentStatus::Created,
                    attempt_number,
                    created_at: now,
                    updated_at: now,
                };
                state
                    .collaboration
                    .assignments
                    .insert(assignment.id.clone(), assignment.clone());
                if let Some(work_unit) =
                    state.collaboration.work_units.get_mut(&params.work_unit_id)
                {
                    work_unit.current_assignment_id = Some(assignment.id.clone());
                    work_unit.status = WorkUnitStatus::Ready;
                    work_unit.updated_at = now;
                }
                PreparedAssignment {
                    assignment,
                    created_new: true,
                }
            }
        };
        let _ = self
            .ensure_assignment_communication_record(
                &outcome.assignment.id,
                requested_model,
                requested_cwd,
            )
            .await?;
        self.persist_collaboration_state().await?;
        Ok(outcome)
    }

    async fn ensure_projected_authority_work_unit_available(
        &self,
        work_unit_id: &str,
    ) -> OrcasResult<()> {
        // Compatibility bridge: assignment execution still reads collaboration-owned work units,
        // but planning hierarchy reads come from authority queries. Only assignment-start paths
        // inject authority rows into collaboration state, and those rows are tracked explicitly.
        let authority_work_unit_id = orcas_core::authority::WorkUnitId::parse(work_unit_id)?;
        let authority_work_unit = self
            .authority_store
            .get_work_unit(&authority_work_unit_id)
            .await?
            .ok_or_else(|| OrcasError::Protocol(format!("unknown work unit `{work_unit_id}`")))?;
        if authority_work_unit.deleted_at.is_some() {
            return Err(OrcasError::Protocol(format!(
                "authority work unit `{work_unit_id}` has been deleted"
            )));
        }
        let authority_workstream = self
            .authority_store
            .get_workstream(&authority_work_unit.workstream_id)
            .await?
            .ok_or_else(|| {
                OrcasError::Protocol(format!(
                    "unknown workstream `{}`",
                    authority_work_unit.workstream_id
                ))
            })?;
        if authority_workstream.deleted_at.is_some() {
            return Err(OrcasError::Protocol(format!(
                "authority workstream `{}` has been deleted",
                authority_workstream.id
            )));
        }

        let mut state = self.state.write().await;
        state
            .collaboration
            .workstreams
            .entry(authority_workstream.id.to_string())
            .or_insert_with(|| {
                Self::authority_workstream_record_to_collaboration(&authority_workstream)
            });
        state
            .collaboration
            .authority_workstream_bridges
            .insert(authority_workstream.id.to_string());
        state
            .collaboration
            .work_units
            .entry(authority_work_unit.id.to_string())
            .or_insert_with(|| {
                Self::authority_work_unit_record_to_collaboration(&authority_work_unit)
            });
        state
            .collaboration
            .authority_work_unit_bridges
            .insert(authority_work_unit.id.to_string());
        Ok(())
    }

    async fn ensure_assignment_communication_record(
        &self,
        assignment_id: &str,
        requested_model: Option<String>,
        requested_cwd: Option<String>,
    ) -> OrcasResult<()> {
        let started_at = Instant::now();
        if self
            .state
            .read()
            .await
            .collaboration
            .assignment_communications
            .contains_key(assignment_id)
        {
            debug!(
                assignment_id,
                result = "already_present",
                "assignment communication record available"
            );
            return Ok(());
        }

        let record = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            if state
                .collaboration
                .assignment_communications
                .contains_key(assignment_id)
            {
                debug!(
                    assignment_id,
                    result = "already_present",
                    "assignment communication record available"
                );
                return Ok(());
            }
            let assignment = state
                .collaboration
                .assignments
                .get(assignment_id)
                .cloned()
                .ok_or_else(|| {
                    warn!(
                        assignment_id,
                        stage = "resolve_assignment",
                        "assignment communication record generation failed"
                    );
                    OrcasError::Protocol(format!(
                        "unknown assignment `{assignment_id}` for communication record"
                    ))
                })?;
            let record = match build_assignment_communication_record(
                &state.collaboration,
                &assignment,
                requested_model,
                requested_cwd,
                self.config.defaults.cwd.as_ref(),
                now,
            ) {
                Ok(record) => record,
                Err(error) => {
                    warn!(
                        assignment_id,
                        work_unit_id = %assignment.work_unit_id,
                        stage = "build_assignment_communication_record",
                        duration_ms = started_at.elapsed().as_millis() as u64,
                        error = %error,
                        "assignment communication record generation failed"
                    );
                    return Err(error);
                }
            };
            if let Err(error) = validate_assignment_packet(&record.packet) {
                warn!(
                    assignment_id,
                    work_unit_id = %assignment.work_unit_id,
                    packet_id = %record.packet.packet_id,
                    stage = "validate_assignment_packet",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "assignment communication record generation failed"
                );
                return Err(error);
            }
            state
                .collaboration
                .assignment_communications
                .insert(assignment_id.to_string(), record.clone());
            record
        };
        info!(
            assignment_id,
            work_unit_id = %record.work_unit_id,
            workstream_id = %record.workstream_id,
            packet_id = %record.packet.packet_id,
            prompt_bytes = record.prompt_render.prompt_text.len(),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "assignment communication record persisted"
        );
        self.persist_collaboration_state().await?;
        Ok(())
    }

    async fn ensure_worker_session_thread(
        &self,
        worker_session_id: &str,
        model: Option<String>,
        cwd: Option<String>,
    ) -> OrcasResult<WorkerSession> {
        // Worker sessions may be reused across assignments, but that reuse only preserves the
        // runtime thread anchor. It does not carry hidden workflow continuity; each assignment
        // still records its own explicit worker_session binding and executes as a distinct step.
        let existing = self
            .state
            .read()
            .await
            .collaboration
            .worker_sessions
            .get(worker_session_id)
            .cloned()
            .ok_or_else(|| {
                OrcasError::Protocol(format!("unknown worker session `{worker_session_id}`"))
            })?;
        if let Some(thread_id) = existing.thread_id.as_ref()
            && self
                .thread_get(ipc::ThreadGetRequest {
                    thread_id: thread_id.clone(),
                })
                .await
                .is_ok()
        {
            return Ok(existing);
        }

        let thread = self
            .thread_start(ipc::ThreadStartRequest {
                cwd,
                model,
                ephemeral: false,
            })
            .await?;
        let session = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let session = state
                .collaboration
                .worker_sessions
                .get_mut(worker_session_id)
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown worker session `{worker_session_id}`"))
                })?;
            session.thread_id = Some(thread.thread.id);
            session.runtime_status = WorkerSessionRuntimeStatus::Idle;
            session.attachability = WorkerSessionAttachability::Unknown;
            session.updated_at = now;
            session.clone()
        };
        self.persist_collaboration_state().await?;
        Ok(session)
    }

    async fn wait_for_turn_terminal(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> OrcasResult<ipc::TurnStateView> {
        for _ in 0..1500 {
            if let Some(turn) = self.resolve_turn_state(thread_id, turn_id).await?
                && (turn.terminal
                    || matches!(
                        turn.lifecycle,
                        ipc::TurnLifecycleState::Completed
                            | ipc::TurnLifecycleState::Failed
                            | ipc::TurnLifecycleState::Interrupted
                            | ipc::TurnLifecycleState::Lost
                            | ipc::TurnLifecycleState::Unknown
                    ))
            {
                return Ok(turn);
            }
            sleep(Duration::from_millis(200)).await;
        }
        Err(OrcasError::Transport(format!(
            "timed out waiting for turn `{turn_id}` on thread `{thread_id}`"
        )))
    }

    async fn raw_output_for_turn(&self, thread_id: &str, turn_id: &str) -> Option<String> {
        if let Some(thread) = self.thread_from_state(thread_id).await
            && let Some(raw_output) = Self::full_turn_output_from_view(&thread, turn_id)
        {
            return Some(raw_output);
        }
        if let Ok(response) = self
            .thread_get(ipc::ThreadGetRequest {
                thread_id: thread_id.to_string(),
            })
            .await
            && let Some(raw_output) = Self::full_turn_output_from_view(&response.thread, turn_id)
        {
            return Some(raw_output);
        }
        self.turn_get(ipc::TurnGetRequest {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
        })
        .await
        .ok()
        .and_then(|response| response.turn.and_then(|turn| turn.recent_output))
    }

    async fn persist_collaboration_state(&self) -> OrcasResult<()> {
        let collaboration = self.state.read().await.collaboration.clone();
        let mut stored = self.store.load().await.unwrap_or_default();
        stored.collaboration = collaboration;
        self.store.save(&stored).await
    }

    async fn emit_workstream_lifecycle(
        &self,
        action: ipc::CollaborationLifecycleAction,
        workstream: &Workstream,
    ) {
        let source_kind = {
            let state = self.state.read().await;
            if state
                .collaboration
                .authority_workstream_bridges
                .contains(&workstream.id)
            {
                ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
            } else {
                ipc::PlanningSummarySourceKind::Collaboration
            }
        };
        self.emit(ipc::DaemonEvent::WorkstreamLifecycle {
            action,
            workstream: Self::workstream_summary(workstream, source_kind),
        })
        .await;
    }

    async fn emit_work_unit_lifecycle(
        &self,
        action: ipc::CollaborationLifecycleAction,
        work_unit: &WorkUnit,
    ) {
        let work_unit = {
            let state = self.state.read().await;
            let source_kind = if state
                .collaboration
                .authority_work_unit_bridges
                .contains(&work_unit.id)
            {
                ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
            } else {
                ipc::PlanningSummarySourceKind::Collaboration
            };
            state
                .collaboration
                .work_units
                .get(&work_unit.id)
                .map(|work_unit| {
                    Self::work_unit_summary_for_collaboration(
                        work_unit,
                        &state.collaboration,
                        source_kind,
                    )
                })
                .unwrap_or_else(|| {
                    Self::work_unit_summary_for_collaboration(
                        work_unit,
                        &state.collaboration,
                        source_kind,
                    )
                })
        };
        self.emit(ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit })
            .await;
    }

    async fn emit_assignment_lifecycle(
        &self,
        action: ipc::AssignmentLifecycleAction,
        assignment: &Assignment,
    ) {
        self.emit(ipc::DaemonEvent::AssignmentLifecycle {
            action,
            assignment: Self::assignment_summary(assignment),
        })
        .await;
    }

    async fn emit_codex_assignment_lifecycle(
        &self,
        action: ipc::CodexAssignmentLifecycleAction,
        assignment: &CodexThreadAssignment,
    ) {
        self.emit(ipc::DaemonEvent::CodexAssignmentLifecycle {
            action,
            assignment: Self::codex_assignment_summary(assignment),
        })
        .await;
    }

    async fn emit_supervisor_decision_lifecycle(
        &self,
        action: ipc::SupervisorDecisionLifecycleAction,
        decision: &SupervisorTurnDecision,
    ) {
        let decision = {
            let state = self.state.read().await;
            Self::supervisor_turn_decision_summary(&state.collaboration, decision)
        };
        self.emit(ipc::DaemonEvent::SupervisorDecisionLifecycle { action, decision })
            .await;
    }

    async fn emit_report_recorded(&self, report: &Report) {
        self.emit(ipc::DaemonEvent::ReportRecorded {
            report: Self::report_summary(report),
        })
        .await;
    }

    async fn emit_decision_applied(&self, decision: &Decision) {
        self.emit(ipc::DaemonEvent::DecisionApplied {
            decision: Self::decision_summary(decision),
        })
        .await;
    }

    async fn emit_proposal_lifecycle(
        &self,
        action: ipc::ProposalLifecycleAction,
        proposal: &SupervisorProposalRecord,
    ) {
        let work_unit = {
            let state = self.state.read().await;
            state
                .collaboration
                .work_units
                .get(&proposal.primary_work_unit_id)
                .map(|work_unit| {
                    Self::work_unit_summary_for_collaboration(
                        work_unit,
                        &state.collaboration,
                        if state
                            .collaboration
                            .authority_work_unit_bridges
                            .contains(&proposal.primary_work_unit_id)
                        {
                            ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
                        } else {
                            ipc::PlanningSummarySourceKind::Collaboration
                        },
                    )
                })
                .unwrap_or_else(|| ipc::WorkUnitSummary {
                    id: proposal.primary_work_unit_id.clone(),
                    workstream_id: proposal.workstream_id.clone(),
                    title: proposal.context_pack.primary_work_unit.title.clone(),
                    status: WorkUnitStatus::AwaitingDecision,
                    dependency_count: proposal.context_pack.primary_work_unit.dependencies.len(),
                    current_assignment_id: Some(
                        proposal.context_pack.current_assignment.id.clone(),
                    ),
                    latest_report_id: Some(proposal.source_report_id.clone()),
                    proposal: None,
                    source_kind: if state
                        .collaboration
                        .authority_work_unit_bridges
                        .contains(&proposal.primary_work_unit_id)
                    {
                        ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
                    } else {
                        ipc::PlanningSummarySourceKind::Collaboration
                    },
                    updated_at: proposal.created_at,
                })
        };
        self.emit(ipc::DaemonEvent::ProposalLifecycle {
            action,
            proposal: Self::proposal_summary(proposal),
            work_unit,
        })
        .await;
    }

    async fn snapshot(&self) -> OrcasResult<ipc::StateSnapshot> {
        let daemon = self.daemon_status().await?;
        let state = self.state.read().await;
        let threads = Self::scoped_thread_summaries(&state.threads);
        let active_thread = Self::focus_thread_view(&state, &threads);
        let session = state.session.clone();
        let collaboration_state = state.collaboration.clone();
        drop(state);
        // `state/get` is now a collaboration-first snapshot plus explicit assignment-compatibility
        // bridges. Authority planning hierarchy reads come from authority queries, not from this
        // summary surface. Bridged collaboration rows are hidden if their authority source has
        // been tombstoned, even though the legacy collaboration copy may still exist on disk.
        let bridge_metadata = self.bridge_snapshot_metadata(&collaboration_state).await?;
        let collaboration = Self::collaboration_snapshot(&collaboration_state, &bridge_metadata);

        Ok(ipc::StateSnapshot {
            daemon,
            session,
            threads,
            active_thread,
            collaboration,
            recent_events: self.recent_events.lock().await.iter().cloned().collect(),
        })
    }

    fn new_object_id(prefix: &str) -> String {
        format!("{prefix}-{}", Uuid::new_v4().simple())
    }

    async fn bridge_snapshot_metadata(
        &self,
        collaboration: &CollaborationState,
    ) -> OrcasResult<BridgeSnapshotMetadata> {
        let mut metadata = BridgeSnapshotMetadata {
            workstream_bridge_ids: collaboration.authority_workstream_bridges.clone(),
            work_unit_bridge_ids: collaboration.authority_work_unit_bridges.clone(),
            ..BridgeSnapshotMetadata::default()
        };
        if metadata.workstream_bridge_ids.is_empty() && metadata.work_unit_bridge_ids.is_empty() {
            return Ok(metadata);
        }

        let live_workstream_ids = self
            .authority_store
            .list_workstreams(false)
            .await?
            .into_iter()
            .map(|workstream| workstream.id.to_string())
            .collect::<BTreeSet<_>>();
        let live_work_unit_parents = self
            .authority_store
            .list_work_units(None, false)
            .await?
            .into_iter()
            .map(|work_unit| {
                (
                    work_unit.id.to_string(),
                    work_unit.workstream_id.to_string(),
                )
            })
            .collect::<HashMap<_, _>>();

        for workstream_id in &metadata.workstream_bridge_ids {
            if !live_workstream_ids.contains(workstream_id) {
                metadata.hidden_workstream_ids.insert(workstream_id.clone());
            }
        }
        for work_unit_id in &metadata.work_unit_bridge_ids {
            match live_work_unit_parents.get(work_unit_id) {
                Some(parent_id) if !metadata.hidden_workstream_ids.contains(parent_id) => {}
                _ => {
                    metadata.hidden_work_unit_ids.insert(work_unit_id.clone());
                }
            }
        }

        Ok(metadata)
    }

    fn collaboration_snapshot(
        collaboration: &CollaborationState,
        bridge_metadata: &BridgeSnapshotMetadata,
    ) -> ipc::CollaborationSnapshot {
        let mut workstreams = collaboration
            .workstreams
            .values()
            .filter(|workstream| {
                !bridge_metadata
                    .hidden_workstream_ids
                    .contains(&workstream.id)
            })
            .map(|workstream| {
                Self::workstream_summary(
                    workstream,
                    bridge_metadata.workstream_source_kind(&workstream.id),
                )
            })
            .collect::<Vec<_>>();
        workstreams.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut work_units = collaboration
            .work_units
            .values()
            .filter(|work_unit| !bridge_metadata.hidden_work_unit_ids.contains(&work_unit.id))
            .map(|work_unit| {
                Self::work_unit_summary_for_collaboration(
                    work_unit,
                    collaboration,
                    bridge_metadata.work_unit_source_kind(&work_unit.id),
                )
            })
            .collect::<Vec<_>>();
        work_units.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut assignments = collaboration
            .assignments
            .values()
            .filter(|assignment| assignment.status != AssignmentStatus::Closed)
            .map(Self::assignment_summary)
            .collect::<Vec<_>>();
        assignments.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut codex_thread_assignments = collaboration
            .codex_thread_assignments
            .values()
            .map(Self::codex_assignment_summary)
            .collect::<Vec<_>>();
        codex_thread_assignments.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.assignment_id.cmp(&right.assignment_id))
        });

        let mut supervisor_turn_decisions = collaboration
            .supervisor_turn_decisions
            .values()
            .map(|decision| Self::supervisor_turn_decision_summary(collaboration, decision))
            .collect::<Vec<_>>();
        supervisor_turn_decisions.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.decision_id.cmp(&right.decision_id))
        });

        let mut reports = collaboration
            .reports
            .values()
            .map(Self::report_summary)
            .collect::<Vec<_>>();
        reports.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut decisions = collaboration
            .decisions
            .values()
            .map(Self::decision_summary)
            .collect::<Vec<_>>();
        decisions.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        ipc::CollaborationSnapshot {
            workstreams,
            work_units,
            assignments,
            codex_thread_assignments,
            supervisor_turn_decisions,
            reports,
            decisions,
        }
    }

    fn workstream_summary(
        workstream: &Workstream,
        source_kind: ipc::PlanningSummarySourceKind,
    ) -> ipc::WorkstreamSummary {
        ipc::WorkstreamSummary {
            id: workstream.id.clone(),
            title: workstream.title.clone(),
            objective: workstream.objective.clone(),
            status: workstream.status,
            priority: workstream.priority.clone(),
            source_kind,
            updated_at: workstream.updated_at,
        }
    }

    fn authority_workstream_record_to_collaboration(
        workstream: &orcas_core::authority::WorkstreamRecord,
    ) -> Workstream {
        Workstream {
            id: workstream.id.to_string(),
            title: workstream.title.clone(),
            objective: workstream.objective.clone(),
            status: workstream.status,
            priority: workstream.priority.clone(),
            created_at: workstream.created_at,
            updated_at: workstream.updated_at,
        }
    }

    fn authority_work_unit_record_to_collaboration(
        work_unit: &orcas_core::authority::WorkUnitRecord,
    ) -> WorkUnit {
        WorkUnit {
            id: work_unit.id.to_string(),
            workstream_id: work_unit.workstream_id.to_string(),
            title: work_unit.title.clone(),
            task_statement: work_unit.task_statement.clone(),
            status: work_unit.status,
            dependencies: Vec::new(),
            latest_report_id: None,
            current_assignment_id: None,
            created_at: work_unit.created_at,
            updated_at: work_unit.updated_at,
        }
    }

    async fn emit_authority_workstream_lifecycle(
        &self,
        action: ipc::CollaborationLifecycleAction,
        workstream: &orcas_core::authority::WorkstreamRecord,
    ) {
        self.emit(ipc::DaemonEvent::WorkstreamLifecycle {
            action,
            workstream: ipc::WorkstreamSummary {
                id: workstream.id.to_string(),
                title: workstream.title.clone(),
                objective: workstream.objective.clone(),
                status: workstream.status,
                priority: workstream.priority.clone(),
                source_kind: ipc::PlanningSummarySourceKind::AuthorityProjection,
                updated_at: workstream.updated_at,
            },
        })
        .await;
    }

    async fn emit_authority_work_unit_lifecycle(
        &self,
        action: ipc::CollaborationLifecycleAction,
        work_unit: &orcas_core::authority::WorkUnitRecord,
    ) {
        self.emit(ipc::DaemonEvent::WorkUnitLifecycle {
            action,
            work_unit: ipc::WorkUnitSummary {
                id: work_unit.id.to_string(),
                workstream_id: work_unit.workstream_id.to_string(),
                title: work_unit.title.clone(),
                status: work_unit.status,
                dependency_count: 0,
                current_assignment_id: None,
                latest_report_id: None,
                proposal: None,
                source_kind: ipc::PlanningSummarySourceKind::AuthorityProjection,
                updated_at: work_unit.updated_at,
            },
        })
        .await;
    }

    async fn emit_authority_tracked_thread_lifecycle(
        &self,
        action: ipc::CollaborationLifecycleAction,
        tracked_thread: &orcas_core::authority::TrackedThreadRecord,
    ) {
        self.emit(ipc::DaemonEvent::TrackedThreadLifecycle {
            action,
            tracked_thread: tracked_thread.into(),
        })
        .await;
    }

    fn work_unit_summary_for_collaboration(
        work_unit: &WorkUnit,
        collaboration: &CollaborationState,
        source_kind: ipc::PlanningSummarySourceKind,
    ) -> ipc::WorkUnitSummary {
        ipc::WorkUnitSummary {
            id: work_unit.id.clone(),
            workstream_id: work_unit.workstream_id.clone(),
            title: work_unit.title.clone(),
            status: work_unit.status,
            dependency_count: work_unit.dependencies.len(),
            current_assignment_id: work_unit.current_assignment_id.clone(),
            latest_report_id: work_unit.latest_report_id.clone(),
            proposal: Self::work_unit_proposal_summary(collaboration, &work_unit.id),
            source_kind,
            updated_at: work_unit.updated_at,
        }
    }

    fn assignment_summary(assignment: &Assignment) -> ipc::AssignmentSummary {
        ipc::AssignmentSummary {
            id: assignment.id.clone(),
            work_unit_id: assignment.work_unit_id.clone(),
            worker_id: assignment.worker_id.clone(),
            worker_session_id: assignment.worker_session_id.clone(),
            status: assignment.status,
            attempt_number: assignment.attempt_number,
            updated_at: assignment.updated_at,
        }
    }

    fn codex_assignment_status_is_active(status: CodexThreadAssignmentStatus) -> bool {
        matches!(
            status,
            CodexThreadAssignmentStatus::Proposed | CodexThreadAssignmentStatus::Active
        )
    }

    fn codex_assignment_bootstrap_state_for_thread(
        thread: &ipc::ThreadView,
    ) -> CodexThreadBootstrapState {
        if thread.summary.last_seen_turn_id.is_some() || thread.summary.active_turn_id.is_some() {
            CodexThreadBootstrapState::Pending
        } else {
            CodexThreadBootstrapState::Pending
        }
    }

    fn merge_assignment_notes(notes: &mut Option<String>, update: Option<String>) {
        let Some(update) = update.map(|note| note.trim().to_string()) else {
            return;
        };
        if update.is_empty() {
            return;
        }
        match notes {
            Some(existing) if !existing.trim().is_empty() => {
                existing.push('\n');
                existing.push_str(&update);
            }
            _ => *notes = Some(update),
        }
    }

    fn codex_assignment_summary(
        assignment: &CodexThreadAssignment,
    ) -> ipc::CodexThreadAssignmentSummary {
        ipc::CodexThreadAssignmentSummary {
            assignment_id: assignment.assignment_id.clone(),
            codex_thread_id: assignment.codex_thread_id.clone(),
            workstream_id: assignment.workstream_id.clone(),
            work_unit_id: assignment.work_unit_id.clone(),
            supervisor_id: assignment.supervisor_id.clone(),
            assigned_by: assignment.assigned_by.clone(),
            assigned_at: assignment.assigned_at,
            updated_at: assignment.updated_at,
            status: assignment.status,
            send_policy: assignment.send_policy,
            bootstrap_state: assignment.bootstrap_state,
            latest_basis_turn_id: assignment.latest_basis_turn_id.clone(),
            latest_decision_id: assignment.latest_decision_id.clone(),
            notes: assignment.notes.clone(),
            active: Self::codex_assignment_status_is_active(assignment.status),
        }
    }

    fn supervisor_decision_is_open(status: SupervisorTurnDecisionStatus) -> bool {
        matches!(
            status,
            SupervisorTurnDecisionStatus::Draft | SupervisorTurnDecisionStatus::ProposedToHuman
        )
    }

    fn supervisor_turn_decision_summary(
        collaboration: &CollaborationState,
        decision: &SupervisorTurnDecision,
    ) -> ipc::SupervisorTurnDecisionSummary {
        let assignment = collaboration
            .codex_thread_assignments
            .get(&decision.assignment_id);
        ipc::SupervisorTurnDecisionSummary {
            decision_id: decision.decision_id.clone(),
            assignment_id: decision.assignment_id.clone(),
            codex_thread_id: decision.codex_thread_id.clone(),
            workstream_id: assignment.map(|assignment| assignment.workstream_id.clone()),
            work_unit_id: assignment.map(|assignment| assignment.work_unit_id.clone()),
            supervisor_id: assignment.map(|assignment| assignment.supervisor_id.clone()),
            basis_turn_id: decision.basis_turn_id.clone(),
            kind: decision.kind,
            proposal_kind: decision.proposal_kind,
            proposed_text: decision.proposed_text.clone(),
            rationale_summary: decision.rationale_summary.clone(),
            status: decision.status,
            created_at: decision.created_at,
            approved_at: decision.approved_at,
            rejected_at: decision.rejected_at,
            sent_at: decision.sent_at,
            superseded_by: decision.superseded_by.clone(),
            sent_turn_id: decision.sent_turn_id.clone(),
            notes: decision.notes.clone(),
            open: Self::supervisor_decision_is_open(decision.status),
        }
    }

    fn codex_assignment_supports_decisions(status: CodexThreadAssignmentStatus) -> bool {
        status == CodexThreadAssignmentStatus::Active
    }

    fn next_turn_decision_basis_reason(
        assignment: &CodexThreadAssignment,
        thread: &ipc::ThreadView,
        decision: &SupervisorTurnDecision,
    ) -> Option<String> {
        if !Self::codex_assignment_supports_decisions(assignment.status) {
            return Some(format!(
                "assignment `{}` is not active",
                assignment.assignment_id
            ));
        }
        if thread.summary.active_turn_id.is_some() {
            return Some(format!(
                "thread `{}` has an active turn and is not idle",
                thread.summary.id
            ));
        }
        if thread.summary.last_seen_turn_id != decision.basis_turn_id {
            return Some(format!(
                "thread basis changed from {:?} to {:?}",
                decision.basis_turn_id, thread.summary.last_seen_turn_id
            ));
        }
        if decision.status != SupervisorTurnDecisionStatus::ProposedToHuman {
            return Some(format!(
                "decision `{}` is no longer pending human review",
                decision.decision_id
            ));
        }
        None
    }

    fn interrupt_decision_basis_reason(
        assignment: &CodexThreadAssignment,
        thread: &ipc::ThreadView,
        decision: &SupervisorTurnDecision,
    ) -> Option<String> {
        if !Self::codex_assignment_supports_decisions(assignment.status) {
            return Some(format!(
                "assignment `{}` is not active",
                assignment.assignment_id
            ));
        }
        let active_turn_id = thread.summary.active_turn_id.as_ref().ok_or_else(|| {
            format!(
                "thread `{}` has no active turn to interrupt",
                thread.summary.id
            )
        });
        let active_turn_id = match active_turn_id {
            Ok(turn_id) => turn_id,
            Err(reason) => return Some(reason),
        };
        if decision.basis_turn_id.as_deref() != Some(active_turn_id.as_str()) {
            return Some(format!(
                "active turn changed from {:?} to {:?}",
                decision.basis_turn_id,
                Some(active_turn_id)
            ));
        }
        if decision.status != SupervisorTurnDecisionStatus::ProposedToHuman {
            return Some(format!(
                "decision `{}` is no longer pending human review",
                decision.decision_id
            ));
        }
        None
    }

    fn steer_decision_basis_reason(
        assignment: &CodexThreadAssignment,
        thread: &ipc::ThreadView,
        decision: &SupervisorTurnDecision,
    ) -> Option<String> {
        if !Self::codex_assignment_supports_decisions(assignment.status) {
            return Some(format!(
                "assignment `{}` is not active",
                assignment.assignment_id
            ));
        }
        let active_turn_id =
            thread.summary.active_turn_id.as_ref().ok_or_else(|| {
                format!("thread `{}` has no active turn to steer", thread.summary.id)
            });
        let active_turn_id = match active_turn_id {
            Ok(turn_id) => turn_id,
            Err(reason) => return Some(reason),
        };
        if decision.basis_turn_id.as_deref() != Some(active_turn_id.as_str()) {
            return Some(format!(
                "active turn changed from {:?} to {:?}",
                decision.basis_turn_id,
                Some(active_turn_id)
            ));
        }
        if decision
            .proposed_text
            .as_ref()
            .map(|text| text.trim())
            .filter(|text| !text.is_empty())
            .is_none()
        {
            return Some(format!(
                "steer decision `{}` has no proposed text to send",
                decision.decision_id
            ));
        }
        if decision.status != SupervisorTurnDecisionStatus::ProposedToHuman {
            return Some(format!(
                "decision `{}` is no longer pending human review",
                decision.decision_id
            ));
        }
        None
    }

    fn supervisor_decision_basis_reason(
        assignment: &CodexThreadAssignment,
        thread: &ipc::ThreadView,
        decision: &SupervisorTurnDecision,
    ) -> Option<String> {
        match decision.kind {
            SupervisorTurnDecisionKind::NextTurn => {
                Self::next_turn_decision_basis_reason(assignment, thread, decision)
            }
            SupervisorTurnDecisionKind::SteerActiveTurn => {
                Self::steer_decision_basis_reason(assignment, thread, decision)
            }
            SupervisorTurnDecisionKind::InterruptActiveTurn => {
                Self::interrupt_decision_basis_reason(assignment, thread, decision)
            }
            SupervisorTurnDecisionKind::NoAction => {
                if !Self::codex_assignment_supports_decisions(assignment.status) {
                    Some(format!(
                        "assignment `{}` is not active",
                        assignment.assignment_id
                    ))
                } else if decision.status != SupervisorTurnDecisionStatus::Recorded {
                    Some(format!(
                        "decision `{}` is not a recorded no_action decision",
                        decision.decision_id
                    ))
                } else if thread.summary.active_turn_id.is_some() {
                    Some(format!(
                        "thread `{}` has an active turn and is not idle",
                        thread.summary.id
                    ))
                } else if thread.summary.last_seen_turn_id != decision.basis_turn_id {
                    Some(format!(
                        "thread basis changed from {:?} to {:?}",
                        decision.basis_turn_id, thread.summary.last_seen_turn_id
                    ))
                } else {
                    None
                }
            }
        }
    }

    async fn validate_supervisor_send_basis(
        &self,
        decision: &SupervisorTurnDecision,
    ) -> Result<(), String> {
        let state = self.state.read().await;
        let assignment = state
            .collaboration
            .codex_thread_assignments
            .get(&decision.assignment_id)
            .ok_or_else(|| format!("assignment `{}` no longer exists", decision.assignment_id))?;
        let thread = state
            .threads
            .get(&decision.codex_thread_id)
            .ok_or_else(|| {
                format!(
                    "thread `{}` is not loaded in Orcas state",
                    decision.codex_thread_id
                )
            })?;
        Self::supervisor_decision_basis_reason(assignment, thread, decision).map_or(Ok(()), Err)
    }

    fn open_supervisor_decision_id_for_assignment(
        collaboration: &CollaborationState,
        assignment_id: &str,
    ) -> Option<String> {
        collaboration
            .supervisor_turn_decisions
            .values()
            .filter(|decision| {
                decision.assignment_id == assignment_id
                    && Self::supervisor_decision_is_open(decision.status)
            })
            .max_by(|left, right| {
                left.created_at
                    .cmp(&right.created_at)
                    .then_with(|| left.decision_id.cmp(&right.decision_id))
            })
            .map(|decision| decision.decision_id.clone())
    }

    fn latest_decision_for_assignment_basis<'a>(
        collaboration: &'a CollaborationState,
        assignment_id: &str,
        basis_turn_id: Option<&str>,
    ) -> Option<&'a SupervisorTurnDecision> {
        collaboration
            .supervisor_turn_decisions
            .values()
            .filter(|decision| {
                decision.assignment_id == assignment_id
                    && decision.basis_turn_id.as_deref() == basis_turn_id
            })
            .max_by(|left, right| {
                left.created_at
                    .cmp(&right.created_at)
                    .then_with(|| left.decision_id.cmp(&right.decision_id))
            })
    }

    fn latest_current_basis_recorded_no_action<'a>(
        collaboration: &'a CollaborationState,
        assignment_id: &str,
        basis_turn_id: Option<&str>,
    ) -> Option<&'a SupervisorTurnDecision> {
        Self::latest_decision_for_assignment_basis(collaboration, assignment_id, basis_turn_id)
            .filter(|decision| {
                decision.kind == SupervisorTurnDecisionKind::NoAction
                    && decision.status == SupervisorTurnDecisionStatus::Recorded
            })
    }

    fn generate_bootstrap_turn_text(
        assignment: &CodexThreadAssignment,
        workstream: Option<&Workstream>,
        work_unit: Option<&WorkUnit>,
    ) -> String {
        let workstream_label = workstream
            .map(|workstream| format!("{} ({})", workstream.id, workstream.title))
            .unwrap_or_else(|| assignment.workstream_id.clone());
        let work_unit_label = work_unit
            .map(|work_unit| format!("{} ({})", work_unit.id, work_unit.title))
            .unwrap_or_else(|| assignment.work_unit_id.clone());
        format!(
            "Orcas supervisor is now managing this thread for workstream {workstream_label} and work unit {work_unit_label}.\n\
Reply with a concise status summary, current blockers or risks, and the next bounded action you intend to take.\n\
If the next step is risky or destructive, ask for clarification before proceeding.\n\
Then continue with the next bounded step for this assigned work unit."
        )
    }

    fn generate_continue_turn_text(
        assignment: &CodexThreadAssignment,
        workstream: Option<&Workstream>,
        work_unit: Option<&WorkUnit>,
        basis_turn_id: Option<&str>,
    ) -> String {
        let workstream_label = workstream
            .map(|workstream| format!("{} ({})", workstream.id, workstream.title))
            .unwrap_or_else(|| assignment.workstream_id.clone());
        let work_unit_label = work_unit
            .map(|work_unit| format!("{} ({})", work_unit.id, work_unit.title))
            .unwrap_or_else(|| assignment.work_unit_id.clone());
        let basis_clause = basis_turn_id
            .map(|turn_id| format!(" from turn `{turn_id}`"))
            .unwrap_or_default();
        format!(
            "Continue under Orcas supervision for workstream {workstream_label} and work unit {work_unit_label}.\n\
Briefly summarize what the prior turn completed{basis_clause}.\n\
State the next bounded step, then proceed with that step.\n\
Call out blockers, uncertainty, or risky/destructive changes before taking them."
        )
    }

    fn stale_open_supervisor_decisions_for_assignment_locked(
        collaboration: &mut CollaborationState,
        assignment_id: &str,
        reason: &str,
    ) -> (Vec<SupervisorTurnDecision>, Vec<CodexThreadAssignment>) {
        let candidate_ids = collaboration
            .supervisor_turn_decisions
            .values()
            .filter(|decision| {
                decision.assignment_id == assignment_id
                    && Self::supervisor_decision_is_open(decision.status)
            })
            .map(|decision| decision.decision_id.clone())
            .collect::<Vec<_>>();
        let mut stale = Vec::new();
        let mut updated_assignments = Vec::new();
        for decision_id in candidate_ids {
            let proposal_kind = collaboration
                .supervisor_turn_decisions
                .get(&decision_id)
                .map(|decision| decision.proposal_kind);
            if let Some(decision) = collaboration
                .supervisor_turn_decisions
                .get_mut(&decision_id)
            {
                decision.status = SupervisorTurnDecisionStatus::Stale;
                Self::merge_assignment_notes(&mut decision.notes, Some(reason.to_string()));
                stale.push(decision.clone());
            }
            if proposal_kind == Some(SupervisorTurnProposalKind::Bootstrap)
                && let Some(assignment) = collaboration
                    .codex_thread_assignments
                    .get_mut(assignment_id)
                && assignment.bootstrap_state == CodexThreadBootstrapState::Proposed
            {
                assignment.bootstrap_state = CodexThreadBootstrapState::Pending;
                assignment.updated_at = Utc::now();
                updated_assignments.push(assignment.clone());
            }
        }
        (stale, updated_assignments)
    }

    fn stale_open_supervisor_decisions_for_thread_locked(
        collaboration: &mut CollaborationState,
        thread: &ipc::ThreadView,
    ) -> (Vec<SupervisorTurnDecision>, Vec<CodexThreadAssignment>) {
        let candidate_ids = collaboration
            .supervisor_turn_decisions
            .values()
            .filter(|decision| {
                decision.codex_thread_id == thread.summary.id
                    && Self::supervisor_decision_is_open(decision.status)
            })
            .map(|decision| decision.decision_id.clone())
            .collect::<Vec<_>>();
        let mut stale = Vec::new();
        let mut updated_assignments = Vec::new();
        for decision_id in candidate_ids {
            let Some(existing) = collaboration
                .supervisor_turn_decisions
                .get(&decision_id)
                .cloned()
            else {
                continue;
            };
            let reason = collaboration
                .codex_thread_assignments
                .get(&existing.assignment_id)
                .and_then(|assignment| {
                    Self::supervisor_decision_basis_reason(assignment, thread, &existing)
                });
            let Some(reason) = reason else {
                continue;
            };
            if let Some(decision) = collaboration
                .supervisor_turn_decisions
                .get_mut(&decision_id)
            {
                decision.status = SupervisorTurnDecisionStatus::Stale;
                Self::merge_assignment_notes(&mut decision.notes, Some(reason.clone()));
                stale.push(decision.clone());
            }
            if existing.proposal_kind == SupervisorTurnProposalKind::Bootstrap
                && let Some(assignment) = collaboration
                    .codex_thread_assignments
                    .get_mut(&existing.assignment_id)
                && assignment.bootstrap_state == CodexThreadBootstrapState::Proposed
            {
                assignment.bootstrap_state = CodexThreadBootstrapState::Pending;
                assignment.updated_at = Utc::now();
                updated_assignments.push(assignment.clone());
            }
        }
        (stale, updated_assignments)
    }

    async fn stale_supervisor_decisions_for_inactive_assignment(
        &self,
        assignment_id: &str,
    ) -> OrcasResult<()> {
        let (stale, updated_assignments) = {
            let mut state = self.state.write().await;
            Self::stale_open_supervisor_decisions_for_assignment_locked(
                &mut state.collaboration,
                assignment_id,
                "assignment is no longer active for supervisor sends",
            )
        };
        if stale.is_empty() && updated_assignments.is_empty() {
            return Ok(());
        }
        self.persist_collaboration_state().await?;
        for assignment in &updated_assignments {
            self.emit_codex_assignment_lifecycle(
                ipc::CodexAssignmentLifecycleAction::Updated,
                assignment,
            )
            .await;
        }
        for decision in &stale {
            self.emit_supervisor_decision_lifecycle(
                ipc::SupervisorDecisionLifecycleAction::Stale,
                decision,
            )
            .await;
        }
        Ok(())
    }

    async fn refresh_codex_supervisor_state_for_thread(&self, thread_id: &str) -> OrcasResult<()> {
        let (created, stale, updated_assignments) = {
            let now = Utc::now();
            let mut state = self.state.write().await;
            let Some(thread) = state.threads.get(thread_id).cloned() else {
                return Ok(());
            };

            let (stale, mut updated_assignments) =
                Self::stale_open_supervisor_decisions_for_thread_locked(
                    &mut state.collaboration,
                    &thread,
                );
            let mut created = Vec::new();

            if thread.summary.active_turn_id.is_none() {
                let assignment_ids = state
                    .collaboration
                    .codex_thread_assignments
                    .values()
                    .filter(|assignment| {
                        assignment.codex_thread_id == thread.summary.id
                            && Self::codex_assignment_supports_decisions(assignment.status)
                    })
                    .map(|assignment| assignment.assignment_id.clone())
                    .collect::<Vec<_>>();

                for assignment_id in assignment_ids {
                    if Self::open_supervisor_decision_id_for_assignment(
                        &state.collaboration,
                        &assignment_id,
                    )
                    .is_some()
                    {
                        continue;
                    }
                    let Some(assignment_snapshot) = state
                        .collaboration
                        .codex_thread_assignments
                        .get(&assignment_id)
                        .cloned()
                    else {
                        continue;
                    };
                    let basis_turn_id = thread.summary.last_seen_turn_id.clone();
                    if Self::latest_current_basis_recorded_no_action(
                        &state.collaboration,
                        &assignment_snapshot.assignment_id,
                        basis_turn_id.as_deref(),
                    )
                    .is_some()
                    {
                        continue;
                    }

                    let proposal_kind = if assignment_snapshot.bootstrap_state
                        == CodexThreadBootstrapState::Pending
                    {
                        SupervisorTurnProposalKind::Bootstrap
                    } else {
                        SupervisorTurnProposalKind::ContinueAfterTurn
                    };
                    let proposed_text = Some(match proposal_kind {
                        SupervisorTurnProposalKind::Bootstrap => {
                            Self::generate_bootstrap_turn_text(
                                &assignment_snapshot,
                                state
                                    .collaboration
                                    .workstreams
                                    .get(&assignment_snapshot.workstream_id),
                                state
                                    .collaboration
                                    .work_units
                                    .get(&assignment_snapshot.work_unit_id),
                            )
                        }
                        SupervisorTurnProposalKind::ContinueAfterTurn
                        | SupervisorTurnProposalKind::ManualRefresh => {
                            Self::generate_continue_turn_text(
                                &assignment_snapshot,
                                state
                                    .collaboration
                                    .workstreams
                                    .get(&assignment_snapshot.workstream_id),
                                state
                                    .collaboration
                                    .work_units
                                    .get(&assignment_snapshot.work_unit_id),
                                basis_turn_id.as_deref(),
                            )
                        }
                        SupervisorTurnProposalKind::OperatorSteer => {
                            unreachable!("operator steer proposals are only created explicitly")
                        }
                        SupervisorTurnProposalKind::OperatorInterrupt => {
                            unreachable!("operator interrupt proposals are only created explicitly")
                        }
                    });
                    let rationale_summary = match proposal_kind {
                        SupervisorTurnProposalKind::Bootstrap => format!(
                            "Assignment `{}` is active, the thread is idle, and the bootstrap proposal has not been sent yet.",
                            assignment_snapshot.assignment_id
                        ),
                        SupervisorTurnProposalKind::ContinueAfterTurn
                        | SupervisorTurnProposalKind::ManualRefresh => format!(
                            "Assignment `{}` remains active and thread `{}` is idle after basis turn {:?}.",
                            assignment_snapshot.assignment_id, thread.summary.id, basis_turn_id
                        ),
                        SupervisorTurnProposalKind::OperatorSteer => {
                            unreachable!("operator steer proposals are only created explicitly")
                        }
                        SupervisorTurnProposalKind::OperatorInterrupt => {
                            unreachable!("operator interrupt proposals are only created explicitly")
                        }
                    };
                    let decision = SupervisorTurnDecision {
                        decision_id: Self::new_object_id("std"),
                        assignment_id: assignment_snapshot.assignment_id.clone(),
                        codex_thread_id: assignment_snapshot.codex_thread_id.clone(),
                        basis_turn_id: basis_turn_id.clone(),
                        kind: SupervisorTurnDecisionKind::NextTurn,
                        proposal_kind,
                        proposed_text,
                        rationale_summary,
                        status: SupervisorTurnDecisionStatus::ProposedToHuman,
                        created_at: now,
                        approved_at: None,
                        rejected_at: None,
                        sent_at: None,
                        superseded_by: None,
                        sent_turn_id: None,
                        notes: None,
                    };
                    state
                        .collaboration
                        .supervisor_turn_decisions
                        .insert(decision.decision_id.clone(), decision.clone());
                    if let Some(assignment) = state
                        .collaboration
                        .codex_thread_assignments
                        .get_mut(&assignment_snapshot.assignment_id)
                    {
                        assignment.latest_decision_id = Some(decision.decision_id.clone());
                        assignment.latest_basis_turn_id = basis_turn_id.clone();
                        assignment.updated_at = now;
                        if proposal_kind == SupervisorTurnProposalKind::Bootstrap {
                            assignment.bootstrap_state = CodexThreadBootstrapState::Proposed;
                        }
                        updated_assignments.push(assignment.clone());
                    }
                    created.push(decision);
                }
            }
            (created, stale, updated_assignments)
        };
        if created.is_empty() && stale.is_empty() && updated_assignments.is_empty() {
            return Ok(());
        }
        self.persist_collaboration_state().await?;
        for assignment in &updated_assignments {
            self.emit_codex_assignment_lifecycle(
                ipc::CodexAssignmentLifecycleAction::Updated,
                assignment,
            )
            .await;
        }
        for decision in &stale {
            self.emit_supervisor_decision_lifecycle(
                ipc::SupervisorDecisionLifecycleAction::Stale,
                decision,
            )
            .await;
        }
        for decision in &created {
            self.emit_supervisor_decision_lifecycle(
                ipc::SupervisorDecisionLifecycleAction::Created,
                decision,
            )
            .await;
        }
        Ok(())
    }

    async fn mark_supervisor_decision_stale(
        &self,
        decision_id: &str,
        reason: &str,
    ) -> OrcasResult<SupervisorTurnDecision> {
        let (decision, updated_assignment) = {
            let mut state = self.state.write().await;
            let decision_snapshot = state
                .collaboration
                .supervisor_turn_decisions
                .get(decision_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!("unknown supervisor decision `{decision_id}`"))
                })?;
            let decision = state
                .collaboration
                .supervisor_turn_decisions
                .get_mut(decision_id)
                .expect("decision exists");
            decision.status = SupervisorTurnDecisionStatus::Stale;
            Self::merge_assignment_notes(&mut decision.notes, Some(reason.to_string()));
            let decision = decision.clone();

            let updated_assignment =
                if decision_snapshot.proposal_kind == SupervisorTurnProposalKind::Bootstrap {
                    state
                        .collaboration
                        .codex_thread_assignments
                        .get_mut(&decision_snapshot.assignment_id)
                        .and_then(|assignment| {
                            if assignment.bootstrap_state == CodexThreadBootstrapState::Proposed {
                                assignment.bootstrap_state = CodexThreadBootstrapState::Pending;
                                assignment.updated_at = Utc::now();
                                Some(assignment.clone())
                            } else {
                                None
                            }
                        })
                } else {
                    None
                };
            (decision, updated_assignment)
        };
        self.persist_collaboration_state().await?;
        if let Some(assignment) = updated_assignment.as_ref() {
            self.emit_codex_assignment_lifecycle(
                ipc::CodexAssignmentLifecycleAction::Updated,
                assignment,
            )
            .await;
        }
        self.emit_supervisor_decision_lifecycle(
            ipc::SupervisorDecisionLifecycleAction::Stale,
            &decision,
        )
        .await;
        Ok(decision)
    }

    fn report_summary(report: &Report) -> ipc::ReportSummary {
        ipc::ReportSummary {
            id: report.id.clone(),
            work_unit_id: report.work_unit_id.clone(),
            assignment_id: report.assignment_id.clone(),
            worker_id: report.worker_id.clone(),
            disposition: report.disposition,
            summary: report.summary.clone(),
            confidence: report.confidence,
            parse_result: report.parse_result,
            needs_supervisor_review: report.needs_supervisor_review,
            created_at: report.created_at,
        }
    }

    fn decision_summary(decision: &Decision) -> ipc::DecisionSummary {
        ipc::DecisionSummary {
            id: decision.id.clone(),
            work_unit_id: decision.work_unit_id.clone(),
            report_id: decision.report_id.clone(),
            decision_type: decision.decision_type,
            rationale: decision.rationale.clone(),
            created_at: decision.created_at,
        }
    }

    fn proposal_summary(proposal: &SupervisorProposalRecord) -> ipc::ProposalSummary {
        ipc::ProposalSummary {
            id: proposal.id.clone(),
            primary_work_unit_id: proposal.primary_work_unit_id.clone(),
            source_report_id: proposal.source_report_id.clone(),
            status: proposal.status,
            proposed_decision_type: Self::proposal_decision_type(proposal),
            created_at: proposal.created_at,
            reviewed_at: proposal.reviewed_at,
            has_approval_edits: proposal
                .approval_edits
                .as_ref()
                .is_some_and(|edits| !edits.is_empty()),
            generation_failure_stage: proposal.generation_failure.as_ref().map(|f| f.stage),
            reasoner_model: proposal.reasoner_model.clone(),
        }
    }

    fn proposal_decision_type(proposal: &SupervisorProposalRecord) -> Option<DecisionType> {
        proposal
            .approved_proposal
            .as_ref()
            .or(proposal.proposal.as_ref())
            .map(|proposal| proposal.proposed_decision.decision_type)
    }

    fn work_unit_proposal_summary(
        collaboration: &CollaborationState,
        work_unit_id: &str,
    ) -> Option<ipc::WorkUnitProposalSummary> {
        let proposals = collaboration
            .supervisor_proposals
            .values()
            .filter(|proposal| proposal.primary_work_unit_id == work_unit_id)
            .collect::<Vec<_>>();
        let latest = proposals.into_iter().max_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        })?;

        let open = collaboration
            .supervisor_proposals
            .values()
            .filter(|proposal| {
                proposal.primary_work_unit_id == work_unit_id
                    && proposal.status == SupervisorProposalStatus::Open
            })
            .max_by(|left, right| {
                left.created_at
                    .cmp(&right.created_at)
                    .then_with(|| left.id.cmp(&right.id))
            });

        Some(ipc::WorkUnitProposalSummary {
            latest_proposal_id: latest.id.clone(),
            latest_status: latest.status,
            latest_proposed_decision_type: Self::proposal_decision_type(latest),
            latest_created_at: latest.created_at,
            latest_reviewed_at: latest.reviewed_at,
            latest_has_approval_edits: latest
                .approval_edits
                .as_ref()
                .is_some_and(|edits| !edits.is_empty()),
            latest_failure_stage: latest
                .generation_failure
                .as_ref()
                .map(|failure| failure.stage),
            has_open_proposal: open.is_some(),
            open_proposal_id: open.map(|proposal| proposal.id.clone()),
            open_proposed_decision_type: open.and_then(Self::proposal_decision_type),
            has_generation_failed: collaboration.supervisor_proposals.values().any(|proposal| {
                proposal.primary_work_unit_id == work_unit_id
                    && proposal.status == SupervisorProposalStatus::GenerationFailed
            }),
            has_stale_or_superseded: collaboration.supervisor_proposals.values().any(|proposal| {
                proposal.primary_work_unit_id == work_unit_id
                    && matches!(
                        proposal.status,
                        SupervisorProposalStatus::Stale | SupervisorProposalStatus::Superseded
                    )
            }),
        })
    }

    async fn persist_failed_proposal_record(
        &self,
        proposal_id: String,
        context_pack: SupervisorContextPack,
        reasoner_backend: String,
        reasoner_model: String,
        reasoner_response_id: Option<String>,
        reasoner_usage: Option<SupervisorReasonerUsage>,
        reasoner_output_text: Option<String>,
        prompt_render: Option<orcas_core::SupervisorPromptRenderArtifact>,
        response_artifact: Option<orcas_core::SupervisorResponseArtifact>,
        proposal: Option<SupervisorProposal>,
        failure: SupervisorProposalFailure,
    ) -> OrcasResult<SupervisorProposalRecord> {
        let record = SupervisorProposalRecord {
            id: proposal_id,
            workstream_id: context_pack.workstream.id.clone(),
            primary_work_unit_id: context_pack.primary_work_unit.id.clone(),
            source_report_id: context_pack.source_report.id.clone(),
            trigger: context_pack.trigger.clone(),
            status: SupervisorProposalStatus::GenerationFailed,
            created_at: Utc::now(),
            reasoner_backend,
            reasoner_model,
            reasoner_response_id,
            reasoner_usage,
            reasoner_output_text,
            context_pack,
            prompt_render,
            response_artifact,
            proposal,
            approval_edits: None,
            approved_proposal: None,
            generation_failure: Some(failure),
            validated_at: None,
            reviewed_at: None,
            reviewed_by: None,
            review_note: None,
            approved_decision_id: None,
            approved_assignment_id: None,
        };

        {
            let mut state = self.state.write().await;
            state
                .collaboration
                .supervisor_proposals
                .insert(record.id.clone(), record.clone());
        }
        self.persist_collaboration_state().await?;
        Ok(record)
    }

    fn proposal_generation_failure_error(record: &SupervisorProposalRecord) -> OrcasError {
        let detail = record
            .generation_failure
            .as_ref()
            .map(|failure| failure.message.clone())
            .unwrap_or_else(|| "unknown supervisor proposal generation failure".to_string());
        let message = format!(
            "supervisor proposal generation failed; inspect proposal `{}` for details: {}",
            record.id, detail
        );

        match record
            .generation_failure
            .as_ref()
            .map(|failure| failure.stage)
            .unwrap_or(SupervisorProposalFailureStage::Backend)
        {
            SupervisorProposalFailureStage::Backend => OrcasError::Transport(message),
            SupervisorProposalFailureStage::ResponseMalformed
            | SupervisorProposalFailureStage::ProposalMalformed
            | SupervisorProposalFailureStage::ProposalValidation => OrcasError::Protocol(message),
        }
    }

    fn refresh_stale_proposals_for_work_unit(
        collaboration: &mut CollaborationState,
        work_unit_id: &str,
    ) -> Vec<SupervisorProposalRecord> {
        let mut changed = Vec::new();
        let candidate_ids = collaboration
            .supervisor_proposals
            .values()
            .filter(|proposal| {
                proposal.primary_work_unit_id == work_unit_id
                    && proposal.status == SupervisorProposalStatus::Open
            })
            .map(|proposal| proposal.id.clone())
            .collect::<Vec<_>>();
        for proposal_id in candidate_ids {
            let reason = collaboration
                .supervisor_proposals
                .get(&proposal_id)
                .and_then(|proposal| proposal_freshness_error(proposal, collaboration));
            if let Some(reason) = reason {
                if let Some(proposal) = collaboration.supervisor_proposals.get_mut(&proposal_id) {
                    proposal.status = SupervisorProposalStatus::Stale;
                    if proposal.review_note.is_none() {
                        proposal.review_note = Some(format!("Proposal became stale: {reason}"));
                    }
                    changed.push(proposal.clone());
                }
            }
        }
        changed
    }

    fn work_unit_has_live_attachable_turn(state: &DaemonState, work_unit_id: &str) -> bool {
        let Some(work_unit) = state.collaboration.work_units.get(work_unit_id) else {
            return false;
        };
        let Some(assignment_id) = work_unit.current_assignment_id.as_ref() else {
            return false;
        };
        let Some(assignment) = state.collaboration.assignments.get(assignment_id) else {
            return false;
        };
        let Some(worker_session) = state
            .collaboration
            .worker_sessions
            .get(&assignment.worker_session_id)
        else {
            return false;
        };
        let Some(thread_id) = worker_session.thread_id.as_ref() else {
            return false;
        };
        let Some(turn_id) = worker_session.active_turn_id.as_ref() else {
            return false;
        };
        state
            .turns
            .get(&TurnKey::new(thread_id, turn_id))
            .is_some_and(|turn| {
                turn.attachable && matches!(turn.lifecycle, ipc::TurnLifecycleState::Active)
            })
    }

    fn select_worker_session_for_assignment(
        collaboration: &mut CollaborationState,
        worker_id: &str,
        worker_kind: String,
    ) -> String {
        collaboration
            .workers
            .entry(worker_id.to_string())
            .or_insert_with(|| Worker {
                id: worker_id.to_string(),
                kind: worker_kind,
                status: WorkerStatus::Idle,
                current_assignment_id: None,
            });

        if let Some(session_id) = collaboration
            .worker_sessions
            .values()
            .filter(|session| {
                session.worker_id == worker_id
                    && session.runtime_status != WorkerSessionRuntimeStatus::Lost
            })
            .max_by(|left, right| {
                left.updated_at
                    .cmp(&right.updated_at)
                    .then_with(|| left.id.cmp(&right.id))
            })
            .map(|session| session.id.clone())
        {
            return session_id;
        }

        let session = WorkerSession {
            id: Self::new_object_id("session"),
            worker_id: worker_id.to_string(),
            backend_type: "codex_thread".to_string(),
            thread_id: None,
            active_turn_id: None,
            runtime_status: WorkerSessionRuntimeStatus::Idle,
            attachability: WorkerSessionAttachability::Unknown,
            updated_at: Utc::now(),
        };
        let session_id = session.id.clone();
        collaboration
            .worker_sessions
            .insert(session.id.clone(), session);
        session_id
    }

    async fn worker_session_anchor_is_lost(&self, worker_session_id: &str) -> OrcasResult<bool> {
        let thread_id = self
            .state
            .read()
            .await
            .collaboration
            .worker_sessions
            .get(worker_session_id)
            .and_then(|session| session.thread_id.clone());
        let Some(thread_id) = thread_id else {
            return Ok(false);
        };
        Ok(self
            .thread_get(ipc::ThreadGetRequest { thread_id })
            .await
            .is_err())
    }

    fn full_turn_output_from_view(thread: &ipc::ThreadView, turn_id: &str) -> Option<String> {
        let turn = thread.turns.iter().find(|turn| turn.id == turn_id)?;
        let raw_output = turn
            .items
            .iter()
            .filter_map(|item| item.text.as_deref())
            .collect::<String>();
        (!raw_output.is_empty()).then_some(raw_output)
    }

    fn dependencies_satisfied(collaboration: &CollaborationState, dependencies: &[String]) -> bool {
        dependencies.iter().all(|dependency_id| {
            collaboration
                .work_units
                .get(dependency_id)
                .map(|work_unit| work_unit.status == WorkUnitStatus::Completed)
                .unwrap_or(false)
        })
    }

    fn refresh_blocked_work_units(collaboration: &mut CollaborationState) {
        let work_unit_ids = collaboration.work_units.keys().cloned().collect::<Vec<_>>();
        for work_unit_id in work_unit_ids {
            let Some(snapshot) = collaboration.work_units.get(&work_unit_id).cloned() else {
                continue;
            };
            if matches!(
                snapshot.status,
                WorkUnitStatus::Blocked | WorkUnitStatus::Ready | WorkUnitStatus::Accepted
            ) {
                let next_status =
                    if Self::dependencies_satisfied(collaboration, &snapshot.dependencies) {
                        if snapshot.status == WorkUnitStatus::Accepted {
                            WorkUnitStatus::Accepted
                        } else {
                            WorkUnitStatus::Ready
                        }
                    } else {
                        WorkUnitStatus::Blocked
                    };
                if let Some(work_unit) = collaboration.work_units.get_mut(&work_unit_id) {
                    work_unit.status = next_status;
                    work_unit.updated_at = Utc::now();
                }
            }
        }
    }

    fn refresh_workstream_statuses(collaboration: &mut CollaborationState) {
        let workstream_ids = collaboration
            .workstreams
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for workstream_id in workstream_ids {
            let units = collaboration
                .work_units
                .values()
                .filter(|work_unit| work_unit.workstream_id == workstream_id)
                .collect::<Vec<_>>();
            let status = if !units.is_empty()
                && units
                    .iter()
                    .all(|work_unit| work_unit.status == WorkUnitStatus::Completed)
            {
                WorkstreamStatus::Completed
            } else if !units.is_empty()
                && units.iter().all(|work_unit| {
                    matches!(
                        work_unit.status,
                        WorkUnitStatus::Blocked
                            | WorkUnitStatus::NeedsHuman
                            | WorkUnitStatus::Completed
                    )
                })
                && units
                    .iter()
                    .any(|work_unit| work_unit.status != WorkUnitStatus::Completed)
            {
                WorkstreamStatus::Blocked
            } else {
                WorkstreamStatus::Active
            };
            if let Some(workstream) = collaboration.workstreams.get_mut(&workstream_id) {
                workstream.status = status;
                workstream.updated_at = Utc::now();
            }
        }
    }

    async fn connect_upstream(&self) -> OrcasResult<()> {
        debug!(
            mode = ?self.config.codex.connection_mode,
            listen_url = %self.config.codex.listen_url,
            "connecting to upstream codex"
        );
        let _guard = self.connect_gate.lock().await;
        let launch = match self.config.codex.connection_mode {
            CodexConnectionMode::ConnectOnly => CodexDaemonLaunch::Never,
            CodexConnectionMode::SpawnIfNeeded => CodexDaemonLaunch::IfNeeded,
            CodexConnectionMode::SpawnAlways => CodexDaemonLaunch::Always,
        };
        self.codex_daemon.ensure_running(launch).await?;
        self.codex_client.connect().await?;
        let _ = self
            .codex_client
            .initialize(types::InitializeParams {
                client_info: types::ClientInfo {
                    name: "orcasd".to_string(),
                    title: Some("Orcas Daemon".to_string()),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
                capabilities: Some(types::InitializeCapabilities {
                    experimental_api: true,
                    opt_out_notification_methods: None,
                }),
            })
            .await?;
        if let Ok(response) = self
            .codex_client
            .thread_list(types::ThreadListParams::default())
            .await
        {
            let _ = self
                .sync_threads(&response.data, None, Some("upstream_discovered"))
                .await;
        }
        Ok(())
    }

    async fn sync_threads(
        &self,
        threads: &[types::Thread],
        model: Option<String>,
        scope: Option<&str>,
    ) -> OrcasResult<()> {
        for thread in threads {
            self.sync_thread(thread, model.clone(), scope).await?;
        }
        Ok(())
    }

    async fn sync_thread(
        &self,
        thread: &types::Thread,
        model: Option<String>,
        scope: Option<&str>,
    ) -> OrcasResult<ipc::ThreadView> {
        let existing = self.thread_from_state(&thread.id).await;
        let view = Self::thread_view_from_codex(thread.clone(), existing.as_ref(), scope);
        self.persist_thread_view(&view, model).await?;
        let mut state = self.state.write().await;
        state.recent_thread_id = Some(view.summary.id.clone());
        state.threads.insert(view.summary.id.clone(), view.clone());
        Ok(view)
    }

    async fn persist_thread_view(
        &self,
        thread: &ipc::ThreadView,
        model: Option<String>,
    ) -> OrcasResult<()> {
        let created_at = Utc
            .timestamp_opt(thread.summary.created_at, 0)
            .single()
            .unwrap_or_else(Utc::now);
        let updated_at = Utc
            .timestamp_opt(thread.summary.updated_at, 0)
            .single()
            .unwrap_or(created_at);
        self.store
            .upsert_thread(ThreadMetadata {
                id: thread.summary.id.clone(),
                name: thread.summary.name.clone(),
                preview: thread.summary.preview.clone(),
                model,
                model_provider: Some(thread.summary.model_provider.clone()),
                cwd: (!thread.summary.cwd.is_empty()).then(|| PathBuf::from(&thread.summary.cwd)),
                endpoint: Some(self.config.codex.listen_url.clone()),
                created_at,
                updated_at,
                status: thread.summary.status.clone(),
                scope: thread.summary.scope.clone(),
                archived: thread.summary.archived,
                loaded_status: thread.summary.loaded_status,
                active_flags: thread.summary.active_flags.clone(),
                active_turn_id: thread.summary.active_turn_id.clone(),
                last_seen_turn_id: thread.summary.last_seen_turn_id.clone(),
                recent_output: thread.summary.recent_output.clone(),
                recent_event: thread.summary.recent_event.clone(),
                turn_in_flight: thread.summary.turn_in_flight,
                monitor_state: thread.summary.monitor_state,
                last_sync_at: thread.summary.last_sync_at,
                source_kind: thread.summary.source_kind.clone(),
                raw_summary: thread.summary.raw_summary.clone(),
            })
            .await?;
        self.store.upsert_thread_view(thread.clone()).await
    }

    async fn persist_turn_state_view(&self, turn: &ipc::TurnStateView) -> OrcasResult<()> {
        self.store.upsert_turn_state(turn.clone()).await
    }

    async fn set_thread_monitor_state(
        &self,
        thread_id: &str,
        monitor_state: ipc::ThreadMonitorState,
    ) -> OrcasResult<()> {
        let maybe_thread = {
            let mut state = self.state.write().await;
            let Some(thread) = state.threads.get_mut(thread_id) else {
                return Ok(());
            };
            thread.summary.monitor_state = monitor_state;
            thread.summary.last_sync_at = Utc::now();
            Some(thread.clone())
        };
        if let Some(thread) = maybe_thread.as_ref() {
            self.persist_thread_view(thread, None).await?;
        }
        Ok(())
    }

    async fn thread_from_state(&self, thread_id: &str) -> Option<ipc::ThreadView> {
        self.state.read().await.threads.get(thread_id).cloned()
    }

    async fn turn_from_registry(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<ipc::TurnStateView> {
        self.state
            .read()
            .await
            .turns
            .get(&TurnKey::new(thread_id, turn_id))
            .cloned()
    }

    async fn resolve_turn_state(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> OrcasResult<Option<ipc::TurnStateView>> {
        if let Some(turn) = self.turn_from_registry(thread_id, turn_id).await {
            return Ok(Some(turn));
        }

        if let Some(thread) = self.thread_from_state(thread_id).await
            && let Some(turn) = Self::turn_state_from_thread_view(&thread, turn_id)
        {
            return Ok(Some(turn));
        }

        match self
            .thread_get(ipc::ThreadGetRequest {
                thread_id: thread_id.to_string(),
            })
            .await
        {
            Ok(response) => Ok(Self::turn_state_from_thread_view(&response.thread, turn_id)),
            Err(_) => Ok(None),
        }
    }

    async fn known_thread_summaries(&self) -> Vec<ipc::ThreadSummary> {
        let state = self.state.read().await;
        Self::sorted_thread_summaries(&state.threads)
    }

    async fn scoped_known_thread_summaries(&self) -> Vec<ipc::ThreadSummary> {
        let state = self.state.read().await;
        Self::scoped_thread_summaries(&state.threads)
    }

    async fn set_active_thread(&self, thread_id: &str) {
        let session = {
            let mut state = self.state.write().await;
            state.session.active_thread_id = Some(thread_id.to_string());
            state.recent_thread_id = Some(thread_id.to_string());
            state.session.clone()
        };
        self.emit(ipc::DaemonEvent::SessionChanged { session })
            .await;
    }

    async fn record_turn_started(&self, thread_id: &str, turn_id: &str, status: &str) {
        let (session, turn, thread, turn_state) = {
            let mut state = self.state.write().await;
            let (turn, thread_summary, turn_state) = {
                let thread = Self::ensure_thread_entry(&mut state, thread_id);
                Self::touch_thread(thread);
                thread.summary.scope = Self::prefer_scope(&thread.summary.scope, "orcas_managed");
                thread.summary.recent_event = Some(format!("turn {status}"));
                let turn = Self::upsert_turn(
                    thread,
                    ipc::TurnView {
                        id: turn_id.to_string(),
                        status: status.to_string(),
                        error_message: None,
                        error_summary: None,
                        started_at: Some(Utc::now()),
                        completed_at: None,
                        latest_diff: None,
                        latest_plan_snapshot: None,
                        token_usage_snapshot: None,
                        items: Vec::new(),
                    },
                );
                Self::refresh_thread_summary(thread);
                let recent_output =
                    Self::turn_output(&turn).or_else(|| thread.summary.recent_output.clone());
                let recent_event = Some(format!("turn {status}"));
                (
                    turn.clone(),
                    thread.summary.clone(),
                    ipc::TurnStateView {
                        thread_id: thread_id.to_string(),
                        turn_id: turn_id.to_string(),
                        lifecycle: ipc::TurnLifecycleState::Active,
                        status: status.to_string(),
                        attachable: true,
                        live_stream: true,
                        terminal: false,
                        recent_output,
                        recent_event,
                        updated_at: Utc::now(),
                        error_message: turn.error_message.clone(),
                    },
                )
            };
            Self::upsert_turn_state(&mut state, turn_state.clone());
            Self::refresh_session_from_turns(&mut state);
            state.session.active_thread_id = Some(thread_id.to_string());
            state.recent_thread_id = Some(thread_id.to_string());
            (state.session.clone(), turn, thread_summary, turn_state)
        };
        if let Some(thread_view) = self.thread_from_state(thread_id).await.as_ref() {
            let _ = self.persist_thread_view(thread_view, None).await;
        }
        let _ = self.persist_turn_state_view(&turn_state).await;
        self.emit(ipc::DaemonEvent::ThreadUpdated { thread }).await;
        self.emit(ipc::DaemonEvent::SessionChanged { session })
            .await;
        self.emit(ipc::DaemonEvent::TurnUpdated {
            thread_id: thread_id.to_string(),
            turn,
        })
        .await;
        let _ = self
            .refresh_codex_supervisor_state_for_thread(thread_id)
            .await;
    }

    async fn apply_codex_event(&self, envelope: EventEnvelope) {
        match envelope.event {
            OrcasEvent::ConnectionStateChanged(upstream) => {
                let (maybe_session, threads_to_persist, turns_to_persist) = {
                    let mut state = self.state.write().await;
                    state.upstream = upstream.clone();
                    if upstream.status != "connected" {
                        Self::mark_turns_lost(&mut state);
                    }
                    Self::refresh_session_from_turns(&mut state);
                    (
                        state.session.clone(),
                        state.threads.values().cloned().collect::<Vec<_>>(),
                        state.turns.values().cloned().collect::<Vec<_>>(),
                    )
                };
                if upstream.status != "connected" {
                    for thread in &threads_to_persist {
                        let _ = self.persist_thread_view(thread, None).await;
                    }
                    for turn in &turns_to_persist {
                        let _ = self.persist_turn_state_view(turn).await;
                    }
                }
                self.emit(ipc::DaemonEvent::UpstreamStatusChanged { upstream })
                    .await;
                self.emit(ipc::DaemonEvent::SessionChanged {
                    session: maybe_session,
                })
                .await;
            }
            OrcasEvent::ThreadStarted { thread_id, preview } => {
                let maybe_thread = self.codex_client.snapshot_thread(&thread_id).await;
                let summary = if let Some(thread) = maybe_thread {
                    let existing = self.thread_from_state(&thread_id).await;
                    let view = Self::thread_view_from_codex(
                        thread,
                        existing.as_ref(),
                        Some("live_observed"),
                    );
                    let _ = self.persist_thread_view(&view, None).await;
                    let mut state = self.state.write().await;
                    state.recent_thread_id = Some(view.summary.id.clone());
                    state.threads.insert(view.summary.id.clone(), view.clone());
                    view.summary
                } else {
                    let mut state = self.state.write().await;
                    let summary = {
                        let thread = Self::ensure_thread_entry(&mut state, &thread_id);
                        thread.summary.preview = preview;
                        thread.summary.status = "started".to_string();
                        thread.summary.scope =
                            Self::prefer_scope(&thread.summary.scope, "live_observed");
                        thread.summary.recent_event = Some("thread started".to_string());
                        Self::touch_thread(thread);
                        Self::refresh_thread_summary(thread);
                        thread.summary.clone()
                    };
                    state.recent_thread_id = Some(thread_id.clone());
                    summary
                };
                if let Some(thread_view) = self.thread_from_state(&thread_id).await.as_ref() {
                    let _ = self.persist_thread_view(thread_view, None).await;
                }
                self.emit(ipc::DaemonEvent::ThreadUpdated { thread: summary })
                    .await;
            }
            OrcasEvent::ThreadStatusChanged { thread_id, status } => {
                let summary = {
                    let mut state = self.state.write().await;
                    let summary = {
                        let thread = Self::ensure_thread_entry(&mut state, &thread_id);
                        thread.summary.status = status;
                        thread.summary.loaded_status =
                            Self::thread_loaded_status_from_label(&thread.summary.status);
                        thread.summary.recent_event =
                            Some(format!("thread {}", thread.summary.status));
                        Self::touch_thread(thread);
                        Self::refresh_thread_summary(thread);
                        thread.summary.clone()
                    };
                    state.recent_thread_id = Some(thread_id.clone());
                    summary
                };
                if let Some(thread_view) = self.thread_from_state(&thread_id).await.as_ref() {
                    let _ = self.persist_thread_view(thread_view, None).await;
                }
                self.emit(ipc::DaemonEvent::ThreadUpdated { thread: summary })
                    .await;
            }
            OrcasEvent::TurnStarted { thread_id, turn_id } => {
                self.record_turn_started(&thread_id, &turn_id, "in_progress")
                    .await;
            }
            OrcasEvent::TurnCompleted {
                thread_id,
                turn_id,
                status,
            } => {
                let (session, turn, thread, turn_state) = {
                    let mut state = self.state.write().await;
                    let (turn, thread_summary, turn_state) = {
                        let thread = Self::ensure_thread_entry(&mut state, &thread_id);
                        Self::touch_thread(thread);
                        thread.summary.recent_event = Some(format!("turn {status}"));
                        let turn = Self::upsert_turn(
                            thread,
                            ipc::TurnView {
                                id: turn_id.clone(),
                                status: status.clone(),
                                error_message: None,
                                error_summary: None,
                                started_at: None,
                                completed_at: Some(Utc::now()),
                                latest_diff: None,
                                latest_plan_snapshot: None,
                                token_usage_snapshot: None,
                                items: Vec::new(),
                            },
                        );
                        Self::refresh_thread_summary(thread);
                        let recent_output = Self::turn_output(&turn)
                            .or_else(|| thread.summary.recent_output.clone());
                        let recent_event = Some(format!("turn {status}"));
                        (
                            turn.clone(),
                            thread.summary.clone(),
                            ipc::TurnStateView {
                                thread_id: thread_id.clone(),
                                turn_id: turn_id.clone(),
                                lifecycle: Self::turn_lifecycle_from_status(&status),
                                status: status.clone(),
                                attachable: false,
                                live_stream: false,
                                terminal: true,
                                recent_output,
                                recent_event,
                                updated_at: Utc::now(),
                                error_message: turn.error_message.clone(),
                            },
                        )
                    };
                    Self::upsert_turn_state(&mut state, turn_state.clone());
                    Self::refresh_session_from_turns(&mut state);
                    if state.session.active_turns.is_empty() {
                        state.session.active_thread_id = Some(thread_id.clone());
                    }
                    state.recent_thread_id = Some(thread_id.clone());
                    (state.session.clone(), turn, thread_summary, turn_state)
                };
                if let Some(thread_view) = self.thread_from_state(&thread_id).await.as_ref() {
                    let _ = self.persist_thread_view(thread_view, None).await;
                }
                let _ = self.persist_turn_state_view(&turn_state).await;
                self.emit(ipc::DaemonEvent::ThreadUpdated { thread }).await;
                self.emit(ipc::DaemonEvent::SessionChanged { session })
                    .await;
                self.emit(ipc::DaemonEvent::TurnUpdated {
                    thread_id: thread_id.clone(),
                    turn,
                })
                .await;
                let _ = self
                    .refresh_codex_supervisor_state_for_thread(&thread_id)
                    .await;
            }
            OrcasEvent::ItemStarted {
                thread_id,
                turn_id,
                item_id,
                item_type,
            } => {
                let item = self
                    .update_item_state(
                        &thread_id,
                        &turn_id,
                        &item_id,
                        &item_type,
                        Some("started"),
                        None,
                    )
                    .await;
                if let Some(thread_view) = self.thread_from_state(&thread_id).await.as_ref() {
                    let _ = self.persist_thread_view(thread_view, None).await;
                }
                self.emit(ipc::DaemonEvent::ItemUpdated {
                    thread_id,
                    turn_id,
                    item,
                })
                .await;
            }
            OrcasEvent::ItemCompleted {
                thread_id,
                turn_id,
                item_id,
                item_type,
            } => {
                let item = self
                    .update_item_state(
                        &thread_id,
                        &turn_id,
                        &item_id,
                        &item_type,
                        Some("completed"),
                        None,
                    )
                    .await;
                if let Some(thread_view) = self.thread_from_state(&thread_id).await.as_ref() {
                    let _ = self.persist_thread_view(thread_view, None).await;
                }
                self.emit(ipc::DaemonEvent::ItemUpdated {
                    thread_id,
                    turn_id,
                    item,
                })
                .await;
            }
            OrcasEvent::AgentMessageDelta {
                thread_id,
                turn_id,
                item_id,
                delta,
            } => {
                let _ = self
                    .update_item_state(
                        &thread_id,
                        &turn_id,
                        &item_id,
                        "agent_message",
                        Some("streaming"),
                        Some(delta.clone()),
                    )
                    .await;
                self.emit(ipc::DaemonEvent::OutputDelta {
                    thread_id,
                    turn_id,
                    item_id,
                    delta,
                })
                .await;
            }
            OrcasEvent::ServerRequest { method } => {
                self.emit(ipc::DaemonEvent::Warning {
                    message: format!("server request pending: {method}"),
                })
                .await;
            }
            OrcasEvent::Warning { message } => {
                self.emit(ipc::DaemonEvent::Warning { message }).await;
            }
        }
    }

    async fn update_item_state(
        &self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        item_type: &str,
        status: Option<&str>,
        delta: Option<String>,
    ) -> ipc::ItemView {
        let mut state = self.state.write().await;
        let thread = Self::ensure_thread_entry(&mut state, thread_id);
        Self::touch_thread(thread);
        thread.summary.scope = Self::prefer_scope(&thread.summary.scope, "live_observed");
        let saw_delta = delta.is_some();
        let item = {
            let turn = Self::ensure_turn_entry(thread, turn_id);
            let item = Self::ensure_item_entry(turn, item_id, item_type);
            if let Some(item_status) = status {
                item.status = Some(item_status.to_string());
            }
            if let Some(text_delta) = delta.as_ref() {
                item.text
                    .get_or_insert_with(String::new)
                    .push_str(&text_delta);
                item.summary = item.text.as_ref().map(|text| Self::truncate_snippet(text));
            }
            item.clone()
        };
        thread.summary.recent_event = Some(match status {
            Some(item_status) => format!("{item_type} {item_status}"),
            None => format!("{item_type} updated"),
        });
        Self::refresh_thread_summary(thread);
        let (turn_status, live_output, recent_event, error_message) = {
            let current_turn = thread.turns.iter().find(|turn| turn.id == turn_id);
            (
                current_turn
                    .map(|turn| turn.status.clone())
                    .unwrap_or_else(|| "in_progress".to_string()),
                current_turn
                    .and_then(Self::turn_output)
                    .or_else(|| thread.summary.recent_output.clone()),
                thread.summary.recent_event.clone(),
                current_turn.and_then(|turn| turn.error_message.clone()),
            )
        };
        let lifecycle = Self::turn_lifecycle_from_status(&turn_status);
        let attachable = !Self::is_final_turn_lifecycle(lifecycle);
        let live_stream = saw_delta || status.is_some();
        let turn_state = ipc::TurnStateView {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            lifecycle,
            status: turn_status.clone(),
            attachable,
            live_stream,
            terminal: Self::is_terminal_status(&turn_status),
            recent_output: live_output,
            recent_event,
            updated_at: Utc::now(),
            error_message,
        };
        Self::upsert_turn_state(&mut state, turn_state.clone());
        Self::refresh_session_from_turns(&mut state);
        drop(state);
        let _ = self.persist_turn_state_view(&turn_state).await;
        item
    }

    async fn emit(&self, event: ipc::DaemonEvent) {
        let envelope = ipc::DaemonEventEnvelope::new(event);
        if let Some(summary) = Self::event_summary(&envelope) {
            let mut recent = self.recent_events.lock().await;
            if recent.len() >= RECENT_EVENT_LIMIT {
                recent.pop_front();
            }
            recent.push_back(summary);
        }
        let _ = self.event_tx.send(envelope);
    }

    fn event_summary(envelope: &ipc::DaemonEventEnvelope) -> Option<ipc::EventSummary> {
        let (kind, message, thread_id, turn_id) = match &envelope.event {
            ipc::DaemonEvent::UpstreamStatusChanged { upstream } => (
                "upstream",
                format!("upstream {} {}", upstream.endpoint, upstream.status),
                None,
                None,
            ),
            ipc::DaemonEvent::SessionChanged { session } => (
                "session",
                format!("active turns {}", session.active_turns.len()),
                session.active_thread_id.clone(),
                None,
            ),
            ipc::DaemonEvent::ThreadUpdated { thread } => (
                "thread",
                format!("thread {} {}", thread.id, thread.status),
                Some(thread.id.clone()),
                None,
            ),
            ipc::DaemonEvent::TurnUpdated { thread_id, turn } => (
                "turn",
                format!("turn {} {}", turn.id, turn.status),
                Some(thread_id.clone()),
                Some(turn.id.clone()),
            ),
            ipc::DaemonEvent::ItemUpdated {
                thread_id,
                turn_id,
                item,
            } => (
                "item",
                format!(
                    "item {} {}",
                    item.id,
                    item.status.clone().unwrap_or_else(|| "updated".to_string())
                ),
                Some(thread_id.clone()),
                Some(turn_id.clone()),
            ),
            ipc::DaemonEvent::OutputDelta {
                thread_id,
                turn_id,
                delta,
                ..
            } => (
                "delta",
                format!("delta {}", delta.replace('\n', "\\n")),
                Some(thread_id.clone()),
                Some(turn_id.clone()),
            ),
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream } => (
                "workstream",
                format!("workstream {} {:?}", workstream.id, action),
                None,
                None,
            ),
            ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit } => (
                "work_unit",
                format!("work_unit {} {:?}", work_unit.id, action),
                None,
                None,
            ),
            ipc::DaemonEvent::TrackedThreadLifecycle {
                action,
                tracked_thread,
            } => (
                "tracked_thread",
                format!("tracked_thread {} {:?}", tracked_thread.id, action),
                None,
                None,
            ),
            ipc::DaemonEvent::AssignmentLifecycle { action, assignment } => (
                "assignment",
                format!("assignment {} {:?}", assignment.id, action),
                None,
                None,
            ),
            ipc::DaemonEvent::CodexAssignmentLifecycle { action, assignment } => (
                "codex_assignment",
                format!("codex assignment {} {:?}", assignment.assignment_id, action),
                Some(assignment.codex_thread_id.clone()),
                assignment.latest_basis_turn_id.clone(),
            ),
            ipc::DaemonEvent::SupervisorDecisionLifecycle { action, decision } => (
                "supervisor_decision",
                format!("supervisor decision {} {:?}", decision.decision_id, action),
                Some(decision.codex_thread_id.clone()),
                decision.basis_turn_id.clone(),
            ),
            ipc::DaemonEvent::ReportRecorded { report } => (
                "report",
                format!("report {} {:?}", report.id, report.parse_result),
                None,
                None,
            ),
            ipc::DaemonEvent::DecisionApplied { decision } => (
                "decision",
                format!("decision {} {:?}", decision.id, decision.decision_type),
                None,
                None,
            ),
            ipc::DaemonEvent::ProposalLifecycle {
                action, proposal, ..
            } => (
                "proposal",
                format!("proposal {} {:?}", proposal.id, action),
                None,
                None,
            ),
            ipc::DaemonEvent::Warning { message } => ("warning", message.clone(), None, None),
        };

        Some(ipc::EventSummary {
            timestamp: envelope.emitted_at,
            kind: kind.to_string(),
            message,
            thread_id,
            turn_id,
        })
    }

    fn sorted_thread_summaries(
        threads: &HashMap<String, ipc::ThreadView>,
    ) -> Vec<ipc::ThreadSummary> {
        let mut summaries = threads
            .values()
            .map(|thread| thread.summary.clone())
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        summaries
    }

    fn scoped_thread_summaries(
        threads: &HashMap<String, ipc::ThreadView>,
    ) -> Vec<ipc::ThreadSummary> {
        let summaries = Self::sorted_thread_summaries(threads);
        let scoped = summaries
            .iter()
            .filter(|thread| thread.scope != "upstream_discovered" || thread.turn_in_flight)
            .cloned()
            .collect::<Vec<_>>();
        if scoped.is_empty() {
            summaries.into_iter().take(20).collect()
        } else {
            scoped
        }
    }

    fn focus_thread_view(
        state: &DaemonState,
        threads: &[ipc::ThreadSummary],
    ) -> Option<ipc::ThreadView> {
        let focus_id = state
            .session
            .active_thread_id
            .as_ref()
            .or(state.recent_thread_id.as_ref())
            .or_else(|| threads.first().map(|thread| &thread.id))?;
        state.threads.get(focus_id).cloned()
    }

    fn ensure_thread_entry<'a>(
        state: &'a mut DaemonState,
        thread_id: &str,
    ) -> &'a mut ipc::ThreadView {
        state
            .threads
            .entry(thread_id.to_string())
            .or_insert_with(|| Self::placeholder_thread_view(thread_id))
    }

    fn placeholder_thread_view(thread_id: &str) -> ipc::ThreadView {
        let now = Utc::now().timestamp();
        ipc::ThreadView {
            summary: ipc::ThreadSummary {
                id: thread_id.to_string(),
                preview: String::new(),
                name: None,
                model_provider: String::new(),
                cwd: String::new(),
                status: "pending".to_string(),
                created_at: now,
                updated_at: now,
                scope: "live_observed".to_string(),
                archived: false,
                loaded_status: ipc::ThreadLoadedStatus::Unknown,
                active_flags: Vec::new(),
                active_turn_id: None,
                last_seen_turn_id: None,
                recent_output: None,
                recent_event: None,
                turn_in_flight: false,
                monitor_state: ipc::ThreadMonitorState::Detached,
                last_sync_at: Utc::now(),
                source_kind: None,
                raw_summary: None,
            },
            history_loaded: false,
            turns: Vec::new(),
        }
    }

    fn thread_view_from_metadata(metadata: &ThreadMetadata) -> ipc::ThreadView {
        ipc::ThreadView {
            summary: ipc::ThreadSummary {
                id: metadata.id.clone(),
                preview: metadata.preview.clone(),
                name: metadata.name.clone(),
                model_provider: metadata.model_provider.clone().unwrap_or_default(),
                cwd: metadata
                    .cwd
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                status: metadata.status.clone(),
                created_at: metadata.created_at.timestamp(),
                updated_at: metadata.updated_at.timestamp(),
                scope: if metadata.scope.is_empty() {
                    "orcas_managed".to_string()
                } else {
                    metadata.scope.clone()
                },
                archived: metadata.archived,
                loaded_status: metadata.loaded_status,
                active_flags: metadata.active_flags.clone(),
                active_turn_id: metadata.active_turn_id.clone(),
                last_seen_turn_id: metadata.last_seen_turn_id.clone(),
                recent_output: metadata.recent_output.clone(),
                recent_event: metadata.recent_event.clone(),
                turn_in_flight: metadata.turn_in_flight,
                monitor_state: metadata.monitor_state,
                last_sync_at: metadata.last_sync_at,
                source_kind: metadata.source_kind.clone(),
                raw_summary: metadata.raw_summary.clone(),
            },
            history_loaded: false,
            turns: Vec::new(),
        }
    }

    fn thread_view_from_codex(
        thread: types::Thread,
        existing: Option<&ipc::ThreadView>,
        scope: Option<&str>,
    ) -> ipc::ThreadView {
        let loaded_status = Self::thread_loaded_status_from_codex(&thread.status);
        let active_flags = Self::thread_active_flags(&thread.status);
        let source_kind = Self::thread_source_kind(thread.source.as_ref());
        let raw_summary = serde_json::to_value(&thread).ok();
        let mut view = ipc::ThreadView {
            summary: ipc::ThreadSummary {
                id: thread.id,
                preview: thread.preview,
                name: thread.name,
                model_provider: thread.model_provider,
                cwd: thread.cwd,
                status: thread.status.label().to_string(),
                created_at: thread.created_at,
                updated_at: thread.updated_at,
                scope: scope
                    .map(ToOwned::to_owned)
                    .or_else(|| existing.map(|thread| thread.summary.scope.clone()))
                    .filter(|scope| !scope.is_empty())
                    .unwrap_or_else(|| "upstream_discovered".to_string()),
                archived: thread
                    .extra
                    .get("archived")
                    .and_then(Value::as_bool)
                    .or_else(|| existing.map(|thread| thread.summary.archived))
                    .unwrap_or(false),
                loaded_status,
                active_flags,
                active_turn_id: existing.and_then(|thread| thread.summary.active_turn_id.clone()),
                last_seen_turn_id: existing
                    .and_then(|thread| thread.summary.last_seen_turn_id.clone()),
                recent_output: existing.and_then(|thread| thread.summary.recent_output.clone()),
                recent_event: existing.and_then(|thread| thread.summary.recent_event.clone()),
                turn_in_flight: existing
                    .map(|thread| thread.summary.turn_in_flight)
                    .unwrap_or(false),
                monitor_state: existing
                    .map(|thread| thread.summary.monitor_state)
                    .unwrap_or(ipc::ThreadMonitorState::Detached),
                last_sync_at: Utc::now(),
                source_kind: source_kind
                    .or_else(|| existing.and_then(|thread| thread.summary.source_kind.clone())),
                raw_summary: raw_summary
                    .or_else(|| existing.and_then(|thread| thread.summary.raw_summary.clone())),
            },
            history_loaded: !thread.turns.is_empty()
                || existing
                    .map(|thread| thread.history_loaded)
                    .unwrap_or(false),
            turns: if thread.turns.is_empty() {
                existing
                    .map(|thread| thread.turns.clone())
                    .unwrap_or_default()
            } else {
                thread
                    .turns
                    .into_iter()
                    .map(Self::turn_view_from_codex)
                    .collect()
            },
        };
        Self::refresh_thread_summary(&mut view);
        view
    }

    fn turn_view_from_codex(turn: types::Turn) -> ipc::TurnView {
        let error_summary = turn.error.as_ref().map(|error| {
            error
                .additional_details
                .as_ref()
                .map(|details| format!("{} ({details})", error.message))
                .unwrap_or_else(|| error.message.clone())
        });
        let items = turn
            .items
            .into_iter()
            .map(Self::item_view_from_codex)
            .collect::<Vec<_>>();
        ipc::TurnView {
            id: turn.id,
            status: turn.status.label().to_string(),
            error_message: turn.error.map(|error| error.message),
            error_summary,
            started_at: None,
            completed_at: None,
            latest_diff: Self::latest_diff_from_items(&items),
            latest_plan_snapshot: Self::latest_plan_snapshot_from_items(&items),
            token_usage_snapshot: Self::token_usage_snapshot_from_items(&items),
            items,
        }
    }

    fn item_view_from_codex(item: types::ThreadItem) -> ipc::ItemView {
        let text = item.text().map(ToOwned::to_owned);
        let payload = (!item.extra.is_empty()).then_some(Value::Object(item.extra));
        let summary = text
            .as_ref()
            .map(|text| Self::truncate_snippet(text))
            .or_else(|| payload.as_ref().map(Self::payload_summary));
        ipc::ItemView {
            id: item.id,
            item_type: item.item_type,
            status: None,
            text,
            summary,
            payload,
        }
    }

    fn touch_thread(thread: &mut ipc::ThreadView) {
        thread.summary.updated_at = Utc::now().timestamp();
        thread.summary.last_sync_at = Utc::now();
    }

    fn refresh_thread_summary(thread: &mut ipc::ThreadView) {
        thread.summary.last_seen_turn_id = thread.turns.last().map(|turn| turn.id.clone());
        thread.summary.active_turn_id = thread
            .turns
            .iter()
            .rev()
            .find(|turn| !Self::is_terminal_status(&turn.status))
            .map(|turn| turn.id.clone());
        thread.summary.turn_in_flight = thread.summary.active_turn_id.is_some();
        if thread.summary.turn_in_flight {
            thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
        } else if thread.summary.loaded_status == ipc::ThreadLoadedStatus::Unknown {
            thread.summary.loaded_status =
                Self::thread_loaded_status_from_label(&thread.summary.status);
        }
        if let Some(turn) = thread
            .turns
            .iter()
            .rev()
            .find(|turn| !Self::is_terminal_status(&turn.status))
        {
            thread.summary.active_flags = vec![format!("turn:{}", turn.id)];
        }
        if let Some(output) = thread
            .turns
            .iter()
            .rev()
            .flat_map(|turn| turn.items.iter().rev())
            .find_map(|item| item.text.as_deref().or(item.summary.as_deref()))
            .filter(|text| !text.trim().is_empty())
        {
            thread.summary.recent_output = Some(Self::truncate_snippet(output));
        }
        if thread.summary.recent_event.is_none() {
            thread.summary.recent_event = Some(if thread.summary.turn_in_flight {
                "turn in progress".to_string()
            } else {
                format!("thread {}", thread.summary.status)
            });
        }
    }

    fn turn_output(turn: &ipc::TurnView) -> Option<String> {
        let text = turn
            .items
            .iter()
            .filter_map(|item| item.text.as_deref().or(item.summary.as_deref()))
            .collect::<String>();
        (!text.is_empty()).then_some(Self::truncate_snippet(&text))
    }

    fn payload_summary(payload: &Value) -> String {
        match payload {
            Value::Object(map) if map.is_empty() => "empty payload".to_string(),
            Value::Object(map) => {
                let keys = map.keys().take(4).cloned().collect::<Vec<_>>().join(", ");
                format!("payload keys: {keys}")
            }
            Value::Array(values) => format!("payload array ({} items)", values.len()),
            other => Self::truncate_snippet(&other.to_string()),
        }
    }

    fn latest_diff_from_items(items: &[ipc::ItemView]) -> Option<String> {
        items.iter().rev().find_map(|item| {
            (item.item_type.contains("diff") || item.item_type.contains("patch"))
                .then(|| item.text.clone().or(item.summary.clone()))
                .flatten()
        })
    }

    fn latest_plan_snapshot_from_items(items: &[ipc::ItemView]) -> Option<Value> {
        items.iter().rev().find_map(|item| {
            item.item_type
                .contains("plan")
                .then(|| item.payload.clone())
                .flatten()
        })
    }

    fn token_usage_snapshot_from_items(items: &[ipc::ItemView]) -> Option<Value> {
        items.iter().rev().find_map(|item| {
            item.payload
                .as_ref()
                .and_then(|payload| payload.get("usage"))
                .cloned()
        })
    }

    fn truncate_snippet(text: &str) -> String {
        let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut snippet = single_line.chars().take(160).collect::<String>();
        if single_line.chars().count() > 160 {
            snippet.push_str("...");
        }
        snippet
    }

    fn prefer_scope(current: &str, fallback: &str) -> String {
        if current.is_empty() || current == "upstream_discovered" {
            fallback.to_string()
        } else {
            current.to_string()
        }
    }

    fn thread_loaded_status_from_codex(status: &types::ThreadStatus) -> ipc::ThreadLoadedStatus {
        match status {
            types::ThreadStatus::NotLoaded => ipc::ThreadLoadedStatus::NotLoaded,
            types::ThreadStatus::Idle => ipc::ThreadLoadedStatus::Idle,
            types::ThreadStatus::SystemError => ipc::ThreadLoadedStatus::SystemError,
            types::ThreadStatus::Active { .. } => ipc::ThreadLoadedStatus::Active,
        }
    }

    fn thread_loaded_status_from_label(label: &str) -> ipc::ThreadLoadedStatus {
        match label {
            "notLoaded" | "not_loaded" => ipc::ThreadLoadedStatus::NotLoaded,
            "idle" => ipc::ThreadLoadedStatus::Idle,
            "systemError" | "system_error" => ipc::ThreadLoadedStatus::SystemError,
            "active" => ipc::ThreadLoadedStatus::Active,
            _ => ipc::ThreadLoadedStatus::Unknown,
        }
    }

    fn thread_active_flags(status: &types::ThreadStatus) -> Vec<String> {
        match status {
            types::ThreadStatus::Active { active_flags } => active_flags.clone(),
            _ => Vec::new(),
        }
    }

    fn thread_source_kind(source: Option<&Value>) -> Option<String> {
        source.and_then(|value| {
            value
                .get("kind")
                .and_then(Value::as_str)
                .or_else(|| value.get("type").and_then(Value::as_str))
                .map(ToOwned::to_owned)
        })
    }

    fn turn_lifecycle_from_status(status: &str) -> ipc::TurnLifecycleState {
        match status {
            "completed" => ipc::TurnLifecycleState::Completed,
            "failed" => ipc::TurnLifecycleState::Failed,
            "cancelled" | "interrupted" => ipc::TurnLifecycleState::Interrupted,
            "lost" => ipc::TurnLifecycleState::Lost,
            "unknown" => ipc::TurnLifecycleState::Unknown,
            _ => ipc::TurnLifecycleState::Active,
        }
    }

    fn is_final_turn_lifecycle(lifecycle: ipc::TurnLifecycleState) -> bool {
        !matches!(lifecycle, ipc::TurnLifecycleState::Active)
    }

    fn is_terminal_status(status: &str) -> bool {
        matches!(
            status,
            "completed" | "failed" | "cancelled" | "interrupted" | "lost" | "unknown"
        )
    }

    fn upsert_turn(thread: &mut ipc::ThreadView, turn: ipc::TurnView) -> ipc::TurnView {
        if let Some(existing) = thread
            .turns
            .iter_mut()
            .find(|existing| existing.id == turn.id)
        {
            if !turn.status.is_empty() {
                existing.status = turn.status;
            }
            if turn.error_message.is_some() {
                existing.error_message = turn.error_message;
            }
            if turn.error_summary.is_some() {
                existing.error_summary = turn.error_summary;
            }
            if turn.started_at.is_some() {
                existing.started_at = turn.started_at;
            }
            if turn.completed_at.is_some() {
                existing.completed_at = turn.completed_at;
            }
            if turn.latest_diff.is_some() {
                existing.latest_diff = turn.latest_diff;
            }
            if turn.latest_plan_snapshot.is_some() {
                existing.latest_plan_snapshot = turn.latest_plan_snapshot;
            }
            if turn.token_usage_snapshot.is_some() {
                existing.token_usage_snapshot = turn.token_usage_snapshot;
            }
            for item in turn.items {
                let _ = Self::upsert_item(existing, item);
            }
            return existing.clone();
        }
        thread.turns.push(turn.clone());
        turn
    }

    fn upsert_item(turn: &mut ipc::TurnView, item: ipc::ItemView) -> ipc::ItemView {
        if let Some(existing) = turn
            .items
            .iter_mut()
            .find(|existing| existing.id == item.id)
        {
            if let Some(status) = item.status {
                existing.status = Some(status);
            }
            if let Some(text) = item.text {
                existing.text = Some(text);
            }
            if let Some(summary) = item.summary {
                existing.summary = Some(summary);
            }
            if let Some(payload) = item.payload {
                existing.payload = Some(payload);
            }
            if !item.item_type.is_empty() {
                existing.item_type = item.item_type;
            }
            return existing.clone();
        }
        turn.items.push(item.clone());
        item
    }

    fn ensure_turn_entry<'a>(
        thread: &'a mut ipc::ThreadView,
        turn_id: &str,
    ) -> &'a mut ipc::TurnView {
        if let Some(index) = thread.turns.iter().position(|turn| turn.id == turn_id) {
            return &mut thread.turns[index];
        }
        thread.turns.push(ipc::TurnView {
            id: turn_id.to_string(),
            status: "in_progress".to_string(),
            error_message: None,
            error_summary: None,
            started_at: None,
            completed_at: None,
            latest_diff: None,
            latest_plan_snapshot: None,
            token_usage_snapshot: None,
            items: Vec::new(),
        });
        let index = thread.turns.len() - 1;
        &mut thread.turns[index]
    }

    fn ensure_item_entry<'a>(
        turn: &'a mut ipc::TurnView,
        item_id: &str,
        item_type: &str,
    ) -> &'a mut ipc::ItemView {
        if let Some(index) = turn.items.iter().position(|item| item.id == item_id) {
            return &mut turn.items[index];
        }
        turn.items.push(ipc::ItemView {
            id: item_id.to_string(),
            item_type: item_type.to_string(),
            status: None,
            text: None,
            summary: None,
            payload: None,
        });
        let index = turn.items.len() - 1;
        &mut turn.items[index]
    }

    fn upsert_turn_state(state: &mut DaemonState, turn: ipc::TurnStateView) {
        state
            .turns
            .insert(TurnKey::new(&turn.thread_id, &turn.turn_id), turn);
    }

    fn refresh_session_from_turns(state: &mut DaemonState) {
        let mut active_turns = state
            .turns
            .values()
            .filter(|turn| {
                turn.attachable && matches!(turn.lifecycle, ipc::TurnLifecycleState::Active)
            })
            .map(|turn| ipc::ActiveTurn {
                thread_id: turn.thread_id.clone(),
                turn_id: turn.turn_id.clone(),
                status: turn.status.clone(),
                updated_at: turn.updated_at,
            })
            .collect::<Vec<_>>();
        active_turns.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.turn_id.cmp(&right.turn_id))
        });
        state.session.active_turns = active_turns;
    }

    fn mark_turns_lost(state: &mut DaemonState) {
        let lost_message = "daemon lost turn continuity".to_string();
        for turn in state.turns.values_mut() {
            if turn.attachable && matches!(turn.lifecycle, ipc::TurnLifecycleState::Active) {
                turn.lifecycle = ipc::TurnLifecycleState::Lost;
                turn.status = "lost".to_string();
                turn.attachable = false;
                turn.live_stream = false;
                turn.terminal = true;
                turn.recent_event = Some(lost_message.clone());
                turn.updated_at = Utc::now();
            }
        }

        for thread in state.threads.values_mut() {
            for turn in &mut thread.turns {
                if turn.status == "submitted"
                    || turn.status == "started"
                    || turn.status == "in_progress"
                {
                    turn.status = "lost".to_string();
                    if turn.error_message.is_none() {
                        turn.error_message = Some(lost_message.clone());
                    }
                }
            }
            if thread.summary.turn_in_flight {
                thread.summary.recent_event = Some(lost_message.clone());
                Self::refresh_thread_summary(thread);
            }
        }
    }

    fn turn_state_from_thread_view(
        thread: &ipc::ThreadView,
        turn_id: &str,
    ) -> Option<ipc::TurnStateView> {
        let turn = thread.turns.iter().find(|turn| turn.id == turn_id)?;
        let lifecycle = if Self::is_terminal_status(&turn.status) {
            Self::turn_lifecycle_from_status(&turn.status)
        } else {
            ipc::TurnLifecycleState::Unknown
        };
        Some(ipc::TurnStateView {
            thread_id: thread.summary.id.clone(),
            turn_id: turn.id.clone(),
            lifecycle,
            status: turn.status.clone(),
            attachable: false,
            live_stream: false,
            terminal: Self::is_terminal_status(&turn.status),
            recent_output: Self::turn_output(turn).or_else(|| thread.summary.recent_output.clone()),
            recent_event: thread
                .summary
                .recent_event
                .clone()
                .or_else(|| Some(format!("turn {}", turn.status))),
            updated_at: Utc
                .timestamp_opt(thread.summary.updated_at, 0)
                .single()
                .unwrap_or_else(Utc::now),
            error_message: turn.error_message.clone(),
        })
    }

    fn turn_attach_failure_reason(turn: &ipc::TurnStateView) -> String {
        match turn.lifecycle {
            ipc::TurnLifecycleState::Completed => {
                "turn already completed; only terminal state is queryable".to_string()
            }
            ipc::TurnLifecycleState::Failed => {
                "turn already failed; only terminal state is queryable".to_string()
            }
            ipc::TurnLifecycleState::Interrupted => {
                "turn was interrupted; live attachment is no longer available".to_string()
            }
            ipc::TurnLifecycleState::Lost => {
                "turn continuity was lost when Orcas lost daemon/upstream ownership".to_string()
            }
            ipc::TurnLifecycleState::Unknown => {
                "turn exists only as cached/query state; live attachment cannot be proven"
                    .to_string()
            }
            ipc::TurnLifecycleState::Active => {
                "turn is active but not attachable in this daemon instance".to_string()
            }
        }
    }

    fn overrides_from_env() -> OrcasRuntimeOverrides {
        OrcasRuntimeOverrides::from_env()
    }

    fn decode_params<T>(params: Option<Value>) -> OrcasResult<T>
    where
        T: DeserializeOwned,
    {
        serde_json::from_value(params.unwrap_or_else(|| serde_json::json!({}))).map_err(Into::into)
    }

    async fn send_response(
        outbound: &mpsc::Sender<String>,
        id: orcas_core::RequestId,
        result: Value,
    ) -> OrcasResult<()> {
        let raw = serde_json::to_string(&JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result,
        })?;
        outbound
            .send(raw)
            .await
            .map_err(|error| OrcasError::Transport(format!("failed to send IPC response: {error}")))
    }

    async fn send_error(
        outbound: &mpsc::Sender<String>,
        id: Option<orcas_core::RequestId>,
        code: i64,
        message: &str,
        data: Option<Value>,
    ) -> OrcasResult<()> {
        let Some(id) = id else {
            return Ok(());
        };
        let raw = serde_json::to_string(&JsonRpcError {
            jsonrpc: "2.0".to_string(),
            id,
            error: JsonRpcErrorObject {
                code,
                message: message.to_string(),
                data,
            },
        })?;
        outbound
            .send(raw)
            .await
            .map_err(|error| OrcasError::Transport(format!("failed to send IPC error: {error}")))
    }
}

struct SocketGuard {
    path: PathBuf,
    metadata_path: PathBuf,
}

impl SocketGuard {
    fn new(path: PathBuf, metadata_path: PathBuf) -> Self {
        Self {
            path,
            metadata_path,
        }
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(&self.metadata_path);
    }
}

struct ClientGuard {
    service: Arc<OrcasDaemonService>,
}

impl ClientGuard {
    fn new(service: Arc<OrcasDaemonService>) -> Self {
        Self { service }
    }
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.service.client_count.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use serde_json::{Map, Value};
    use tokio::sync::{Mutex, Notify, RwLock, broadcast};
    use tokio::time::Duration;
    use uuid::Uuid;

    use super::OrcasDaemonService;
    use super::{DaemonState, TurnKey};
    use crate::assignment_comm::parse::{parse_worker_report, parse_worker_report_for_turn};
    use crate::assignment_comm::render::{build_assignment_communication_record, render_prompt};
    use crate::authority_store::AuthoritySqliteStore;
    use crate::supervisor::{
        SupervisorReasoner, SupervisorReasonerFailure, SupervisorReasonerResult,
        render_supervisor_prompt, render_supervisor_response_artifact,
    };
    use orcas_codex::{
        CodexClient, CodexDaemonManager, CodexTransport, DaemonLaunch, DaemonStatus,
        LocalCodexDaemonManager, RejectingApprovalRouter, WebSocketTransport, methods,
        protocol::jsonrpc as codex_jsonrpc, transport::TransportConnection, types,
    };
    use orcas_core::{
        AppConfig, AppPaths, Assignment, AssignmentCommunicationSeed, AssignmentModeSpec,
        AssignmentStatus, CodexThreadAssignmentStatus, CodexThreadBootstrapState,
        CollaborationState, DecisionType, DraftAssignment, EventEnvelope, ImplementModeSpec,
        JsonSessionStore, OrcasError, OrcasEvent, OrcasResult, OrcasSessionStore, ProposedDecision,
        Report, ReportConfidence, ReportDisposition, ReportParseResult, SupervisorContextPack,
        SupervisorProposal, SupervisorProposalEdits, SupervisorProposalFailureStage,
        SupervisorProposalStatus, SupervisorProposalTriggerKind, SupervisorSummary,
        SupervisorTurnDecision, SupervisorTurnDecisionKind, SupervisorTurnDecisionStatus,
        SupervisorTurnProposalKind, WorkUnit, WorkUnitStatus, WorkerSessionAttachability,
        WorkerSessionRuntimeStatus, WorkerStatus, Workstream, WorkstreamStatus, ipc,
    };

    #[derive(Debug)]
    struct FakeCodexDaemonManager {
        endpoint: String,
    }

    #[async_trait]
    impl CodexDaemonManager for FakeCodexDaemonManager {
        async fn status(&self) -> OrcasResult<DaemonStatus> {
            Ok(DaemonStatus {
                endpoint: self.endpoint.clone(),
                reachable: true,
                binary_path: PathBuf::from("fake-codex"),
                log_path: PathBuf::from("/tmp/fake-codex.log"),
            })
        }

        async fn ensure_running(&self, _launch: DaemonLaunch) -> OrcasResult<DaemonStatus> {
            self.status().await
        }

        async fn spawn_background(&self) -> OrcasResult<DaemonStatus> {
            self.status().await
        }
    }

    #[derive(Debug, Default)]
    struct FakeCodexRuntimeState {
        next_thread_id: usize,
        next_turn_id: usize,
        threads: HashMap<String, types::Thread>,
        last_turn_start_text: Option<String>,
        last_turn_steer_thread_id: Option<String>,
        last_turn_steer_expected_turn_id: Option<String>,
        last_turn_steer_text: Option<String>,
        last_turn_interrupt_thread_id: Option<String>,
        last_turn_interrupt_turn_id: Option<String>,
    }

    #[derive(Debug, Clone, Copy)]
    enum FakeCodexTerminalOutcome {
        Completed,
        Interrupted,
    }

    impl FakeCodexTerminalOutcome {
        fn turn_status(self) -> types::TurnStatus {
            match self {
                Self::Completed => types::TurnStatus::Completed,
                Self::Interrupted => types::TurnStatus::Interrupted,
            }
        }

        fn turn_error(self) -> Option<types::TurnError> {
            match self {
                Self::Completed => None,
                Self::Interrupted => Some(types::TurnError {
                    message: "interrupted".to_string(),
                    additional_details: None,
                    codex_error_info: None,
                }),
            }
        }
    }

    #[derive(Debug)]
    struct FakeCodexTransport {
        endpoint: String,
        turn_output: String,
        terminal_outcome: FakeCodexTerminalOutcome,
        state: Arc<Mutex<FakeCodexRuntimeState>>,
    }

    impl FakeCodexTransport {
        fn new(
            endpoint: impl Into<String>,
            turn_output: impl Into<String>,
            terminal_outcome: FakeCodexTerminalOutcome,
        ) -> Self {
            Self {
                endpoint: endpoint.into(),
                turn_output: turn_output.into(),
                terminal_outcome,
                state: Arc::new(Mutex::new(FakeCodexRuntimeState::default())),
            }
        }

        async fn handle_request(
            state: Arc<Mutex<FakeCodexRuntimeState>>,
            turn_output: String,
            terminal_outcome: FakeCodexTerminalOutcome,
            inbound_tx: tokio::sync::mpsc::Sender<String>,
            request: codex_jsonrpc::JsonRpcRequest,
        ) -> OrcasResult<()> {
            match request.method.as_str() {
                methods::INITIALIZE => {
                    Self::send_response(
                        &inbound_tx,
                        request.id,
                        &types::InitializeResponse::default(),
                    )
                    .await?;
                }
                methods::THREAD_LIST => {
                    let threads = state.lock().await.threads.values().cloned().collect();
                    Self::send_response(
                        &inbound_tx,
                        request.id,
                        &types::ThreadListResponse {
                            data: threads,
                            next_cursor: None,
                        },
                    )
                    .await?;
                }
                methods::THREAD_START => {
                    let params: types::ThreadStartParams =
                        serde_json::from_value(request.params.unwrap_or(Value::Null))?;
                    let thread = {
                        let mut state = state.lock().await;
                        state.next_thread_id += 1;
                        let thread = types::Thread {
                            id: format!("thread-fake-{}", state.next_thread_id),
                            preview: String::new(),
                            ephemeral: params.ephemeral.unwrap_or(false),
                            model_provider: "openai".to_string(),
                            created_at: Utc::now().timestamp(),
                            updated_at: Utc::now().timestamp(),
                            status: types::ThreadStatus::Idle,
                            path: None,
                            cwd: params.cwd.unwrap_or_default(),
                            cli_version: "test".to_string(),
                            source: None,
                            name: None,
                            turns: Vec::new(),
                            extra: Map::new(),
                        };
                        state.threads.insert(thread.id.clone(), thread.clone());
                        thread
                    };
                    let model = params.model.unwrap_or_else(|| "gpt-5.4".to_string());
                    Self::send_response(
                        &inbound_tx,
                        request.id,
                        &types::ThreadStartResponse {
                            thread: thread.clone(),
                            model,
                            model_provider: "openai".to_string(),
                            cwd: thread.cwd.clone(),
                        },
                    )
                    .await?;
                }
                methods::THREAD_READ => {
                    let params: types::ThreadReadParams =
                        serde_json::from_value(request.params.unwrap_or(Value::Null))?;
                    let thread = state
                        .lock()
                        .await
                        .threads
                        .get(&params.thread_id)
                        .cloned()
                        .ok_or_else(|| {
                            OrcasError::Protocol(format!(
                                "fake codex runtime missing thread `{}`",
                                params.thread_id
                            ))
                        })?;
                    Self::send_response(
                        &inbound_tx,
                        request.id,
                        &types::ThreadReadResponse { thread },
                    )
                    .await?;
                }
                methods::THREAD_RESUME => {
                    let params: types::ThreadResumeParams =
                        serde_json::from_value(request.params.unwrap_or(Value::Null))?;
                    let (thread, cwd) = {
                        let mut state = state.lock().await;
                        let thread = state.threads.get_mut(&params.thread_id).ok_or_else(|| {
                            OrcasError::Protocol(format!(
                                "fake codex runtime missing thread `{}`",
                                params.thread_id
                            ))
                        })?;
                        if let Some(cwd) = params.cwd.clone() {
                            thread.cwd = cwd;
                        }
                        thread.updated_at = Utc::now().timestamp();
                        (thread.clone(), thread.cwd.clone())
                    };
                    Self::send_response(
                        &inbound_tx,
                        request.id,
                        &types::ThreadResumeResponse {
                            thread,
                            model: params.model.unwrap_or_else(|| "gpt-5.4".to_string()),
                            model_provider: "openai".to_string(),
                            cwd,
                        },
                    )
                    .await?;
                }
                methods::TURN_START => {
                    let params: types::TurnStartParams =
                        serde_json::from_value(request.params.unwrap_or(Value::Null))?;
                    let turn_prompt = Self::turn_start_text(&params);
                    let rendered_output =
                        Self::substitute_prompt_placeholders(&turn_output, turn_prompt.as_deref());
                    let turn = {
                        let mut state = state.lock().await;
                        state.next_turn_id += 1;
                        state.last_turn_start_text = turn_prompt.clone();
                        let turn = types::Turn {
                            id: format!("turn-fake-{}", state.next_turn_id),
                            items: Vec::new(),
                            status: types::TurnStatus::InProgress,
                            error: None,
                        };
                        let thread = state.threads.get_mut(&params.thread_id).ok_or_else(|| {
                            OrcasError::Protocol(format!(
                                "fake codex runtime missing thread `{}`",
                                params.thread_id
                            ))
                        })?;
                        thread.status = types::ThreadStatus::Active {
                            active_flags: vec!["turn_running".to_string()],
                        };
                        thread.updated_at = Utc::now().timestamp();
                        thread.turns.push(turn.clone());
                        turn
                    };
                    Self::send_response(
                        &inbound_tx,
                        request.id,
                        &types::TurnStartResponse { turn: turn.clone() },
                    )
                    .await?;

                    let state = Arc::clone(&state);
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        let item_id = format!("item-{}", turn.id);
                        let _ = Self::send_notification(
                            &inbound_tx,
                            methods::TURN_STARTED,
                            &types::TurnStartedNotification {
                                thread_id: params.thread_id.clone(),
                                turn: turn.clone(),
                            },
                        )
                        .await;
                        let _ = Self::send_notification(
                            &inbound_tx,
                            methods::ITEM_STARTED,
                            &types::ItemStartedNotification {
                                item: types::ThreadItem {
                                    id: item_id.clone(),
                                    item_type: "agent_message".to_string(),
                                    extra: Map::new(),
                                },
                                thread_id: params.thread_id.clone(),
                                turn_id: turn.id.clone(),
                            },
                        )
                        .await;
                        let _ = Self::send_notification(
                            &inbound_tx,
                            methods::AGENT_MESSAGE_DELTA,
                            &types::AgentMessageDeltaNotification {
                                thread_id: params.thread_id.clone(),
                                turn_id: turn.id.clone(),
                                item_id: item_id.clone(),
                                delta: rendered_output.clone(),
                            },
                        )
                        .await;
                        let _ = Self::send_notification(
                            &inbound_tx,
                            methods::ITEM_COMPLETED,
                            &types::ItemCompletedNotification {
                                item: types::ThreadItem {
                                    id: item_id.clone(),
                                    item_type: "agent_message".to_string(),
                                    extra: Map::new(),
                                },
                                thread_id: params.thread_id.clone(),
                                turn_id: turn.id.clone(),
                            },
                        )
                        .await;

                        let completed_turn = {
                            let mut state = state.lock().await;
                            let thread = match state.threads.get_mut(&params.thread_id) {
                                Some(thread) => thread,
                                None => return,
                            };
                            let completed_item = {
                                let mut extra = Map::new();
                                extra.insert(
                                    "text".to_string(),
                                    Value::String(rendered_output.clone()),
                                );
                                types::ThreadItem {
                                    id: item_id,
                                    item_type: "agent_message".to_string(),
                                    extra,
                                }
                            };
                            let completed_turn = types::Turn {
                                id: turn.id.clone(),
                                items: vec![completed_item],
                                status: terminal_outcome.turn_status(),
                                error: terminal_outcome.turn_error(),
                            };
                            if let Some(existing_turn) = thread
                                .turns
                                .iter_mut()
                                .find(|existing| existing.id == turn.id)
                            {
                                *existing_turn = completed_turn.clone();
                            } else {
                                thread.turns.push(completed_turn.clone());
                            }
                            thread.status = types::ThreadStatus::Idle;
                            thread.updated_at = Utc::now().timestamp();
                            completed_turn
                        };

                        let _ = Self::send_notification(
                            &inbound_tx,
                            methods::TURN_COMPLETED,
                            &types::TurnCompletedNotification {
                                thread_id: params.thread_id.clone(),
                                turn: completed_turn,
                            },
                        )
                        .await;
                    });
                }
                methods::TURN_STEER => {
                    let params: types::TurnSteerParams =
                        serde_json::from_value(request.params.unwrap_or(Value::Null))?;
                    let steer_text = params.input.iter().find_map(|input| match input {
                        types::UserInput::Text { text, .. } => Some(text.clone()),
                    });
                    let active_turn_id = {
                        let mut state = state.lock().await;
                        let thread = state.threads.get(&params.thread_id).ok_or_else(|| {
                            OrcasError::Protocol(format!(
                                "fake codex runtime missing thread `{}`",
                                params.thread_id
                            ))
                        })?;
                        let active_turn_id = match &thread.status {
                            types::ThreadStatus::Active { .. } => {
                                thread.turns.last().map(|turn| turn.id.clone())
                            }
                            _ => None,
                        };
                        if active_turn_id.is_none() {
                            Self::send_rpc_error(
                                &inbound_tx,
                                request.id,
                                -32600,
                                "no active turn to steer",
                            )
                            .await?;
                            return Ok(());
                        }
                        if active_turn_id.as_deref() != Some(params.expected_turn_id.as_str()) {
                            Self::send_rpc_error(
                                &inbound_tx,
                                request.id,
                                -32600,
                                "no active turn to steer",
                            )
                            .await?;
                            return Ok(());
                        }
                        state.last_turn_steer_thread_id = Some(params.thread_id.clone());
                        state.last_turn_steer_expected_turn_id =
                            Some(params.expected_turn_id.clone());
                        state.last_turn_steer_text = steer_text;
                        active_turn_id.expect("checked above")
                    };
                    Self::send_response(
                        &inbound_tx,
                        request.id,
                        &types::TurnSteerResponse {
                            turn_id: active_turn_id,
                        },
                    )
                    .await?;
                }
                methods::TURN_INTERRUPT => {
                    let params: types::TurnInterruptParams =
                        serde_json::from_value(request.params.unwrap_or(Value::Null))?;
                    {
                        let mut state = state.lock().await;
                        state.last_turn_interrupt_thread_id = Some(params.thread_id.clone());
                        state.last_turn_interrupt_turn_id = Some(params.turn_id.clone());
                    }
                    Self::send_response(&inbound_tx, request.id, &types::TurnInterruptResponse {})
                        .await?;
                    let state = Arc::clone(&state);
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        let completed_turn = {
                            let mut state = state.lock().await;
                            let Some(thread) = state.threads.get_mut(&params.thread_id) else {
                                return;
                            };
                            if let Some(turn) = thread
                                .turns
                                .iter_mut()
                                .find(|turn| turn.id == params.turn_id)
                            {
                                turn.status = types::TurnStatus::Interrupted;
                                turn.error = Some(types::TurnError {
                                    message: "interrupted".to_string(),
                                    additional_details: None,
                                    codex_error_info: None,
                                });
                            }
                            let completed_turn = thread
                                .turns
                                .iter()
                                .find(|turn| turn.id == params.turn_id)
                                .cloned()
                                .unwrap_or(types::Turn {
                                    id: params.turn_id.clone(),
                                    items: Vec::new(),
                                    status: types::TurnStatus::Interrupted,
                                    error: Some(types::TurnError {
                                        message: "interrupted".to_string(),
                                        additional_details: None,
                                        codex_error_info: None,
                                    }),
                                });
                            thread.status = types::ThreadStatus::Idle;
                            thread.updated_at = Utc::now().timestamp();
                            completed_turn
                        };
                        let _ = Self::send_notification(
                            &inbound_tx,
                            methods::TURN_COMPLETED,
                            &types::TurnCompletedNotification {
                                thread_id: params.thread_id.clone(),
                                turn: completed_turn,
                            },
                        )
                        .await;
                    });
                }
                methods::MODEL_LIST => {
                    Self::send_response(
                        &inbound_tx,
                        request.id,
                        &types::ModelListResponse {
                            data: Vec::new(),
                            next_cursor: None,
                        },
                    )
                    .await?;
                }
                other => {
                    return Err(OrcasError::Protocol(format!(
                        "fake codex transport does not implement `{other}`"
                    )));
                }
            }
            Ok(())
        }

        async fn send_response<T: serde::Serialize>(
            inbound_tx: &tokio::sync::mpsc::Sender<String>,
            id: codex_jsonrpc::RequestId,
            result: &T,
        ) -> OrcasResult<()> {
            let raw = serde_json::to_string(&codex_jsonrpc::JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: serde_json::to_value(result)?,
            })?;
            inbound_tx.send(raw).await.map_err(|error| {
                OrcasError::Transport(format!(
                    "failed to send fake codex response to client: {error}"
                ))
            })
        }

        async fn send_rpc_error(
            inbound_tx: &tokio::sync::mpsc::Sender<String>,
            id: codex_jsonrpc::RequestId,
            code: i64,
            message: &str,
        ) -> OrcasResult<()> {
            let raw = serde_json::to_string(&codex_jsonrpc::JsonRpcError {
                jsonrpc: "2.0".to_string(),
                id,
                error: codex_jsonrpc::JsonRpcErrorObject {
                    code,
                    message: message.to_string(),
                    data: None,
                },
            })?;
            inbound_tx.send(raw).await.map_err(|error| {
                OrcasError::Transport(format!(
                    "failed to send fake codex rpc error to client: {error}"
                ))
            })
        }

        async fn send_notification<T: serde::Serialize>(
            inbound_tx: &tokio::sync::mpsc::Sender<String>,
            method: &str,
            params: &T,
        ) -> OrcasResult<()> {
            let raw = serde_json::to_string(&codex_jsonrpc::JsonRpcNotification::new(
                method,
                Some(serde_json::to_value(params)?),
            ))?;
            inbound_tx.send(raw).await.map_err(|error| {
                OrcasError::Transport(format!(
                    "failed to send fake codex notification to client: {error}"
                ))
            })
        }

        fn turn_start_text(params: &types::TurnStartParams) -> Option<String> {
            params.input.iter().find_map(|input| match input {
                types::UserInput::Text { text, .. } => Some(text.clone()),
            })
        }

        fn substitute_prompt_placeholders(template: &str, prompt_text: Option<&str>) -> String {
            let Some(prompt_text) = prompt_text else {
                return template.to_string();
            };
            let assignment_id = extract_prompt_line_value(prompt_text, "Assignment id:");
            let packet_id = extract_prompt_line_value(prompt_text, "Packet id:");
            let output = template.replace(
                "{{assignment_id}}",
                assignment_id.as_deref().unwrap_or("assignment-missing"),
            );
            output.replace(
                "{{packet_id}}",
                packet_id.as_deref().unwrap_or("packet-missing"),
            )
        }
    }

    #[async_trait]
    impl CodexTransport for FakeCodexTransport {
        async fn connect(&self) -> OrcasResult<TransportConnection> {
            let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel::<String>(256);
            let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel::<String>(256);
            let state = Arc::clone(&self.state);
            let turn_output = self.turn_output.clone();
            let terminal_outcome = self.terminal_outcome;

            tokio::spawn(async move {
                while let Some(raw) = outbound_rx.recv().await {
                    let Ok(message) = serde_json::from_str::<codex_jsonrpc::JsonRpcMessage>(&raw)
                    else {
                        continue;
                    };
                    match message {
                        codex_jsonrpc::JsonRpcMessage::Request(request) => {
                            let _ = Self::handle_request(
                                Arc::clone(&state),
                                turn_output.clone(),
                                terminal_outcome,
                                inbound_tx.clone(),
                                request,
                            )
                            .await;
                        }
                        codex_jsonrpc::JsonRpcMessage::Notification(notification) => {
                            if notification.method == methods::INITIALIZED {
                                continue;
                            }
                        }
                        codex_jsonrpc::JsonRpcMessage::Response(_)
                        | codex_jsonrpc::JsonRpcMessage::Error(_) => {}
                    }
                }
            });

            Ok(TransportConnection {
                outbound: outbound_tx,
                inbound: inbound_rx,
            })
        }

        fn endpoint(&self) -> &str {
            &self.endpoint
        }
    }

    fn fixed_supervisor_prompt_rendered_at() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 4, 5, 6, 7, 8)
            .single()
            .expect("valid prompt render timestamp")
    }

    fn fixed_supervisor_response_captured_at() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 4, 5, 6, 7, 9)
            .single()
            .expect("valid response artifact timestamp")
    }

    fn sample_response_artifact(
        model: &str,
        response_id: &str,
        output_text: Option<&str>,
    ) -> orcas_core::SupervisorResponseArtifact {
        let raw_response = output_text.map(|output_text| {
            serde_json::json!({
                "id": response_id,
                "model": model,
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": output_text,
                    }],
                }],
            })
        });
        let raw_response_body = raw_response.as_ref().map(ToString::to_string);
        render_supervisor_response_artifact(
            "test",
            model,
            raw_response.as_ref(),
            raw_response_body.as_deref(),
            output_text,
            fixed_supervisor_response_captured_at(),
        )
        .expect("render sample response artifact")
    }

    fn sample_failure_response_artifact(
        model: &str,
        response_id: &str,
        output_text: Option<&str>,
    ) -> orcas_core::SupervisorResponseArtifact {
        render_supervisor_response_artifact(
            "test",
            model,
            None,
            output_text,
            output_text,
            fixed_supervisor_response_captured_at(),
        )
        .map(|artifact| orcas_core::SupervisorResponseArtifact {
            response_id: Some(response_id.to_string()),
            ..artifact
        })
        .expect("render failure response artifact")
    }

    #[derive(Clone)]
    enum StaticSupervisorReasonerOutcome {
        Success {
            proposal: SupervisorProposal,
            output_text: Option<String>,
        },
        Failure {
            stage: SupervisorProposalFailureStage,
            message: String,
            output_text: Option<String>,
        },
    }

    #[derive(Default)]
    struct StaticSupervisorReasoner {
        outcome: Mutex<Option<StaticSupervisorReasonerOutcome>>,
        last_pack: Mutex<Option<SupervisorContextPack>>,
        last_prompt_render: Mutex<Option<orcas_core::supervisor::SupervisorPromptRenderArtifact>>,
        last_response_artifact: Mutex<Option<orcas_core::SupervisorResponseArtifact>>,
        propose_calls: AtomicUsize,
    }

    impl StaticSupervisorReasoner {
        async fn set_proposal(&self, proposal: SupervisorProposal) {
            let output_text = serde_json::to_string(&proposal).expect("serialize proposal");
            *self.outcome.lock().await = Some(StaticSupervisorReasonerOutcome::Success {
                proposal,
                output_text: Some(output_text),
            });
        }

        async fn set_failure(
            &self,
            stage: SupervisorProposalFailureStage,
            message: impl Into<String>,
            output_text: Option<String>,
        ) {
            *self.outcome.lock().await = Some(StaticSupervisorReasonerOutcome::Failure {
                stage,
                message: message.into(),
                output_text,
            });
        }

        async fn last_pack(&self) -> Option<SupervisorContextPack> {
            self.last_pack.lock().await.clone()
        }

        async fn last_prompt_render(
            &self,
        ) -> Option<orcas_core::supervisor::SupervisorPromptRenderArtifact> {
            self.last_prompt_render.lock().await.clone()
        }

        async fn last_response_artifact(&self) -> Option<orcas_core::SupervisorResponseArtifact> {
            self.last_response_artifact.lock().await.clone()
        }

        fn propose_call_count(&self) -> usize {
            self.propose_calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SupervisorReasoner for StaticSupervisorReasoner {
        async fn propose(
            &self,
            pack: SupervisorContextPack,
        ) -> Result<SupervisorReasonerResult, SupervisorReasonerFailure> {
            self.propose_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let prompt_render =
                render_supervisor_prompt(&pack, fixed_supervisor_prompt_rendered_at())
                    .expect("render test supervisor prompt");
            *self.last_prompt_render.lock().await = Some(prompt_render.clone());
            *self.last_pack.lock().await = Some(pack);
            match self.outcome.lock().await.clone() {
                Some(StaticSupervisorReasonerOutcome::Success {
                    proposal,
                    output_text,
                }) => {
                    let response_artifact = sample_response_artifact(
                        "test-supervisor",
                        "resp-test",
                        output_text.as_deref(),
                    );
                    *self.last_response_artifact.lock().await = Some(response_artifact.clone());
                    Ok(SupervisorReasonerResult {
                        proposal,
                        backend_kind: "test".to_string(),
                        model: "test-supervisor".to_string(),
                        response_id: Some("resp-test".to_string()),
                        usage: None,
                        output_text,
                        prompt_render,
                        response_artifact,
                    })
                }
                Some(StaticSupervisorReasonerOutcome::Failure {
                    stage,
                    message,
                    output_text,
                }) => {
                    let response_artifact = output_text.as_deref().map(|output_text| {
                        sample_failure_response_artifact(
                            "test-supervisor",
                            "resp-test",
                            Some(output_text),
                        )
                    });
                    *self.last_response_artifact.lock().await = response_artifact.clone();
                    Err(SupervisorReasonerFailure {
                        stage,
                        message,
                        backend_kind: "test".to_string(),
                        model: "test-supervisor".to_string(),
                        response_id: Some("resp-test".to_string()),
                        output_text,
                        prompt_render: Some(prompt_render),
                        response_artifact,
                    })
                }
                None => {
                    *self.last_response_artifact.lock().await = None;
                    Err(SupervisorReasonerFailure {
                        stage: SupervisorProposalFailureStage::Backend,
                        message: "missing test supervisor reasoner outcome".to_string(),
                        backend_kind: "test".to_string(),
                        model: "test-supervisor".to_string(),
                        response_id: Some("resp-test".to_string()),
                        output_text: None,
                        prompt_render: Some(prompt_render),
                        response_artifact: None,
                    })
                }
            }
        }
    }

    struct PackDrivenSupervisorReasoner {
        decision_type: DecisionType,
        last_pack: Mutex<Option<SupervisorContextPack>>,
        propose_calls: AtomicUsize,
    }

    impl PackDrivenSupervisorReasoner {
        fn new(decision_type: DecisionType) -> Self {
            Self {
                decision_type,
                last_pack: Mutex::new(None),
                propose_calls: AtomicUsize::new(0),
            }
        }

        fn propose_call_count(&self) -> usize {
            self.propose_calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SupervisorReasoner for PackDrivenSupervisorReasoner {
        async fn propose(
            &self,
            pack: SupervisorContextPack,
        ) -> Result<SupervisorReasonerResult, SupervisorReasonerFailure> {
            self.propose_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.last_pack.lock().await = Some(pack.clone());
            let proposal = sample_proposal_for_pack(self.decision_type, &pack);
            let output_text = serde_json::to_string(&proposal).expect("serialize proposal");
            let prompt_render =
                render_supervisor_prompt(&pack, fixed_supervisor_prompt_rendered_at())
                    .expect("render pack-driven supervisor prompt");
            let response_artifact = sample_response_artifact(
                "test-pack-driven",
                "resp-pack-driven",
                Some(&output_text),
            );
            Ok(SupervisorReasonerResult {
                proposal,
                backend_kind: "test".to_string(),
                model: "test-pack-driven".to_string(),
                response_id: Some("resp-pack-driven".to_string()),
                usage: None,
                output_text: Some(output_text),
                prompt_render,
                response_artifact,
            })
        }
    }

    fn sample_proposal_for_decision(
        decision_type: DecisionType,
        work_unit_id: &str,
        report_id: &str,
        assignment_id: &str,
        worker_id: &str,
    ) -> SupervisorProposal {
        let requires_assignment = matches!(
            decision_type,
            DecisionType::Continue | DecisionType::Redirect
        );
        let recommended_action = match decision_type {
            DecisionType::Accept => "Accept the work as a valid intermediate result.",
            DecisionType::Continue => "Continue with one more bounded assignment.",
            DecisionType::Redirect => "Redirect the next bounded assignment.",
            DecisionType::MarkComplete => "Mark the work unit complete.",
            DecisionType::EscalateToHuman => "Escalate this decision point to a human.",
        };
        SupervisorProposal {
            schema_version: "supervisor_proposal.v1".to_string(),
            summary: SupervisorSummary {
                headline: format!("Decision proposal: {:?}", decision_type),
                situation: "The latest report reached a bounded decision point.".to_string(),
                recommended_action: recommended_action.to_string(),
                key_evidence: vec!["The source report is explicit and reviewable.".to_string()],
                risks: Vec::new(),
                review_focus: vec!["Verify the decision remains bounded.".to_string()],
            },
            proposed_decision: ProposedDecision {
                decision_type,
                target_work_unit_id: work_unit_id.to_string(),
                source_report_id: report_id.to_string(),
                rationale: format!("Apply {:?} for this decision point.", decision_type),
                expected_work_unit_status: match decision_type {
                    DecisionType::Accept => "accepted",
                    DecisionType::Continue | DecisionType::Redirect => "ready",
                    DecisionType::MarkComplete => "completed",
                    DecisionType::EscalateToHuman => "needs_human",
                }
                .to_string(),
                requires_assignment,
            },
            draft_next_assignment: requires_assignment.then(|| DraftAssignment {
                target_work_unit_id: work_unit_id.to_string(),
                predecessor_assignment_id: assignment_id.to_string(),
                derived_from_decision_type: decision_type,
                preferred_worker_id: Some(worker_id.to_string()),
                worker_kind: Some("codex".to_string()),
                objective: match decision_type {
                    DecisionType::Redirect => {
                        "Re-run the bounded follow-up with the redirected focus.".to_string()
                    }
                    _ => "Resolve the remaining bounded follow-up.".to_string(),
                },
                instructions: vec![
                    "Inspect the remaining bounded gap.".to_string(),
                    "Record the exact result without broadening scope.".to_string(),
                ],
                acceptance_criteria: vec!["The bounded question is resolved.".to_string()],
                stop_conditions: vec!["Stop if human input is required.".to_string()],
                required_context_refs: vec![report_id.to_string()],
                expected_report_fields: vec![
                    "summary".to_string(),
                    "findings".to_string(),
                    "questions".to_string(),
                ],
                boundedness_note: "This assignment stays within one bounded follow-up question."
                    .to_string(),
            }),
            confidence: ReportConfidence::High,
            warnings: Vec::new(),
            open_questions: Vec::new(),
        }
    }

    fn sample_proposal_for_pack(
        decision_type: DecisionType,
        pack: &SupervisorContextPack,
    ) -> SupervisorProposal {
        sample_proposal_for_decision(
            decision_type,
            &pack.primary_work_unit.id,
            &pack.source_report.id,
            &pack.current_assignment.id,
            &pack.current_assignment.worker_id,
        )
    }

    fn auto_proposal_config(enabled: bool) -> AppConfig {
        let mut config = AppConfig::default();
        config.supervisor.proposals.auto_create_on_report_recorded = enabled;
        config
    }

    fn sample_thread(id: &str, scope: &str, updated_at: i64) -> ipc::ThreadView {
        ipc::ThreadView {
            summary: ipc::ThreadSummary {
                id: id.to_string(),
                preview: format!("preview {id}"),
                name: None,
                model_provider: "openai".to_string(),
                cwd: "/tmp/orcas".to_string(),
                status: "idle".to_string(),
                created_at: updated_at - 10,
                updated_at,
                scope: scope.to_string(),
                archived: false,
                loaded_status: ipc::ThreadLoadedStatus::Idle,
                active_flags: Vec::new(),
                active_turn_id: None,
                last_seen_turn_id: None,
                recent_output: None,
                recent_event: None,
                turn_in_flight: false,
                monitor_state: ipc::ThreadMonitorState::Detached,
                last_sync_at: Utc::now(),
                source_kind: None,
                raw_summary: None,
            },
            history_loaded: false,
            turns: Vec::new(),
        }
    }

    fn sample_active_thread(
        id: &str,
        scope: &str,
        updated_at: i64,
        turn_id: &str,
    ) -> ipc::ThreadView {
        let mut thread = sample_thread(id, scope, updated_at);
        thread.summary.status = "active".to_string();
        thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
        thread.summary.active_turn_id = Some(turn_id.to_string());
        thread.summary.last_seen_turn_id = Some(turn_id.to_string());
        thread.summary.turn_in_flight = true;
        thread.turns.push(ipc::TurnView {
            id: turn_id.to_string(),
            status: "in_progress".to_string(),
            error_message: None,
            error_summary: None,
            started_at: Some(Utc::now()),
            completed_at: None,
            latest_diff: None,
            latest_plan_snapshot: None,
            token_usage_snapshot: None,
            items: Vec::new(),
        });
        thread
    }

    async fn seed_codex_thread_assignment_fixture(
        service: &Arc<OrcasDaemonService>,
        thread: ipc::ThreadView,
    ) -> (Workstream, WorkUnit) {
        {
            let mut state = service.state.write().await;
            state
                .threads
                .insert(thread.summary.id.clone(), thread.clone());
        }
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Codex assignment stream".to_string(),
                objective: "Track one Codex thread assignment".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Codex assignment unit".to_string(),
                task_statement: "Bind an external Codex thread.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        (workstream, work_unit)
    }

    fn extract_prompt_line_value(prompt: &str, prefix: &str) -> Option<String> {
        prompt
            .lines()
            .find_map(|line| line.trim().strip_prefix(prefix))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    async fn test_service() -> Arc<OrcasDaemonService> {
        let base = std::env::temp_dir().join(format!("orcas-collab-test-{}", Uuid::new_v4()));
        test_service_at(base).await
    }

    async fn test_service_with_reasoner(
        reasoner: Arc<dyn SupervisorReasoner>,
    ) -> Arc<OrcasDaemonService> {
        test_service_with_reasoner_and_config(reasoner, AppConfig::default()).await
    }

    async fn test_service_with_reasoner_and_config(
        reasoner: Arc<dyn SupervisorReasoner>,
        config: AppConfig,
    ) -> Arc<OrcasDaemonService> {
        let base = std::env::temp_dir().join(format!("orcas-collab-test-{}", Uuid::new_v4()));
        test_service_at_with_reasoner_and_config(base, reasoner, config).await
    }

    async fn test_service_at(base: PathBuf) -> Arc<OrcasDaemonService> {
        test_service_at_with_reasoner(base, Arc::new(StaticSupervisorReasoner::default())).await
    }

    async fn test_service_at_with_reasoner(
        base: PathBuf,
        supervisor_reasoner: Arc<dyn SupervisorReasoner>,
    ) -> Arc<OrcasDaemonService> {
        test_service_at_with_reasoner_and_config(base, supervisor_reasoner, AppConfig::default())
            .await
    }

    async fn test_service_at_with_reasoner_and_config(
        base: PathBuf,
        supervisor_reasoner: Arc<dyn SupervisorReasoner>,
        config: AppConfig,
    ) -> Arc<OrcasDaemonService> {
        let paths = AppPaths {
            config_dir: base.join("config"),
            config_file: base.join("config/config.toml"),
            data_dir: base.join("data"),
            state_file: base.join("data/state.json"),
            state_db_file: base.join("data/state.db"),
            logs_dir: base.join("logs"),
            runtime_dir: base.join("runtime"),
            socket_file: base.join("runtime/orcasd.sock"),
            daemon_metadata_file: base.join("runtime/orcasd.json"),
            daemon_log_file: base.join("logs/orcasd.log"),
        };
        paths.ensure().await.expect("paths");
        let codex_daemon: Arc<dyn CodexDaemonManager> = Arc::new(LocalCodexDaemonManager::new(
            config.codex.clone(),
            &paths,
            config.defaults.cwd.clone(),
        ));
        let codex_client = CodexClient::new(
            Arc::new(WebSocketTransport::new(config.codex.listen_url.clone())),
            config.codex.reconnect.clone(),
            Arc::new(RejectingApprovalRouter),
        );
        test_service_at_with_components(
            paths,
            config,
            supervisor_reasoner,
            codex_daemon,
            codex_client,
            false,
        )
        .await
    }

    async fn test_service_at_with_components(
        paths: AppPaths,
        config: AppConfig,
        supervisor_reasoner: Arc<dyn SupervisorReasoner>,
        codex_daemon: Arc<dyn CodexDaemonManager>,
        codex_client: Arc<CodexClient>,
        spawn_codex_bridge: bool,
    ) -> Arc<OrcasDaemonService> {
        let store = Arc::new(JsonSessionStore::new(paths.clone(), config.clone()));
        let authority_store =
            Arc::new(AuthoritySqliteStore::open(paths.clone()).expect("authority"));
        let (event_tx, _) = broadcast::channel(32);
        let service = Arc::new(OrcasDaemonService {
            paths,
            config,
            runtime: ipc::DaemonRuntimeMetadata {
                pid: std::process::id(),
                started_at: Utc::now(),
                version: "test".to_string(),
                build_fingerprint: "test".to_string(),
                binary_path: "test".to_string(),
                socket_path: "/tmp/test.sock".to_string(),
                metadata_path: "/tmp/test.json".to_string(),
                git_commit: None,
            },
            store,
            authority_store,
            codex_daemon,
            codex_client,
            state: RwLock::new(DaemonState::default()),
            recent_events: Mutex::new(std::collections::VecDeque::new()),
            connect_gate: Mutex::new(()),
            event_tx,
            client_count: AtomicUsize::new(0),
            shutdown: Notify::new(),
            supervisor_reasoner,
        });
        service.initialize_state().await.expect("initialize state");
        if spawn_codex_bridge {
            service.spawn_codex_event_bridge();
        }
        service
    }

    async fn test_service_with_fake_codex_runtime(
        config: AppConfig,
        supervisor_reasoner: Arc<dyn SupervisorReasoner>,
        turn_output: &str,
        terminal_outcome: FakeCodexTerminalOutcome,
    ) -> Arc<OrcasDaemonService> {
        test_service_with_fake_codex_runtime_capture(
            config,
            supervisor_reasoner,
            turn_output,
            terminal_outcome,
        )
        .await
        .0
    }

    async fn test_service_with_fake_codex_runtime_capture(
        config: AppConfig,
        supervisor_reasoner: Arc<dyn SupervisorReasoner>,
        turn_output: &str,
        terminal_outcome: FakeCodexTerminalOutcome,
    ) -> (Arc<OrcasDaemonService>, Arc<Mutex<FakeCodexRuntimeState>>) {
        let base = std::env::temp_dir().join(format!("orcas-collab-test-{}", Uuid::new_v4()));
        let paths = AppPaths {
            config_dir: base.join("config"),
            config_file: base.join("config/config.toml"),
            data_dir: base.join("data"),
            state_file: base.join("data/state.json"),
            state_db_file: base.join("data/state.db"),
            logs_dir: base.join("logs"),
            runtime_dir: base.join("runtime"),
            socket_file: base.join("runtime/orcasd.sock"),
            daemon_metadata_file: base.join("runtime/orcasd.json"),
            daemon_log_file: base.join("logs/orcasd.log"),
        };
        paths.ensure().await.expect("paths");
        let codex_daemon: Arc<dyn CodexDaemonManager> = Arc::new(FakeCodexDaemonManager {
            endpoint: config.codex.listen_url.clone(),
        });
        let fake_transport = Arc::new(FakeCodexTransport::new(
            config.codex.listen_url.clone(),
            turn_output.to_string(),
            terminal_outcome,
        ));
        let fake_runtime_state = Arc::clone(&fake_transport.state);
        let codex_client = CodexClient::new(
            fake_transport,
            config.codex.reconnect.clone(),
            Arc::new(RejectingApprovalRouter),
        );
        let service = test_service_at_with_components(
            paths,
            config,
            supervisor_reasoner,
            codex_daemon,
            codex_client,
            true,
        )
        .await;
        (service, fake_runtime_state)
    }

    async fn seed_awaiting_decision_fixture(
        service: &Arc<OrcasDaemonService>,
        label: &str,
    ) -> (Workstream, WorkUnit, Assignment, Report) {
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: format!("Proposal {label}"),
                objective: "Exercise supervisor proposal flow".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: format!("Work unit {label}"),
                task_statement: "Inspect the report and propose one bounded next step.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let assignment = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: format!("worker-{label}"),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Review the current result and report back.".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment")
            .assignment;

        let report = {
            let now = Utc::now();
            let mut state = service.state.write().await;
            state
                .collaboration
                .assignments
                .get_mut(&assignment.id)
                .expect("assignment")
                .status = AssignmentStatus::AwaitingDecision;
            let worker_session = state
                .collaboration
                .worker_sessions
                .get_mut(&assignment.worker_session_id)
                .expect("worker session");
            worker_session.runtime_status = WorkerSessionRuntimeStatus::Completed;
            worker_session.attachability = WorkerSessionAttachability::NotAttachable;
            worker_session.updated_at = now;
            let work_unit_entry = state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .expect("work unit");
            work_unit_entry.status = WorkUnitStatus::AwaitingDecision;
            work_unit_entry.updated_at = now;

            let report = Report {
                id: format!("report-{}", Uuid::new_v4().simple()),
                work_unit_id: work_unit.id.clone(),
                assignment_id: assignment.id.clone(),
                worker_id: assignment.worker_id.clone(),
                disposition: ReportDisposition::Completed,
                summary: "The first pass was useful but left one bounded follow-up question."
                    .to_string(),
                findings: vec!["Reconnect handling was partially verified.".to_string()],
                blockers: Vec::new(),
                questions: vec!["Does the interrupted path surface honest status?".to_string()],
                recommended_next_actions: vec![
                    "Run one more narrow verification step.".to_string(),
                ],
                confidence: ReportConfidence::Medium,
                raw_output: "raw worker output".to_string(),
                parse_result: ReportParseResult::Parsed,
                needs_supervisor_review: false,
                created_at: now,
            };
            state
                .collaboration
                .reports
                .insert(report.id.clone(), report.clone());
            let work_unit_entry = state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .expect("work unit");
            work_unit_entry.latest_report_id = Some(report.id.clone());
            work_unit_entry.updated_at = now;
            report
        };
        service
            .persist_collaboration_state()
            .await
            .expect("persist fixture");

        (workstream, work_unit, assignment, report)
    }

    async fn seed_manual_proposal_fixture(
        service: &Arc<OrcasDaemonService>,
        label: &str,
    ) -> (Workstream, WorkUnit, Assignment, Report) {
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: format!("Manual proposal {label}"),
                objective: "Exercise supervisor proposal persistence".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: format!("Manual work unit {label}"),
                task_statement: "Inspect the report and propose one bounded next step.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;

        let (assignment, report) = {
            let now = Utc::now();
            let worker_id = format!("worker-manual-{label}");
            let mut state = service.state.write().await;
            let worker_session_id = OrcasDaemonService::select_worker_session_for_assignment(
                &mut state.collaboration,
                &worker_id,
                "codex".to_string(),
            );
            let assignment = Assignment {
                id: OrcasDaemonService::new_object_id("assignment"),
                work_unit_id: work_unit.id.clone(),
                worker_id: worker_id.clone(),
                worker_session_id: worker_session_id.clone(),
                instructions: "Review the current result and report back.".to_string(),
                communication_seed: None,
                status: AssignmentStatus::AwaitingDecision,
                attempt_number: 1,
                created_at: now,
                updated_at: now,
            };
            state
                .collaboration
                .assignments
                .insert(assignment.id.clone(), assignment.clone());
            let worker = state
                .collaboration
                .workers
                .get_mut(&worker_id)
                .expect("worker");
            worker.current_assignment_id = Some(assignment.id.clone());
            worker.status = WorkerStatus::Busy;
            let worker_session = state
                .collaboration
                .worker_sessions
                .get_mut(&worker_session_id)
                .expect("worker session");
            worker_session.runtime_status = WorkerSessionRuntimeStatus::Completed;
            worker_session.attachability = WorkerSessionAttachability::NotAttachable;
            worker_session.updated_at = now;
            let report = Report {
                id: format!("report-{}", Uuid::new_v4().simple()),
                work_unit_id: work_unit.id.clone(),
                assignment_id: assignment.id.clone(),
                worker_id,
                disposition: ReportDisposition::Completed,
                summary: "Bounded work completed cleanly.".to_string(),
                findings: vec!["Parser contract tightened.".to_string()],
                blockers: Vec::new(),
                questions: Vec::new(),
                recommended_next_actions: vec!["Continue with one bounded follow-up.".to_string()],
                confidence: ReportConfidence::High,
                raw_output: "raw output".to_string(),
                parse_result: ReportParseResult::Parsed,
                needs_supervisor_review: false,
                created_at: now,
            };
            state
                .collaboration
                .reports
                .insert(report.id.clone(), report.clone());
            let work_unit_entry = state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .expect("work unit");
            work_unit_entry.status = WorkUnitStatus::AwaitingDecision;
            work_unit_entry.current_assignment_id = Some(assignment.id.clone());
            work_unit_entry.latest_report_id = Some(report.id.clone());
            work_unit_entry.updated_at = now;
            (assignment, report)
        };
        service
            .persist_collaboration_state()
            .await
            .expect("persist manual proposal fixture");
        (workstream, work_unit, assignment, report)
    }

    async fn seed_running_assignment_fixture(
        service: &Arc<OrcasDaemonService>,
        label: &str,
    ) -> (Workstream, WorkUnit, Assignment, ipc::TurnStateView, String) {
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: format!("Running {label}"),
                objective: "Exercise real report ingestion".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: format!("Work unit {label}"),
                task_statement: "Execute one bounded step and stop with a structured report."
                    .to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let assignment = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: format!("worker-{label}"),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Run the bounded step and emit a report.".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment")
            .assignment;

        {
            let now = Utc::now();
            let mut state = service.state.write().await;
            state
                .collaboration
                .assignments
                .get_mut(&assignment.id)
                .expect("assignment")
                .status = AssignmentStatus::Running;
            let worker = state
                .collaboration
                .workers
                .get_mut(&assignment.worker_id)
                .expect("worker");
            worker.status = WorkerStatus::Busy;
            worker.current_assignment_id = Some(assignment.id.clone());
            let worker_session = state
                .collaboration
                .worker_sessions
                .get_mut(&assignment.worker_session_id)
                .expect("worker session");
            worker_session.active_turn_id = Some(format!("turn-{label}"));
            worker_session.runtime_status = WorkerSessionRuntimeStatus::Running;
            worker_session.attachability = WorkerSessionAttachability::Attachable;
            worker_session.updated_at = now;
            let work_unit_entry = state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .expect("work unit");
            work_unit_entry.status = WorkUnitStatus::Running;
            work_unit_entry.updated_at = now;
        }
        service
            .persist_collaboration_state()
            .await
            .expect("persist running fixture");

        let turn_state = ipc::TurnStateView {
            thread_id: format!("thread-{label}"),
            turn_id: format!("turn-{label}"),
            lifecycle: ipc::TurnLifecycleState::Completed,
            status: "completed".to_string(),
            attachable: false,
            live_stream: false,
            terminal: true,
            recent_output: Some("structured worker report".to_string()),
            recent_event: Some("turn completed".to_string()),
            updated_at: Utc::now(),
            error_message: None,
        };
        let packet_id = service
            .state
            .read()
            .await
            .collaboration
            .assignment_communications
            .get(&assignment.id)
            .expect("communication record")
            .packet
            .packet_id
            .clone();
        let raw_output = sample_runtime_report_output_for(&assignment.id, &packet_id);

        (workstream, work_unit, assignment, turn_state, raw_output)
    }

    async fn create_proposal_for_decision(
        service: &Arc<OrcasDaemonService>,
        reasoner: &Arc<StaticSupervisorReasoner>,
        work_unit: &WorkUnit,
        assignment: &Assignment,
        report: &Report,
        decision_type: DecisionType,
    ) -> ipc::ProposalCreateResponse {
        reasoner
            .set_proposal(sample_proposal_for_decision(
                decision_type,
                &work_unit.id,
                &report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;
        service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: work_unit.id.clone(),
                source_report_id: Some(report.id.clone()),
                requested_by: Some("tester".to_string()),
                note: Some("Synthesize the next bounded step.".to_string()),
                supersede_open: false,
            })
            .await
            .expect("proposal create")
    }

    async fn create_default_proposal(
        service: &Arc<OrcasDaemonService>,
        reasoner: &Arc<StaticSupervisorReasoner>,
        work_unit: &WorkUnit,
        assignment: &Assignment,
        report: &Report,
    ) -> ipc::ProposalCreateResponse {
        create_proposal_for_decision(
            service,
            reasoner,
            work_unit,
            assignment,
            report,
            DecisionType::Continue,
        )
        .await
    }

    async fn approve_proposal(
        service: &Arc<OrcasDaemonService>,
        proposal_id: &str,
        edits: SupervisorProposalEdits,
    ) -> ipc::ProposalApproveResponse {
        service
            .proposal_approve(ipc::ProposalApproveRequest {
                proposal_id: proposal_id.to_string(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("approved".to_string()),
                edits,
            })
            .await
            .expect("proposal approve")
    }

    async fn latest_proposal_record_for_workunit(
        service: &Arc<OrcasDaemonService>,
        work_unit_id: &str,
    ) -> orcas_core::SupervisorProposalRecord {
        let state = service.state.read().await;
        state
            .collaboration
            .supervisor_proposals
            .values()
            .filter(|proposal| proposal.primary_work_unit_id == work_unit_id)
            .cloned()
            .max_by(|left, right| {
                left.created_at
                    .cmp(&right.created_at)
                    .then_with(|| left.id.cmp(&right.id))
            })
            .expect("latest proposal")
    }

    async fn wait_for_proposal_action(
        events: &mut broadcast::Receiver<ipc::DaemonEventEnvelope>,
        expected: ipc::ProposalLifecycleAction,
    ) -> ipc::ProposalSummary {
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
                .await
                .expect("event timeout")
                .expect("event");
            if let ipc::DaemonEvent::ProposalLifecycle {
                action,
                work_unit,
                proposal,
            } = event.event
            {
                assert_eq!(proposal.primary_work_unit_id, work_unit.id);
                assert!(work_unit.proposal.is_some());
                if action == expected {
                    return proposal;
                }
            }
        }
    }

    async fn latest_supervisor_turn_decision_for_assignment(
        service: &Arc<OrcasDaemonService>,
        assignment_id: &str,
    ) -> SupervisorTurnDecision {
        service
            .state
            .read()
            .await
            .collaboration
            .supervisor_turn_decisions
            .values()
            .filter(|decision| decision.assignment_id == assignment_id)
            .cloned()
            .max_by(|left, right| {
                left.created_at
                    .cmp(&right.created_at)
                    .then_with(|| left.decision_id.cmp(&right.decision_id))
            })
            .expect("latest supervisor turn decision")
    }

    async fn wait_for_report_recorded(
        events: &mut broadcast::Receiver<ipc::DaemonEventEnvelope>,
    ) -> ipc::ReportSummary {
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
                .await
                .expect("event timeout")
                .expect("event");
            if let ipc::DaemonEvent::ReportRecorded { report } = event.event {
                return report;
            }
        }
    }

    fn sample_runtime_report_output_template() -> &'static str {
        r#"ORCAS_REPORT_BEGIN
{
  "schema_version": "worker_report_envelope.v1",
  "assignment_id": "{{assignment_id}}",
  "packet_id": "{{packet_id}}",
  "task_mode": "implement",
  "disposition": "completed",
  "summary": "finished the bounded task",
  "confidence": "high",
  "acceptance_results": [],
  "triggered_stop_condition_ids": [],
  "touched_files": [],
  "commands_run": [],
  "artifacts": [],
  "blockers": [],
  "questions": [],
  "recommended_next_actions": ["apply supervisor decision"],
  "uncertainties": [],
  "review_signal": {
    "level": "normal",
    "reasons": [],
    "focus": []
  },
  "mode_payload": {
    "kind": "implement",
    "semantic_changes": ["root cause isolated"],
    "tests_run": ["cargo test -p orcasd"],
    "rough_edges": []
  }
}
ORCAS_REPORT_END"#
    }

    fn sample_runtime_report_output_for(assignment_id: &str, packet_id: &str) -> String {
        sample_runtime_report_output_template()
            .replace("{{assignment_id}}", assignment_id)
            .replace("{{packet_id}}", packet_id)
    }

    fn sample_structured_assignment_seed(
        predecessor_assignment_id: &str,
        source_report_id: &str,
    ) -> AssignmentCommunicationSeed {
        AssignmentCommunicationSeed {
            source_decision_id: None,
            source_report_id: Some(source_report_id.to_string()),
            source_proposal_id: Some("proposal-structured".to_string()),
            predecessor_assignment_id: Some(predecessor_assignment_id.to_string()),
            objective: "Implement the structured recovery pass.".to_string(),
            instructions: vec![
                "Inspect only the structured recovery branch.".to_string(),
                "Do not broaden beyond the current implement slice.".to_string(),
            ],
            acceptance_criteria: vec![
                "The structured recovery branch behavior is confirmed.".to_string(),
            ],
            stop_conditions: vec!["Stop if the recovery branch is not reproducible.".to_string()],
            required_context_refs: vec!["ctx/recovery".to_string()],
            expected_report_fields: vec![
                "summary".to_string(),
                "recommended_next_actions".to_string(),
            ],
            boundedness_note: Some("Keep the follow-up strictly bounded.".to_string()),
            mode_spec: AssignmentModeSpec::Implement(ImplementModeSpec {
                expected_verification_commands: Vec::new(),
            }),
        }
    }

    fn wrap_report_envelope(json: &str) -> String {
        format!("ORCAS_REPORT_BEGIN\n{json}\nORCAS_REPORT_END")
    }

    fn sample_assignment_and_communication_record()
    -> (Assignment, orcas_core::AssignmentCommunicationRecord) {
        let now = Utc::now();
        let workstream = Workstream {
            id: "ws-parse".to_string(),
            title: "Parse".to_string(),
            objective: "Exercise worker report parsing".to_string(),
            status: WorkstreamStatus::Active,
            priority: "normal".to_string(),
            created_at: now,
            updated_at: now,
        };
        let work_unit = WorkUnit {
            id: "wu-parse".to_string(),
            workstream_id: workstream.id.clone(),
            title: "Implement".to_string(),
            task_statement: "Implement one bounded step.".to_string(),
            status: WorkUnitStatus::Ready,
            dependencies: Vec::new(),
            latest_report_id: None,
            current_assignment_id: Some("assignment-parse".to_string()),
            created_at: now,
            updated_at: now,
        };
        let assignment = Assignment {
            id: "assignment-parse".to_string(),
            work_unit_id: work_unit.id.clone(),
            worker_id: "worker-parse".to_string(),
            worker_session_id: "session-parse".to_string(),
            instructions: "Implement the bounded task and report honestly.".to_string(),
            communication_seed: Some(OrcasDaemonService::manual_implement_assignment_seed(
                &work_unit,
                Some("Implement the bounded task and report honestly."),
                None,
            )),
            status: AssignmentStatus::Created,
            attempt_number: 1,
            created_at: now,
            updated_at: now,
        };
        let mut collaboration = CollaborationState::default();
        collaboration
            .workstreams
            .insert(workstream.id.clone(), workstream);
        collaboration
            .work_units
            .insert(work_unit.id.clone(), work_unit);
        collaboration
            .assignments
            .insert(assignment.id.clone(), assignment.clone());
        let record = build_assignment_communication_record(
            &collaboration,
            &assignment,
            None,
            None,
            None,
            now,
        )
        .expect("communication record");
        (assignment, record)
    }

    async fn assert_terminal_approval_path(
        decision_type: DecisionType,
        expected_work_unit_status: WorkUnitStatus,
        expected_workstream_status: WorkstreamStatus,
    ) {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, &format!("{decision_type:?}")).await;
        let proposal = create_proposal_for_decision(
            &service,
            &reasoner,
            &work_unit,
            &assignment,
            &report,
            decision_type,
        )
        .await
        .proposal;

        let response =
            approve_proposal(&service, &proposal.id, SupervisorProposalEdits::default()).await;

        assert_eq!(response.decision.decision_type, decision_type);
        assert!(response.next_assignment.is_none());
        assert_eq!(response.proposal.status, SupervisorProposalStatus::Approved);
        let state = service.state.read().await;
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].status,
            expected_work_unit_status
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id]
                .current_assignment_id
                .as_deref(),
            None
        );
        assert_eq!(
            state.collaboration.assignments[&assignment.id].status,
            AssignmentStatus::Closed
        );
        assert_eq!(
            state.collaboration.workstreams[&workstream.id].status,
            expected_workstream_status
        );
    }

    async fn assert_assignment_approval_path(decision_type: DecisionType) {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, &format!("{decision_type:?}")).await;
        let proposal = create_proposal_for_decision(
            &service,
            &reasoner,
            &work_unit,
            &assignment,
            &report,
            decision_type,
        )
        .await
        .proposal;

        let response =
            approve_proposal(&service, &proposal.id, SupervisorProposalEdits::default()).await;
        let next_assignment = response.next_assignment.expect("next assignment");
        assert_eq!(response.decision.decision_type, decision_type);
        assert_eq!(response.proposal.status, SupervisorProposalStatus::Approved);
        assert_eq!(next_assignment.status, AssignmentStatus::Created);
        assert_eq!(
            next_assignment.attempt_number,
            assignment.attempt_number + 1
        );
        let state = service.state.read().await;
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].status,
            WorkUnitStatus::Ready
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id]
                .current_assignment_id
                .as_deref(),
            Some(next_assignment.id.as_str())
        );
        assert_eq!(
            state.collaboration.assignments[&assignment.id].status,
            AssignmentStatus::Closed
        );
    }

    #[tokio::test]
    async fn proposal_create_persists_open_record_with_context_pack() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "create").await;
        let response =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report).await;

        assert_eq!(response.proposal.status, SupervisorProposalStatus::Open);
        assert_eq!(
            response
                .proposal
                .proposal
                .as_ref()
                .expect("model proposal")
                .proposed_decision
                .decision_type,
            DecisionType::Continue,
        );
        assert_eq!(
            response.proposal.context_pack.primary_work_unit.id,
            work_unit.id
        );
        assert_eq!(response.proposal.context_pack.source_report.id, report.id);
        assert_eq!(
            response.proposal.trigger.kind,
            SupervisorProposalTriggerKind::HumanRequested
        );
        assert_eq!(response.proposal.trigger.requested_by, "tester".to_string());

        let pack = reasoner.last_pack().await.expect("captured context pack");
        assert_eq!(
            pack.trigger.kind,
            SupervisorProposalTriggerKind::HumanRequested
        );
        assert_eq!(pack.trigger.source_report_id, report.id);
        assert!(
            pack.decision_policy
                .allowed_decisions
                .contains(&DecisionType::Continue)
        );

        let state = service.state.read().await;
        let stored = state
            .collaboration
            .supervisor_proposals
            .get(&response.proposal.id)
            .expect("stored proposal");
        assert_eq!(stored.status, SupervisorProposalStatus::Open);
        assert_eq!(stored.reasoner_backend, "test");
        let expected_prompt_render = reasoner
            .last_prompt_render()
            .await
            .expect("captured prompt render");
        let expected_response_artifact = reasoner
            .last_response_artifact()
            .await
            .expect("captured response artifact");
        assert_eq!(stored.prompt_render.as_ref(), Some(&expected_prompt_render));
        assert_eq!(
            stored.response_artifact.as_ref(),
            Some(&expected_response_artifact)
        );
        let prompt_render = stored.prompt_render.as_ref().expect("stored prompt render");
        assert_eq!(
            prompt_render.render_spec.template_version,
            crate::supervisor::SUPERVISOR_PROMPT_TEMPLATE_VERSION
        );
        assert!(
            prompt_render
                .instructions_text
                .contains("Orcas supervisor reasoner")
        );
        assert!(prompt_render.context_pack_text.contains(&work_unit.id));
        assert_eq!(prompt_render.request_body_hash, None);
    }

    #[tokio::test]
    async fn proposal_create_persists_prompt_render_artifact_on_open_record() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_manual_proposal_fixture(&service, "prompt-open").await;

        let response =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report).await;
        let stored = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        let expected_prompt_render = reasoner
            .last_prompt_render()
            .await
            .expect("captured prompt render");

        assert_eq!(response.proposal.id, stored.id);
        assert_eq!(stored.prompt_render.as_ref(), Some(&expected_prompt_render));
        let prompt_render = stored.prompt_render.as_ref().expect("stored prompt render");
        assert_eq!(
            prompt_render.render_spec.template_version,
            crate::supervisor::SUPERVISOR_PROMPT_TEMPLATE_VERSION
        );
        assert_eq!(
            prompt_render.render_spec.context_schema_version,
            stored.context_pack.schema_version
        );
        assert!(prompt_render.context_pack_text.contains(&report.id));
        assert!(
            prompt_render
                .user_content_text
                .contains("SupervisorContextPack:")
        );
        assert!(!prompt_render.prompt_hash.is_empty());
        let response_artifact = stored
            .response_artifact
            .as_ref()
            .expect("stored response artifact");
        assert_eq!(
            response_artifact.extracted_output_text.as_deref(),
            stored.reasoner_output_text.as_deref()
        );
        assert_eq!(response_artifact.backend_kind, stored.reasoner_backend);
        assert_eq!(response_artifact.model, stored.reasoner_model);
        assert_eq!(response_artifact.response_id, stored.reasoner_response_id);
        assert_eq!(response_artifact.usage, stored.reasoner_usage);
        assert!(!response_artifact.response_hash.is_empty());
        assert!(response_artifact.raw_response_body.is_some());
    }

    #[tokio::test]
    async fn proposal_approve_continue_creates_decision_and_next_assignment() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "approve").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;

        let response = service
            .proposal_approve(ipc::ProposalApproveRequest {
                proposal_id: proposal.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("Approved as proposed.".to_string()),
                edits: Default::default(),
            })
            .await
            .expect("proposal approve");

        let next_assignment = response.next_assignment.expect("next assignment");
        assert_eq!(response.decision.decision_type, DecisionType::Continue);
        assert_eq!(response.proposal.status, SupervisorProposalStatus::Approved);
        assert_eq!(
            response.proposal.approved_decision_id.as_deref(),
            Some(response.decision.id.as_str())
        );
        assert_eq!(
            response.proposal.approved_assignment_id.as_deref(),
            Some(next_assignment.id.as_str())
        );
        assert_eq!(next_assignment.status, AssignmentStatus::Created);
        assert_eq!(
            next_assignment.attempt_number,
            assignment.attempt_number + 1
        );
        assert!(
            next_assignment
                .instructions
                .contains("Objective: Resolve the remaining bounded follow-up.")
        );
        assert_eq!(
            response.proposal.approval_edits,
            Some(SupervisorProposalEdits::default())
        );
        assert_eq!(
            response
                .proposal
                .proposal
                .as_ref()
                .expect("original proposal")
                .proposed_decision
                .decision_type,
            DecisionType::Continue
        );
        assert_eq!(
            response
                .proposal
                .approved_proposal
                .as_ref()
                .expect("approved proposal")
                .proposed_decision
                .decision_type,
            DecisionType::Continue
        );

        let state = service.state.read().await;
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].status,
            WorkUnitStatus::Ready
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id]
                .current_assignment_id
                .as_deref(),
            Some(next_assignment.id.as_str())
        );
        assert_eq!(
            state.collaboration.assignments[&assignment.id].status,
            AssignmentStatus::Closed
        );
        assert_eq!(
            state.collaboration.assignments[&next_assignment.id].worker_id,
            assignment.worker_id
        );
    }

    #[tokio::test]
    async fn proposal_approve_continue_with_manual_fixture_creates_next_assignment() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_manual_proposal_fixture(&service, "approve-manual").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;

        let response =
            approve_proposal(&service, &proposal.id, SupervisorProposalEdits::default()).await;

        assert_eq!(response.decision.decision_type, DecisionType::Continue);
        assert_eq!(response.proposal.status, SupervisorProposalStatus::Approved);
        assert!(response.next_assignment.is_some());
        let stored = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        assert!(stored.prompt_render.is_some());
        assert!(stored.response_artifact.is_some());
    }

    #[tokio::test]
    async fn proposal_becomes_stale_after_newer_report_and_cannot_be_approved() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "stale").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;

        {
            let newer_time = Utc::now();
            let mut state = service.state.write().await;
            let newer_report = Report {
                id: format!("report-{}", Uuid::new_v4().simple()),
                work_unit_id: work_unit.id.clone(),
                assignment_id: assignment.id.clone(),
                worker_id: assignment.worker_id.clone(),
                disposition: ReportDisposition::Completed,
                summary: "A newer report superseded the earlier decision point.".to_string(),
                findings: vec!["The second pass changed the state anchor.".to_string()],
                blockers: Vec::new(),
                questions: Vec::new(),
                recommended_next_actions: vec!["Regenerate the proposal.".to_string()],
                confidence: ReportConfidence::High,
                raw_output: "newer raw worker output".to_string(),
                parse_result: ReportParseResult::Parsed,
                needs_supervisor_review: false,
                created_at: newer_time,
            };
            state
                .collaboration
                .reports
                .insert(newer_report.id.clone(), newer_report.clone());
            let work_unit_entry = state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .expect("work unit");
            work_unit_entry.latest_report_id = Some(newer_report.id);
            work_unit_entry.updated_at = newer_time;
        }
        service
            .persist_collaboration_state()
            .await
            .expect("persist newer report");

        let get_response = service
            .proposal_get(ipc::ProposalGetRequest {
                proposal_id: proposal.id.clone(),
            })
            .await
            .expect("proposal get");
        assert_eq!(
            get_response.proposal.status,
            SupervisorProposalStatus::Stale
        );

        let error = service
            .proposal_approve(ipc::ProposalApproveRequest {
                proposal_id: proposal.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
                edits: Default::default(),
            })
            .await
            .expect_err("stale proposal should not approve");
        assert!(
            error
                .to_string()
                .contains("is not open and cannot be approved")
        );
    }

    #[tokio::test]
    async fn proposal_approve_accept_marks_work_unit_accepted() {
        assert_terminal_approval_path(
            DecisionType::Accept,
            WorkUnitStatus::Accepted,
            WorkstreamStatus::Active,
        )
        .await;
    }

    #[tokio::test]
    async fn proposal_approve_redirect_creates_next_assignment() {
        assert_assignment_approval_path(DecisionType::Redirect).await;
    }

    #[tokio::test]
    async fn proposal_approve_mark_complete_completes_workstream() {
        assert_terminal_approval_path(
            DecisionType::MarkComplete,
            WorkUnitStatus::Completed,
            WorkstreamStatus::Completed,
        )
        .await;
    }

    #[tokio::test]
    async fn proposal_approve_escalate_to_human_blocks_workstream() {
        assert_terminal_approval_path(
            DecisionType::EscalateToHuman,
            WorkUnitStatus::NeedsHuman,
            WorkstreamStatus::Blocked,
        )
        .await;
    }

    #[tokio::test]
    async fn proposal_reject_is_durable_and_blocks_later_approval() {
        let base = std::env::temp_dir().join(format!("orcas-collab-reject-{}", Uuid::new_v4()));
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_at_with_reasoner(base.clone(), reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "reject").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;

        let reject_response = service
            .proposal_reject(ipc::ProposalRejectRequest {
                proposal_id: proposal.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("reject for now".to_string()),
            })
            .await
            .expect("proposal reject");
        assert_eq!(
            reject_response.proposal.status,
            SupervisorProposalStatus::Rejected
        );

        let approve_error = service
            .proposal_approve(ipc::ProposalApproveRequest {
                proposal_id: proposal.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
                edits: Default::default(),
            })
            .await
            .expect_err("rejected proposal should not approve");
        assert!(
            approve_error
                .to_string()
                .contains("is not open and cannot be approved")
        );

        drop(service);
        let restarted = test_service_at(base).await;
        let stored = restarted
            .proposal_get(ipc::ProposalGetRequest {
                proposal_id: proposal.id.clone(),
            })
            .await
            .expect("proposal get after restart");
        assert_eq!(stored.proposal.status, SupervisorProposalStatus::Rejected);
        assert_eq!(stored.proposal.reviewed_by.as_deref(), Some("reviewer"));
    }

    #[tokio::test]
    async fn proposal_edit_before_approve_preserves_original_and_records_edits() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "edit").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;

        let edits = SupervisorProposalEdits {
            decision_type: Some(DecisionType::Redirect),
            decision_rationale: Some(
                "Use redirect so the next pass is explicitly redirected.".to_string(),
            ),
            preferred_worker_id: Some(assignment.worker_id.clone()),
            worker_kind: Some("codex".to_string()),
            objective: Some("Verify the redirected reconnect branch only.".to_string()),
            instructions: vec![
                "Inspect only the redirected reconnect branch.".to_string(),
                "Do not broaden beyond the reconnect decision point.".to_string(),
            ],
            acceptance_criteria: vec!["The redirected branch behavior is confirmed.".to_string()],
            stop_conditions: vec!["Stop if product intent is unclear.".to_string()],
            expected_report_fields: vec!["summary".to_string(), "findings".to_string()],
        };
        let response = approve_proposal(&service, &proposal.id, edits.clone()).await;

        let next_assignment = response.next_assignment.expect("redirect assignment");
        let original = response
            .proposal
            .proposal
            .as_ref()
            .expect("original model proposal");
        let approved = response
            .proposal
            .approved_proposal
            .as_ref()
            .expect("approved proposal");

        assert_eq!(
            original.proposed_decision.decision_type,
            DecisionType::Continue
        );
        assert_eq!(
            approved.proposed_decision.decision_type,
            DecisionType::Redirect
        );
        assert_eq!(response.proposal.approval_edits, Some(edits));
        assert_eq!(response.decision.decision_type, DecisionType::Redirect);
        assert!(
            next_assignment
                .instructions
                .contains("Objective: Verify the redirected reconnect branch only.")
        );
    }

    #[tokio::test]
    async fn proposal_superseded_by_successor_cannot_be_approved() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "supersede").await;
        let first = create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
            .await
            .proposal;

        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Continue,
                &work_unit.id,
                &report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;
        let second = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: work_unit.id.clone(),
                source_report_id: Some(report.id.clone()),
                requested_by: Some("tester".to_string()),
                note: Some("resynthesize".to_string()),
                supersede_open: true,
            })
            .await
            .expect("second proposal")
            .proposal;

        let first_record = service
            .proposal_get(ipc::ProposalGetRequest {
                proposal_id: first.id.clone(),
            })
            .await
            .expect("first proposal get");
        assert_eq!(
            first_record.proposal.status,
            SupervisorProposalStatus::Superseded
        );
        let approve_error = service
            .proposal_approve(ipc::ProposalApproveRequest {
                proposal_id: first.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
                edits: Default::default(),
            })
            .await
            .expect_err("superseded proposal should not approve");
        assert!(
            approve_error
                .to_string()
                .contains("is not open and cannot be approved")
        );
        assert_eq!(second.status, SupervisorProposalStatus::Open);
    }

    #[tokio::test]
    async fn supersede_open_failure_keeps_existing_open_proposal() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "supersede-fail").await;
        let first = create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
            .await
            .proposal;

        reasoner
            .set_failure(
                SupervisorProposalFailureStage::Backend,
                "timeout contacting test provider",
                None,
            )
            .await;
        let error = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: work_unit.id.clone(),
                source_report_id: Some(report.id.clone()),
                requested_by: Some("tester".to_string()),
                note: Some("retry create".to_string()),
                supersede_open: true,
            })
            .await
            .expect_err("generation failure");
        assert!(error.to_string().contains("inspect proposal"));

        let first_record = service
            .proposal_get(ipc::ProposalGetRequest {
                proposal_id: first.id.clone(),
            })
            .await
            .expect("first proposal get");
        assert_eq!(first_record.proposal.status, SupervisorProposalStatus::Open);

        let state = service.state.read().await;
        let failed = state
            .collaboration
            .supervisor_proposals
            .values()
            .find(|proposal| proposal.status == SupervisorProposalStatus::GenerationFailed)
            .expect("failed proposal record");
        assert_eq!(
            failed.generation_failure.as_ref().expect("failure").stage,
            SupervisorProposalFailureStage::Backend
        );
    }

    #[tokio::test]
    async fn duplicate_open_proposal_without_supersede_is_rejected() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "duplicate-create").await;
        let _ = create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
            .await
            .proposal;

        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Continue,
                &work_unit.id,
                &report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;
        let error = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: work_unit.id.clone(),
                source_report_id: Some(report.id.clone()),
                requested_by: Some("tester".to_string()),
                note: None,
                supersede_open: false,
            })
            .await
            .expect_err("duplicate open proposal should be rejected");
        assert!(
            error
                .to_string()
                .contains("an open proposal already exists")
        );
    }

    #[tokio::test]
    async fn proposal_stales_after_authoritative_decision_and_cannot_be_rejected() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "decision-stale").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;

        let _ = service
            .decision_apply(ipc::DecisionApplyRequest {
                work_unit_id: work_unit.id.clone(),
                report_id: Some(report.id.clone()),
                decision_type: DecisionType::EscalateToHuman,
                rationale: "a later authoritative decision was already applied".to_string(),
                instructions: None,
                worker_id: None,
                worker_kind: None,
            })
            .await
            .expect("authoritative decision");

        let refreshed = service
            .proposal_get(ipc::ProposalGetRequest {
                proposal_id: proposal.id.clone(),
            })
            .await
            .expect("proposal get");
        assert_eq!(refreshed.proposal.status, SupervisorProposalStatus::Stale);

        let reject_error = service
            .proposal_reject(ipc::ProposalRejectRequest {
                proposal_id: proposal.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("stale proposal should not reject");
        assert!(
            reject_error
                .to_string()
                .contains("is not open and cannot be rejected")
        );
    }

    #[tokio::test]
    async fn proposal_stales_after_current_assignment_changes() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "assignment-stale").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;

        {
            let now = Utc::now();
            let mut state = service.state.write().await;
            let successor = Assignment {
                id: format!("assignment-{}", Uuid::new_v4().simple()),
                work_unit_id: work_unit.id.clone(),
                worker_id: assignment.worker_id.clone(),
                worker_session_id: assignment.worker_session_id.clone(),
                instructions: "manual replacement".to_string(),
                communication_seed: None,
                status: AssignmentStatus::Created,
                attempt_number: assignment.attempt_number + 1,
                created_at: now,
                updated_at: now,
            };
            state
                .collaboration
                .assignments
                .insert(successor.id.clone(), successor.clone());
            let work_unit_entry = state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .expect("work unit");
            work_unit_entry.current_assignment_id = Some(successor.id);
            work_unit_entry.updated_at = now;
        }
        service
            .persist_collaboration_state()
            .await
            .expect("persist stale assignment");

        let refreshed = service
            .proposal_get(ipc::ProposalGetRequest {
                proposal_id: proposal.id.clone(),
            })
            .await
            .expect("proposal get");
        assert_eq!(refreshed.proposal.status, SupervisorProposalStatus::Stale);
    }

    #[tokio::test]
    async fn backend_generation_failure_persists_failed_record() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, _assignment, report) =
            seed_awaiting_decision_fixture(&service, "backend-fail").await;

        reasoner
            .set_failure(
                SupervisorProposalFailureStage::Backend,
                "timeout contacting provider",
                Some("provider timeout".to_string()),
            )
            .await;
        let error = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: work_unit.id.clone(),
                source_report_id: Some(report.id.clone()),
                requested_by: Some("tester".to_string()),
                note: None,
                supersede_open: false,
            })
            .await
            .expect_err("backend failure");
        assert!(error.to_string().contains("inspect proposal"));

        let failed = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        assert_eq!(failed.status, SupervisorProposalStatus::GenerationFailed);
        assert!(failed.proposal.is_none());
        assert_eq!(
            failed.generation_failure.as_ref().expect("failure").stage,
            SupervisorProposalFailureStage::Backend
        );
        assert_eq!(
            failed.reasoner_output_text.as_deref(),
            Some("provider timeout")
        );
        let expected_prompt_render = reasoner
            .last_prompt_render()
            .await
            .expect("captured failed prompt render");
        assert_eq!(failed.prompt_render.as_ref(), Some(&expected_prompt_render));
        let expected_response_artifact = reasoner
            .last_response_artifact()
            .await
            .expect("captured failed response artifact");
        assert_eq!(
            failed.response_artifact.as_ref(),
            Some(&expected_response_artifact)
        );
    }

    #[tokio::test]
    async fn backend_generation_failure_with_manual_fixture_persists_prompt_render() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, _assignment, report) =
            seed_manual_proposal_fixture(&service, "backend-fail-manual").await;

        reasoner
            .set_failure(
                SupervisorProposalFailureStage::Backend,
                "timeout contacting provider",
                Some("provider timeout".to_string()),
            )
            .await;
        let _ = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: work_unit.id.clone(),
                source_report_id: Some(report.id.clone()),
                requested_by: Some("tester".to_string()),
                note: None,
                supersede_open: false,
            })
            .await
            .expect_err("backend failure");

        let failed = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        let expected_prompt_render = reasoner
            .last_prompt_render()
            .await
            .expect("captured failed prompt render");
        let expected_response_artifact = reasoner
            .last_response_artifact()
            .await
            .expect("captured failed response artifact");
        assert_eq!(failed.status, SupervisorProposalStatus::GenerationFailed);
        assert_eq!(failed.prompt_render.as_ref(), Some(&expected_prompt_render));
        assert_eq!(
            failed.response_artifact.as_ref(),
            Some(&expected_response_artifact)
        );
    }

    #[tokio::test]
    async fn malformed_output_generation_failure_with_manual_fixture_persists_response_artifact() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, _assignment, report) =
            seed_manual_proposal_fixture(&service, "malformed-fail-manual").await;

        reasoner
            .set_failure(
                SupervisorProposalFailureStage::ProposalMalformed,
                "failed to decode supervisor proposal JSON: missing field `summary`",
                Some("{\"schema_version\":\"supervisor_proposal.v1\"}".to_string()),
            )
            .await;
        let _ = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: work_unit.id.clone(),
                source_report_id: Some(report.id.clone()),
                requested_by: Some("tester".to_string()),
                note: None,
                supersede_open: false,
            })
            .await
            .expect_err("malformed output failure");

        let failed = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        let expected_response_artifact = reasoner
            .last_response_artifact()
            .await
            .expect("captured failed response artifact");
        assert_eq!(failed.status, SupervisorProposalStatus::GenerationFailed);
        assert_eq!(
            failed.generation_failure.as_ref().expect("failure").stage,
            SupervisorProposalFailureStage::ProposalMalformed
        );
        assert_eq!(
            failed.response_artifact.as_ref(),
            Some(&expected_response_artifact)
        );
        assert_eq!(
            failed
                .response_artifact
                .as_ref()
                .and_then(|artifact| artifact.extracted_output_text.as_deref()),
            Some("{\"schema_version\":\"supervisor_proposal.v1\"}")
        );
    }

    #[tokio::test]
    async fn malformed_output_generation_failure_persists_failed_record() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, _assignment, report) =
            seed_awaiting_decision_fixture(&service, "malformed-fail").await;

        reasoner
            .set_failure(
                SupervisorProposalFailureStage::ProposalMalformed,
                "failed to decode supervisor proposal JSON: missing field `summary`",
                Some("{\"schema_version\":\"supervisor_proposal.v1\"}".to_string()),
            )
            .await;
        let _ = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: work_unit.id.clone(),
                source_report_id: Some(report.id.clone()),
                requested_by: Some("tester".to_string()),
                note: None,
                supersede_open: false,
            })
            .await
            .expect_err("malformed output failure");

        let failed = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        assert_eq!(failed.status, SupervisorProposalStatus::GenerationFailed);
        assert_eq!(
            failed.generation_failure.as_ref().expect("failure").stage,
            SupervisorProposalFailureStage::ProposalMalformed
        );
        assert!(failed.proposal.is_none());
        assert!(failed.response_artifact.is_some());
        assert_eq!(
            failed
                .response_artifact
                .as_ref()
                .and_then(|artifact| artifact.extracted_output_text.as_deref()),
            Some("{\"schema_version\":\"supervisor_proposal.v1\"}")
        );
    }

    #[tokio::test]
    async fn policy_invalid_decision_generation_failure_persists_failed_record() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "policy-fail").await;
        {
            let mut state = service.state.write().await;
            let worker_session = state
                .collaboration
                .worker_sessions
                .get_mut(&assignment.worker_session_id)
                .expect("worker session");
            worker_session.attachability = WorkerSessionAttachability::Unknown;
        }
        service
            .persist_collaboration_state()
            .await
            .expect("persist");

        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::MarkComplete,
                &work_unit.id,
                &report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;
        let _ = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: work_unit.id.clone(),
                source_report_id: Some(report.id.clone()),
                requested_by: Some("tester".to_string()),
                note: None,
                supersede_open: false,
            })
            .await
            .expect_err("policy validation failure");

        let failed = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        assert_eq!(failed.status, SupervisorProposalStatus::GenerationFailed);
        assert_eq!(
            failed.generation_failure.as_ref().expect("failure").stage,
            SupervisorProposalFailureStage::ProposalValidation
        );
        assert_eq!(
            failed
                .proposal
                .as_ref()
                .expect("invalid proposal kept for inspection")
                .proposed_decision
                .decision_type,
            DecisionType::MarkComplete
        );
        assert!(failed.response_artifact.is_some());
    }

    #[tokio::test]
    async fn invalid_draft_generation_failure_persists_failed_record() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "draft-fail").await;
        let mut invalid = sample_proposal_for_decision(
            DecisionType::Continue,
            &work_unit.id,
            &report.id,
            &assignment.id,
            &assignment.worker_id,
        );
        invalid
            .draft_next_assignment
            .as_mut()
            .expect("draft")
            .instructions
            .clear();
        reasoner.set_proposal(invalid).await;

        let _ = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: work_unit.id.clone(),
                source_report_id: Some(report.id.clone()),
                requested_by: Some("tester".to_string()),
                note: None,
                supersede_open: false,
            })
            .await
            .expect_err("invalid draft failure");

        let failed = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        assert_eq!(failed.status, SupervisorProposalStatus::GenerationFailed);
        assert_eq!(
            failed.generation_failure.as_ref().expect("failure").stage,
            SupervisorProposalFailureStage::ProposalValidation
        );
    }

    #[tokio::test]
    async fn double_approve_and_reject_after_approve_fail_closed() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "double-approve").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;

        let _ = approve_proposal(&service, &proposal.id, SupervisorProposalEdits::default()).await;

        let second_approve = service
            .proposal_approve(ipc::ProposalApproveRequest {
                proposal_id: proposal.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
                edits: Default::default(),
            })
            .await
            .expect_err("second approve should fail");
        assert!(
            second_approve
                .to_string()
                .contains("is not open and cannot be approved")
        );

        let reject_after_approve = service
            .proposal_reject(ipc::ProposalRejectRequest {
                proposal_id: proposal.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("reject after approve should fail");
        assert!(
            reject_after_approve
                .to_string()
                .contains("is not open and cannot be rejected")
        );
    }

    #[tokio::test]
    async fn full_assignment_runtime_path_creates_auto_proposal_with_fake_codex_runtime() {
        let reasoner = Arc::new(PackDrivenSupervisorReasoner::new(DecisionType::Continue));
        let (service, fake_runtime_state) = test_service_with_fake_codex_runtime_capture(
            auto_proposal_config(true),
            reasoner.clone(),
            sample_runtime_report_output_template(),
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        let mut events = service.event_tx.subscribe();
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Full runtime".to_string(),
                objective: "Exercise the assignment runtime path with fake Codex".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Runtime work unit".to_string(),
                task_statement: "Run one bounded worker step and stop with a report.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;

        let response = service
            .assignment_start(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-runtime".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the bounded task and report honestly.".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment runtime");

        let recorded_report = wait_for_report_recorded(&mut events).await;
        let proposal_event =
            wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Created).await;

        assert_eq!(response.assignment.work_unit_id, work_unit.id);
        assert_eq!(
            response.assignment.status,
            AssignmentStatus::AwaitingDecision
        );
        assert_eq!(response.worker.id, "worker-runtime");
        assert_eq!(response.worker.status, WorkerStatus::Idle);
        assert_eq!(response.worker.current_assignment_id, None);
        assert_eq!(
            response.assignment.worker_session_id,
            response.worker_session.id
        );
        assert_eq!(response.worker_session.worker_id, response.worker.id);
        assert!(response.worker_session.thread_id.is_some());
        assert_eq!(response.worker_session.active_turn_id, None);
        assert_eq!(
            response.worker_session.runtime_status,
            WorkerSessionRuntimeStatus::Completed
        );
        assert_eq!(
            response.worker_session.attachability,
            WorkerSessionAttachability::NotAttachable
        );
        assert_eq!(response.report.assignment_id, response.assignment.id);
        assert_eq!(response.report.work_unit_id, work_unit.id);
        assert_eq!(response.report.parse_result, ReportParseResult::Parsed);
        assert_eq!(
            response.report.findings,
            vec!["root cause isolated".to_string()]
        );
        assert_eq!(recorded_report.id, response.report.id);
        assert_eq!(reasoner.propose_call_count(), 1);
        assert_eq!(proposal_event.source_report_id, response.report.id);

        let snapshot = service.snapshot().await.expect("snapshot");
        let summary = snapshot
            .collaboration
            .work_units
            .iter()
            .find(|summary| summary.id == work_unit.id)
            .and_then(|summary| summary.proposal.as_ref())
            .expect("proposal summary");
        assert!(summary.has_open_proposal);
        assert_eq!(
            summary.latest_proposed_decision_type,
            Some(DecisionType::Continue)
        );

        let detail = service
            .workunit_get(ipc::WorkunitGetRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("workunit detail");
        assert_eq!(detail.reports.len(), 1);
        assert_eq!(detail.proposals.len(), 1);

        let proposal = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        assert_eq!(proposal.status, SupervisorProposalStatus::Open);
        assert_eq!(
            proposal.trigger.kind,
            SupervisorProposalTriggerKind::ReportRecorded
        );

        let state = service.state.read().await;
        assert!(state.collaboration.decisions.is_empty());
        assert_eq!(state.collaboration.assignments.len(), 1);
        let communication = state
            .collaboration
            .assignment_communications
            .get(&response.assignment.id)
            .expect("assignment communication");
        assert_eq!(communication.assignment_id, response.assignment.id);
        assert_eq!(
            communication.packet.task_mode,
            orcas_core::AssignmentTaskMode::Implement
        );
        assert_eq!(
            communication.prompt_render.render_spec.template_version,
            "assignment_prompt.v1"
        );
        assert_eq!(
            communication
                .validation
                .as_ref()
                .expect("validation")
                .parse_result,
            ReportParseResult::Parsed
        );
        assert_eq!(
            communication
                .response_envelope
                .as_ref()
                .expect("response envelope")
                .assignment_id,
            response.assignment.id
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].status,
            WorkUnitStatus::AwaitingDecision
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].current_assignment_id,
            Some(response.assignment.id.clone())
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].latest_report_id,
            Some(response.report.id.clone())
        );
        drop(state);

        let sent_prompt = fake_runtime_state
            .lock()
            .await
            .last_turn_start_text
            .clone()
            .expect("sent prompt");
        let stored_prompt = service
            .state
            .read()
            .await
            .collaboration
            .assignment_communications
            .get(&response.assignment.id)
            .expect("communication record")
            .prompt_render
            .prompt_text
            .clone();
        assert_eq!(sent_prompt, stored_prompt);
    }

    #[tokio::test]
    async fn full_assignment_runtime_path_skips_auto_proposal_when_disabled() {
        let reasoner = Arc::new(PackDrivenSupervisorReasoner::new(DecisionType::Continue));
        let service = test_service_with_fake_codex_runtime(
            auto_proposal_config(false),
            reasoner.clone(),
            sample_runtime_report_output_template(),
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Full runtime disabled".to_string(),
                objective: "Exercise runtime path without auto proposals".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Runtime work unit".to_string(),
                task_statement: "Run one bounded worker step and stop with a report.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;

        let response = service
            .assignment_start(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-runtime-disabled".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the bounded task and report honestly.".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment runtime");

        assert_eq!(
            response.assignment.status,
            AssignmentStatus::AwaitingDecision
        );
        assert_eq!(response.report.work_unit_id, work_unit.id);
        assert_eq!(reasoner.propose_call_count(), 0);

        let proposals = service
            .proposal_list_for_workunit(ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("proposal list");
        assert!(proposals.proposals.is_empty());

        let state = service.state.read().await;
        assert!(state.collaboration.decisions.is_empty());
        assert_eq!(state.collaboration.assignments.len(), 1);
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].latest_report_id,
            Some(response.report.id.clone())
        );
    }

    #[tokio::test]
    async fn full_assignment_runtime_path_preserves_interrupted_turn_semantics_with_fake_codex_runtime()
     {
        let reasoner = Arc::new(PackDrivenSupervisorReasoner::new(DecisionType::Continue));
        let service = test_service_with_fake_codex_runtime(
            auto_proposal_config(true),
            reasoner.clone(),
            "partial raw output",
            FakeCodexTerminalOutcome::Interrupted,
        )
        .await;
        let mut events = service.event_tx.subscribe();
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Interrupted runtime".to_string(),
                objective: "Exercise the interrupted assignment runtime path".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Interrupted runtime work unit".to_string(),
                task_statement: "Run one bounded worker step and interrupt before completion."
                    .to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;

        let response = service
            .assignment_start(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-runtime-interrupted".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Attempt one bounded step and stop if interrupted.".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment runtime");

        let recorded_report = wait_for_report_recorded(&mut events).await;
        let proposal_event =
            wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Created).await;

        assert_eq!(response.assignment.work_unit_id, work_unit.id);
        assert_eq!(response.assignment.status, AssignmentStatus::Interrupted);
        assert_eq!(response.worker.id, "worker-runtime-interrupted");
        assert_eq!(response.worker.status, WorkerStatus::Idle);
        assert_eq!(response.worker.current_assignment_id, None);
        assert_eq!(
            response.assignment.worker_session_id,
            response.worker_session.id
        );
        assert_eq!(response.worker_session.worker_id, response.worker.id);
        assert!(response.worker_session.thread_id.is_some());
        assert_eq!(response.worker_session.active_turn_id, None);
        assert_eq!(
            response.worker_session.runtime_status,
            WorkerSessionRuntimeStatus::Interrupted
        );
        assert_eq!(
            response.worker_session.attachability,
            WorkerSessionAttachability::NotAttachable
        );

        assert_eq!(response.report.assignment_id, response.assignment.id);
        assert_eq!(response.report.work_unit_id, work_unit.id);
        assert_eq!(response.report.disposition, ReportDisposition::Interrupted);
        assert_eq!(
            response.report.summary,
            "Execution was interrupted. Raw output was retained for supervisor review."
        );
        assert_eq!(response.report.parse_result, ReportParseResult::Invalid);
        assert_eq!(response.report.confidence, ReportConfidence::Unknown);
        assert!(response.report.needs_supervisor_review);
        assert!(response.report.findings.is_empty());
        assert!(response.report.blockers.is_empty());
        assert!(response.report.questions.is_empty());
        assert!(response.report.recommended_next_actions.is_empty());
        assert_eq!(response.report.raw_output, "partial raw output");

        assert_eq!(recorded_report.id, response.report.id);
        assert_eq!(reasoner.propose_call_count(), 1);
        assert_eq!(proposal_event.source_report_id, response.report.id);

        let snapshot = service.snapshot().await.expect("snapshot");
        let summary = snapshot
            .collaboration
            .work_units
            .iter()
            .find(|summary| summary.id == work_unit.id)
            .and_then(|summary| summary.proposal.as_ref())
            .expect("proposal summary");
        assert!(summary.has_open_proposal);
        assert_eq!(
            summary.latest_proposed_decision_type,
            Some(DecisionType::Continue)
        );

        let detail = service
            .workunit_get(ipc::WorkunitGetRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("workunit detail");
        assert_eq!(detail.reports.len(), 1);
        assert_eq!(detail.proposals.len(), 1);

        let proposal = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        assert_eq!(proposal.status, SupervisorProposalStatus::Open);
        assert_eq!(
            proposal.trigger.kind,
            SupervisorProposalTriggerKind::ReportRecorded
        );
        assert_eq!(proposal.source_report_id, response.report.id);

        let state = service.state.read().await;
        assert!(state.collaboration.decisions.is_empty());
        assert_eq!(state.collaboration.assignments.len(), 1);
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].status,
            WorkUnitStatus::AwaitingDecision
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].current_assignment_id,
            Some(response.assignment.id.clone())
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].latest_report_id,
            Some(response.report.id.clone())
        );
    }

    #[tokio::test]
    async fn real_report_ingestion_path_respects_auto_proposal_config_when_disabled() {
        let reasoner = Arc::new(PackDrivenSupervisorReasoner::new(DecisionType::Continue));
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(false))
                .await;
        let (_workstream, work_unit, assignment, turn_state, raw_output) =
            seed_running_assignment_fixture(&service, "real-ingest-disabled").await;

        let (report, assignment_after_report, work_unit_after_report) = service
            .ingest_assignment_turn_outcome(
                &assignment.id,
                &assignment.worker_id,
                &assignment.worker_session_id,
                turn_state,
                raw_output,
            )
            .await
            .expect("ingest report");

        assert_eq!(report.work_unit_id, work_unit.id);
        assert_eq!(
            assignment_after_report.status,
            AssignmentStatus::AwaitingDecision
        );
        assert_eq!(
            work_unit_after_report.status,
            WorkUnitStatus::AwaitingDecision
        );
        assert_eq!(reasoner.propose_call_count(), 0);

        let proposals = service
            .proposal_list_for_workunit(ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("proposal list");
        assert!(proposals.proposals.is_empty());

        let state = service.state.read().await;
        assert!(state.collaboration.decisions.is_empty());
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].latest_report_id,
            Some(report.id.clone())
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].current_assignment_id,
            Some(assignment.id.clone())
        );
    }

    #[tokio::test]
    async fn real_report_ingestion_path_creates_auto_proposal_and_suppresses_redundant_trigger() {
        let reasoner = Arc::new(PackDrivenSupervisorReasoner::new(DecisionType::Continue));
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(true))
                .await;
        let mut events = service.event_tx.subscribe();
        let (_workstream, work_unit, assignment, turn_state, raw_output) =
            seed_running_assignment_fixture(&service, "real-ingest-enabled").await;

        let (report, assignment_after_report, work_unit_after_report) = service
            .ingest_assignment_turn_outcome(
                &assignment.id,
                &assignment.worker_id,
                &assignment.worker_session_id,
                turn_state,
                raw_output,
            )
            .await
            .expect("ingest report");

        let recorded_report = wait_for_report_recorded(&mut events).await;
        assert_eq!(recorded_report.id, report.id);
        let proposal_event =
            wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Created).await;

        assert_eq!(
            assignment_after_report.status,
            AssignmentStatus::AwaitingDecision
        );
        assert_eq!(
            work_unit_after_report.status,
            WorkUnitStatus::AwaitingDecision
        );
        assert_eq!(reasoner.propose_call_count(), 1);
        assert_eq!(proposal_event.source_report_id, report.id);

        let stored = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        assert_eq!(stored.status, SupervisorProposalStatus::Open);
        assert_eq!(stored.source_report_id, report.id);
        assert_eq!(
            stored.trigger.kind,
            SupervisorProposalTriggerKind::ReportRecorded
        );

        let snapshot = service.snapshot().await.expect("snapshot");
        let summary = snapshot
            .collaboration
            .work_units
            .iter()
            .find(|summary| summary.id == work_unit.id)
            .and_then(|summary| summary.proposal.as_ref())
            .expect("proposal summary");
        assert!(summary.has_open_proposal);
        assert_eq!(
            summary.latest_proposed_decision_type,
            Some(DecisionType::Continue)
        );

        let state = service.state.read().await;
        assert!(state.collaboration.decisions.is_empty());
        assert_eq!(state.collaboration.assignments.len(), 1);
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].current_assignment_id,
            Some(assignment.id.clone())
        );
        drop(state);

        service.maybe_auto_create_proposal_for_report(&report).await;

        let proposals = service
            .proposal_list_for_workunit(ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("proposal list");
        assert_eq!(proposals.proposals.len(), 1);
        assert_eq!(reasoner.propose_call_count(), 1);
    }

    #[tokio::test]
    async fn auto_proposal_disabled_does_not_generate_on_report_recorded() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Continue,
                "wu-disabled",
                "report-disabled",
                "assignment-disabled",
                "worker-disabled",
            ))
            .await;
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(false))
                .await;
        let (_workstream, work_unit, _assignment, report) =
            seed_awaiting_decision_fixture(&service, "auto-disabled").await;

        service.maybe_auto_create_proposal_for_report(&report).await;

        let proposals = service
            .proposal_list_for_workunit(ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("proposal list");
        assert!(proposals.proposals.is_empty());
        assert_eq!(reasoner.propose_call_count(), 0);
        assert!(reasoner.last_pack().await.is_none());
    }

    #[tokio::test]
    async fn auto_proposal_enabled_generates_for_eligible_report_and_emits_created_event() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(true))
                .await;
        let mut events = service.event_tx.subscribe();
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "auto-enabled").await;
        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Continue,
                &work_unit.id,
                &report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;

        service.maybe_auto_create_proposal_for_report(&report).await;

        let stored = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        assert_eq!(stored.status, SupervisorProposalStatus::Open);
        assert_eq!(
            stored.trigger.kind,
            SupervisorProposalTriggerKind::ReportRecorded
        );
        let state = service.state.read().await;
        assert!(state.collaboration.decisions.is_empty());
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].current_assignment_id,
            Some(assignment.id.clone())
        );
        drop(state);
        assert_eq!(reasoner.propose_call_count(), 1);

        let event =
            wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Created).await;
        assert_eq!(event.primary_work_unit_id, work_unit.id);

        let snapshot = service.snapshot().await.expect("snapshot");
        let summary = snapshot
            .collaboration
            .work_units
            .iter()
            .find(|summary| summary.id == work_unit.id)
            .and_then(|summary| summary.proposal.as_ref())
            .expect("proposal summary");
        assert!(summary.has_open_proposal);
        assert_eq!(
            summary.latest_proposed_decision_type,
            Some(DecisionType::Continue)
        );
    }

    #[tokio::test]
    async fn repeated_auto_trigger_does_not_create_duplicate_open_proposals() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(true))
                .await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "auto-repeat").await;
        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Continue,
                &work_unit.id,
                &report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;

        service.maybe_auto_create_proposal_for_report(&report).await;
        service.maybe_auto_create_proposal_for_report(&report).await;

        let proposals = service
            .proposal_list_for_workunit(ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("proposal list");
        assert_eq!(proposals.proposals.len(), 1);
        assert_eq!(
            proposals.proposals[0].status,
            SupervisorProposalStatus::Open
        );
        assert_eq!(reasoner.propose_call_count(), 1);
    }

    #[tokio::test]
    async fn existing_manual_open_proposal_suppresses_auto_generation_for_same_report() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(true))
                .await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "auto-existing-open").await;
        let _manual =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;
        assert_eq!(reasoner.propose_call_count(), 1);

        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Redirect,
                &work_unit.id,
                &report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;
        service.maybe_auto_create_proposal_for_report(&report).await;

        let proposals = service
            .proposal_list_for_workunit(ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("proposal list");
        assert_eq!(proposals.proposals.len(), 1);
        assert_eq!(reasoner.propose_call_count(), 1);
    }

    #[tokio::test]
    async fn rejected_same_report_proposal_suppresses_auto_generation() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(true))
                .await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "auto-rejected").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;
        let _ = service
            .proposal_reject(ipc::ProposalRejectRequest {
                proposal_id: proposal.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("reject".to_string()),
            })
            .await
            .expect("reject");
        assert_eq!(reasoner.propose_call_count(), 1);

        service.maybe_auto_create_proposal_for_report(&report).await;

        let proposals = service
            .proposal_list_for_workunit(ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("proposal list");
        assert_eq!(proposals.proposals.len(), 1);
        assert_eq!(
            proposals.proposals[0].status,
            SupervisorProposalStatus::Rejected
        );
        assert_eq!(reasoner.propose_call_count(), 1);
    }

    #[tokio::test]
    async fn auto_generation_failed_record_suppresses_repeated_same_report_trigger() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(true))
                .await;
        let (_workstream, work_unit, _assignment, report) =
            seed_awaiting_decision_fixture(&service, "auto-failed-repeat").await;
        reasoner
            .set_failure(
                SupervisorProposalFailureStage::Backend,
                "timeout contacting provider",
                Some("provider timeout".to_string()),
            )
            .await;

        service.maybe_auto_create_proposal_for_report(&report).await;
        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Continue,
                &work_unit.id,
                &report.id,
                &report.assignment_id,
                &report.worker_id,
            ))
            .await;
        service.maybe_auto_create_proposal_for_report(&report).await;

        let proposals = service
            .proposal_list_for_workunit(ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("proposal list");
        assert_eq!(proposals.proposals.len(), 1);
        assert_eq!(
            proposals.proposals[0].status,
            SupervisorProposalStatus::GenerationFailed
        );
        assert_eq!(reasoner.propose_call_count(), 1);
    }

    #[tokio::test]
    async fn auto_proposal_stales_old_report_and_generates_for_new_current_report() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(true))
                .await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "auto-stale-old").await;
        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Continue,
                &work_unit.id,
                &report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;
        service.maybe_auto_create_proposal_for_report(&report).await;
        let first = latest_proposal_record_for_workunit(&service, &work_unit.id).await;

        let newer_report = {
            let now = Utc::now();
            let mut state = service.state.write().await;
            let newer_report = Report {
                id: format!("report-{}", Uuid::new_v4().simple()),
                work_unit_id: work_unit.id.clone(),
                assignment_id: assignment.id.clone(),
                worker_id: assignment.worker_id.clone(),
                disposition: ReportDisposition::Partial,
                summary: "A newer authoritative report replaced the earlier decision point."
                    .to_string(),
                findings: vec!["The current bounded gap narrowed further.".to_string()],
                blockers: Vec::new(),
                questions: vec!["Should the final step redirect?".to_string()],
                recommended_next_actions: vec!["Synthesize a fresh proposal.".to_string()],
                confidence: ReportConfidence::High,
                raw_output: "raw newer output".to_string(),
                parse_result: ReportParseResult::Parsed,
                needs_supervisor_review: false,
                created_at: now,
            };
            state
                .collaboration
                .reports
                .insert(newer_report.id.clone(), newer_report.clone());
            let work_unit_entry = state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .expect("work unit");
            work_unit_entry.latest_report_id = Some(newer_report.id.clone());
            work_unit_entry.updated_at = now;
            newer_report
        };
        service
            .persist_collaboration_state()
            .await
            .expect("persist newer report");

        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Redirect,
                &work_unit.id,
                &newer_report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;
        service
            .maybe_auto_create_proposal_for_report(&newer_report)
            .await;

        let proposals = service
            .proposal_list_for_workunit(ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("proposal list");
        assert_eq!(proposals.proposals.len(), 2);
        let first_record = service
            .proposal_get(ipc::ProposalGetRequest {
                proposal_id: first.id.clone(),
            })
            .await
            .expect("first proposal");
        assert_eq!(
            first_record.proposal.status,
            SupervisorProposalStatus::Stale
        );
        assert_eq!(proposals.proposals[0].source_report_id, newer_report.id);
        assert_eq!(
            proposals.proposals[0].status,
            SupervisorProposalStatus::Open
        );
    }

    #[tokio::test]
    async fn auto_proposal_failure_persists_generation_failed_without_authoritative_changes() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(true))
                .await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "auto-failure").await;
        reasoner
            .set_failure(
                SupervisorProposalFailureStage::Backend,
                "timeout contacting provider",
                Some("provider timeout".to_string()),
            )
            .await;

        service.maybe_auto_create_proposal_for_report(&report).await;

        let stored = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        assert_eq!(stored.status, SupervisorProposalStatus::GenerationFailed);
        assert_eq!(
            stored.trigger.kind,
            SupervisorProposalTriggerKind::ReportRecorded
        );
        let state = service.state.read().await;
        assert!(state.collaboration.decisions.is_empty());
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].current_assignment_id,
            Some(assignment.id.clone())
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].status,
            WorkUnitStatus::AwaitingDecision
        );
    }

    #[tokio::test]
    async fn approval_still_works_after_auto_generated_proposal_exists() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(true))
                .await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "auto-approve").await;
        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Continue,
                &work_unit.id,
                &report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;

        service.maybe_auto_create_proposal_for_report(&report).await;
        let proposal = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        let response =
            approve_proposal(&service, &proposal.id, SupervisorProposalEdits::default()).await;

        assert_eq!(response.proposal.status, SupervisorProposalStatus::Approved);
        assert_eq!(
            response.proposal.trigger.kind,
            SupervisorProposalTriggerKind::ReportRecorded
        );
        assert_eq!(response.decision.decision_type, DecisionType::Continue);
        assert!(response.next_assignment.is_some());
    }

    #[tokio::test]
    async fn approved_same_report_does_not_regenerate_auto_proposal() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service =
            test_service_with_reasoner_and_config(reasoner.clone(), auto_proposal_config(true))
                .await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "auto-approved-suppressed").await;
        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Continue,
                &work_unit.id,
                &report.id,
                &assignment.id,
                &assignment.worker_id,
            ))
            .await;

        service.maybe_auto_create_proposal_for_report(&report).await;
        let proposal = latest_proposal_record_for_workunit(&service, &work_unit.id).await;
        let _ = approve_proposal(&service, &proposal.id, SupervisorProposalEdits::default()).await;
        assert_eq!(reasoner.propose_call_count(), 1);

        service.maybe_auto_create_proposal_for_report(&report).await;

        let proposals = service
            .proposal_list_for_workunit(ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit.id.clone(),
            })
            .await
            .expect("proposal list");
        assert_eq!(proposals.proposals.len(), 1);
        assert_eq!(
            proposals.proposals[0].status,
            SupervisorProposalStatus::Approved
        );
        assert_eq!(reasoner.propose_call_count(), 1);
    }

    #[tokio::test]
    async fn snapshot_includes_bounded_proposal_summary_after_creation() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "snapshot-proposal").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;

        let snapshot = service.snapshot().await.expect("snapshot");
        let summary = snapshot
            .collaboration
            .work_units
            .iter()
            .find(|work_unit| work_unit.id == proposal.primary_work_unit_id)
            .and_then(|work_unit| work_unit.proposal.as_ref())
            .expect("proposal summary");
        assert_eq!(summary.latest_proposal_id, proposal.id);
        assert_eq!(summary.latest_status, SupervisorProposalStatus::Open);
        assert_eq!(
            summary.latest_proposed_decision_type,
            Some(DecisionType::Continue)
        );
        assert!(summary.has_open_proposal);
        assert_eq!(
            summary.open_proposal_id.as_deref(),
            Some(proposal.id.as_str())
        );

        let snapshot_json = serde_json::to_string(&snapshot).expect("snapshot json");
        assert!(!snapshot_json.contains("reasoner_output_text"));
        assert!(!snapshot_json.contains("Objective:"));
        assert!(!snapshot_json.contains("prompt_render"));
        assert!(!snapshot_json.contains("response_artifact"));
        assert!(!snapshot_json.contains("You are the Orcas supervisor reasoner."));
        assert!(!snapshot_json.contains("SupervisorContextPack:"));
    }

    #[tokio::test]
    async fn snapshot_with_manual_proposal_fixture_omits_prompt_render_artifact_text() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let (_workstream, work_unit, assignment, report) =
            seed_manual_proposal_fixture(&service, "snapshot-manual").await;
        let proposal =
            create_default_proposal(&service, &reasoner, &work_unit, &assignment, &report)
                .await
                .proposal;

        let snapshot = service.snapshot().await.expect("snapshot");
        let summary = snapshot
            .collaboration
            .work_units
            .iter()
            .find(|work_unit| work_unit.id == proposal.primary_work_unit_id)
            .and_then(|work_unit| work_unit.proposal.as_ref())
            .expect("proposal summary");
        assert_eq!(summary.latest_proposal_id, proposal.id);
        assert!(summary.has_open_proposal);

        let snapshot_json = serde_json::to_string(&snapshot).expect("snapshot json");
        assert!(!snapshot_json.contains("prompt_render"));
        assert!(!snapshot_json.contains("response_artifact"));
        assert!(!snapshot_json.contains("You are the Orcas supervisor reasoner."));
        assert!(!snapshot_json.contains("SupervisorContextPack:"));
        assert!(!snapshot_json.contains("\"output\":["));
    }

    #[tokio::test]
    async fn snapshot_reflects_generation_failed_approved_rejected_stale_and_superseded_proposals()
    {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;

        let (_ws_approved, wu_approved, assignment_approved, report_approved) =
            seed_awaiting_decision_fixture(&service, "approved-summary").await;
        let approved = create_proposal_for_decision(
            &service,
            &reasoner,
            &wu_approved,
            &assignment_approved,
            &report_approved,
            DecisionType::Accept,
        )
        .await
        .proposal;
        let _ = approve_proposal(&service, &approved.id, SupervisorProposalEdits::default()).await;

        let (_ws_rejected, wu_rejected, assignment_rejected, report_rejected) =
            seed_awaiting_decision_fixture(&service, "rejected-summary").await;
        let rejected = create_default_proposal(
            &service,
            &reasoner,
            &wu_rejected,
            &assignment_rejected,
            &report_rejected,
        )
        .await
        .proposal;
        let _ = service
            .proposal_reject(ipc::ProposalRejectRequest {
                proposal_id: rejected.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("reject".to_string()),
            })
            .await
            .expect("reject");

        let (_ws_stale, wu_stale, assignment_stale, report_stale) =
            seed_awaiting_decision_fixture(&service, "stale-summary").await;
        let _stale = create_default_proposal(
            &service,
            &reasoner,
            &wu_stale,
            &assignment_stale,
            &report_stale,
        )
        .await
        .proposal;
        let _ = service
            .decision_apply(ipc::DecisionApplyRequest {
                work_unit_id: wu_stale.id.clone(),
                report_id: Some(report_stale.id.clone()),
                decision_type: DecisionType::EscalateToHuman,
                rationale: "force stale".to_string(),
                instructions: None,
                worker_id: None,
                worker_kind: None,
            })
            .await
            .expect("decision");

        let (_ws_superseded, wu_superseded, assignment_superseded, report_superseded) =
            seed_awaiting_decision_fixture(&service, "superseded-summary").await;
        let first = create_default_proposal(
            &service,
            &reasoner,
            &wu_superseded,
            &assignment_superseded,
            &report_superseded,
        )
        .await
        .proposal;
        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Redirect,
                &wu_superseded.id,
                &report_superseded.id,
                &assignment_superseded.id,
                &assignment_superseded.worker_id,
            ))
            .await;
        let _second = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: wu_superseded.id.clone(),
                source_report_id: Some(report_superseded.id.clone()),
                requested_by: Some("tester".to_string()),
                note: Some("resynthesize".to_string()),
                supersede_open: true,
            })
            .await
            .expect("supersede create")
            .proposal;
        let first_record = service
            .proposal_get(ipc::ProposalGetRequest {
                proposal_id: first.id.clone(),
            })
            .await
            .expect("first proposal");
        assert_eq!(
            first_record.proposal.status,
            SupervisorProposalStatus::Superseded
        );

        let (_ws_failed, wu_failed, _assignment_failed, report_failed) =
            seed_awaiting_decision_fixture(&service, "failed-summary").await;
        reasoner
            .set_failure(
                SupervisorProposalFailureStage::Backend,
                "timeout contacting provider",
                Some("provider timeout".to_string()),
            )
            .await;
        let _ = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: wu_failed.id.clone(),
                source_report_id: Some(report_failed.id.clone()),
                requested_by: Some("tester".to_string()),
                note: None,
                supersede_open: false,
            })
            .await
            .expect_err("generation failure");

        let snapshot = service.snapshot().await.expect("snapshot");
        let summary_for = |work_unit_id: &str| {
            snapshot
                .collaboration
                .work_units
                .iter()
                .find(|work_unit| work_unit.id == work_unit_id)
                .and_then(|work_unit| work_unit.proposal.as_ref())
                .cloned()
                .expect("proposal summary for work unit")
        };

        let approved_summary = summary_for(&wu_approved.id);
        assert_eq!(
            approved_summary.latest_status,
            SupervisorProposalStatus::Approved
        );
        assert!(!approved_summary.has_open_proposal);

        let rejected_summary = summary_for(&wu_rejected.id);
        assert_eq!(
            rejected_summary.latest_status,
            SupervisorProposalStatus::Rejected
        );
        assert!(!rejected_summary.has_open_proposal);

        let stale_summary = summary_for(&wu_stale.id);
        assert_eq!(stale_summary.latest_status, SupervisorProposalStatus::Stale);
        assert!(stale_summary.has_stale_or_superseded);

        let superseded_summary = summary_for(&wu_superseded.id);
        assert_eq!(
            superseded_summary.latest_status,
            SupervisorProposalStatus::Open
        );
        assert_eq!(
            superseded_summary.latest_proposed_decision_type,
            Some(DecisionType::Redirect)
        );
        assert!(superseded_summary.has_stale_or_superseded);

        let failed_summary = summary_for(&wu_failed.id);
        assert_eq!(
            failed_summary.latest_status,
            SupervisorProposalStatus::GenerationFailed
        );
        assert_eq!(
            failed_summary.latest_failure_stage,
            Some(SupervisorProposalFailureStage::Backend)
        );
        assert!(failed_summary.has_generation_failed);
        assert!(!failed_summary.has_open_proposal);
    }

    #[tokio::test]
    async fn daemon_emits_proposal_lifecycle_events_for_main_transitions() {
        let reasoner = Arc::new(StaticSupervisorReasoner::default());
        let service = test_service_with_reasoner(reasoner.clone()).await;
        let mut events = service.event_tx.subscribe();

        let (_ws_created, wu_created, assignment_created, report_created) =
            seed_awaiting_decision_fixture(&service, "events-created").await;
        let _ = create_default_proposal(
            &service,
            &reasoner,
            &wu_created,
            &assignment_created,
            &report_created,
        )
        .await
        .proposal;
        let created_event =
            wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Created).await;
        assert_eq!(created_event.primary_work_unit_id, wu_created.id);

        let (_ws_rejected, wu_rejected, assignment_rejected, report_rejected) =
            seed_awaiting_decision_fixture(&service, "events-rejected").await;
        let rejected = create_default_proposal(
            &service,
            &reasoner,
            &wu_rejected,
            &assignment_rejected,
            &report_rejected,
        )
        .await
        .proposal;
        let _ = wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Created).await;
        let _ = service
            .proposal_reject(ipc::ProposalRejectRequest {
                proposal_id: rejected.id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("reject".to_string()),
            })
            .await
            .expect("reject");
        let rejected_event =
            wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Rejected).await;
        assert_eq!(rejected_event.primary_work_unit_id, wu_rejected.id);

        let (_ws_approved, wu_approved, assignment_approved, report_approved) =
            seed_awaiting_decision_fixture(&service, "events-approved").await;
        let approved = create_proposal_for_decision(
            &service,
            &reasoner,
            &wu_approved,
            &assignment_approved,
            &report_approved,
            DecisionType::Accept,
        )
        .await
        .proposal;
        let _ = wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Created).await;
        let _ = approve_proposal(&service, &approved.id, SupervisorProposalEdits::default()).await;
        let approved_event =
            wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Approved).await;
        assert_eq!(approved_event.primary_work_unit_id, wu_approved.id);

        let (_ws_stale, wu_stale, assignment_stale, report_stale) =
            seed_awaiting_decision_fixture(&service, "events-stale").await;
        let _stale = create_default_proposal(
            &service,
            &reasoner,
            &wu_stale,
            &assignment_stale,
            &report_stale,
        )
        .await
        .proposal;
        let _ = wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Created).await;
        let _ = service
            .decision_apply(ipc::DecisionApplyRequest {
                work_unit_id: wu_stale.id.clone(),
                report_id: Some(report_stale.id.clone()),
                decision_type: DecisionType::EscalateToHuman,
                rationale: "stale event".to_string(),
                instructions: None,
                worker_id: None,
                worker_kind: None,
            })
            .await
            .expect("decision");
        let stale_event =
            wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Stale).await;
        assert_eq!(stale_event.primary_work_unit_id, wu_stale.id);

        let (_ws_superseded, wu_superseded, assignment_superseded, report_superseded) =
            seed_awaiting_decision_fixture(&service, "events-superseded").await;
        let _first = create_default_proposal(
            &service,
            &reasoner,
            &wu_superseded,
            &assignment_superseded,
            &report_superseded,
        )
        .await
        .proposal;
        let _ = wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Created).await;
        reasoner
            .set_proposal(sample_proposal_for_decision(
                DecisionType::Redirect,
                &wu_superseded.id,
                &report_superseded.id,
                &assignment_superseded.id,
                &assignment_superseded.worker_id,
            ))
            .await;
        let _ = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: wu_superseded.id.clone(),
                source_report_id: Some(report_superseded.id.clone()),
                requested_by: Some("tester".to_string()),
                note: Some("supersede".to_string()),
                supersede_open: true,
            })
            .await
            .expect("supersede");
        let superseded_event =
            wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::Superseded).await;
        assert_eq!(superseded_event.primary_work_unit_id, wu_superseded.id);

        let (_ws_failed, wu_failed, _assignment_failed, report_failed) =
            seed_awaiting_decision_fixture(&service, "events-failed").await;
        reasoner
            .set_failure(
                SupervisorProposalFailureStage::Backend,
                "timeout contacting provider",
                Some("provider timeout".to_string()),
            )
            .await;
        let _ = service
            .proposal_create(ipc::ProposalCreateRequest {
                work_unit_id: wu_failed.id.clone(),
                source_report_id: Some(report_failed.id.clone()),
                requested_by: Some("tester".to_string()),
                note: None,
                supersede_open: false,
            })
            .await
            .expect_err("generation failure");
        let failed_event =
            wait_for_proposal_action(&mut events, ipc::ProposalLifecycleAction::GenerationFailed)
                .await;
        assert_eq!(failed_event.primary_work_unit_id, wu_failed.id);
    }

    #[test]
    fn scoped_thread_summaries_prefer_orcas_relevant_threads() {
        let mut threads = HashMap::new();
        threads.insert(
            "upstream".to_string(),
            sample_thread("upstream", "upstream_discovered", 100),
        );
        threads.insert(
            "managed".to_string(),
            sample_thread("managed", "orcas_managed", 200),
        );

        let scoped = OrcasDaemonService::scoped_thread_summaries(&threads);
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "managed");
    }

    #[test]
    fn refresh_thread_summary_tracks_recent_output_and_in_flight() {
        let mut thread = sample_thread("thread-1", "orcas_managed", 200);
        thread.turns.push(ipc::TurnView {
            id: "turn-1".to_string(),
            status: "in_progress".to_string(),
            error_message: None,
            error_summary: None,
            started_at: None,
            completed_at: None,
            latest_diff: None,
            latest_plan_snapshot: None,
            token_usage_snapshot: None,
            items: vec![ipc::ItemView {
                id: "item-1".to_string(),
                item_type: "agent_message".to_string(),
                status: Some("streaming".to_string()),
                text: Some("hello world".to_string()),
                summary: Some("hello world".to_string()),
                payload: None,
            }],
        });

        OrcasDaemonService::refresh_thread_summary(&mut thread);

        assert!(thread.summary.turn_in_flight);
        assert_eq!(thread.summary.recent_output.as_deref(), Some("hello world"));
        assert_eq!(
            thread.summary.recent_event.as_deref(),
            Some("turn in progress")
        );
    }

    #[test]
    fn mark_turns_lost_clears_attachment_and_session() {
        let mut state = DaemonState::default();
        let mut thread = sample_thread("thread-1", "orcas_managed", 200);
        thread.summary.turn_in_flight = true;
        thread.turns.push(ipc::TurnView {
            id: "turn-1".to_string(),
            status: "in_progress".to_string(),
            error_message: None,
            error_summary: None,
            started_at: None,
            completed_at: None,
            latest_diff: None,
            latest_plan_snapshot: None,
            token_usage_snapshot: None,
            items: vec![ipc::ItemView {
                id: "item-1".to_string(),
                item_type: "agent_message".to_string(),
                status: Some("streaming".to_string()),
                text: Some("partial output".to_string()),
                summary: Some("partial output".to_string()),
                payload: None,
            }],
        });
        state.threads.insert("thread-1".to_string(), thread);
        state.turns.insert(
            TurnKey::new("thread-1", "turn-1"),
            ipc::TurnStateView {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                lifecycle: ipc::TurnLifecycleState::Active,
                status: "in_progress".to_string(),
                attachable: true,
                live_stream: true,
                terminal: false,
                recent_output: Some("partial output".to_string()),
                recent_event: Some("turn in progress".to_string()),
                updated_at: Utc::now(),
                error_message: None,
            },
        );

        OrcasDaemonService::refresh_session_from_turns(&mut state);
        assert_eq!(state.session.active_turns.len(), 1);

        OrcasDaemonService::mark_turns_lost(&mut state);
        OrcasDaemonService::refresh_session_from_turns(&mut state);

        let turn = state
            .turns
            .get(&TurnKey::new("thread-1", "turn-1"))
            .expect("turn exists");
        assert_eq!(turn.lifecycle, ipc::TurnLifecycleState::Lost);
        assert!(!turn.attachable);
        assert_eq!(turn.status, "lost");
        assert!(state.session.active_turns.is_empty());
        assert_eq!(state.threads["thread-1"].turns[0].status, "lost");
    }

    #[test]
    fn turn_state_from_thread_view_distinguishes_terminal_and_unknown() {
        let mut completed = sample_thread("thread-1", "orcas_managed", 200);
        completed.turns.push(ipc::TurnView {
            id: "turn-1".to_string(),
            status: "completed".to_string(),
            error_message: None,
            error_summary: None,
            started_at: None,
            completed_at: None,
            latest_diff: None,
            latest_plan_snapshot: None,
            token_usage_snapshot: None,
            items: vec![ipc::ItemView {
                id: "item-1".to_string(),
                item_type: "agent_message".to_string(),
                status: Some("completed".to_string()),
                text: Some("hello world".to_string()),
                summary: Some("hello world".to_string()),
                payload: None,
            }],
        });

        let completed_state =
            OrcasDaemonService::turn_state_from_thread_view(&completed, "turn-1").unwrap();
        assert_eq!(
            completed_state.lifecycle,
            ipc::TurnLifecycleState::Completed
        );
        assert!(!completed_state.attachable);
        assert_eq!(
            completed_state.recent_output.as_deref(),
            Some("hello world")
        );

        let mut unknown = sample_thread("thread-2", "orcas_managed", 210);
        unknown.turns.push(ipc::TurnView {
            id: "turn-2".to_string(),
            status: "in_progress".to_string(),
            error_message: None,
            error_summary: None,
            started_at: None,
            completed_at: None,
            latest_diff: None,
            latest_plan_snapshot: None,
            token_usage_snapshot: None,
            items: Vec::new(),
        });

        let unknown_state =
            OrcasDaemonService::turn_state_from_thread_view(&unknown, "turn-2").unwrap();
        assert_eq!(unknown_state.lifecycle, ipc::TurnLifecycleState::Unknown);
        assert!(!unknown_state.attachable);
        assert!(!unknown_state.live_stream);
    }

    #[tokio::test]
    async fn read_history_discovers_and_persists_headless_external_thread() {
        let (service, runtime) = test_service_with_fake_codex_runtime_capture(
            AppConfig::default(),
            Arc::new(StaticSupervisorReasoner::default()),
            "unused",
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        runtime.lock().await.threads.insert(
            "thread-headless".to_string(),
            types::Thread {
                id: "thread-headless".to_string(),
                preview: "headless preview".to_string(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: 10,
                updated_at: 20,
                status: types::ThreadStatus::Idle,
                path: None,
                cwd: "/tmp/headless".to_string(),
                cli_version: "test".to_string(),
                source: Some(serde_json::json!({ "kind": "headless" })),
                name: Some("Headless".to_string()),
                turns: vec![types::Turn {
                    id: "turn-headless-1".to_string(),
                    items: vec![types::ThreadItem {
                        id: "item-headless-1".to_string(),
                        item_type: "agent_message".to_string(),
                        extra: Map::from_iter([(
                            "text".to_string(),
                            Value::String("persisted history".to_string()),
                        )]),
                    }],
                    status: types::TurnStatus::Completed,
                    error: None,
                }],
                extra: Map::new(),
            },
        );

        let listed = service.threads_list().await.expect("list threads");
        assert!(
            listed
                .data
                .iter()
                .any(|thread| thread.id == "thread-headless")
        );

        let response = service
            .thread_read_history(ipc::ThreadReadHistoryRequest {
                thread_id: "thread-headless".to_string(),
            })
            .await
            .expect("read history");
        assert!(response.thread.history_loaded);
        assert_eq!(response.thread.turns.len(), 1);
        assert_eq!(
            response.thread.turns[0].items[0].text.as_deref(),
            Some("persisted history")
        );
        assert_eq!(
            response.thread.summary.source_kind.as_deref(),
            Some("headless")
        );

        let stored = service.store.load().await.expect("stored thread mirror");
        let persisted = stored
            .thread_views
            .get("thread-headless")
            .expect("persisted headless thread");
        assert!(persisted.history_loaded);
        assert_eq!(persisted.turns.len(), 1);
    }

    #[tokio::test]
    async fn attach_and_detach_update_monitor_state_without_dropping_thread_history() {
        let (service, runtime) = test_service_with_fake_codex_runtime_capture(
            AppConfig::default(),
            Arc::new(StaticSupervisorReasoner::default()),
            "unused",
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        runtime.lock().await.threads.insert(
            "thread-headless".to_string(),
            types::Thread {
                id: "thread-headless".to_string(),
                preview: "headless preview".to_string(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: 10,
                updated_at: 20,
                status: types::ThreadStatus::Idle,
                path: None,
                cwd: "/tmp/headless".to_string(),
                cli_version: "test".to_string(),
                source: Some(serde_json::json!({ "kind": "headless" })),
                name: Some("Headless".to_string()),
                turns: Vec::new(),
                extra: Map::new(),
            },
        );

        service.threads_list().await.expect("discover thread");
        let attached = service
            .thread_attach(ipc::ThreadAttachRequest {
                thread_id: "thread-headless".to_string(),
                cwd: None,
                model: None,
            })
            .await
            .expect("attach");
        assert!(attached.attached);
        assert_eq!(
            attached
                .thread
                .as_ref()
                .expect("attached thread")
                .summary
                .monitor_state,
            ipc::ThreadMonitorState::Attached
        );

        let detached = service
            .thread_detach(ipc::ThreadDetachRequest {
                thread_id: "thread-headless".to_string(),
            })
            .await
            .expect("detach");
        assert!(detached.detached);
        assert_eq!(
            detached
                .thread
                .as_ref()
                .expect("detached thread")
                .summary
                .monitor_state,
            ipc::ThreadMonitorState::Detached
        );

        let persisted = service
            .thread_get(ipc::ThreadGetRequest {
                thread_id: "thread-headless".to_string(),
            })
            .await
            .expect("thread still queryable")
            .thread;
        assert_eq!(persisted.summary.id, "thread-headless");
    }

    #[tokio::test]
    async fn attached_headless_thread_ingests_future_events() {
        let (service, runtime) = test_service_with_fake_codex_runtime_capture(
            AppConfig::default(),
            Arc::new(StaticSupervisorReasoner::default()),
            "unused",
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        runtime.lock().await.threads.insert(
            "thread-headless".to_string(),
            types::Thread {
                id: "thread-headless".to_string(),
                preview: "headless preview".to_string(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: 10,
                updated_at: 20,
                status: types::ThreadStatus::Idle,
                path: None,
                cwd: "/tmp/headless".to_string(),
                cli_version: "test".to_string(),
                source: Some(serde_json::json!({ "kind": "headless" })),
                name: Some("Headless".to_string()),
                turns: Vec::new(),
                extra: Map::new(),
            },
        );

        service.threads_list().await.expect("discover thread");
        service
            .thread_attach(ipc::ThreadAttachRequest {
                thread_id: "thread-headless".to_string(),
                cwd: None,
                model: None,
            })
            .await
            .expect("attach");

        service
            .apply_codex_event(EventEnvelope::new(
                "test",
                OrcasEvent::TurnStarted {
                    thread_id: "thread-headless".to_string(),
                    turn_id: "turn-live-1".to_string(),
                },
            ))
            .await;
        service
            .apply_codex_event(EventEnvelope::new(
                "test",
                OrcasEvent::ItemStarted {
                    thread_id: "thread-headless".to_string(),
                    turn_id: "turn-live-1".to_string(),
                    item_id: "item-live-1".to_string(),
                    item_type: "agent_message".to_string(),
                },
            ))
            .await;
        service
            .apply_codex_event(EventEnvelope::new(
                "test",
                OrcasEvent::AgentMessageDelta {
                    thread_id: "thread-headless".to_string(),
                    turn_id: "turn-live-1".to_string(),
                    item_id: "item-live-1".to_string(),
                    delta: "live text".to_string(),
                },
            ))
            .await;
        service
            .apply_codex_event(EventEnvelope::new(
                "test",
                OrcasEvent::ItemCompleted {
                    thread_id: "thread-headless".to_string(),
                    turn_id: "turn-live-1".to_string(),
                    item_id: "item-live-1".to_string(),
                    item_type: "agent_message".to_string(),
                },
            ))
            .await;
        service
            .apply_codex_event(EventEnvelope::new(
                "test",
                OrcasEvent::TurnCompleted {
                    thread_id: "thread-headless".to_string(),
                    turn_id: "turn-live-1".to_string(),
                    status: "completed".to_string(),
                },
            ))
            .await;

        let thread = service
            .thread_get(ipc::ThreadGetRequest {
                thread_id: "thread-headless".to_string(),
            })
            .await
            .expect("query thread")
            .thread;
        assert_eq!(thread.turns.len(), 1);
        assert_eq!(thread.turns[0].items[0].text.as_deref(), Some("live text"));
        let turn_state = service
            .turn_get(ipc::TurnGetRequest {
                thread_id: "thread-headless".to_string(),
                turn_id: "turn-live-1".to_string(),
            })
            .await
            .expect("turn state")
            .turn
            .expect("turn exists");
        assert_eq!(turn_state.lifecycle, ipc::TurnLifecycleState::Completed);
    }

    #[tokio::test]
    async fn restart_preserves_persisted_headless_thread_history() {
        let (service, runtime) = test_service_with_fake_codex_runtime_capture(
            AppConfig::default(),
            Arc::new(StaticSupervisorReasoner::default()),
            "unused",
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        runtime.lock().await.threads.insert(
            "thread-headless".to_string(),
            types::Thread {
                id: "thread-headless".to_string(),
                preview: "headless preview".to_string(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: 10,
                updated_at: 20,
                status: types::ThreadStatus::Idle,
                path: None,
                cwd: "/tmp/headless".to_string(),
                cli_version: "test".to_string(),
                source: Some(serde_json::json!({ "kind": "headless" })),
                name: Some("Headless".to_string()),
                turns: vec![types::Turn {
                    id: "turn-headless-1".to_string(),
                    items: vec![types::ThreadItem {
                        id: "item-headless-1".to_string(),
                        item_type: "agent_message".to_string(),
                        extra: Map::from_iter([(
                            "text".to_string(),
                            Value::String("persisted history".to_string()),
                        )]),
                    }],
                    status: types::TurnStatus::Completed,
                    error: None,
                }],
                extra: Map::new(),
            },
        );

        service
            .thread_read_history(ipc::ThreadReadHistoryRequest {
                thread_id: "thread-headless".to_string(),
            })
            .await
            .expect("read history");

        let restarted = test_service_at_with_components(
            service.paths.clone(),
            service.config.clone(),
            Arc::new(StaticSupervisorReasoner::default()),
            Arc::new(FakeCodexDaemonManager {
                endpoint: service.config.codex.listen_url.clone(),
            }),
            CodexClient::new(
                Arc::new(FakeCodexTransport::new(
                    service.config.codex.listen_url.clone(),
                    "unused",
                    FakeCodexTerminalOutcome::Completed,
                )),
                service.config.codex.reconnect.clone(),
                Arc::new(RejectingApprovalRouter),
            ),
            false,
        )
        .await;

        let thread = restarted
            .thread_get(ipc::ThreadGetRequest {
                thread_id: "thread-headless".to_string(),
            })
            .await
            .expect("reloaded thread")
            .thread;
        assert!(thread.history_loaded);
        assert_eq!(thread.turns.len(), 1);
        assert_eq!(
            thread.turns[0].items[0].text.as_deref(),
            Some("persisted history")
        );
    }

    #[tokio::test]
    async fn create_codex_assignment_for_unassigned_thread() {
        let service = test_service().await;
        let thread = sample_thread("thread-assigned", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;

        let response = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id.clone(),
                work_unit_id: work_unit.id.clone(),
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: Some("monitor this thread".to_string()),
            })
            .await
            .expect("create codex assignment");

        assert_eq!(response.assignment.codex_thread_id, thread.summary.id);
        assert_eq!(
            response.assignment.status,
            CodexThreadAssignmentStatus::Active
        );
        assert_eq!(
            response.assignment.send_policy,
            orcas_core::CodexThreadSendPolicy::HumanApprovalRequired
        );
        let listed = service
            .codex_assignment_list(ipc::CodexAssignmentListRequest {
                codex_thread_id: Some("thread-assigned".to_string()),
                workstream_id: None,
                work_unit_id: None,
                include_inactive: true,
            })
            .await
            .expect("list assignments");
        assert_eq!(listed.assignments.len(), 1);
        assert!(listed.assignments[0].active);
    }

    #[tokio::test]
    async fn reject_second_active_codex_assignment_for_same_thread() {
        let service = test_service().await;
        let thread = sample_thread("thread-assigned", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let work_unit_two = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Second unit".to_string(),
                task_statement: "Attempt conflicting assignment.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("second workunit")
            .work_unit;

        service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id.clone(),
                work_unit_id: work_unit.id.clone(),
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("first assignment");

        let error = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit_two.id,
                supervisor_id: "supervisor-b".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect_err("conflicting active assignment must fail");
        assert!(error.to_string().contains("already has active assignment"));
    }

    #[tokio::test]
    async fn create_codex_assignment_for_active_thread_does_not_send_turn() {
        let (service, runtime) = test_service_with_fake_codex_runtime_capture(
            AppConfig::default(),
            Arc::new(StaticSupervisorReasoner::default()),
            "unused",
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        let mut thread = sample_thread("thread-active", "live_observed", 200);
        thread.summary.status = "active".to_string();
        thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
        thread.summary.active_turn_id = Some("turn-live".to_string());
        thread.summary.last_seen_turn_id = Some("turn-live".to_string());
        thread.summary.turn_in_flight = true;
        thread.turns.push(ipc::TurnView {
            id: "turn-live".to_string(),
            status: "in_progress".to_string(),
            error_message: None,
            error_summary: None,
            started_at: Some(Utc::now()),
            completed_at: None,
            latest_diff: None,
            latest_plan_snapshot: None,
            token_usage_snapshot: None,
            items: Vec::new(),
        });
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;

        let response = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create");

        assert_eq!(
            response.assignment.latest_basis_turn_id.as_deref(),
            Some("turn-live")
        );
        assert_eq!(runtime.lock().await.last_turn_start_text, None);
        let thread_after = service
            .thread_get(ipc::ThreadGetRequest {
                thread_id: "thread-active".to_string(),
            })
            .await
            .expect("thread get")
            .thread;
        assert_eq!(thread_after.turns.len(), 1);
        assert_eq!(thread_after.turns[0].id, "turn-live");
    }

    #[tokio::test]
    async fn generate_bootstrap_proposal_for_newly_active_assigned_idle_thread() {
        let service = test_service().await;
        let mut thread = sample_thread("thread-idle", "live_observed", 200);
        thread.summary.last_seen_turn_id = Some("turn-1".to_string());
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;

        let created = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;

        let decisions = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(created.assignment_id.clone()),
                codex_thread_id: None,
                include_closed: true,
                ..Default::default()
            })
            .await
            .expect("decision list");
        assert_eq!(decisions.decisions.len(), 1);
        assert_eq!(
            decisions.decisions[0].status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );
        assert_eq!(
            decisions.decisions[0].proposal_kind,
            SupervisorTurnProposalKind::Bootstrap
        );
        assert!(
            decisions.decisions[0]
                .proposed_text
                .as_deref()
                .unwrap_or_default()
                .contains("Orcas supervisor is now managing this thread")
        );

        let assignment = service
            .codex_assignment_get(ipc::CodexAssignmentGetRequest {
                assignment_id: created.assignment_id,
            })
            .await
            .expect("assignment get")
            .assignment;
        assert_eq!(
            assignment.bootstrap_state,
            CodexThreadBootstrapState::Proposed
        );
        assert_eq!(assignment.latest_basis_turn_id.as_deref(), Some("turn-1"));
    }

    #[tokio::test]
    async fn unassigned_thread_does_not_generate_supervisor_decision() {
        let service = test_service().await;
        let thread = sample_thread("thread-unassigned", "live_observed", 200);
        {
            let mut state = service.state.write().await;
            state
                .threads
                .insert(thread.summary.id.clone(), thread.clone());
        }

        service
            .refresh_codex_supervisor_state_for_thread(&thread.summary.id)
            .await
            .expect("refresh supervisor state");

        let decisions = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: None,
                codex_thread_id: Some(thread.summary.id.clone()),
                include_closed: true,
                ..Default::default()
            })
            .await
            .expect("decision list");
        assert!(decisions.decisions.is_empty());
    }

    #[tokio::test]
    async fn assigned_active_thread_does_not_generate_proposal_until_idle() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let created = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decisions = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(created.assignment_id.clone()),
                codex_thread_id: None,
                include_closed: true,
                ..Default::default()
            })
            .await
            .expect("decision list");
        assert!(decisions.decisions.is_empty());

        service
            .apply_codex_event(EventEnvelope::new(
                "test",
                OrcasEvent::TurnCompleted {
                    thread_id: thread.summary.id.clone(),
                    turn_id: "turn-live".to_string(),
                    status: "completed".to_string(),
                },
            ))
            .await;

        let decisions = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(created.assignment_id),
                codex_thread_id: None,
                include_closed: true,
                ..Default::default()
            })
            .await
            .expect("decision list after idle");
        assert_eq!(decisions.decisions.len(), 1);
        assert_eq!(
            decisions.decisions[0].status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );
    }

    #[tokio::test]
    async fn duplicate_open_supervisor_decision_generation_is_suppressed() {
        let service = test_service().await;
        let thread = sample_thread("thread-idle", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let created = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;

        service
            .refresh_codex_supervisor_state_for_thread(&thread.summary.id)
            .await
            .expect("refresh");

        let decisions = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(created.assignment_id),
                codex_thread_id: None,
                include_closed: true,
                ..Default::default()
            })
            .await
            .expect("decision list");
        assert_eq!(decisions.decisions.len(), 1);
    }

    #[tokio::test]
    async fn record_no_action_succeeds_from_pending_next_turn_and_supersedes_it() {
        let service = test_service().await;
        let mut thread = sample_thread("thread-idle", "live_observed", 200);
        thread.summary.last_seen_turn_id = Some("turn-1".to_string());
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let pending =
            latest_supervisor_turn_decision_for_assignment(&service, &assignment.assignment_id)
                .await;

        let recorded = service
            .supervisor_decision_record_no_action(ipc::SupervisorDecisionRecordNoActionRequest {
                decision_id: pending.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("wait on the current basis".to_string()),
            })
            .await
            .expect("record no_action")
            .decision;

        assert_eq!(recorded.kind, SupervisorTurnDecisionKind::NoAction);
        assert_eq!(recorded.status, SupervisorTurnDecisionStatus::Recorded);
        assert_eq!(recorded.basis_turn_id.as_deref(), Some("turn-1"));

        let previous = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: pending.decision_id,
            })
            .await
            .expect("previous decision get")
            .decision;
        assert_eq!(previous.status, SupervisorTurnDecisionStatus::Superseded);
        assert_eq!(
            previous.superseded_by.as_deref(),
            Some(recorded.decision_id.as_str())
        );
    }

    #[tokio::test]
    async fn record_no_action_rejects_non_next_turn_decisions() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let interrupt = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("interrupt proposal")
            .decision;

        let error = service
            .supervisor_decision_record_no_action(ipc::SupervisorDecisionRecordNoActionRequest {
                decision_id: interrupt.decision_id,
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("non-next-turn decision should fail");
        assert!(error.to_string().contains("is not a next-turn decision"));
    }

    #[tokio::test]
    async fn recording_no_action_against_bootstrap_sets_bootstrap_state_not_needed() {
        let service = test_service().await;
        let thread = sample_thread("thread-idle", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let pending =
            latest_supervisor_turn_decision_for_assignment(&service, &assignment.assignment_id)
                .await;
        assert_eq!(pending.proposal_kind, SupervisorTurnProposalKind::Bootstrap);

        service
            .supervisor_decision_record_no_action(ipc::SupervisorDecisionRecordNoActionRequest {
                decision_id: pending.decision_id,
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("bootstrap not needed".to_string()),
            })
            .await
            .expect("record no_action");

        let assignment = service
            .codex_assignment_get(ipc::CodexAssignmentGetRequest {
                assignment_id: assignment.assignment_id,
            })
            .await
            .expect("assignment get")
            .assignment;
        assert_eq!(
            assignment.bootstrap_state,
            CodexThreadBootstrapState::NotNeeded
        );
    }

    #[tokio::test]
    async fn recorded_no_action_suppresses_auto_generation_until_basis_changes() {
        let service = test_service().await;
        let mut thread = sample_thread("thread-idle", "live_observed", 200);
        thread.summary.last_seen_turn_id = Some("turn-1".to_string());
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let pending =
            latest_supervisor_turn_decision_for_assignment(&service, &assignment.assignment_id)
                .await;
        let recorded = service
            .supervisor_decision_record_no_action(ipc::SupervisorDecisionRecordNoActionRequest {
                decision_id: pending.decision_id,
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("wait".to_string()),
            })
            .await
            .expect("record no_action")
            .decision;

        service
            .refresh_codex_supervisor_state_for_thread(&thread.summary.id)
            .await
            .expect("refresh same basis");

        let decisions = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(assignment.assignment_id.clone()),
                codex_thread_id: None,
                include_closed: true,
                include_superseded: true,
                ..Default::default()
            })
            .await
            .expect("decision list");
        assert_eq!(decisions.decisions.len(), 2);
        assert_eq!(decisions.decisions[0].decision_id, recorded.decision_id);
        assert_eq!(
            decisions.decisions[0].status,
            SupervisorTurnDecisionStatus::Recorded
        );

        {
            let mut state = service.state.write().await;
            let thread = state.threads.get_mut("thread-idle").expect("thread exists");
            thread.summary.last_seen_turn_id = Some("turn-2".to_string());
            thread.summary.updated_at += 1;
        }

        service
            .refresh_codex_supervisor_state_for_thread("thread-idle")
            .await
            .expect("refresh after basis change");

        let decisions = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(assignment.assignment_id),
                codex_thread_id: None,
                include_closed: true,
                include_superseded: true,
                ..Default::default()
            })
            .await
            .expect("decision list after basis change");
        assert_eq!(decisions.decisions.len(), 3);
        assert_eq!(
            decisions.decisions[0].kind,
            SupervisorTurnDecisionKind::NextTurn
        );
        assert_eq!(
            decisions.decisions[0].status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );
        assert_eq!(
            decisions.decisions[0].basis_turn_id.as_deref(),
            Some("turn-2")
        );
    }

    #[tokio::test]
    async fn manual_refresh_creates_fresh_pending_next_turn_proposal() {
        let service = test_service().await;
        let mut thread = sample_thread("thread-idle", "live_observed", 200);
        thread.summary.last_seen_turn_id = Some("turn-1".to_string());
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let pending =
            latest_supervisor_turn_decision_for_assignment(&service, &assignment.assignment_id)
                .await;
        service
            .supervisor_decision_record_no_action(ipc::SupervisorDecisionRecordNoActionRequest {
                decision_id: pending.decision_id,
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("wait".to_string()),
            })
            .await
            .expect("record no_action");

        let refreshed = service
            .supervisor_decision_manual_refresh(ipc::SupervisorDecisionManualRefreshRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: Some("try again now".to_string()),
            })
            .await
            .expect("manual refresh")
            .decision;
        assert_eq!(refreshed.kind, SupervisorTurnDecisionKind::NextTurn);
        assert_eq!(
            refreshed.proposal_kind,
            SupervisorTurnProposalKind::ManualRefresh
        );
        assert_eq!(
            refreshed.status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );
        assert_eq!(refreshed.basis_turn_id.as_deref(), Some("turn-1"));
    }

    #[tokio::test]
    async fn manual_refresh_rejects_when_thread_is_active_assignment_inactive_open_or_missing_wait()
    {
        let service = test_service().await;
        let mut idle_thread = sample_thread("thread-idle", "live_observed", 200);
        idle_thread.summary.last_seen_turn_id = Some("turn-1".to_string());
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, idle_thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: idle_thread.summary.id.clone(),
                workstream_id: workstream.id.clone(),
                work_unit_id: work_unit.id.clone(),
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let pending =
            latest_supervisor_turn_decision_for_assignment(&service, &assignment.assignment_id)
                .await;
        service
            .supervisor_decision_record_no_action(ipc::SupervisorDecisionRecordNoActionRequest {
                decision_id: pending.decision_id,
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect("record no_action");
        {
            let mut state = service.state.write().await;
            let thread = state.threads.get_mut("thread-idle").expect("thread exists");
            thread.summary.active_turn_id = Some("turn-live".to_string());
            thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
            thread.summary.status = "active".to_string();
            thread.summary.turn_in_flight = true;
        }
        let active_error = service
            .supervisor_decision_manual_refresh(ipc::SupervisorDecisionManualRefreshRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("active thread should block manual refresh");
        assert!(active_error.to_string().contains("has an active turn"));
        {
            let mut state = service.state.write().await;
            let thread = state.threads.get_mut("thread-idle").expect("thread exists");
            thread.summary.active_turn_id = None;
            thread.summary.loaded_status = ipc::ThreadLoadedStatus::Idle;
            thread.summary.status = "idle".to_string();
            thread.summary.turn_in_flight = false;
        }

        let refreshed = service
            .supervisor_decision_manual_refresh(ipc::SupervisorDecisionManualRefreshRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("manual refresh")
            .decision;
        let open_error = service
            .supervisor_decision_manual_refresh(ipc::SupervisorDecisionManualRefreshRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("open decision should block manual refresh");
        assert!(
            open_error
                .to_string()
                .contains("already has open supervisor decision")
        );
        assert_eq!(
            refreshed.status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );

        let paused = service
            .codex_assignment_pause(ipc::CodexAssignmentPauseRequest {
                assignment_id: assignment.assignment_id.clone(),
                notes: Some("pause".to_string()),
            })
            .await
            .expect("pause")
            .assignment;
        let inactive_error = service
            .supervisor_decision_manual_refresh(ipc::SupervisorDecisionManualRefreshRequest {
                assignment_id: paused.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("inactive assignment should block manual refresh");
        assert!(inactive_error.to_string().contains("is not active"));

        let thread_two = sample_thread("thread-two", "live_observed", 220);
        let (workstream_two, work_unit_two) =
            seed_codex_thread_assignment_fixture(&service, thread_two.clone()).await;
        let assignment_two = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread_two.summary.id.clone(),
                workstream_id: workstream_two.id,
                work_unit_id: work_unit_two.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("second assignment create")
            .assignment;
        let pending_two =
            latest_supervisor_turn_decision_for_assignment(&service, &assignment_two.assignment_id)
                .await;
        service
            .supervisor_decision_reject(ipc::SupervisorDecisionRejectRequest {
                decision_id: pending_two.decision_id,
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("not refreshing yet".to_string()),
            })
            .await
            .expect("reject bootstrap for second assignment");
        let missing_wait_error = service
            .supervisor_decision_manual_refresh(ipc::SupervisorDecisionManualRefreshRequest {
                assignment_id: assignment_two.assignment_id,
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("manual refresh requires recorded no_action");
        assert!(
            missing_wait_error
                .to_string()
                .contains("has no recorded no_action for the current basis")
        );
    }

    #[tokio::test]
    async fn recorded_no_action_and_suppression_persist_across_restart() {
        let base = std::env::temp_dir().join(format!("orcas-no-action-{}", Uuid::new_v4()));
        let service = test_service_at(base.clone()).await;
        let mut thread = sample_thread("thread-idle", "live_observed", 200);
        thread.summary.last_seen_turn_id = Some("turn-1".to_string());
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let pending =
            latest_supervisor_turn_decision_for_assignment(&service, &assignment.assignment_id)
                .await;
        let recorded = service
            .supervisor_decision_record_no_action(ipc::SupervisorDecisionRecordNoActionRequest {
                decision_id: pending.decision_id,
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("wait".to_string()),
            })
            .await
            .expect("record no_action")
            .decision;

        let restarted = test_service_at(base).await;
        restarted
            .refresh_codex_supervisor_state_for_thread("thread-idle")
            .await
            .expect("refresh after restart");

        let stored = restarted
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: recorded.decision_id,
            })
            .await
            .expect("recorded decision get")
            .decision;
        assert_eq!(stored.kind, SupervisorTurnDecisionKind::NoAction);
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::Recorded);

        let decisions = restarted
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(assignment.assignment_id),
                codex_thread_id: None,
                include_closed: true,
                include_superseded: true,
                ..Default::default()
            })
            .await
            .expect("decision list after restart");
        assert_eq!(decisions.decisions.len(), 2);
        assert_eq!(
            decisions.decisions[0].kind,
            SupervisorTurnDecisionKind::NoAction
        );
    }

    #[tokio::test]
    async fn supervisor_decision_list_supports_cross_thread_filters_and_actionable_queue() {
        let service = test_service().await;
        let thread_one = sample_thread("thread-1", "live_observed", 200);
        let thread_two = sample_active_thread("thread-2", "live_observed", 220, "turn-2");
        let (ws_one, wu_one) =
            seed_codex_thread_assignment_fixture(&service, thread_one.clone()).await;
        let (ws_two, wu_two) =
            seed_codex_thread_assignment_fixture(&service, thread_two.clone()).await;
        let assignment_one = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread_one.summary.id.clone(),
                workstream_id: ws_one.id.clone(),
                work_unit_id: wu_one.id.clone(),
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment one")
            .assignment;
        let assignment_two = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread_two.summary.id.clone(),
                workstream_id: ws_two.id.clone(),
                work_unit_id: wu_two.id.clone(),
                supervisor_id: "supervisor-b".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment two")
            .assignment;
        {
            let mut state = service.state.write().await;
            state.collaboration.supervisor_turn_decisions.clear();
            state.collaboration.supervisor_turn_decisions.insert(
                "std-next".to_string(),
                SupervisorTurnDecision {
                    decision_id: "std-next".to_string(),
                    assignment_id: assignment_one.assignment_id.clone(),
                    codex_thread_id: thread_one.summary.id.clone(),
                    basis_turn_id: Some("turn-1".to_string()),
                    kind: SupervisorTurnDecisionKind::NextTurn,
                    proposal_kind: SupervisorTurnProposalKind::ManualRefresh,
                    proposed_text: Some("continue".to_string()),
                    rationale_summary: "pending next turn".to_string(),
                    status: SupervisorTurnDecisionStatus::ProposedToHuman,
                    created_at: Utc::now(),
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: None,
                },
            );
            state.collaboration.supervisor_turn_decisions.insert(
                "std-no-action".to_string(),
                SupervisorTurnDecision {
                    decision_id: "std-no-action".to_string(),
                    assignment_id: assignment_one.assignment_id.clone(),
                    codex_thread_id: thread_one.summary.id.clone(),
                    basis_turn_id: Some("turn-1".to_string()),
                    kind: SupervisorTurnDecisionKind::NoAction,
                    proposal_kind: SupervisorTurnProposalKind::Bootstrap,
                    proposed_text: None,
                    rationale_summary: "wait".to_string(),
                    status: SupervisorTurnDecisionStatus::Recorded,
                    created_at: Utc::now(),
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: None,
                },
            );
            state.collaboration.supervisor_turn_decisions.insert(
                "std-steer".to_string(),
                SupervisorTurnDecision {
                    decision_id: "std-steer".to_string(),
                    assignment_id: assignment_two.assignment_id.clone(),
                    codex_thread_id: thread_two.summary.id.clone(),
                    basis_turn_id: Some("turn-2".to_string()),
                    kind: SupervisorTurnDecisionKind::SteerActiveTurn,
                    proposal_kind: SupervisorTurnProposalKind::OperatorSteer,
                    proposed_text: Some("focus logs".to_string()),
                    rationale_summary: "pending steer".to_string(),
                    status: SupervisorTurnDecisionStatus::ProposedToHuman,
                    created_at: Utc::now(),
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: None,
                },
            );
        }

        let by_status = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: None,
                codex_thread_id: None,
                workstream_id: None,
                work_unit_id: None,
                supervisor_id: None,
                status: Some(SupervisorTurnDecisionStatus::Recorded),
                kind: None,
                include_closed: true,
                include_superseded: true,
                actionable_only: false,
                limit: None,
            })
            .await
            .expect("status filter");
        assert_eq!(by_status.decisions.len(), 1);
        assert_eq!(by_status.decisions[0].decision_id, "std-no-action");

        let by_kind = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: None,
                codex_thread_id: None,
                workstream_id: None,
                work_unit_id: None,
                supervisor_id: None,
                status: None,
                kind: Some(SupervisorTurnDecisionKind::SteerActiveTurn),
                include_closed: true,
                include_superseded: true,
                actionable_only: false,
                limit: None,
            })
            .await
            .expect("kind filter");
        assert_eq!(by_kind.decisions.len(), 1);
        assert_eq!(by_kind.decisions[0].decision_id, "std-steer");
        assert_eq!(
            by_kind.decisions[0].workstream_id.as_deref(),
            Some(ws_two.id.as_str())
        );
        assert_eq!(
            by_kind.decisions[0].work_unit_id.as_deref(),
            Some(wu_two.id.as_str())
        );
        assert_eq!(
            by_kind.decisions[0].supervisor_id.as_deref(),
            Some("supervisor-b")
        );

        let by_thread = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: None,
                codex_thread_id: Some("thread-1".to_string()),
                workstream_id: None,
                work_unit_id: None,
                supervisor_id: None,
                status: None,
                kind: None,
                include_closed: true,
                include_superseded: true,
                actionable_only: false,
                limit: None,
            })
            .await
            .expect("thread filter");
        assert_eq!(by_thread.decisions.len(), 2);

        let actionable = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: None,
                codex_thread_id: None,
                workstream_id: Some(ws_one.id),
                work_unit_id: None,
                supervisor_id: None,
                status: None,
                kind: None,
                include_closed: false,
                include_superseded: false,
                actionable_only: true,
                limit: None,
            })
            .await
            .expect("actionable queue");
        assert_eq!(actionable.decisions.len(), 1);
        assert_eq!(actionable.decisions[0].decision_id, "std-next");
    }

    #[tokio::test]
    async fn supervisor_decision_list_honors_include_superseded_and_limit() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        {
            let mut state = service.state.write().await;
            state.collaboration.supervisor_turn_decisions.clear();
            state.collaboration.supervisor_turn_decisions.insert(
                "std-1".to_string(),
                SupervisorTurnDecision {
                    decision_id: "std-1".to_string(),
                    assignment_id: assignment.assignment_id.clone(),
                    codex_thread_id: thread.summary.id.clone(),
                    basis_turn_id: Some("turn-live".to_string()),
                    kind: SupervisorTurnDecisionKind::SteerActiveTurn,
                    proposal_kind: SupervisorTurnProposalKind::OperatorSteer,
                    proposed_text: Some("first".to_string()),
                    rationale_summary: "first".to_string(),
                    status: SupervisorTurnDecisionStatus::Superseded,
                    created_at: Utc::now(),
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: Some("std-2".to_string()),
                    sent_turn_id: None,
                    notes: None,
                },
            );
            state.collaboration.supervisor_turn_decisions.insert(
                "std-2".to_string(),
                SupervisorTurnDecision {
                    decision_id: "std-2".to_string(),
                    assignment_id: assignment.assignment_id.clone(),
                    codex_thread_id: thread.summary.id.clone(),
                    basis_turn_id: Some("turn-live".to_string()),
                    kind: SupervisorTurnDecisionKind::SteerActiveTurn,
                    proposal_kind: SupervisorTurnProposalKind::OperatorSteer,
                    proposed_text: Some("second".to_string()),
                    rationale_summary: "second".to_string(),
                    status: SupervisorTurnDecisionStatus::ProposedToHuman,
                    created_at: Utc::now(),
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: None,
                },
            );
        }

        let without_superseded = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(assignment.assignment_id.clone()),
                codex_thread_id: None,
                workstream_id: None,
                work_unit_id: None,
                supervisor_id: None,
                status: None,
                kind: None,
                include_closed: true,
                include_superseded: false,
                actionable_only: false,
                limit: None,
            })
            .await
            .expect("without superseded");
        assert_eq!(without_superseded.decisions.len(), 1);
        assert_eq!(without_superseded.decisions[0].decision_id, "std-2");

        let limited = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(assignment.assignment_id),
                codex_thread_id: None,
                workstream_id: None,
                work_unit_id: None,
                supervisor_id: None,
                status: None,
                kind: None,
                include_closed: true,
                include_superseded: true,
                actionable_only: false,
                limit: Some(1),
            })
            .await
            .expect("limit");
        assert_eq!(limited.decisions.len(), 1);
    }

    #[tokio::test]
    async fn interrupt_proposal_requires_active_assignment() {
        let service = test_service().await;
        let error = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: "missing".to_string(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("unknown assignment should fail");
        assert!(
            error
                .to_string()
                .contains("unknown Codex thread assignment")
        );
    }

    #[tokio::test]
    async fn assigned_idle_thread_cannot_create_interrupt_proposal() {
        let service = test_service().await;
        let thread = sample_thread("thread-idle", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        {
            let mut state = service.state.write().await;
            state.collaboration.supervisor_turn_decisions.clear();
        }

        let error = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("idle thread cannot propose interrupt");
        assert!(error.to_string().contains("has no active turn"));
    }

    #[tokio::test]
    async fn operator_can_create_interrupt_proposal_for_assigned_active_thread() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;

        let proposed = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: Some("interrupt for review".to_string()),
            })
            .await
            .expect("interrupt proposal")
            .decision;
        assert_eq!(
            proposed.kind,
            SupervisorTurnDecisionKind::InterruptActiveTurn
        );
        assert_eq!(
            proposed.proposal_kind,
            SupervisorTurnProposalKind::OperatorInterrupt
        );
        assert_eq!(proposed.basis_turn_id.as_deref(), Some("turn-live"));
        assert!(proposed.proposed_text.is_none());

        let listed = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(assignment.assignment_id),
                codex_thread_id: None,
                include_closed: true,
                ..Default::default()
            })
            .await
            .expect("decision list");
        assert_eq!(listed.decisions.len(), 1);
        assert_eq!(
            listed.decisions[0].status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );
    }

    #[tokio::test]
    async fn duplicate_interrupt_proposals_are_rejected() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;

        service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("first interrupt proposal");
        let error = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("duplicate interrupt proposal should fail");
        assert!(
            error
                .to_string()
                .contains("already has open supervisor decision")
        );
    }

    #[tokio::test]
    async fn interrupt_proposal_conflicts_with_existing_open_next_turn_decision() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        {
            let mut state = service.state.write().await;
            state.collaboration.supervisor_turn_decisions.insert(
                "std-open".to_string(),
                SupervisorTurnDecision {
                    decision_id: "std-open".to_string(),
                    assignment_id: assignment.assignment_id.clone(),
                    codex_thread_id: thread.summary.id.clone(),
                    basis_turn_id: Some("turn-prev".to_string()),
                    kind: SupervisorTurnDecisionKind::NextTurn,
                    proposal_kind: SupervisorTurnProposalKind::ManualRefresh,
                    proposed_text: Some("continue".to_string()),
                    rationale_summary: "existing open decision".to_string(),
                    status: SupervisorTurnDecisionStatus::ProposedToHuman,
                    created_at: Utc::now(),
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: None,
                },
            );
        }

        let error = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("conflicting open decision should block interrupt");
        assert!(
            error
                .to_string()
                .contains("already has open supervisor decision")
        );
    }

    #[tokio::test]
    async fn steer_proposal_requires_active_assignment() {
        let service = test_service().await;
        let error = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: "missing".to_string(),
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("focus on tests".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("unknown assignment should fail");
        assert!(
            error
                .to_string()
                .contains("unknown Codex thread assignment")
        );
    }

    #[tokio::test]
    async fn assigned_idle_thread_cannot_create_steer_proposal() {
        let service = test_service().await;
        let thread = sample_thread("thread-idle", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        {
            let mut state = service.state.write().await;
            state.collaboration.supervisor_turn_decisions.clear();
        }

        let error = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("focus on tests".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("idle thread cannot propose steer");
        assert!(error.to_string().contains("has no active turn"));
    }

    #[tokio::test]
    async fn operator_can_create_steer_proposal_for_assigned_active_thread() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;

        let proposed = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("Focus on the current failing test only.".to_string()),
                rationale_note: Some("steer for review".to_string()),
            })
            .await
            .expect("steer proposal")
            .decision;
        assert_eq!(proposed.kind, SupervisorTurnDecisionKind::SteerActiveTurn);
        assert_eq!(
            proposed.proposal_kind,
            SupervisorTurnProposalKind::OperatorSteer
        );
        assert_eq!(proposed.basis_turn_id.as_deref(), Some("turn-live"));
        assert!(
            proposed
                .proposed_text
                .as_deref()
                .unwrap_or_default()
                .contains("Focus on the current failing test only.")
        );

        let listed = service
            .supervisor_decision_list(ipc::SupervisorDecisionListRequest {
                assignment_id: Some(assignment.assignment_id),
                codex_thread_id: None,
                include_closed: true,
                ..Default::default()
            })
            .await
            .expect("decision list");
        assert_eq!(listed.decisions.len(), 1);
        assert_eq!(
            listed.decisions[0].status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );
    }

    #[tokio::test]
    async fn steer_proposal_requires_non_empty_text_when_supplied() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;

        let error = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("   ".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("blank steer text should fail");
        assert!(error.to_string().contains("non-empty proposed_text"));
    }

    #[tokio::test]
    async fn pending_steer_text_can_be_replaced_before_send() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let original = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("Original steer text".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;

        let replacement = service
            .supervisor_decision_replace_pending_steer(
                ipc::SupervisorDecisionReplacePendingSteerRequest {
                    decision_id: original.decision_id.clone(),
                    requested_by: Some("reviewer".to_string()),
                    proposed_text: "Replacement steer text".to_string(),
                    rationale_note: Some("tighten scope".to_string()),
                },
            )
            .await
            .expect("replace pending steer")
            .decision;
        assert_ne!(replacement.decision_id, original.decision_id);
        assert_eq!(
            replacement.proposed_text.as_deref(),
            Some("Replacement steer text")
        );
        assert_eq!(
            replacement.status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );

        let original_after = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: original.decision_id.clone(),
            })
            .await
            .expect("original decision get")
            .decision;
        assert_eq!(
            original_after.status,
            SupervisorTurnDecisionStatus::Superseded
        );
        assert_eq!(
            original_after.superseded_by.as_deref(),
            Some(replacement.decision_id.as_str())
        );
    }

    #[tokio::test]
    async fn replacing_pending_steer_rejects_empty_text() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("Original steer text".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;

        let error = service
            .supervisor_decision_replace_pending_steer(
                ipc::SupervisorDecisionReplacePendingSteerRequest {
                    decision_id: decision.decision_id,
                    requested_by: Some("reviewer".to_string()),
                    proposed_text: "   ".to_string(),
                    rationale_note: None,
                },
            )
            .await
            .expect_err("blank replacement text should fail");
        assert!(error.to_string().contains("non-empty proposed_text"));
    }

    #[tokio::test]
    async fn duplicate_steer_proposals_are_rejected() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;

        service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("focus on tests".to_string()),
                rationale_note: None,
            })
            .await
            .expect("first steer proposal");
        let error = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("focus on logs".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("duplicate steer proposal should fail");
        assert!(
            error
                .to_string()
                .contains("already has open supervisor decision")
        );
    }

    #[tokio::test]
    async fn steer_proposal_conflicts_with_existing_open_next_turn_decision() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        {
            let mut state = service.state.write().await;
            state.collaboration.supervisor_turn_decisions.insert(
                "std-open".to_string(),
                SupervisorTurnDecision {
                    decision_id: "std-open".to_string(),
                    assignment_id: assignment.assignment_id.clone(),
                    codex_thread_id: thread.summary.id.clone(),
                    basis_turn_id: Some("turn-prev".to_string()),
                    kind: SupervisorTurnDecisionKind::NextTurn,
                    proposal_kind: SupervisorTurnProposalKind::ManualRefresh,
                    proposed_text: Some("continue".to_string()),
                    rationale_summary: "existing open decision".to_string(),
                    status: SupervisorTurnDecisionStatus::ProposedToHuman,
                    created_at: Utc::now(),
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: None,
                },
            );
        }

        let error = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("focus on tests".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("conflicting next-turn decision should block steer");
        assert!(
            error
                .to_string()
                .contains("already has open supervisor decision")
        );
    }

    #[tokio::test]
    async fn steer_proposal_conflicts_with_existing_open_interrupt_decision() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("interrupt proposal");

        let error = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("focus on tests".to_string()),
                rationale_note: None,
            })
            .await
            .expect_err("conflicting interrupt decision should block steer");
        assert!(
            error
                .to_string()
                .contains("already has open supervisor decision")
        );
    }

    #[tokio::test]
    async fn approve_and_send_starts_new_turn_and_records_sent_state() {
        let (service, runtime) = test_service_with_fake_codex_runtime_capture(
            AppConfig::default(),
            Arc::new(StaticSupervisorReasoner::default()),
            "unused",
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        let thread = sample_thread("thread-idle", "live_observed", 200);
        runtime.lock().await.threads.insert(
            thread.summary.id.clone(),
            types::Thread {
                id: thread.summary.id.clone(),
                preview: thread.summary.preview.clone(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: thread.summary.created_at,
                updated_at: thread.summary.updated_at,
                status: types::ThreadStatus::Idle,
                path: None,
                cwd: thread.summary.cwd.clone(),
                cli_version: "test".to_string(),
                source: None,
                name: thread.summary.name.clone(),
                turns: Vec::new(),
                extra: Map::new(),
            },
        );
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let created = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision =
            latest_supervisor_turn_decision_for_assignment(&service, &created.assignment_id).await;

        let response = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("send it".to_string()),
            })
            .await
            .expect("approve and send");
        assert_eq!(response.decision.status, SupervisorTurnDecisionStatus::Sent);
        assert_eq!(
            response.decision.sent_turn_id.as_deref(),
            Some("turn-fake-1")
        );
        assert!(
            runtime
                .lock()
                .await
                .last_turn_start_text
                .as_deref()
                .unwrap_or_default()
                .contains("Orcas supervisor is now managing this thread")
        );

        let assignment = service
            .codex_assignment_get(ipc::CodexAssignmentGetRequest {
                assignment_id: created.assignment_id,
            })
            .await
            .expect("assignment get")
            .assignment;
        assert_eq!(assignment.bootstrap_state, CodexThreadBootstrapState::Sent);
    }

    #[tokio::test]
    async fn approve_and_send_interrupt_maps_to_turn_interrupt() {
        let (service, runtime) = test_service_with_fake_codex_runtime_capture(
            AppConfig::default(),
            Arc::new(StaticSupervisorReasoner::default()),
            "unused",
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        runtime.lock().await.threads.insert(
            thread.summary.id.clone(),
            types::Thread {
                id: thread.summary.id.clone(),
                preview: thread.summary.preview.clone(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: thread.summary.created_at,
                updated_at: thread.summary.updated_at,
                status: types::ThreadStatus::Active {
                    active_flags: vec!["turn_running".to_string()],
                },
                path: None,
                cwd: thread.summary.cwd.clone(),
                cli_version: "test".to_string(),
                source: None,
                name: thread.summary.name.clone(),
                turns: vec![types::Turn {
                    id: "turn-live".to_string(),
                    items: Vec::new(),
                    status: types::TurnStatus::InProgress,
                    error: None,
                }],
                extra: Map::new(),
            },
        );
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("interrupt proposal")
            .decision;

        let sent = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("interrupt now".to_string()),
            })
            .await
            .expect("approve and send interrupt")
            .decision;
        assert_eq!(sent.status, SupervisorTurnDecisionStatus::Sent);
        assert!(sent.sent_turn_id.is_none());

        let runtime_state = runtime.lock().await;
        assert_eq!(
            runtime_state.last_turn_interrupt_thread_id.as_deref(),
            Some("thread-active")
        );
        assert_eq!(
            runtime_state.last_turn_interrupt_turn_id.as_deref(),
            Some("turn-live")
        );
    }

    #[tokio::test]
    async fn approve_and_send_steer_maps_to_turn_steer() {
        let (service, runtime) = test_service_with_fake_codex_runtime_capture(
            AppConfig::default(),
            Arc::new(StaticSupervisorReasoner::default()),
            "unused",
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        runtime.lock().await.threads.insert(
            thread.summary.id.clone(),
            types::Thread {
                id: thread.summary.id.clone(),
                preview: thread.summary.preview.clone(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: thread.summary.created_at,
                updated_at: thread.summary.updated_at,
                status: types::ThreadStatus::Active {
                    active_flags: vec!["turn_running".to_string()],
                },
                path: None,
                cwd: thread.summary.cwd.clone(),
                cli_version: "test".to_string(),
                source: None,
                name: thread.summary.name.clone(),
                turns: vec![types::Turn {
                    id: "turn-live".to_string(),
                    items: Vec::new(),
                    status: types::TurnStatus::InProgress,
                    error: None,
                }],
                extra: Map::new(),
            },
        );
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("Focus on the current test failure only.".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;

        let sent = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("steer now".to_string()),
            })
            .await
            .expect("approve and send steer")
            .decision;
        assert_eq!(sent.status, SupervisorTurnDecisionStatus::Sent);
        assert!(sent.sent_turn_id.is_none());

        let runtime_state = runtime.lock().await;
        assert_eq!(
            runtime_state.last_turn_steer_thread_id.as_deref(),
            Some("thread-active")
        );
        assert_eq!(
            runtime_state.last_turn_steer_expected_turn_id.as_deref(),
            Some("turn-live")
        );
        assert_eq!(
            runtime_state.last_turn_steer_text.as_deref(),
            Some("Focus on the current test failure only.")
        );
    }

    #[tokio::test]
    async fn approve_and_send_uses_current_replacement_steer_text() {
        let (service, runtime) = test_service_with_fake_codex_runtime_capture(
            AppConfig::default(),
            Arc::new(StaticSupervisorReasoner::default()),
            "unused",
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        runtime.lock().await.threads.insert(
            thread.summary.id.clone(),
            types::Thread {
                id: thread.summary.id.clone(),
                preview: thread.summary.preview.clone(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: thread.summary.created_at,
                updated_at: thread.summary.updated_at,
                status: types::ThreadStatus::Active {
                    active_flags: vec!["turn_running".to_string()],
                },
                path: None,
                cwd: thread.summary.cwd.clone(),
                cli_version: "test".to_string(),
                source: None,
                name: thread.summary.name.clone(),
                turns: vec![types::Turn {
                    id: "turn-live".to_string(),
                    items: Vec::new(),
                    status: types::TurnStatus::InProgress,
                    error: None,
                }],
                extra: Map::new(),
            },
        );
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let original = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("Original steer text".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;
        let replacement = service
            .supervisor_decision_replace_pending_steer(
                ipc::SupervisorDecisionReplacePendingSteerRequest {
                    decision_id: original.decision_id,
                    requested_by: Some("reviewer".to_string()),
                    proposed_text: "Use the replacement steer text".to_string(),
                    rationale_note: None,
                },
            )
            .await
            .expect("replace steer")
            .decision;

        let sent = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: replacement.decision_id,
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect("approve and send steer")
            .decision;
        assert_eq!(sent.status, SupervisorTurnDecisionStatus::Sent);
        assert_eq!(
            runtime.lock().await.last_turn_steer_text.as_deref(),
            Some("Use the replacement steer text")
        );
    }

    #[tokio::test]
    async fn stale_validation_prevents_send_if_newer_turn_state_exists() {
        let service = test_service().await;
        let thread = sample_thread("thread-idle", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let created = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision =
            latest_supervisor_turn_decision_for_assignment(&service, &created.assignment_id).await;

        service
            .record_turn_started(&thread.summary.id, "turn-new", "submitted")
            .await;

        let error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("stale decision should not send");
        assert!(
            error.to_string().contains("became stale")
                || error.to_string().contains("pending human review")
        );

        let stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get")
            .decision;
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn interrupt_send_stales_if_active_turn_changes_before_send() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("interrupt proposal")
            .decision;
        {
            let mut state = service.state.write().await;
            let thread = state
                .threads
                .get_mut("thread-active")
                .expect("thread exists");
            thread.summary.active_turn_id = Some("turn-new".to_string());
            thread.summary.last_seen_turn_id = Some("turn-new".to_string());
        }

        let error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("changed active turn should stale interrupt");
        assert!(error.to_string().contains("became stale"));

        let stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get")
            .decision;
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn interrupt_send_stales_if_thread_becomes_idle_before_send() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("interrupt proposal")
            .decision;
        {
            let mut state = service.state.write().await;
            let thread = state
                .threads
                .get_mut("thread-active")
                .expect("thread exists");
            thread.summary.status = "idle".to_string();
            thread.summary.loaded_status = ipc::ThreadLoadedStatus::Idle;
            thread.summary.active_turn_id = None;
            thread.summary.turn_in_flight = false;
        }

        let error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("idle thread should stale interrupt");
        assert!(error.to_string().contains("became stale"));

        let stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get")
            .decision;
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn steer_send_stales_if_active_turn_changes_before_send() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("stay on the current bounded step".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;
        {
            let mut state = service.state.write().await;
            let thread = state
                .threads
                .get_mut("thread-active")
                .expect("thread exists");
            thread.summary.active_turn_id = Some("turn-new".to_string());
            thread.summary.last_seen_turn_id = Some("turn-new".to_string());
        }

        let error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("changed active turn should stale steer");
        assert!(error.to_string().contains("became stale"));

        let stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get")
            .decision;
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn steer_send_stales_if_thread_becomes_idle_before_send() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("stay on the current bounded step".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;
        {
            let mut state = service.state.write().await;
            let thread = state
                .threads
                .get_mut("thread-active")
                .expect("thread exists");
            thread.summary.status = "idle".to_string();
            thread.summary.loaded_status = ipc::ThreadLoadedStatus::Idle;
            thread.summary.active_turn_id = None;
            thread.summary.turn_in_flight = false;
        }

        let error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("idle thread should stale steer");
        assert!(error.to_string().contains("became stale"));

        let stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get")
            .decision;
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn replacement_steer_send_stales_if_active_turn_changes_before_send() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let original = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("Original steer text".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;
        let replacement = service
            .supervisor_decision_replace_pending_steer(
                ipc::SupervisorDecisionReplacePendingSteerRequest {
                    decision_id: original.decision_id,
                    requested_by: Some("reviewer".to_string()),
                    proposed_text: "Edited steer text".to_string(),
                    rationale_note: None,
                },
            )
            .await
            .expect("replace steer")
            .decision;
        {
            let mut state = service.state.write().await;
            let thread = state
                .threads
                .get_mut("thread-active")
                .expect("thread exists");
            thread.summary.active_turn_id = Some("turn-new".to_string());
            thread.summary.last_seen_turn_id = Some("turn-new".to_string());
        }

        let error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: replacement.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("changed active turn should stale edited steer");
        assert!(error.to_string().contains("became stale"));

        let stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: replacement.decision_id,
            })
            .await
            .expect("decision get")
            .decision;
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn reject_bootstrap_decision_marks_rejected_and_not_needed() {
        let service = test_service().await;
        let thread = sample_thread("thread-idle", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let created = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision =
            latest_supervisor_turn_decision_for_assignment(&service, &created.assignment_id).await;

        let rejected = service
            .supervisor_decision_reject(ipc::SupervisorDecisionRejectRequest {
                decision_id: decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: Some("not needed".to_string()),
            })
            .await
            .expect("reject")
            .decision;
        assert_eq!(rejected.status, SupervisorTurnDecisionStatus::Rejected);

        let assignment = service
            .codex_assignment_get(ipc::CodexAssignmentGetRequest {
                assignment_id: created.assignment_id,
            })
            .await
            .expect("assignment get")
            .assignment;
        assert_eq!(
            assignment.bootstrap_state,
            CodexThreadBootstrapState::NotNeeded
        );
    }

    #[tokio::test]
    async fn paused_or_released_assignment_prevents_send() {
        let service = test_service().await;
        let thread = sample_thread("thread-idle", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let created = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision =
            latest_supervisor_turn_decision_for_assignment(&service, &created.assignment_id).await;

        let _ = service
            .codex_assignment_pause(ipc::CodexAssignmentPauseRequest {
                assignment_id: created.assignment_id.clone(),
                notes: Some("pause automation".to_string()),
            })
            .await
            .expect("pause");
        let error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("paused assignment should prevent send");
        assert!(
            error.to_string().contains("pending human review")
                || error.to_string().contains("became stale")
        );

        let stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get")
            .decision;
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn paused_or_released_assignment_prevents_interrupt_send() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let paused_decision = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("interrupt proposal")
            .decision;

        service
            .codex_assignment_pause(ipc::CodexAssignmentPauseRequest {
                assignment_id: assignment.assignment_id.clone(),
                notes: Some("pause automation".to_string()),
            })
            .await
            .expect("pause");
        let paused_error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: paused_decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("paused assignment should prevent interrupt send");
        assert!(
            paused_error.to_string().contains("pending human review")
                || paused_error.to_string().contains("became stale")
        );
        let paused_stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: paused_decision.decision_id,
            })
            .await
            .expect("paused decision get")
            .decision;
        assert_eq!(paused_stored.status, SupervisorTurnDecisionStatus::Stale);

        service
            .codex_assignment_resume(ipc::CodexAssignmentResumeRequest {
                assignment_id: assignment.assignment_id.clone(),
                notes: Some("resume automation".to_string()),
            })
            .await
            .expect("resume");
        let released_decision = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("interrupt proposal after resume")
            .decision;
        service
            .codex_assignment_release(ipc::CodexAssignmentReleaseRequest {
                assignment_id: assignment.assignment_id,
                notes: Some("release management".to_string()),
            })
            .await
            .expect("release");
        let released_error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: released_decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("released assignment should prevent interrupt send");
        assert!(
            released_error.to_string().contains("pending human review")
                || released_error.to_string().contains("became stale")
        );
        let released_stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: released_decision.decision_id,
            })
            .await
            .expect("released decision get")
            .decision;
        assert_eq!(released_stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn paused_or_released_assignment_prevents_steer_send() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let paused_decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("focus on the current bounded step".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;

        service
            .codex_assignment_pause(ipc::CodexAssignmentPauseRequest {
                assignment_id: assignment.assignment_id.clone(),
                notes: Some("pause automation".to_string()),
            })
            .await
            .expect("pause");
        let paused_error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: paused_decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("paused assignment should prevent steer send");
        assert!(
            paused_error.to_string().contains("pending human review")
                || paused_error.to_string().contains("became stale")
        );
        let paused_stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: paused_decision.decision_id,
            })
            .await
            .expect("paused decision get")
            .decision;
        assert_eq!(paused_stored.status, SupervisorTurnDecisionStatus::Stale);

        service
            .codex_assignment_resume(ipc::CodexAssignmentResumeRequest {
                assignment_id: assignment.assignment_id.clone(),
                notes: Some("resume automation".to_string()),
            })
            .await
            .expect("resume");
        let released_decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("focus on the current bounded step".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal after resume")
            .decision;
        service
            .codex_assignment_release(ipc::CodexAssignmentReleaseRequest {
                assignment_id: assignment.assignment_id,
                notes: Some("release management".to_string()),
            })
            .await
            .expect("release");
        let released_error = service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: released_decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("released assignment should prevent steer send");
        assert!(
            released_error.to_string().contains("pending human review")
                || released_error.to_string().contains("became stale")
        );
        let released_stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: released_decision.decision_id,
            })
            .await
            .expect("released decision get")
            .decision;
        assert_eq!(released_stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn sent_rejected_or_stale_steer_decisions_cannot_be_replaced() {
        let (service, runtime) = test_service_with_fake_codex_runtime_capture(
            AppConfig::default(),
            Arc::new(StaticSupervisorReasoner::default()),
            "unused",
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        runtime.lock().await.threads.insert(
            thread.summary.id.clone(),
            types::Thread {
                id: thread.summary.id.clone(),
                preview: thread.summary.preview.clone(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: thread.summary.created_at,
                updated_at: thread.summary.updated_at,
                status: types::ThreadStatus::Active {
                    active_flags: vec!["turn_running".to_string()],
                },
                path: None,
                cwd: thread.summary.cwd.clone(),
                cli_version: "test".to_string(),
                source: None,
                name: thread.summary.name.clone(),
                turns: vec![types::Turn {
                    id: "turn-live".to_string(),
                    items: Vec::new(),
                    status: types::TurnStatus::InProgress,
                    error: None,
                }],
                extra: Map::new(),
            },
        );
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;

        let sent_decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("sent steer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;
        service
            .supervisor_decision_approve_and_send(ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: sent_decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect("send steer");
        let sent_error = service
            .supervisor_decision_replace_pending_steer(
                ipc::SupervisorDecisionReplacePendingSteerRequest {
                    decision_id: sent_decision.decision_id,
                    requested_by: Some("reviewer".to_string()),
                    proposed_text: "edited after send".to_string(),
                    rationale_note: None,
                },
            )
            .await
            .expect_err("sent steer should not be replaceable");
        assert!(sent_error.to_string().contains("no longer editable"));

        service
            .record_turn_started(&thread.summary.id, "turn-new", "submitted")
            .await;
        let rejected_decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id.clone(),
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("rejected steer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;
        service
            .supervisor_decision_reject(ipc::SupervisorDecisionRejectRequest {
                decision_id: rejected_decision.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect("reject steer");
        let rejected_error = service
            .supervisor_decision_replace_pending_steer(
                ipc::SupervisorDecisionReplacePendingSteerRequest {
                    decision_id: rejected_decision.decision_id,
                    requested_by: Some("reviewer".to_string()),
                    proposed_text: "edited after reject".to_string(),
                    rationale_note: None,
                },
            )
            .await
            .expect_err("rejected steer should not be replaceable");
        assert!(rejected_error.to_string().contains("no longer editable"));

        let stale_decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("stale steer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;
        {
            let mut state = service.state.write().await;
            let thread = state
                .threads
                .get_mut("thread-active")
                .expect("thread exists");
            thread.summary.active_turn_id = Some("turn-other".to_string());
        }
        let stale_error = service
            .supervisor_decision_replace_pending_steer(
                ipc::SupervisorDecisionReplacePendingSteerRequest {
                    decision_id: stale_decision.decision_id.clone(),
                    requested_by: Some("reviewer".to_string()),
                    proposed_text: "edited after stale".to_string(),
                    rationale_note: None,
                },
            )
            .await
            .expect_err("stale steer should not be replaceable");
        assert!(stale_error.to_string().contains("became stale"));
        let stale_stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: stale_decision.decision_id,
            })
            .await
            .expect("stale decision get")
            .decision;
        assert_eq!(stale_stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn supervisor_decisions_persist_across_restart() {
        let base = std::env::temp_dir().join(format!("orcas-decision-{}", Uuid::new_v4()));
        let service = test_service_at(base.clone()).await;
        let thread = sample_thread("thread-idle", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let created = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision =
            latest_supervisor_turn_decision_for_assignment(&service, &created.assignment_id).await;

        let restarted = test_service_at(base).await;
        let decision_after = restarted
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get after restart")
            .decision;
        assert_eq!(
            decision_after.status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );
        assert_eq!(
            decision_after.proposal_kind,
            SupervisorTurnProposalKind::Bootstrap
        );
    }

    #[tokio::test]
    async fn pending_interrupt_proposal_stales_when_active_turn_completes_naturally() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("interrupt proposal")
            .decision;

        service
            .apply_codex_event(EventEnvelope::new(
                "test",
                OrcasEvent::TurnCompleted {
                    thread_id: thread.summary.id.clone(),
                    turn_id: "turn-live".to_string(),
                    status: "completed".to_string(),
                },
            ))
            .await;

        let stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get")
            .decision;
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn pending_steer_proposal_stales_when_active_turn_completes_naturally() {
        let service = test_service().await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("focus on the current bounded step".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;

        service
            .apply_codex_event(EventEnvelope::new(
                "test",
                OrcasEvent::TurnCompleted {
                    thread_id: thread.summary.id.clone(),
                    turn_id: "turn-live".to_string(),
                    status: "completed".to_string(),
                },
            ))
            .await;

        let stored = service
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get")
            .decision;
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::Stale);
    }

    #[tokio::test]
    async fn interrupt_decisions_persist_across_restart() {
        let base = std::env::temp_dir().join(format!("orcas-interrupt-{}", Uuid::new_v4()));
        let service = test_service_at(base.clone()).await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_interrupt(ipc::SupervisorDecisionProposeInterruptRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                rationale_note: None,
            })
            .await
            .expect("interrupt proposal")
            .decision;

        let restarted = test_service_at(base).await;
        let stored = restarted
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get after restart")
            .decision;
        assert_eq!(stored.kind, SupervisorTurnDecisionKind::InterruptActiveTurn);
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::ProposedToHuman);
    }

    #[tokio::test]
    async fn steer_decisions_persist_across_restart() {
        let base = std::env::temp_dir().join(format!("orcas-steer-{}", Uuid::new_v4()));
        let service = test_service_at(base.clone()).await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let decision = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("focus on the current bounded step".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;

        let restarted = test_service_at(base).await;
        let stored = restarted
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: decision.decision_id,
            })
            .await
            .expect("decision get after restart")
            .decision;
        assert_eq!(stored.kind, SupervisorTurnDecisionKind::SteerActiveTurn);
        assert_eq!(stored.status, SupervisorTurnDecisionStatus::ProposedToHuman);
        assert_eq!(
            stored.proposed_text.as_deref(),
            Some("focus on the current bounded step")
        );
    }

    #[tokio::test]
    async fn replaced_steer_revision_chain_persists_across_restart() {
        let base = std::env::temp_dir().join(format!("orcas-steer-chain-{}", Uuid::new_v4()));
        let service = test_service_at(base.clone()).await;
        let thread = sample_active_thread("thread-active", "live_observed", 200, "turn-live");
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let assignment = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id.clone(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("assignment create")
            .assignment;
        let original = service
            .supervisor_decision_propose_steer(ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some("reviewer".to_string()),
                proposed_text: Some("first steer revision".to_string()),
                rationale_note: None,
            })
            .await
            .expect("steer proposal")
            .decision;
        let replacement = service
            .supervisor_decision_replace_pending_steer(
                ipc::SupervisorDecisionReplacePendingSteerRequest {
                    decision_id: original.decision_id.clone(),
                    requested_by: Some("reviewer".to_string()),
                    proposed_text: "second steer revision".to_string(),
                    rationale_note: None,
                },
            )
            .await
            .expect("replace steer")
            .decision;

        let restarted = test_service_at(base).await;
        let original_after = restarted
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: original.decision_id,
            })
            .await
            .expect("original decision get after restart")
            .decision;
        let replacement_after = restarted
            .supervisor_decision_get(ipc::SupervisorDecisionGetRequest {
                decision_id: replacement.decision_id.clone(),
            })
            .await
            .expect("replacement decision get after restart")
            .decision;
        assert_eq!(
            original_after.status,
            SupervisorTurnDecisionStatus::Superseded
        );
        assert_eq!(
            original_after.superseded_by.as_deref(),
            Some(replacement.decision_id.as_str())
        );
        assert_eq!(
            replacement_after.proposed_text.as_deref(),
            Some("second steer revision")
        );
        assert_eq!(
            replacement_after.status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );
    }

    #[tokio::test]
    async fn pause_resume_and_release_codex_assignment() {
        let service = test_service().await;
        let thread = sample_thread("thread-assigned", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let created = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id,
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("create")
            .assignment;

        let paused = service
            .codex_assignment_pause(ipc::CodexAssignmentPauseRequest {
                assignment_id: created.assignment_id.clone(),
                notes: Some("paused for operator review".to_string()),
            })
            .await
            .expect("pause")
            .assignment;
        assert_eq!(paused.status, CodexThreadAssignmentStatus::Paused);

        let resumed = service
            .codex_assignment_resume(ipc::CodexAssignmentResumeRequest {
                assignment_id: created.assignment_id.clone(),
                notes: Some("resume after review".to_string()),
            })
            .await
            .expect("resume")
            .assignment;
        assert_eq!(resumed.status, CodexThreadAssignmentStatus::Active);

        let released = service
            .codex_assignment_release(ipc::CodexAssignmentReleaseRequest {
                assignment_id: created.assignment_id.clone(),
                notes: Some("released from Orcas management".to_string()),
            })
            .await
            .expect("release")
            .assignment;
        assert_eq!(released.status, CodexThreadAssignmentStatus::Released);
        assert!(
            released
                .notes
                .as_deref()
                .unwrap_or_default()
                .contains("released from Orcas management")
        );

        let listed = service
            .codex_assignment_list(ipc::CodexAssignmentListRequest {
                codex_thread_id: Some("thread-assigned".to_string()),
                workstream_id: None,
                work_unit_id: None,
                include_inactive: true,
            })
            .await
            .expect("list");
        assert_eq!(listed.assignments.len(), 1);
        assert!(!listed.assignments[0].active);
    }

    #[tokio::test]
    async fn codex_assignment_persists_across_restart() {
        let base = std::env::temp_dir().join(format!("orcas-codex-assignment-{}", Uuid::new_v4()));
        let service = test_service_at(base.clone()).await;
        let thread = sample_thread("thread-assigned", "live_observed", 200);
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;
        let created = service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: thread.summary.id,
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: Some("persist me".to_string()),
            })
            .await
            .expect("create")
            .assignment;

        let restarted = test_service_at(base).await;
        let assignment = restarted
            .codex_assignment_get(ipc::CodexAssignmentGetRequest {
                assignment_id: created.assignment_id,
            })
            .await
            .expect("get after restart")
            .assignment;
        assert_eq!(assignment.status, CodexThreadAssignmentStatus::Active);
        assert_eq!(assignment.notes.as_deref(), Some("persist me"));
    }

    #[tokio::test]
    async fn codex_assignment_does_not_mutate_thread_history_or_mirror() {
        let service = test_service().await;
        let mut thread = sample_thread("thread-assigned", "live_observed", 200);
        thread.history_loaded = true;
        thread.turns.push(ipc::TurnView {
            id: "turn-1".to_string(),
            status: "completed".to_string(),
            error_message: None,
            error_summary: None,
            started_at: None,
            completed_at: None,
            latest_diff: None,
            latest_plan_snapshot: None,
            token_usage_snapshot: None,
            items: vec![ipc::ItemView {
                id: "item-1".to_string(),
                item_type: "agent_message".to_string(),
                status: Some("completed".to_string()),
                text: Some("history".to_string()),
                summary: Some("history".to_string()),
                payload: None,
            }],
        });
        let original_thread = thread.clone();
        let (workstream, work_unit) =
            seed_codex_thread_assignment_fixture(&service, thread.clone()).await;

        service
            .codex_assignment_create(ipc::CodexAssignmentCreateRequest {
                codex_thread_id: "thread-assigned".to_string(),
                workstream_id: workstream.id,
                work_unit_id: work_unit.id,
                supervisor_id: "supervisor-a".to_string(),
                assigned_by: "tester".to_string(),
                send_policy: None,
                notes: None,
            })
            .await
            .expect("create assignment");

        let thread_after = service
            .thread_get(ipc::ThreadGetRequest {
                thread_id: "thread-assigned".to_string(),
            })
            .await
            .expect("thread get")
            .thread;
        assert_eq!(thread_after.summary.id, original_thread.summary.id);
        assert_eq!(
            thread_after.summary.preview,
            original_thread.summary.preview
        );
        assert_eq!(
            thread_after.summary.last_seen_turn_id,
            original_thread.summary.last_seen_turn_id
        );
        assert_eq!(thread_after.turns.len(), original_thread.turns.len());
        assert_eq!(thread_after.turns[0].id, original_thread.turns[0].id);
        assert_eq!(
            thread_after.turns[0].status,
            original_thread.turns[0].status
        );
        assert_eq!(
            thread_after.turns[0].items[0].text,
            original_thread.turns[0].items[0].text
        );
        assert_eq!(thread_after.history_loaded, original_thread.history_loaded);
    }

    #[test]
    fn assignment_prompt_render_is_deterministic_for_same_packet() {
        let (_assignment, record) = sample_assignment_and_communication_record();
        let rerendered = render_prompt(&record.packet, &record.packet_hash, record.created_at)
            .expect("re-render prompt");

        assert_eq!(
            record.prompt_render.render_spec.template_version,
            "assignment_prompt.v1"
        );
        assert_eq!(record.prompt_render.prompt_text, rerendered.prompt_text);
        assert_eq!(record.prompt_render.prompt_hash, rerendered.prompt_hash);
        assert_eq!(
            record.prompt_render.render_spec.section_order,
            rerendered.render_spec.section_order
        );
        assert!(
            record
                .prompt_render
                .prompt_text
                .contains("Response Contract:\n- Emit exactly one JSON envelope")
        );
        let objective_index = record
            .prompt_render
            .prompt_text
            .find("Objective:\n")
            .expect("objective section");
        let instructions_index = record
            .prompt_render
            .prompt_text
            .find("Instructions:\n")
            .expect("instructions section");
        let response_index = record
            .prompt_render
            .prompt_text
            .find("Response Contract:\n")
            .expect("response contract section");
        assert!(objective_index < instructions_index);
        assert!(instructions_index < response_index);
    }

    #[tokio::test]
    async fn assignment_communication_get_returns_record_for_assignment() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Visibility".to_string(),
                objective: "Inspect assignment communication records".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Inspect record".to_string(),
                task_statement: "Inspect the current assignment communication path.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let prepared = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-visibility".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some(
                    "Inspect the current assignment communication path.".to_string(),
                ),
                model: None,
                cwd: None,
            })
            .await
            .expect("prepared assignment");

        let expected_record = service
            .state
            .read()
            .await
            .collaboration
            .assignment_communications
            .get(&prepared.assignment.id)
            .cloned()
            .expect("communication record");

        let response = service
            .assignment_communication_get(ipc::AssignmentCommunicationGetRequest {
                assignment_id: prepared.assignment.id.clone(),
            })
            .await
            .expect("assignment communication record");

        assert_eq!(response.record.assignment_id, expected_record.assignment_id);
        assert_eq!(response.record.work_unit_id, expected_record.work_unit_id);
        assert_eq!(response.record.workstream_id, expected_record.workstream_id);
        assert_eq!(
            response.record.packet.packet_id,
            expected_record.packet.packet_id
        );
        assert_eq!(
            response.record.prompt_render.render_spec.template_version,
            "assignment_prompt.v1"
        );
        assert_eq!(
            response.record.prompt_render.prompt_text,
            expected_record.prompt_render.prompt_text
        );
        assert_eq!(response.record.prompt_hash, expected_record.prompt_hash);
        assert!(response.record.response_envelope.is_none());
        assert!(response.record.validation.is_none());

        let json = serde_json::to_value(&response).expect("serialize response");
        assert_eq!(
            json["record"]["assignment_id"],
            Value::String(expected_record.assignment_id.clone())
        );
        assert_eq!(
            json["record"]["packet"]["schema_version"],
            Value::String("assignment_communication_packet.v1".to_string())
        );
        assert_eq!(
            json["record"]["prompt_render"]["render_spec"]["template_version"],
            Value::String("assignment_prompt.v1".to_string())
        );
    }

    #[tokio::test]
    async fn assignment_communication_get_rejects_empty_assignment_id() {
        let service = test_service().await;

        let error = service
            .assignment_communication_get(ipc::AssignmentCommunicationGetRequest {
                assignment_id: "   ".to_string(),
            })
            .await
            .expect_err("empty id should be rejected");

        assert!(matches!(
            error,
            OrcasError::Protocol(message)
                if message.contains("assignment communication lookup requires a non-empty assignment_id")
        ));
    }

    #[tokio::test]
    async fn assignment_communication_get_rejects_missing_assignment() {
        let service = test_service().await;

        let error = service
            .assignment_communication_get(ipc::AssignmentCommunicationGetRequest {
                assignment_id: "assignment-missing".to_string(),
            })
            .await
            .expect_err("missing assignment should be rejected");

        assert!(matches!(
            error,
            OrcasError::Protocol(message)
                if message.contains("unknown assignment communication record for assignment `assignment-missing`")
        ));
    }

    #[test]
    fn legacy_instruction_fallback_parses_existing_assignment_without_communication_seed() {
        let now = Utc::now();
        let workstream = Workstream {
            id: "ws-legacy".to_string(),
            title: "Legacy".to_string(),
            objective: "Exercise the legacy back-compat path".to_string(),
            status: WorkstreamStatus::Active,
            priority: "normal".to_string(),
            created_at: now,
            updated_at: now,
        };
        let work_unit = WorkUnit {
            id: "wu-legacy".to_string(),
            workstream_id: workstream.id.clone(),
            title: "Legacy work".to_string(),
            task_statement: "Legacy work unit task statement.".to_string(),
            status: WorkUnitStatus::Ready,
            dependencies: Vec::new(),
            latest_report_id: None,
            current_assignment_id: Some("assignment-legacy".to_string()),
            created_at: now,
            updated_at: now,
        };
        let assignment = Assignment {
            id: "assignment-legacy".to_string(),
            work_unit_id: work_unit.id.clone(),
            worker_id: "worker-legacy".to_string(),
            worker_session_id: "session-legacy".to_string(),
            instructions: r#"Objective: Legacy objective
Predecessor assignment: assignment-parent
Source report: report-legacy
Instructions:
- Use the legacy instruction parser only for back-compat.
Acceptance criteria:
- Confirm the back-compat path still works.
Stop conditions:
- Stop if the legacy path is ambiguous.
Required context refs: ctx/legacy
Boundedness note: Stay within the legacy compatibility boundary."#
                .to_string(),
            communication_seed: None,
            status: AssignmentStatus::Created,
            attempt_number: 1,
            created_at: now,
            updated_at: now,
        };
        let mut collaboration = CollaborationState::default();
        collaboration
            .workstreams
            .insert(workstream.id.clone(), workstream);
        collaboration
            .work_units
            .insert(work_unit.id.clone(), work_unit);
        collaboration
            .assignments
            .insert(assignment.id.clone(), assignment.clone());

        let record = build_assignment_communication_record(
            &collaboration,
            &assignment,
            None,
            None,
            None,
            now,
        )
        .expect("legacy communication record");

        assert_eq!(record.packet.objective, "Legacy objective");
        assert_eq!(
            record.packet.predecessor_assignment_id.as_deref(),
            Some("assignment-parent")
        );
        assert_eq!(
            record.packet.source_report_id.as_deref(),
            Some("report-legacy")
        );
        assert_eq!(
            record.packet.instructions,
            vec!["Use the legacy instruction parser only for back-compat.".to_string()]
        );
        assert_eq!(
            record
                .packet
                .acceptance_criteria
                .iter()
                .map(|item| item.text.clone())
                .collect::<Vec<_>>(),
            vec!["Confirm the back-compat path still works.".to_string()]
        );
        assert_eq!(
            record
                .packet
                .stop_conditions
                .iter()
                .map(|item| item.text.clone())
                .collect::<Vec<_>>(),
            vec!["Stop if the legacy path is ambiguous.".to_string()]
        );
    }

    #[test]
    fn parse_worker_report_accepts_clean_contract() {
        let (assignment, record) = sample_assignment_and_communication_record();
        let raw = sample_runtime_report_output_for(&assignment.id, &record.packet.packet_id);

        let parsed = parse_worker_report(&raw, &assignment, &record);
        assert_eq!(parsed.disposition, ReportDisposition::Completed);
        assert_eq!(parsed.confidence, ReportConfidence::High);
        assert_eq!(parsed.validation.parse_result, ReportParseResult::Parsed);
        assert!(!parsed.validation.needs_supervisor_review);
        assert_eq!(parsed.findings, vec!["root cause isolated".to_string()]);
        let envelope = parsed.envelope.expect("parsed envelope");
        assert!(envelope.touched_files.is_empty());
        assert!(envelope.commands_run.is_empty());
        assert!(envelope.acceptance_results.is_empty());
        assert!(envelope.artifacts.is_empty());
    }

    #[test]
    fn parse_worker_report_flags_ambiguous_output_for_review() {
        let (assignment, record) = sample_assignment_and_communication_record();
        let raw = format!(
            "here is the report\n{}",
            sample_runtime_report_output_for(&assignment.id, &record.packet.packet_id)
        );

        let parsed = parse_worker_report(&raw, &assignment, &record);
        assert_eq!(parsed.disposition, ReportDisposition::Completed);
        assert_eq!(parsed.validation.parse_result, ReportParseResult::Ambiguous);
        assert!(parsed.validation.needs_supervisor_review);
    }

    #[test]
    fn parse_worker_report_rejects_malformed_json() {
        let (assignment, record) = sample_assignment_and_communication_record();

        let parsed = parse_worker_report(
            &wrap_report_envelope("{ not valid json }"),
            &assignment,
            &record,
        );

        assert_eq!(parsed.validation.parse_result, ReportParseResult::Invalid);
        assert!(parsed.validation.needs_supervisor_review);
        assert!(parsed.envelope.is_none());
    }

    #[test]
    fn parse_worker_report_rejects_assignment_id_mismatch() {
        let (assignment, record) = sample_assignment_and_communication_record();
        let raw = wrap_report_envelope(&format!(
            r#"{{
  "schema_version": "worker_report_envelope.v1",
  "assignment_id": "assignment-other",
  "packet_id": "{}",
  "task_mode": "implement",
  "disposition": "completed",
  "summary": "finished the bounded task",
  "confidence": "high",
  "acceptance_results": [],
  "triggered_stop_condition_ids": [],
  "touched_files": [],
  "commands_run": [],
  "artifacts": [],
  "blockers": [],
  "questions": [],
  "recommended_next_actions": [],
  "uncertainties": [],
  "review_signal": {{
    "level": "normal",
    "reasons": [],
    "focus": []
  }},
  "mode_payload": {{
    "kind": "implement",
    "semantic_changes": [],
    "tests_run": [],
    "rough_edges": []
  }}
}}"#,
            record.packet.packet_id
        ));

        let parsed = parse_worker_report(&raw, &assignment, &record);

        assert_eq!(parsed.validation.parse_result, ReportParseResult::Invalid);
        assert!(parsed.validation.needs_supervisor_review);
    }

    #[test]
    fn parse_worker_report_rejects_packet_id_mismatch() {
        let (assignment, record) = sample_assignment_and_communication_record();
        let raw = wrap_report_envelope(&format!(
            r#"{{
  "schema_version": "worker_report_envelope.v1",
  "assignment_id": "{}",
  "packet_id": "packet-other",
  "task_mode": "implement",
  "disposition": "completed",
  "summary": "finished the bounded task",
  "confidence": "high",
  "acceptance_results": [],
  "triggered_stop_condition_ids": [],
  "touched_files": [],
  "commands_run": [],
  "artifacts": [],
  "blockers": [],
  "questions": [],
  "recommended_next_actions": [],
  "uncertainties": [],
  "review_signal": {{
    "level": "normal",
    "reasons": [],
    "focus": []
  }},
  "mode_payload": {{
    "kind": "implement",
    "semantic_changes": [],
    "tests_run": [],
    "rough_edges": []
  }}
}}"#,
            assignment.id
        ));

        let parsed = parse_worker_report(&raw, &assignment, &record);

        assert_eq!(parsed.validation.parse_result, ReportParseResult::Invalid);
        assert!(parsed.validation.needs_supervisor_review);
    }

    #[test]
    fn parse_worker_report_requires_common_fields_to_exist() {
        let (assignment, record) = sample_assignment_and_communication_record();
        let raw = wrap_report_envelope(&format!(
            r#"{{
  "schema_version": "worker_report_envelope.v1",
  "assignment_id": "{}",
  "packet_id": "{}",
  "task_mode": "implement",
  "disposition": "completed",
  "summary": "finished the bounded task",
  "confidence": "high",
  "acceptance_results": [],
  "triggered_stop_condition_ids": [],
  "touched_files": [],
  "commands_run": [],
  "artifacts": [],
  "blockers": [],
  "recommended_next_actions": [],
  "uncertainties": [],
  "review_signal": {{
    "level": "normal",
    "reasons": [],
    "focus": []
  }},
  "mode_payload": {{
    "kind": "implement",
    "semantic_changes": [],
    "tests_run": [],
    "rough_edges": []
  }}
}}"#,
            assignment.id, record.packet.packet_id
        ));

        let parsed = parse_worker_report(&raw, &assignment, &record);

        assert_eq!(parsed.validation.parse_result, ReportParseResult::Invalid);
        assert!(parsed.validation.needs_supervisor_review);
    }

    #[test]
    fn parse_worker_report_invalid_output_requires_review_and_preserves_invalid_result() {
        let (assignment, record) = sample_assignment_and_communication_record();

        let parsed = parse_worker_report("no structured report here", &assignment, &record);
        assert_eq!(parsed.validation.parse_result, ReportParseResult::Invalid);
        assert!(parsed.validation.needs_supervisor_review);
    }

    #[test]
    fn interrupted_turn_report_is_downgraded_for_supervisor_review() {
        let (assignment, record) = sample_assignment_and_communication_record();
        let raw = sample_runtime_report_output_for(&assignment.id, &record.packet.packet_id);

        let parsed = parse_worker_report_for_turn(
            &raw,
            ipc::TurnLifecycleState::Interrupted,
            &assignment,
            &record,
        );
        assert_eq!(parsed.disposition, ReportDisposition::Interrupted);
        assert_eq!(parsed.confidence, ReportConfidence::Unknown);
        assert_eq!(parsed.validation.parse_result, ReportParseResult::Ambiguous);
        assert!(parsed.validation.needs_supervisor_review);
        assert!(parsed.findings.is_empty());
    }

    #[test]
    fn lost_turn_report_stays_invalid_when_no_contract_exists() {
        let (assignment, record) = sample_assignment_and_communication_record();

        let parsed = parse_worker_report_for_turn(
            "partial raw output",
            ipc::TurnLifecycleState::Lost,
            &assignment,
            &record,
        );
        assert_eq!(parsed.disposition, ReportDisposition::Failed);
        assert_eq!(parsed.validation.parse_result, ReportParseResult::Invalid);
        assert!(parsed.validation.needs_supervisor_review);
        assert!(parsed.recommended_next_actions.is_empty());
    }

    #[tokio::test]
    async fn collaboration_objects_persist_to_store() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Investigate reconnect".to_string(),
                objective: "Find the regression".to_string(),
                priority: Some("high".to_string()),
            })
            .await
            .expect("workstream")
            .workstream;
        let _work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Isolate root cause".to_string(),
                task_statement: "Find the failure point".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("work unit");

        let stored = service.store.load().await.expect("stored state");
        assert_eq!(stored.collaboration.workstreams.len(), 1);
        assert_eq!(stored.collaboration.work_units.len(), 1);
        assert_eq!(
            stored.collaboration.workstreams[&workstream.id].status,
            WorkstreamStatus::Active
        );
    }

    #[tokio::test]
    async fn snapshot_includes_collaboration_summaries_after_creation() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Snapshot".to_string(),
                objective: "Verify snapshot".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Inspect".to_string(),
                task_statement: "Inspect snapshot".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;

        let snapshot = service.snapshot().await.expect("snapshot");
        assert_eq!(snapshot.collaboration.workstreams.len(), 1);
        assert_eq!(snapshot.collaboration.work_units.len(), 1);
        assert_eq!(snapshot.collaboration.assignments.len(), 0);
        assert_eq!(snapshot.collaboration.workstreams[0].id, workstream.id);
        assert_eq!(snapshot.collaboration.work_units[0].id, work_unit.id);
    }

    #[tokio::test]
    async fn prepare_assignment_binds_worker_to_worker_session() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Reconnect".to_string(),
                objective: "Fix regression".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id,
                title: "Inspect streaming".to_string(),
                task_statement: "Inspect the current streaming path".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;

        let prepared = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the reconnect path".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");
        let assignment_id = prepared.assignment.id.clone();
        let worker_id = prepared.assignment.worker_id.clone();
        let worker_session_id = prepared.assignment.worker_session_id.clone();

        let state = service.state.read().await;
        let assignment = &state.collaboration.assignments[&assignment_id];
        assert_eq!(assignment.worker_id, worker_id);
        assert_eq!(assignment.worker_session_id, worker_session_id);
        assert_eq!(assignment.status, AssignmentStatus::Created);
        assert!(prepared.created_new);
        let communication = state
            .collaboration
            .assignment_communications
            .get(&assignment_id)
            .expect("assignment communication");
        assert_eq!(state.collaboration.assignment_communications.len(), 1);
        assert_eq!(communication.assignment_id, assignment_id);
        assert_eq!(communication.packet.assignment_id, assignment_id);
        assert_eq!(
            communication.prompt_render.render_spec.template_version,
            "assignment_prompt.v1"
        );
        assert_eq!(
            communication.packet_hash,
            communication.prompt_render.packet_hash
        );
        assert_eq!(
            communication.prompt_hash,
            communication.prompt_render.prompt_hash
        );
        assert!(!communication.packet.instructions.is_empty());
        assert!(!communication.packet.acceptance_criteria.is_empty());
        assert!(!communication.packet.stop_conditions.is_empty());
        assert_eq!(
            state.collaboration.workers[&worker_id].status,
            WorkerStatus::Idle
        );
        assert_eq!(
            state.collaboration.worker_sessions[&worker_session_id].worker_id,
            worker_id
        );
    }

    #[tokio::test]
    async fn explicit_failed_start_transition_marks_assignment_failed_without_report() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Failure".to_string(),
                objective: "Exercise failed start".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id,
                title: "Start assignment".to_string(),
                task_statement: "Attempt a start without an available Codex runtime.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;

        let prepared = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("This start should fail cleanly.".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");
        {
            let now = Utc::now();
            let mut state = service.state.write().await;
            state
                .collaboration
                .workers
                .get_mut(&prepared.assignment.worker_id)
                .unwrap()
                .current_assignment_id = Some(prepared.assignment.id.clone());
            state
                .collaboration
                .workers
                .get_mut(&prepared.assignment.worker_id)
                .unwrap()
                .status = WorkerStatus::Busy;
            state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .unwrap()
                .status = WorkUnitStatus::Running;
            state
                .collaboration
                .worker_sessions
                .get_mut(&prepared.assignment.worker_session_id)
                .unwrap()
                .updated_at = now;
        }
        service
            .mark_assignment_start_failed(
                &prepared.assignment.id,
                &prepared.assignment.worker_id,
                &prepared.assignment.worker_session_id,
            )
            .await
            .expect("mark failed start");

        let state = service.state.read().await;
        let assignment = state
            .collaboration
            .assignments
            .get(&prepared.assignment.id)
            .expect("assignment");
        assert_eq!(assignment.status, AssignmentStatus::Failed);
        assert!(state.collaboration.reports.is_empty());
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].status,
            WorkUnitStatus::AwaitingDecision
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id]
                .current_assignment_id
                .as_deref(),
            Some(assignment.id.as_str())
        );
    }

    #[tokio::test]
    async fn interrupted_assignment_records_report_and_requires_supervisor_review() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Interrupt".to_string(),
                objective: "Exercise interrupted execution".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id,
                title: "Interrupt assignment".to_string(),
                task_statement: "Simulate an interrupted bounded execution.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let prepared = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Simulate a single interrupted segment.".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");
        {
            let now = Utc::now();
            let mut state = service.state.write().await;
            state
                .collaboration
                .assignments
                .get_mut(&prepared.assignment.id)
                .unwrap()
                .status = AssignmentStatus::Running;
            state
                .collaboration
                .workers
                .get_mut(&prepared.assignment.worker_id)
                .unwrap()
                .status = WorkerStatus::Busy;
            let session = state
                .collaboration
                .worker_sessions
                .get_mut(&prepared.assignment.worker_session_id)
                .unwrap();
            session.active_turn_id = Some("turn-interrupted".to_string());
            session.runtime_status = WorkerSessionRuntimeStatus::Running;
            session.updated_at = now;
            state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .unwrap()
                .status = WorkUnitStatus::Running;
        }

        let turn_state = ipc::TurnStateView {
            thread_id: "thread-interrupted".to_string(),
            turn_id: "turn-interrupted".to_string(),
            lifecycle: ipc::TurnLifecycleState::Interrupted,
            status: "interrupted".to_string(),
            attachable: false,
            live_stream: false,
            terminal: true,
            recent_output: Some("partial raw output".to_string()),
            recent_event: Some("turn interrupted".to_string()),
            updated_at: Utc::now(),
            error_message: Some("interrupted".to_string()),
        };
        let (report, assignment, updated_work_unit, _stale_proposals) = service
            .record_assignment_turn_outcome(
                &prepared.assignment.id,
                &prepared.assignment.worker_id,
                &prepared.assignment.worker_session_id,
                turn_state,
                "partial raw output".to_string(),
            )
            .await
            .expect("record interrupted outcome");
        service
            .persist_collaboration_state()
            .await
            .expect("persist interrupted outcome");

        assert_eq!(assignment.status, AssignmentStatus::Interrupted);
        assert_eq!(report.disposition, ReportDisposition::Interrupted);
        assert_eq!(report.parse_result, ReportParseResult::Invalid);
        assert!(report.needs_supervisor_review);
        assert_eq!(updated_work_unit.status, WorkUnitStatus::AwaitingDecision);
        assert_eq!(
            service.state.read().await.collaboration.worker_sessions
                [&prepared.assignment.worker_session_id]
                .runtime_status,
            WorkerSessionRuntimeStatus::Interrupted
        );
    }

    #[tokio::test]
    async fn assignment_start_reuses_only_unexecuted_latest_pending_assignment() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Reconnect".to_string(),
                objective: "Fix regression".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id,
                title: "Inspect streaming".to_string(),
                task_statement: "Inspect the current streaming path".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let first = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the reconnect path".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");
        let second = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the reconnect path".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");

        assert_eq!(first.assignment.id, second.assignment.id);
        assert!(!second.created_new);
        let state = service.state.read().await;
        assert_eq!(state.collaboration.assignment_communications.len(), 1);
        assert!(
            state
                .collaboration
                .assignment_communications
                .contains_key(&first.assignment.id)
        );
    }

    #[tokio::test]
    async fn decision_apply_uses_structured_seed_for_packet_not_assignment_instructions() {
        let service = test_service().await;
        let (_workstream, work_unit, assignment, report) =
            seed_awaiting_decision_fixture(&service, "structured-seed").await;
        let structured_seed = sample_structured_assignment_seed(&assignment.id, &report.id);

        let response = service
            .decision_apply_with_seed(
                ipc::DecisionApplyRequest {
                    work_unit_id: work_unit.id.clone(),
                    report_id: Some(report.id.clone()),
                    decision_type: DecisionType::Continue,
                    rationale: "Use the structured seed for the successor assignment.".to_string(),
                    instructions: Some("compatibility preview only".to_string()),
                    worker_id: None,
                    worker_kind: None,
                },
                Some(structured_seed.clone()),
            )
            .await
            .expect("decision with structured seed");

        let next_assignment = response.next_assignment.expect("next assignment");
        assert_eq!(next_assignment.instructions, "compatibility preview only");

        let state = service.state.read().await;
        let record = state
            .collaboration
            .assignment_communications
            .get(&next_assignment.id)
            .expect("structured communication record");
        assert_eq!(record.packet.objective, structured_seed.objective);
        assert_eq!(record.packet.instructions, structured_seed.instructions);
        assert_eq!(
            record.packet.source_proposal_id,
            structured_seed.source_proposal_id
        );
        assert_eq!(
            record.packet.predecessor_assignment_id.as_deref(),
            Some(assignment.id.as_str())
        );
        assert_eq!(
            record.packet.source_report_id.as_deref(),
            Some(report.id.as_str())
        );
        assert_eq!(
            record.packet.source_decision_id.as_deref(),
            Some(response.decision.id.as_str())
        );
        assert_eq!(
            record
                .packet
                .acceptance_criteria
                .iter()
                .map(|item| item.text.clone())
                .collect::<Vec<_>>(),
            structured_seed.acceptance_criteria
        );
        assert_eq!(
            record
                .packet
                .stop_conditions
                .iter()
                .map(|item| item.text.clone())
                .collect::<Vec<_>>(),
            structured_seed.stop_conditions
        );
    }

    #[tokio::test]
    async fn assignment_start_uses_persisted_packet_when_assignment_instructions_change() {
        let reasoner = Arc::new(PackDrivenSupervisorReasoner::new(DecisionType::Continue));
        let (service, fake_runtime_state) = test_service_with_fake_codex_runtime_capture(
            auto_proposal_config(false),
            reasoner,
            sample_runtime_report_output_template(),
            FakeCodexTerminalOutcome::Completed,
        )
        .await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Persisted packet".to_string(),
                objective: "Verify prompt dispatch uses the persisted communication record."
                    .to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Persisted packet work unit".to_string(),
                task_statement: "Use the persisted packet for prompt dispatch.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let prepared = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-persisted".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Initial packet instructions".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("prepared assignment");
        let original_prompt = service
            .state
            .read()
            .await
            .collaboration
            .assignment_communications
            .get(&prepared.assignment.id)
            .expect("communication record")
            .prompt_render
            .prompt_text
            .clone();

        {
            let mut state = service.state.write().await;
            state
                .collaboration
                .assignments
                .get_mut(&prepared.assignment.id)
                .expect("assignment")
                .instructions = "mutated after packet creation".to_string();
        }

        let response = service
            .assignment_start(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-persisted".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("new start text should not rebuild the packet".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment start");

        let sent_prompt = fake_runtime_state
            .lock()
            .await
            .last_turn_start_text
            .clone()
            .expect("sent prompt");
        assert_eq!(sent_prompt, original_prompt);
        assert_eq!(response.report.parse_result, ReportParseResult::Parsed);

        let state = service.state.read().await;
        assert_eq!(state.collaboration.assignment_communications.len(), 1);
        assert_eq!(
            state.collaboration.assignment_communications[&prepared.assignment.id]
                .prompt_render
                .prompt_text,
            original_prompt
        );
    }

    #[tokio::test]
    async fn assignment_start_does_not_reopen_reported_assignment() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Reconnect".to_string(),
                objective: "Fix regression".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id,
                title: "Inspect streaming".to_string(),
                task_statement: "Inspect the current streaming path".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let prepared = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the reconnect path".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");
        {
            let mut state = service.state.write().await;
            state
                .collaboration
                .assignments
                .get_mut(&prepared.assignment.id)
                .unwrap()
                .status = AssignmentStatus::AwaitingDecision;
            state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .unwrap()
                .status = WorkUnitStatus::AwaitingDecision;
        }

        let error = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the reconnect path".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect_err("must not reopen assignment before decision");
        assert!(
            error
                .to_string()
                .contains("is not startable in status `AwaitingDecision`")
        );
    }

    #[tokio::test]
    async fn assignment_start_rejects_multiple_pending_assignments() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Reconnect".to_string(),
                objective: "Fix regression".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id,
                title: "Inspect streaming".to_string(),
                task_statement: "Inspect the current streaming path".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let first = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the reconnect path".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");
        {
            let mut state = service.state.write().await;
            let duplicate_assignment = Assignment {
                id: "assignment-dup".to_string(),
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_session_id: first.assignment.worker_session_id.clone(),
                instructions: "duplicate".to_string(),
                communication_seed: None,
                status: AssignmentStatus::Created,
                attempt_number: first.assignment.attempt_number + 1,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            state
                .collaboration
                .assignments
                .insert(duplicate_assignment.id.clone(), duplicate_assignment);
        }
        let error = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the reconnect path".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect_err("multiple pending assignments must fail");
        assert!(
            error
                .to_string()
                .contains("multiple unexecuted pending assignments")
        );
    }

    #[tokio::test]
    async fn continue_creates_new_assignment_and_reuses_worker_session() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Reconnect".to_string(),
                objective: "Fix regression".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id,
                title: "Inspect streaming".to_string(),
                task_statement: "Inspect the current streaming path".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let prepared = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the reconnect path".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");
        let assignment_id = prepared.assignment.id.clone();
        let worker_id = prepared.assignment.worker_id.clone();
        let worker_session_id = prepared.assignment.worker_session_id.clone();
        let report = {
            let now = Utc::now();
            let mut state = service.state.write().await;
            state
                .collaboration
                .assignments
                .get_mut(&assignment_id)
                .unwrap()
                .status = AssignmentStatus::AwaitingDecision;
            state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .unwrap()
                .status = WorkUnitStatus::AwaitingDecision;
            let report = Report {
                id: "report-1".to_string(),
                work_unit_id: work_unit.id.clone(),
                assignment_id: assignment_id.clone(),
                worker_id: worker_id.clone(),
                disposition: ReportDisposition::Completed,
                summary: "done".to_string(),
                findings: vec!["finding".to_string()],
                blockers: Vec::new(),
                questions: Vec::new(),
                recommended_next_actions: vec!["continue".to_string()],
                confidence: ReportConfidence::High,
                raw_output: "raw".to_string(),
                parse_result: ReportParseResult::Parsed,
                needs_supervisor_review: false,
                created_at: now,
            };
            state
                .collaboration
                .reports
                .insert(report.id.clone(), report.clone());
            state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .unwrap()
                .latest_report_id = Some(report.id.clone());
            report
        };
        service
            .persist_collaboration_state()
            .await
            .expect("persist");

        let response = service
            .decision_apply(ipc::DecisionApplyRequest {
                work_unit_id: work_unit.id.clone(),
                report_id: Some(report.id),
                decision_type: DecisionType::Continue,
                rationale: "Need one more bounded pass".to_string(),
                instructions: Some("Inspect the recovery branch".to_string()),
                worker_id: None,
                worker_kind: None,
            })
            .await
            .expect("decision");

        let next_assignment = response.next_assignment.expect("next assignment");
        assert_ne!(next_assignment.id, assignment_id);
        assert_eq!(next_assignment.worker_session_id, worker_session_id);
        assert_eq!(next_assignment.status, AssignmentStatus::Created);

        let state = service.state.read().await;
        assert_eq!(
            state.collaboration.assignments[&assignment_id].status,
            AssignmentStatus::Closed
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id]
                .current_assignment_id
                .as_deref(),
            Some(next_assignment.id.as_str())
        );
        assert_eq!(
            state.collaboration.work_units[&work_unit.id].status,
            WorkUnitStatus::Ready
        );
        assert_eq!(
            state.collaboration.assignments[&next_assignment.id].worker_session_id,
            worker_session_id
        );
    }

    #[tokio::test]
    async fn lost_worker_session_gets_fresh_session_on_next_assignment() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Reconnect".to_string(),
                objective: "Replace lost worker session".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id,
                title: "Inspect continuity".to_string(),
                task_statement: "Exercise lost worker-session replacement.".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let prepared = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Initial assignment".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");
        let report = {
            let now = Utc::now();
            let mut state = service.state.write().await;
            state
                .collaboration
                .assignments
                .get_mut(&prepared.assignment.id)
                .unwrap()
                .status = AssignmentStatus::AwaitingDecision;
            state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .unwrap()
                .status = WorkUnitStatus::AwaitingDecision;
            state
                .collaboration
                .worker_sessions
                .get_mut(&prepared.assignment.worker_session_id)
                .unwrap()
                .runtime_status = WorkerSessionRuntimeStatus::Lost;
            let report = Report {
                id: "report-lost-session".to_string(),
                work_unit_id: work_unit.id.clone(),
                assignment_id: prepared.assignment.id.clone(),
                worker_id: prepared.assignment.worker_id.clone(),
                disposition: ReportDisposition::Failed,
                summary: "Prior worker session anchor was lost.".to_string(),
                findings: Vec::new(),
                blockers: vec!["Need a fresh session".to_string()],
                questions: Vec::new(),
                recommended_next_actions: vec!["Continue with a fresh session".to_string()],
                confidence: ReportConfidence::Medium,
                raw_output: "raw".to_string(),
                parse_result: ReportParseResult::Parsed,
                needs_supervisor_review: true,
                created_at: now,
            };
            state
                .collaboration
                .reports
                .insert(report.id.clone(), report.clone());
            state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .unwrap()
                .latest_report_id = Some(report.id.clone());
            report
        };
        service
            .persist_collaboration_state()
            .await
            .expect("persist");

        let response = service
            .decision_apply(ipc::DecisionApplyRequest {
                work_unit_id: work_unit.id.clone(),
                report_id: Some(report.id),
                decision_type: DecisionType::Continue,
                rationale: "Create a fresh execution segment.".to_string(),
                instructions: Some("Run with a fresh runtime anchor.".to_string()),
                worker_id: None,
                worker_kind: None,
            })
            .await
            .expect("decision");

        let next_assignment = response.next_assignment.expect("next assignment");
        assert_ne!(
            next_assignment.worker_session_id,
            prepared.assignment.worker_session_id
        );
        let state = service.state.read().await;
        assert_eq!(
            state.collaboration.assignments[&prepared.assignment.id].worker_session_id,
            prepared.assignment.worker_session_id
        );
        assert_eq!(
            state.collaboration.assignments[&next_assignment.id].worker_session_id,
            next_assignment.worker_session_id
        );
        assert_eq!(
            state.collaboration.worker_sessions[&prepared.assignment.worker_session_id]
                .runtime_status,
            WorkerSessionRuntimeStatus::Lost
        );
    }

    #[tokio::test]
    async fn mark_complete_updates_work_unit_and_workstream() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Reconnect".to_string(),
                objective: "Fix regression".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Inspect streaming".to_string(),
                task_statement: "Inspect the current streaming path".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;

        let response = service
            .decision_apply(ipc::DecisionApplyRequest {
                work_unit_id: work_unit.id.clone(),
                report_id: None,
                decision_type: DecisionType::MarkComplete,
                rationale: "Accepted as complete".to_string(),
                instructions: None,
                worker_id: None,
                worker_kind: None,
            })
            .await
            .expect("decision");

        assert_eq!(response.work_unit.status, WorkUnitStatus::Completed);
        let state = service.state.read().await;
        assert_eq!(
            state.collaboration.workstreams[&workstream.id].status,
            WorkstreamStatus::Completed
        );
    }

    #[tokio::test]
    async fn snapshot_reflects_assignment_report_and_decision_transitions() {
        let service = test_service().await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Reconnect".to_string(),
                objective: "Fix regression".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id,
                title: "Inspect streaming".to_string(),
                task_statement: "Inspect the current streaming path".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let prepared = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the reconnect path".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");
        let report = {
            let now = Utc::now();
            let mut state = service.state.write().await;
            state
                .collaboration
                .assignments
                .get_mut(&prepared.assignment.id)
                .unwrap()
                .status = AssignmentStatus::AwaitingDecision;
            state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .unwrap()
                .status = WorkUnitStatus::AwaitingDecision;
            let report = Report {
                id: "report-snapshot".to_string(),
                work_unit_id: work_unit.id.clone(),
                assignment_id: prepared.assignment.id.clone(),
                worker_id: prepared.assignment.worker_id.clone(),
                disposition: ReportDisposition::Completed,
                summary: "done".to_string(),
                findings: Vec::new(),
                blockers: Vec::new(),
                questions: Vec::new(),
                recommended_next_actions: Vec::new(),
                confidence: ReportConfidence::High,
                raw_output: "raw".to_string(),
                parse_result: ReportParseResult::Ambiguous,
                needs_supervisor_review: true,
                created_at: now,
            };
            state
                .collaboration
                .reports
                .insert(report.id.clone(), report.clone());
            state
                .collaboration
                .work_units
                .get_mut(&work_unit.id)
                .unwrap()
                .latest_report_id = Some(report.id.clone());
            report
        };
        service
            .persist_collaboration_state()
            .await
            .expect("persist");
        let mid_snapshot = service.snapshot().await.expect("snapshot");
        assert_eq!(mid_snapshot.collaboration.assignments.len(), 1);
        assert_eq!(mid_snapshot.collaboration.reports.len(), 1);
        assert_eq!(
            mid_snapshot.collaboration.reports[0].parse_result,
            ReportParseResult::Ambiguous
        );
        assert!(mid_snapshot.collaboration.reports[0].needs_supervisor_review);

        let _ = service
            .decision_apply(ipc::DecisionApplyRequest {
                work_unit_id: work_unit.id.clone(),
                report_id: Some(report.id),
                decision_type: DecisionType::MarkComplete,
                rationale: "done".to_string(),
                instructions: None,
                worker_id: None,
                worker_kind: None,
            })
            .await
            .expect("decision");
        let final_snapshot = service.snapshot().await.expect("snapshot");
        assert!(final_snapshot.collaboration.assignments.is_empty());
        assert_eq!(final_snapshot.collaboration.decisions.len(), 1);
        assert_eq!(
            final_snapshot.collaboration.work_units[0].status,
            WorkUnitStatus::Completed
        );
    }

    #[tokio::test]
    async fn daemon_emits_collaboration_lifecycle_events() {
        let service = test_service().await;
        let mut events = service.event_tx.subscribe();
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Reconnect".to_string(),
                objective: "Fix regression".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let work_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id,
                title: "Inspect streaming".to_string(),
                task_statement: "Inspect the current streaming path".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let prepared = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: work_unit.id.clone(),
                worker_id: "worker-a".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("Inspect the reconnect path".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("assignment");
        service
            .emit_assignment_lifecycle(
                ipc::AssignmentLifecycleAction::Created,
                &prepared.assignment,
            )
            .await;
        let report = Report {
            id: "report-event".to_string(),
            work_unit_id: work_unit.id.clone(),
            assignment_id: prepared.assignment.id.clone(),
            worker_id: prepared.assignment.worker_id.clone(),
            disposition: ReportDisposition::Completed,
            summary: "done".to_string(),
            findings: Vec::new(),
            blockers: Vec::new(),
            questions: Vec::new(),
            recommended_next_actions: Vec::new(),
            confidence: ReportConfidence::High,
            raw_output: "raw".to_string(),
            parse_result: ReportParseResult::Parsed,
            needs_supervisor_review: false,
            created_at: Utc::now(),
        };
        service.emit_report_recorded(&report).await;
        let _ = service
            .decision_apply(ipc::DecisionApplyRequest {
                work_unit_id: work_unit.id.clone(),
                report_id: None,
                decision_type: DecisionType::EscalateToHuman,
                rationale: "need review".to_string(),
                instructions: None,
                worker_id: None,
                worker_kind: None,
            })
            .await;

        let mut saw_workstream = false;
        let mut saw_work_unit = false;
        let mut saw_assignment = false;
        let mut saw_report = false;
        let mut saw_decision = false;
        for _ in 0..8 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
                .await
                .expect("event timeout")
                .expect("event");
            match event.event {
                ipc::DaemonEvent::WorkstreamLifecycle { .. } => saw_workstream = true,
                ipc::DaemonEvent::WorkUnitLifecycle { .. } => saw_work_unit = true,
                ipc::DaemonEvent::AssignmentLifecycle { .. } => saw_assignment = true,
                ipc::DaemonEvent::ReportRecorded { .. } => saw_report = true,
                ipc::DaemonEvent::DecisionApplied { .. } => saw_decision = true,
                _ => {}
            }
        }
        assert!(saw_workstream);
        assert!(saw_work_unit);
        assert!(saw_assignment);
        assert!(saw_report);
        assert!(saw_decision);
    }

    #[tokio::test]
    async fn restart_preserves_failed_interrupted_and_lost_collaboration_truth() {
        let base = std::env::temp_dir().join(format!("orcas-collab-restart-{}", Uuid::new_v4()));
        let service = test_service_at(base.clone()).await;
        let workstream = service
            .workstream_create(ipc::WorkstreamCreateRequest {
                title: "Restart truth".to_string(),
                objective: "Keep failure state visible across restart.".to_string(),
                priority: None,
            })
            .await
            .expect("workstream")
            .workstream;
        let failed_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Failed start".to_string(),
                task_statement: "Persist failed state".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;
        let interrupted_unit = service
            .workunit_create(ipc::WorkunitCreateRequest {
                workstream_id: workstream.id.clone(),
                title: "Interrupted run".to_string(),
                task_statement: "Persist interrupted state".to_string(),
                dependencies: Vec::new(),
            })
            .await
            .expect("workunit")
            .work_unit;

        let failed_assignment = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: failed_unit.id.clone(),
                worker_id: "worker-f".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("fail".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("failed assignment")
            .assignment;
        let interrupted_assignment = service
            .prepare_assignment(ipc::AssignmentStartRequest {
                work_unit_id: interrupted_unit.id.clone(),
                worker_id: "worker-i".to_string(),
                worker_kind: Some("codex".to_string()),
                instructions: Some("interrupt".to_string()),
                model: None,
                cwd: None,
            })
            .await
            .expect("interrupted assignment")
            .assignment;
        {
            let now = Utc::now();
            let mut state = service.state.write().await;
            state
                .collaboration
                .assignments
                .get_mut(&failed_assignment.id)
                .unwrap()
                .status = AssignmentStatus::Failed;
            state
                .collaboration
                .work_units
                .get_mut(&failed_unit.id)
                .unwrap()
                .status = WorkUnitStatus::AwaitingDecision;
            state
                .collaboration
                .assignments
                .get_mut(&interrupted_assignment.id)
                .unwrap()
                .status = AssignmentStatus::Interrupted;
            state
                .collaboration
                .worker_sessions
                .get_mut(&interrupted_assignment.worker_session_id)
                .unwrap()
                .runtime_status = WorkerSessionRuntimeStatus::Lost;
            state
                .collaboration
                .work_units
                .get_mut(&interrupted_unit.id)
                .unwrap()
                .status = WorkUnitStatus::AwaitingDecision;
            let report = Report {
                id: "report-restart".to_string(),
                work_unit_id: interrupted_unit.id.clone(),
                assignment_id: interrupted_assignment.id.clone(),
                worker_id: interrupted_assignment.worker_id.clone(),
                disposition: ReportDisposition::Interrupted,
                summary: "Retained interrupted raw output.".to_string(),
                findings: Vec::new(),
                blockers: Vec::new(),
                questions: Vec::new(),
                recommended_next_actions: Vec::new(),
                confidence: ReportConfidence::Unknown,
                raw_output: "partial".to_string(),
                parse_result: ReportParseResult::Invalid,
                needs_supervisor_review: true,
                created_at: now,
            };
            state
                .collaboration
                .reports
                .insert(report.id.clone(), report.clone());
            state
                .collaboration
                .work_units
                .get_mut(&interrupted_unit.id)
                .unwrap()
                .latest_report_id = Some(report.id);
        }
        service
            .persist_collaboration_state()
            .await
            .expect("persist");
        drop(service);

        let restarted = test_service_at(base).await;
        let snapshot = restarted.snapshot().await.expect("snapshot");
        assert!(
            snapshot
                .collaboration
                .assignments
                .iter()
                .any(|assignment| assignment.id == failed_assignment.id
                    && assignment.status == AssignmentStatus::Failed)
        );
        assert!(
            snapshot
                .collaboration
                .assignments
                .iter()
                .any(|assignment| assignment.id == interrupted_assignment.id
                    && assignment.status == AssignmentStatus::Interrupted)
        );
        assert!(
            snapshot
                .collaboration
                .reports
                .iter()
                .any(|report| report.work_unit_id == interrupted_unit.id
                    && report.parse_result == ReportParseResult::Invalid
                    && report.needs_supervisor_review)
        );
        assert_eq!(
            restarted.state.read().await.collaboration.worker_sessions
                [&interrupted_assignment.worker_session_id]
                .runtime_status,
            WorkerSessionRuntimeStatus::Lost
        );
    }

    #[tokio::test]
    async fn authority_queries_and_mutations_round_trip_through_service() {
        let base = std::env::temp_dir().join(format!("orcas-authority-service-{}", Uuid::new_v4()));
        let service = test_service_at(base.clone()).await;
        let origin_node_id = service.authority_store.origin_node_id().expect("origin");

        let metadata = |label: &str| orcas_core::authority::CommandMetadata {
            command_id: orcas_core::authority::CommandId::new(),
            issued_at: Utc::now(),
            origin_node_id: origin_node_id.clone(),
            actor: orcas_core::authority::CommandActor::parse("service_test")
                .expect("command actor"),
            correlation_id: Some(
                orcas_core::authority::CorrelationId::parse(format!("corr-{label}"))
                    .expect("correlation id"),
            ),
        };

        let workstream = service
            .authority_workstream_create(ipc::AuthorityWorkstreamCreateRequest {
                command: orcas_core::authority::CreateWorkstream {
                    metadata: metadata("ws-create"),
                    workstream_id: orcas_core::authority::WorkstreamId::parse("svc-ws")
                        .expect("workstream id"),
                    title: "Service workstream".to_string(),
                    objective: "Exercise authority API".to_string(),
                    status: WorkstreamStatus::Active,
                    priority: "high".to_string(),
                },
            })
            .await
            .expect("authority workstream create")
            .workstream;

        let work_unit = service
            .authority_workunit_create(ipc::AuthorityWorkunitCreateRequest {
                command: orcas_core::authority::CreateWorkUnit {
                    metadata: metadata("wu-create"),
                    work_unit_id: orcas_core::authority::WorkUnitId::parse("svc-wu")
                        .expect("work unit id"),
                    workstream_id: workstream.id.clone(),
                    title: "Service work unit".to_string(),
                    task_statement: "Persist through service".to_string(),
                    status: WorkUnitStatus::Ready,
                },
            })
            .await
            .expect("authority work unit create")
            .work_unit;

        let tracked_thread = service
            .authority_tracked_thread_create(ipc::AuthorityTrackedThreadCreateRequest {
                command: orcas_core::authority::CreateTrackedThread {
                    metadata: metadata("tt-create"),
                    tracked_thread_id: orcas_core::authority::TrackedThreadId::parse("svc-tt")
                        .expect("tracked thread id"),
                    work_unit_id: work_unit.id.clone(),
                    title: "Service tracked thread".to_string(),
                    notes: Some("Local record".to_string()),
                    backend_kind: orcas_core::authority::TrackedThreadBackendKind::Codex,
                    upstream_thread_id: Some("upstream-service-thread".to_string()),
                    preferred_cwd: Some("/tmp/orcas".to_string()),
                    preferred_model: Some("gpt-5.4".to_string()),
                },
            })
            .await
            .expect("authority tracked thread create")
            .tracked_thread;

        let hierarchy = service
            .authority_hierarchy_get(ipc::AuthorityHierarchyGetRequest::default())
            .await
            .expect("authority hierarchy")
            .hierarchy;
        assert_eq!(hierarchy.workstreams.len(), 1);
        assert_eq!(hierarchy.workstreams[0].work_units.len(), 1);
        assert_eq!(
            hierarchy.workstreams[0].work_units[0].tracked_threads.len(),
            1
        );

        let delete_plan = service
            .authority_delete_plan(ipc::AuthorityDeletePlanRequest {
                target: orcas_core::authority::DeleteTarget::Workstream {
                    workstream_id: workstream.id.clone(),
                },
            })
            .await
            .expect("authority delete plan")
            .delete_plan;
        assert_eq!(delete_plan.affected_work_units, 1);
        assert_eq!(delete_plan.affected_tracked_threads, 1);
        assert!(delete_plan.has_upstream_bindings);

        drop(service);

        let restarted = test_service_at(base).await;
        let loaded = restarted
            .authority_tracked_thread_get(ipc::AuthorityTrackedThreadGetRequest {
                tracked_thread_id: tracked_thread.id,
            })
            .await
            .expect("authority tracked thread get after restart")
            .tracked_thread;
        assert_eq!(loaded.title, "Service tracked thread");
        assert_eq!(
            loaded.upstream_thread_id.as_deref(),
            Some("upstream-service-thread")
        );
    }
}
