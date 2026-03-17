use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{TimeZone, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, Notify, RwLock, broadcast, mpsc};
use tokio::time::{Duration, sleep};
use tracing::{info, warn};
use uuid::Uuid;

use orcas_codex::types;
use orcas_codex::{
    CodexClient, CodexDaemonManager, DaemonLaunch as CodexDaemonLaunch, LocalCodexDaemonManager,
    RejectingApprovalRouter, WebSocketTransport,
};
use orcas_core::ipc;
use orcas_core::jsonrpc::{
    JsonRpcError, JsonRpcErrorObject, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse,
};
use orcas_core::{
    AppConfig, AppPaths, Assignment, AssignmentCommunicationPacket, AssignmentCommunicationRecord,
    AssignmentCommunicationSeed, AssignmentModeSpec, AssignmentStatus, CodexConnectionMode,
    CollaborationState, ConnectionState, Decision, DecisionType, DraftAssignment, EventEnvelope,
    ImplementModeSpec, JsonSessionStore, OrcasError, OrcasEvent, OrcasResult, OrcasSessionStore,
    Report, SupervisorContextPack, SupervisorProposal, SupervisorProposalFailure,
    SupervisorProposalFailureStage, SupervisorProposalRecord, SupervisorProposalStatus,
    SupervisorProposalTriggerKind, SupervisorReasonerUsage, ThreadMetadata, WorkUnit,
    WorkUnitStatus, Worker, WorkerSession, WorkerSessionAttachability, WorkerSessionRuntimeStatus,
    WorkerStatus, Workstream, WorkstreamStatus,
};

