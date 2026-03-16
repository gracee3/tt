use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tracing::{debug, warn};

use orcas_core::{
    ConnectionState, EventEnvelope, OrcasError, OrcasEvent, OrcasResult, ReconnectPolicy,
};

use crate::approval::{ApprovalDecision, ApprovalRouter, RejectingApprovalRouter};
use crate::protocol::jsonrpc::{
    JsonRpcError, JsonRpcErrorObject, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, RequestId,
};
use crate::protocol::{methods, types};
use crate::transport::{CodexTransport, ReconnectBackoff, WebSocketTransport};

pub type EventSubscription = broadcast::Receiver<EventEnvelope>;
pub type CodexClientHandle = Arc<CodexClient>;

type PendingResponse = oneshot::Sender<OrcasResult<Value>>;

pub struct CodexClient {
    transport: Arc<dyn CodexTransport>,
    approval_router: Arc<dyn ApprovalRouter>,
    pending: Mutex<HashMap<RequestId, PendingResponse>>,
    outbound: Mutex<Option<mpsc::Sender<String>>>,
    connection_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    initialize_params: Mutex<Option<types::InitializeParams>>,
    initialized: Mutex<bool>,
    initialize_gate: Mutex<()>,
    next_request_id: AtomicI64,
    reconnect: ReconnectBackoff,
    event_tx: broadcast::Sender<EventEnvelope>,
    threads: Mutex<HashMap<String, types::Thread>>,
}

