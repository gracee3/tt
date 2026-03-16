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
    AppConfig, AppPaths, CodexConnectionMode, ConnectionState, EventEnvelope, JsonSessionStore,
    OrcasError, OrcasEvent, OrcasResult, OrcasSessionStore, ThreadMetadata,
};

use crate::process::{
    ENV_CODEX_BIN, ENV_CODEX_LISTEN_URL, ENV_CONNECTION_MODE, ENV_DEFAULT_CWD, ENV_DEFAULT_MODEL,
    OrcasDaemonProcessManager, OrcasRuntimeOverrides, apply_runtime_overrides,
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
        }
    }
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
    codex_daemon: LocalCodexDaemonManager,
    codex_client: Arc<CodexClient>,
    state: RwLock<DaemonState>,
    recent_events: Mutex<VecDeque<ipc::EventSummary>>,
    connect_gate: Mutex<()>,
    event_tx: broadcast::Sender<ipc::DaemonEventEnvelope>,
    client_count: AtomicUsize,
    shutdown: Notify,
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
        let codex_daemon =
            LocalCodexDaemonManager::new(config.codex.clone(), &paths, config.defaults.cwd.clone());
        let codex_client = CodexClient::new(
            Arc::new(WebSocketTransport::new(config.codex.listen_url.clone())),
            config.codex.reconnect.clone(),
            Arc::new(RejectingApprovalRouter),
        );
        let (event_tx, _) = broadcast::channel(512);

        let service = Arc::new(Self {
            paths,
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

    async fn snapshot(&self) -> OrcasResult<ipc::StateSnapshot> {
        let daemon = self.daemon_status().await?;
        let state = self.state.read().await;
        let threads = Self::scoped_thread_summaries(&state.threads);
        let active_thread = Self::focus_thread_view(&state, &threads);
        let session = state.session.clone();
        drop(state);

        Ok(ipc::StateSnapshot {
            daemon,
            session,
            threads,
            active_thread,
            recent_events: self.recent_events.lock().await.iter().cloned().collect(),
        })
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

    use chrono::Utc;

    use super::OrcasDaemonService;
    use super::{DaemonState, TurnKey};
    use orcas_core::ipc;

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
}