use crate::assignment_comm::parse::parse_worker_report_for_turn;
use crate::assignment_comm::policy::validate_assignment_packet;
use crate::assignment_comm::render::build_assignment_communication_record;
use crate::assignment_comm::stable_fingerprint;
use crate::process::{
    ENV_CODEX_BIN, ENV_CODEX_LISTEN_URL, ENV_CONNECTION_MODE, ENV_DEFAULT_CWD, ENV_DEFAULT_MODEL,
    OrcasDaemonProcessManager, OrcasRuntimeOverrides, apply_runtime_overrides,
};
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
        let paths = AppPaths::discover()?;
        paths.ensure().await?;
        let mut config = AppConfig::write_default_if_missing(&paths).await?;
        apply_runtime_overrides(&mut config, &Self::overrides_from_env());
        let runtime =
            OrcasDaemonProcessManager::runtime_metadata_for_current_process(&paths).await?;

        let store = Arc::new(JsonSessionStore::new(paths.clone(), config.clone()));
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

        Ok(service)
    }

    pub async fn run(self: Arc<Self>) -> OrcasResult<()> {
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
                signal = tokio::signal::ctrl_c() => {
                    if let Err(error) = signal {
                        warn!(%error, "failed to listen for shutdown signal");
                    }
                    break;
                }
                _ = self.shutdown.notified() => {
                    break;
                }
            }
        }

        Ok(())
    }

    async fn bind_listener(&self) -> OrcasResult<UnixListener> {
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
        let stored = self.store.load().await.unwrap_or_default();
        let mut state = self.state.write().await;
        state.upstream = ConnectionState {
            endpoint: self.config.codex.listen_url.clone(),
            status: "disconnected".to_string(),
            detail: None,
        };
        state.threads = stored
            .registry
            .threads
            .values()
            .map(|metadata| {
                let view = Self::thread_view_from_metadata(metadata);
                (view.summary.id.clone(), view)
            })
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
                self.handle_request(request, outbound, subscription_task)
                    .await?;
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
            ipc::methods::THREAD_START => {
                let params: ipc::ThreadStartRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.thread_start(params).await?)?
            }
            ipc::methods::THREAD_READ => {
                let params: ipc::ThreadReadRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.thread_read(params).await?)?
            }
            ipc::methods::THREAD_GET => {
                let params: ipc::ThreadGetRequest = Self::decode_params(request.params.clone())?;
                serde_json::to_value(self.thread_get(params).await?)?
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
        self.connect_upstream().await?;
        Ok(ipc::DaemonConnectResponse {
            status: self.daemon_status().await?,
        })
    }

    async fn daemon_stop(&self) -> OrcasResult<ipc::DaemonStopResponse> {
        Ok(ipc::DaemonStopResponse { stopping: true })
    }

    async fn state_get(&self) -> OrcasResult<ipc::StateGetResponse> {
        Ok(ipc::StateGetResponse {
            snapshot: self.snapshot().await?,
        })
    }

    async fn session_get_active(&self) -> OrcasResult<ipc::SessionGetActiveResponse> {
        Ok(ipc::SessionGetActiveResponse {
            session: self.state.read().await.session.clone(),
        })
    }

    async fn models_list(&self) -> OrcasResult<ipc::ModelsListResponse> {
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
        self.set_active_thread(&view.summary.id).await;
        Ok(ipc::ThreadResumeResponse {
            thread: view.summary,
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
                let record = self
                    .persist_failed_proposal_record(
                        proposal_id.clone(),
                        context_pack.clone(),
                        failure.backend_kind.clone(),
                        failure.model.clone(),
                        failure.response_id.clone(),
                        None,
                        failure.output_text.clone(),
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
            let record = self
                .persist_failed_proposal_record(
                    proposal_id.clone(),
                    context_pack.clone(),
                    result.backend_kind.clone(),
                    result.model.clone(),
                    result.response_id.clone(),
                    result.usage.clone(),
                    result.output_text.clone(),
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
        let reviewed_by = params
            .reviewed_by
            .clone()
            .unwrap_or_else(|| "supervisor_cli".to_string());

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
            return Err(OrcasError::Protocol(format!(
                "proposal `{}` is not open and cannot be approved",
                proposal.id
            )));
        }

        let original_proposal = proposal.proposal.as_ref().ok_or_else(|| {
            OrcasError::Protocol(format!(
                "proposal `{}` does not contain a model-generated proposal payload",
                proposal.id
            ))
        })?;
        let approved_proposal = apply_edits(original_proposal, &params.edits);
        validate_proposal(&approved_proposal, &proposal.context_pack, &collaboration)?;

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
        let reviewed_by = params
            .reviewed_by
            .clone()
            .unwrap_or_else(|| "supervisor_cli".to_string());
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
                    OrcasError::Protocol(format!("unknown proposal `{}`", params.proposal_id))
                })?;
            if proposal_record.status != SupervisorProposalStatus::Open {
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

    async fn ensure_assignment_communication_record(
        &self,
        assignment_id: &str,
        requested_model: Option<String>,
        requested_cwd: Option<String>,
    ) -> OrcasResult<()> {
        if self
            .state
            .read()
            .await
            .collaboration
            .assignment_communications
            .contains_key(assignment_id)
        {
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
                return Ok(());
            }
            let assignment = state
                .collaboration
                .assignments
                .get(assignment_id)
                .cloned()
                .ok_or_else(|| {
                    OrcasError::Protocol(format!(
                        "unknown assignment `{assignment_id}` for communication record"
                    ))
                })?;
            let record = build_assignment_communication_record(
                &state.collaboration,
                &assignment,
                requested_model,
                requested_cwd,
                self.config.defaults.cwd.as_ref(),
                now,
            )?;
            validate_assignment_packet(&record.packet)?;
            state
                .collaboration
                .assignment_communications
                .insert(assignment_id.to_string(), record.clone());
            record
        };
        let _ = record;
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
        self.emit(ipc::DaemonEvent::WorkstreamLifecycle {
            action,
            workstream: Self::workstream_summary(workstream),
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
            state
                .collaboration
                .work_units
                .get(&work_unit.id)
                .map(|work_unit| {
                    Self::work_unit_summary_for_collaboration(work_unit, &state.collaboration)
                })
                .unwrap_or_else(|| {
                    Self::work_unit_summary_for_collaboration(work_unit, &state.collaboration)
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
                    Self::work_unit_summary_for_collaboration(work_unit, &state.collaboration)
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
        let collaboration = Self::collaboration_snapshot(&state.collaboration);
        drop(state);

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

    fn collaboration_snapshot(collaboration: &CollaborationState) -> ipc::CollaborationSnapshot {
        let mut workstreams = collaboration
            .workstreams
            .values()
            .map(Self::workstream_summary)
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
            .map(|work_unit| Self::work_unit_summary_for_collaboration(work_unit, collaboration))
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
            reports,
            decisions,
        }
    }

    fn workstream_summary(workstream: &Workstream) -> ipc::WorkstreamSummary {
        ipc::WorkstreamSummary {
            id: workstream.id.clone(),
            title: workstream.title.clone(),
            objective: workstream.objective.clone(),
            status: workstream.status,
            priority: workstream.priority.clone(),
            updated_at: workstream.updated_at,
        }
    }

    fn work_unit_summary_for_collaboration(
        work_unit: &WorkUnit,
        collaboration: &CollaborationState,
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
                recent_output: thread.summary.recent_output.clone(),
                recent_event: thread.summary.recent_event.clone(),
                turn_in_flight: thread.summary.turn_in_flight,
            })
            .await
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
        let (session, turn, thread) = {
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
            Self::upsert_turn_state(&mut state, turn_state);
            Self::refresh_session_from_turns(&mut state);
            state.session.active_thread_id = Some(thread_id.to_string());
            state.recent_thread_id = Some(thread_id.to_string());
            (state.session.clone(), turn, thread_summary)
        };
        if let Some(thread_view) = self.thread_from_state(thread_id).await.as_ref() {
            let _ = self.persist_thread_view(thread_view, None).await;
        }
        self.emit(ipc::DaemonEvent::ThreadUpdated { thread }).await;
        self.emit(ipc::DaemonEvent::SessionChanged { session })
            .await;
        self.emit(ipc::DaemonEvent::TurnUpdated {
            thread_id: thread_id.to_string(),
            turn,
        })
        .await;
    }

    async fn apply_codex_event(&self, envelope: EventEnvelope) {
        match envelope.event {
            OrcasEvent::ConnectionStateChanged(upstream) => {
                let maybe_session = {
                    let mut state = self.state.write().await;
                    state.upstream = upstream.clone();
                    if upstream.status != "connected" {
                        Self::mark_turns_lost(&mut state);
                    }
                    Self::refresh_session_from_turns(&mut state);
                    state.session.clone()
                };
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
                let (session, turn, thread) = {
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
                    Self::upsert_turn_state(&mut state, turn_state);
                    Self::refresh_session_from_turns(&mut state);
                    if state.session.active_turns.is_empty() {
                        state.session.active_thread_id = Some(thread_id.clone());
                    }
                    state.recent_thread_id = Some(thread_id.clone());
                    (state.session.clone(), turn, thread_summary)
                };
                if let Some(thread_view) = self.thread_from_state(&thread_id).await.as_ref() {
                    let _ = self.persist_thread_view(thread_view, None).await;
                }
                self.emit(ipc::DaemonEvent::ThreadUpdated { thread }).await;
                self.emit(ipc::DaemonEvent::SessionChanged { session })
                    .await;
                self.emit(ipc::DaemonEvent::TurnUpdated { thread_id, turn })
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
        Self::upsert_turn_state(
            &mut state,
            ipc::TurnStateView {
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
            },
        );
        Self::refresh_session_from_turns(&mut state);
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
            ipc::DaemonEvent::AssignmentLifecycle { action, assignment } => (
                "assignment",
                format!("assignment {} {:?}", assignment.id, action),
                None,
                None,
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
                recent_output: None,
                recent_event: None,
                turn_in_flight: false,
            },
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
                recent_output: metadata.recent_output.clone(),
                recent_event: metadata.recent_event.clone(),
                turn_in_flight: metadata.turn_in_flight,
            },
            turns: Vec::new(),
        }
    }

    fn thread_view_from_codex(
        thread: types::Thread,
        existing: Option<&ipc::ThreadView>,
        scope: Option<&str>,
    ) -> ipc::ThreadView {
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
                recent_output: existing.and_then(|thread| thread.summary.recent_output.clone()),
                recent_event: existing.and_then(|thread| thread.summary.recent_event.clone()),
                turn_in_flight: existing
                    .map(|thread| thread.summary.turn_in_flight)
                    .unwrap_or(false),
            },
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
        ipc::TurnView {
            id: turn.id,
            status: turn.status.label().to_string(),
            error_message: turn.error.map(|error| error.message),
            items: turn
                .items
                .into_iter()
                .map(|item| {
                    let text = item.text().map(ToOwned::to_owned);
                    ipc::ItemView {
                        id: item.id,
                        item_type: item.item_type,
                        status: None,
                        text,
                    }
                })
                .collect(),
        }
    }

    fn touch_thread(thread: &mut ipc::ThreadView) {
        thread.summary.updated_at = Utc::now().timestamp();
    }

    fn refresh_thread_summary(thread: &mut ipc::ThreadView) {
        thread.summary.turn_in_flight = thread
            .turns
            .iter()
            .any(|turn| !Self::is_terminal_status(&turn.status));
        if let Some(output) = thread
            .turns
            .iter()
            .rev()
            .flat_map(|turn| turn.items.iter().rev())
            .filter_map(|item| item.text.as_deref())
            .find(|text| !text.trim().is_empty())
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
            .filter_map(|item| item.text.as_deref())
            .collect::<String>();
        (!text.is_empty()).then_some(Self::truncate_snippet(&text))
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
        let codex_bin = std::env::var_os(ENV_CODEX_BIN).map(PathBuf::from);
        let listen_url = std::env::var(ENV_CODEX_LISTEN_URL).ok();
        let cwd = std::env::var_os(ENV_DEFAULT_CWD).map(PathBuf::from);
        let model = std::env::var(ENV_DEFAULT_MODEL).ok();
        let mode = std::env::var(ENV_CONNECTION_MODE).ok();
        OrcasRuntimeOverrides {
            codex_bin,
            listen_url,
            cwd,
            model,
            connect_only: mode.as_deref() == Some("connect_only"),
            force_spawn: mode.as_deref() == Some("spawn_always"),
        }
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
    use chrono::Utc;
    use serde_json::{Map, Value};
    use tokio::sync::{Mutex, Notify, RwLock, broadcast};
    use tokio::time::Duration;
    use uuid::Uuid;

    use super::OrcasDaemonService;
    use super::{DaemonState, TurnKey};
    use crate::assignment_comm::parse::{parse_worker_report, parse_worker_report_for_turn};
    use crate::assignment_comm::render::{build_assignment_communication_record, render_prompt};
    use crate::supervisor::{
        SupervisorReasoner, SupervisorReasonerFailure, SupervisorReasonerResult,
    };
    use orcas_codex::{
        CodexClient, CodexDaemonManager, CodexTransport, DaemonLaunch, DaemonStatus,
        LocalCodexDaemonManager, RejectingApprovalRouter, WebSocketTransport, methods,
        protocol::jsonrpc as codex_jsonrpc, transport::TransportConnection, types,
    };
    use orcas_core::{
        AppConfig, AppPaths, Assignment, AssignmentCommunicationSeed, AssignmentModeSpec,
        AssignmentStatus, CollaborationState, DecisionType, DraftAssignment, ImplementModeSpec,
        JsonSessionStore, OrcasError, OrcasResult, OrcasSessionStore, ProposedDecision, Report,
        ReportConfidence, ReportDisposition, ReportParseResult, SupervisorContextPack,
        SupervisorProposal, SupervisorProposalEdits, SupervisorProposalFailureStage,
        SupervisorProposalStatus, SupervisorProposalTriggerKind, SupervisorSummary, WorkUnit,
        WorkUnitStatus, WorkerSessionAttachability, WorkerSessionRuntimeStatus, WorkerStatus,
        Workstream, WorkstreamStatus, ipc,
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
                methods::TURN_INTERRUPT => {
                    Self::send_response(&inbound_tx, request.id, &types::TurnInterruptResponse {})
                        .await?;
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

    #[derive(Clone)]
    enum StaticSupervisorReasonerOutcome {
        Success(SupervisorReasonerResult),
        Failure(SupervisorReasonerFailure),
    }

    #[derive(Default)]
    struct StaticSupervisorReasoner {
        outcome: Mutex<Option<StaticSupervisorReasonerOutcome>>,
        last_pack: Mutex<Option<SupervisorContextPack>>,
        propose_calls: AtomicUsize,
    }

    impl StaticSupervisorReasoner {
        async fn set_proposal(&self, proposal: SupervisorProposal) {
            let output_text = serde_json::to_string(&proposal).expect("serialize proposal");
            *self.outcome.lock().await = Some(StaticSupervisorReasonerOutcome::Success(
                SupervisorReasonerResult {
                    proposal,
                    backend_kind: "test".to_string(),
                    model: "test-supervisor".to_string(),
                    response_id: Some("resp-test".to_string()),
                    usage: None,
                    output_text: Some(output_text),
                },
            ));
        }

        async fn set_failure(
            &self,
            stage: SupervisorProposalFailureStage,
            message: impl Into<String>,
            output_text: Option<String>,
        ) {
            *self.outcome.lock().await = Some(StaticSupervisorReasonerOutcome::Failure(
                SupervisorReasonerFailure {
                    stage,
                    message: message.into(),
                    backend_kind: "test".to_string(),
                    model: "test-supervisor".to_string(),
                    response_id: Some("resp-test".to_string()),
                    output_text,
                },
            ));
        }

        async fn last_pack(&self) -> Option<SupervisorContextPack> {
            self.last_pack.lock().await.clone()
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
            *self.last_pack.lock().await = Some(pack);
            match self.outcome.lock().await.clone() {
                Some(StaticSupervisorReasonerOutcome::Success(result)) => Ok(result),
                Some(StaticSupervisorReasonerOutcome::Failure(failure)) => Err(failure),
                None => Err(SupervisorReasonerFailure {
                    stage: SupervisorProposalFailureStage::Backend,
                    message: "missing test supervisor reasoner outcome".to_string(),
                    backend_kind: "test".to_string(),
                    model: "test-supervisor".to_string(),
                    response_id: Some("resp-test".to_string()),
                    output_text: None,
                }),
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
            Ok(SupervisorReasonerResult {
                proposal,
                backend_kind: "test".to_string(),
                model: "test-pack-driven".to_string(),
                response_id: Some("resp-pack-driven".to_string()),
                usage: None,
                output_text: Some(output_text),
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
                recent_output: None,
                recent_event: None,
                turn_in_flight: false,
            },
            turns: Vec::new(),
        }
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
    "tests_run": ["cargo test -p orcas-daemon"],
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
            items: vec![ipc::ItemView {
                id: "item-1".to_string(),
                item_type: "agent_message".to_string(),
                status: Some("streaming".to_string()),
                text: Some("hello world".to_string()),
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
            items: vec![ipc::ItemView {
                id: "item-1".to_string(),
                item_type: "agent_message".to_string(),
                status: Some("streaming".to_string()),
                text: Some("partial output".to_string()),
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
            items: vec![ipc::ItemView {
                id: "item-1".to_string(),
                item_type: "agent_message".to_string(),
                status: Some("completed".to_string()),
                text: Some("hello world".to_string()),
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
            items: Vec::new(),
        });

        let unknown_state =
            OrcasDaemonService::turn_state_from_thread_view(&unknown, "turn-2").unwrap();
        assert_eq!(unknown_state.lifecycle, ipc::TurnLifecycleState::Unknown);
        assert!(!unknown_state.attachable);
        assert!(!unknown_state.live_stream);
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
}