impl CodexClient {
    pub fn new(
        transport: Arc<dyn CodexTransport>,
        reconnect_policy: ReconnectPolicy,
        approval_router: Arc<dyn ApprovalRouter>,
    ) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(512);
        Arc::new(Self {
            transport,
            approval_router,
            pending: Mutex::new(HashMap::new()),
            outbound: Mutex::new(None),
            connection_task: Mutex::new(None),
            initialize_params: Mutex::new(None),
            initialized: Mutex::new(false),
            initialize_gate: Mutex::new(()),
            next_request_id: AtomicI64::new(1),
            reconnect: ReconnectBackoff::new(reconnect_policy),
            event_tx,
            threads: Mutex::new(HashMap::new()),
        })
    }

    pub fn websocket(endpoint: impl Into<String>, reconnect_policy: ReconnectPolicy) -> Arc<Self> {
        Self::new(
            Arc::new(WebSocketTransport::new(endpoint)),
            reconnect_policy,
            Arc::new(RejectingApprovalRouter),
        )
    }

    pub fn subscribe(&self) -> EventSubscription {
        self.event_tx.subscribe()
    }

    pub async fn connect(self: &Arc<Self>) -> OrcasResult<()> {
        self.ensure_connection_task().await;
        self.wait_until_connected(Duration::from_secs(10)).await
    }

    pub async fn initialize(
        self: &Arc<Self>,
        params: types::InitializeParams,
    ) -> OrcasResult<types::InitializeResponse> {
        {
            let mut initialize_params = self.initialize_params.lock().await;
            *initialize_params = Some(params.clone());
        }
        if *self.initialized.lock().await {
            return Ok(types::InitializeResponse::default());
        }
        let response: types::InitializeResponse = self
            .request_without_initialize(methods::INITIALIZE, &params)
            .await?;
        self.notify(methods::INITIALIZED, None).await?;
        *self.initialized.lock().await = true;
        Ok(response)
    }

    pub async fn model_list(
        &self,
        params: types::ModelListParams,
    ) -> OrcasResult<types::ModelListResponse> {
        self.request(methods::MODEL_LIST, &params).await
    }

    pub async fn thread_list(
        &self,
        params: types::ThreadListParams,
    ) -> OrcasResult<types::ThreadListResponse> {
        let response: types::ThreadListResponse =
            self.request(methods::THREAD_LIST, &params).await?;
        let mut threads = self.threads.lock().await;
        for thread in &response.data {
            threads.insert(thread.id.clone(), thread.clone());
        }
        Ok(response)
    }

    pub async fn thread_read(
        &self,
        params: types::ThreadReadParams,
    ) -> OrcasResult<types::ThreadReadResponse> {
        let response: types::ThreadReadResponse =
            self.request(methods::THREAD_READ, &params).await?;
        self.threads
            .lock()
            .await
            .insert(response.thread.id.clone(), response.thread.clone());
        Ok(response)
    }

    pub async fn thread_start(
        &self,
        params: types::ThreadStartParams,
    ) -> OrcasResult<types::ThreadStartResponse> {
        let response: types::ThreadStartResponse =
            self.request(methods::THREAD_START, &params).await?;
        self.threads
            .lock()
            .await
            .insert(response.thread.id.clone(), response.thread.clone());
        Ok(response)
    }

    pub async fn thread_resume(
        &self,
        params: types::ThreadResumeParams,
    ) -> OrcasResult<types::ThreadResumeResponse> {
        let response: types::ThreadResumeResponse =
            self.request(methods::THREAD_RESUME, &params).await?;
        self.threads
            .lock()
            .await
            .insert(response.thread.id.clone(), response.thread.clone());
        Ok(response)
    }

    pub async fn turn_start(
        &self,
        params: types::TurnStartParams,
    ) -> OrcasResult<types::TurnStartResponse> {
        self.request(methods::TURN_START, &params).await
    }

    pub async fn turn_interrupt(
        &self,
        params: types::TurnInterruptParams,
    ) -> OrcasResult<types::TurnInterruptResponse> {
        self.request(methods::TURN_INTERRUPT, &params).await
    }

    pub async fn snapshot_threads(&self) -> Vec<types::Thread> {
        self.threads.lock().await.values().cloned().collect()
    }

    pub async fn snapshot_thread(&self, thread_id: &str) -> Option<types::Thread> {
        self.threads.lock().await.get(thread_id).cloned()
    }

    async fn request<T>(&self, method: &str, params: &impl Serialize) -> OrcasResult<T>
    where
        T: DeserializeOwned,
    {
        if method != methods::INITIALIZE {
            self.ensure_initialized().await?;
        }
        self.request_without_initialize(method, params).await
    }

    async fn request_without_initialize<T>(
        &self,
        method: &str,
        params: &impl Serialize,
    ) -> OrcasResult<T>
    where
        T: DeserializeOwned,
    {
        let payload = serde_json::to_value(params)?;
        let outbound = self.active_outbound().await?;
        let request_id = RequestId::Integer(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        let request = JsonRpcRequest::new(request_id.clone(), method, Some(payload));
        let raw = serde_json::to_string(&request)?;
        if let Err(error) = outbound.send(raw).await {
            self.pending.lock().await.remove(&request_id);
            return Err(OrcasError::Transport(format!(
                "failed to send `{method}` request: {error}"
            )));
        }

        let value = rx.await.map_err(|error| {
            OrcasError::Transport(format!("response channel dropped for `{method}`: {error}"))
        })??;
        serde_json::from_value(value).map_err(Into::into)
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> OrcasResult<()> {
        let outbound = self.active_outbound().await?;
        let notification = JsonRpcNotification::new(method, params);
        let raw = serde_json::to_string(&notification)?;
        outbound.send(raw).await.map_err(|error| {
            OrcasError::Transport(format!("failed to send `{method}` notification: {error}"))
        })
    }

    async fn ensure_initialized(&self) -> OrcasResult<()> {
        let _guard = self.initialize_gate.lock().await;
        if *self.initialized.lock().await {
            return Ok(());
        }
        let params = self.initialize_params.lock().await.clone().ok_or_else(|| {
            OrcasError::Protocol("Codex client has not been initialized".to_string())
        })?;
        let _: types::InitializeResponse = self
            .request_without_initialize(methods::INITIALIZE, &params)
            .await?;
        self.notify(methods::INITIALIZED, None).await?;
        *self.initialized.lock().await = true;
        Ok(())
    }

    async fn active_outbound(&self) -> OrcasResult<mpsc::Sender<String>> {
        if let Some(outbound) = self.outbound.lock().await.clone() {
            return Ok(outbound);
        }
        Err(OrcasError::Transport(format!(
            "Codex transport is not connected to {}",
            self.transport.endpoint()
        )))
    }

    async fn ensure_connection_task(self: &Arc<Self>) {
        let mut guard = self.connection_task.lock().await;
        if guard.is_some() {
            return;
        }
        let client = Arc::clone(self);
        *guard = Some(tokio::spawn(async move {
            client.connection_loop().await;
        }));
    }

    async fn wait_until_connected(&self, timeout: Duration) -> OrcasResult<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.outbound.lock().await.is_some() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(OrcasError::Transport(format!(
                    "timed out waiting to connect to {}",
                    self.transport.endpoint()
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn connection_loop(self: Arc<Self>) {
        let mut attempt = 0_u32;
        loop {
            self.emit(EventEnvelope::new(
                self.transport.endpoint(),
                OrcasEvent::ConnectionStateChanged(ConnectionState {
                    endpoint: self.transport.endpoint().to_string(),
                    status: "connecting".to_string(),
                    detail: None,
                }),
            ));
            match self.transport.connect().await {
                Ok(connection) => {
                    attempt = 0;
                    *self.outbound.lock().await = Some(connection.outbound.clone());
                    *self.initialized.lock().await = false;
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::ConnectionStateChanged(ConnectionState {
                            endpoint: self.transport.endpoint().to_string(),
                            status: "connected".to_string(),
                            detail: None,
                        }),
                    ));
                    self.read_loop(connection.outbound, connection.inbound)
                        .await;
                    *self.outbound.lock().await = None;
                    self.fail_pending("transport disconnected").await;
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::ConnectionStateChanged(ConnectionState {
                            endpoint: self.transport.endpoint().to_string(),
                            status: "disconnected".to_string(),
                            detail: None,
                        }),
                    ));
                }
                Err(error) => {
                    warn!(endpoint = %self.transport.endpoint(), %error, "transport connect failed");
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::ConnectionStateChanged(ConnectionState {
                            endpoint: self.transport.endpoint().to_string(),
                            status: "connect_failed".to_string(),
                            detail: Some(error.to_string()),
                        }),
                    ));
                }
            }

            if !self.reconnect.should_retry(attempt) {
                break;
            }
            let delay = self.reconnect.delay_for_attempt(attempt);
            attempt = attempt.saturating_add(1);
            tokio::time::sleep(delay).await;
        }
    }

    async fn read_loop(&self, outbound: mpsc::Sender<String>, mut inbound: mpsc::Receiver<String>) {
        while let Some(raw) = inbound.recv().await {
            let message: JsonRpcMessage = match serde_json::from_str(&raw) {
                Ok(message) => message,
                Err(error) => {
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::Warning {
                            message: format!("failed to decode JSON-RPC message: {error}"),
                        },
                    ));
                    continue;
                }
            };

            match message {
                JsonRpcMessage::Response(response) => {
                    self.resolve_response(response).await;
                }
                JsonRpcMessage::Error(error) => {
                    self.resolve_error(error).await;
                }
                JsonRpcMessage::Notification(notification) => {
                    self.handle_notification(notification).await;
                }
                JsonRpcMessage::Request(request) => {
                    self.handle_server_request(outbound.clone(), request).await;
                }
            }
        }
    }

    async fn resolve_response(&self, response: JsonRpcResponse) {
        if let Some(pending) = self.pending.lock().await.remove(&response.id) {
            let _ = pending.send(Ok(response.result));
        }
    }

    async fn resolve_error(&self, error: JsonRpcError) {
        if let Some(pending) = self.pending.lock().await.remove(&error.id) {
            let _ = pending.send(Err(OrcasError::Protocol(format!(
                "json-rpc error {}: {}",
                error.error.code, error.error.message
            ))));
        }
    }

    async fn handle_notification(&self, notification: JsonRpcNotification) {
        match notification.method.as_str() {
            methods::THREAD_STARTED => {
                if let Some(params) = notification.params
                    && let Ok(event) =
                        serde_json::from_value::<types::ThreadStartedNotification>(params)
                {
                    self.threads
                        .lock()
                        .await
                        .insert(event.thread.id.clone(), event.thread.clone());
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::ThreadStarted {
                            thread_id: event.thread.id,
                            preview: event.thread.preview,
                        },
                    ));
                }
            }
            methods::THREAD_STATUS_CHANGED => {
                if let Some(params) = notification.params
                    && let Ok(event) =
                        serde_json::from_value::<types::ThreadStatusChangedNotification>(params)
                {
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::ThreadStatusChanged {
                            thread_id: event.thread_id,
                            status: event.status.label().to_string(),
                        },
                    ));
                }
            }
            methods::TURN_STARTED => {
                if let Some(params) = notification.params
                    && let Ok(event) =
                        serde_json::from_value::<types::TurnStartedNotification>(params)
                {
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::TurnStarted {
                            thread_id: event.thread_id,
                            turn_id: event.turn.id,
                        },
                    ));
                }
            }
            methods::TURN_COMPLETED => {
                if let Some(params) = notification.params
                    && let Ok(event) =
                        serde_json::from_value::<types::TurnCompletedNotification>(params)
                {
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::TurnCompleted {
                            thread_id: event.thread_id,
                            turn_id: event.turn.id,
                            status: event.turn.status.label().to_string(),
                        },
                    ));
                }
            }
            methods::ITEM_STARTED => {
                if let Some(params) = notification.params
                    && let Ok(event) =
                        serde_json::from_value::<types::ItemStartedNotification>(params)
                {
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::ItemStarted {
                            thread_id: event.thread_id,
                            turn_id: event.turn_id,
                            item_id: event.item.id,
                            item_type: event.item.item_type,
                        },
                    ));
                }
            }
            methods::ITEM_COMPLETED => {
                if let Some(params) = notification.params
                    && let Ok(event) =
                        serde_json::from_value::<types::ItemCompletedNotification>(params)
                {
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::ItemCompleted {
                            thread_id: event.thread_id,
                            turn_id: event.turn_id,
                            item_id: event.item.id,
                            item_type: event.item.item_type,
                        },
                    ));
                }
            }
            methods::AGENT_MESSAGE_DELTA => {
                if let Some(params) = notification.params
                    && let Ok(event) =
                        serde_json::from_value::<types::AgentMessageDeltaNotification>(params)
                {
                    self.emit(EventEnvelope::new(
                        self.transport.endpoint(),
                        OrcasEvent::AgentMessageDelta {
                            thread_id: event.thread_id,
                            turn_id: event.turn_id,
                            item_id: event.item_id,
                            delta: event.delta,
                        },
                    ));
                }
            }
            other => {
                debug!(method = %other, "ignoring unsupported notification");
            }
        }
    }

    async fn handle_server_request(&self, outbound: mpsc::Sender<String>, request: JsonRpcRequest) {
        self.emit(EventEnvelope::new(
            self.transport.endpoint(),
            OrcasEvent::ServerRequest {
                method: request.method.clone(),
            },
        ));

        let decision = match self
            .approval_router
            .resolve(&request.method, request.params.clone())
            .await
        {
            Ok(decision) => decision,
            Err(error) => error.into(),
        };

        let message = match decision {
            ApprovalDecision::Result(result) => JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result,
            }),
            ApprovalDecision::Error {
                code,
                message,
                data,
            } => JsonRpcMessage::Error(JsonRpcError {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                error: JsonRpcErrorObject {
                    code,
                    message,
                    data,
                },
            }),
        };

        match serde_json::to_string(&message) {
            Ok(raw) => {
                if let Err(error) = outbound.send(raw).await {
                    warn!(%error, "failed to send server request resolution");
                }
            }
            Err(error) => {
                warn!(%error, "failed to serialize server request resolution");
            }
        }
    }

    async fn fail_pending(&self, message: &str) {
        let mut pending = self.pending.lock().await;
        for (_, waiter) in pending.drain() {
            let _ = waiter.send(Err(OrcasError::Transport(message.to_string())));
        }
    }

    fn emit(&self, event: EventEnvelope) {
        let _ = self.event_tx.send(event);
    }
}
