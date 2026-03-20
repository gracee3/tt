use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio::time::timeout;
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
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

    pub async fn turn_steer(
        &self,
        params: types::TurnSteerParams,
    ) -> OrcasResult<types::TurnSteerResponse> {
        self.request(methods::TURN_STEER, &params).await
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

        let response = timeout(Self::REQUEST_TIMEOUT, rx).await;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                return Err(OrcasError::Transport(format!(
                    "timed out waiting for `{method}` response"
                )));
            }
        };
        let response = response.map_err(|error| {
            OrcasError::Transport(format!("response channel dropped for `{method}`: {error}"))
        })??;
        serde_json::from_value(response).map_err(Into::into)
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::mpsc;

    use orcas_core::{OrcasError, ReconnectPolicy};

    use crate::approval::{ApprovalDecision, ApprovalRouter};
    use crate::protocol::jsonrpc::{
        JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    };
    use crate::transport::{CodexTransport, TransportConnection};

    use super::*;

    struct ScriptedTransport {
        endpoint: String,
        connections: StdMutex<VecDeque<OrcasResult<TransportConnection>>>,
    }

    impl ScriptedTransport {
        fn new(connections: Vec<OrcasResult<TransportConnection>>) -> Self {
            Self {
                endpoint: "test://codex".to_string(),
                connections: StdMutex::new(connections.into()),
            }
        }
    }

    #[async_trait]
    impl CodexTransport for ScriptedTransport {
        async fn connect(&self) -> OrcasResult<TransportConnection> {
            self.connections
                .lock()
                .expect("scripted transport lock poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(OrcasError::Transport(
                        "no scripted Codex connections remain".to_string(),
                    ))
                })
        }

        fn endpoint(&self) -> &str {
            &self.endpoint
        }
    }

    struct ScriptedServer {
        inbound_tx: Option<mpsc::Sender<String>>,
        outbound_rx: mpsc::Receiver<String>,
    }

    impl ScriptedServer {
        async fn recv_message(&mut self) -> JsonRpcMessage {
            let raw = self
                .outbound_rx
                .recv()
                .await
                .expect("expected client outbound payload");
            serde_json::from_str(&raw).expect("client outbound should be valid JSON-RPC")
        }

        async fn send_message(&self, message: JsonRpcMessage) {
            let raw = serde_json::to_string(&message).expect("serialize scripted server message");
            self.send_raw(&raw).await;
        }

        async fn send_raw(&self, raw: &str) {
            self.inbound_tx
                .as_ref()
                .expect("scripted server connection already closed")
                .send(raw.to_string())
                .await
                .expect("send scripted inbound payload");
        }

        fn close(&mut self) {
            self.inbound_tx = None;
        }
    }

    fn scripted_connection() -> (TransportConnection, ScriptedServer) {
        let (outbound_tx, outbound_rx) = mpsc::channel::<String>(32);
        let (inbound_tx, inbound_rx) = mpsc::channel::<String>(32);
        (
            TransportConnection {
                outbound: outbound_tx,
                inbound: inbound_rx,
            },
            ScriptedServer {
                inbound_tx: Some(inbound_tx),
                outbound_rx,
            },
        )
    }

    #[derive(Clone)]
    struct StaticApprovalRouter {
        decision: ApprovalDecision,
    }

    #[async_trait]
    impl ApprovalRouter for StaticApprovalRouter {
        async fn resolve(
            &self,
            _method: &str,
            _params: Option<Value>,
        ) -> OrcasResult<ApprovalDecision> {
            Ok(self.decision.clone())
        }
    }

    fn reconnect_policy(max_attempts: Option<u32>) -> ReconnectPolicy {
        ReconnectPolicy {
            initial_delay_ms: 0,
            max_delay_ms: 0,
            multiplier: 1.0,
            max_attempts,
        }
    }

    fn initialize_params() -> types::InitializeParams {
        types::InitializeParams {
            client_info: types::ClientInfo {
                name: "orcas-tests".to_string(),
                title: Some("Tests".to_string()),
                version: "0.1.0".to_string(),
            },
            capabilities: Some(types::InitializeCapabilities {
                experimental_api: true,
                opt_out_notification_methods: None,
            }),
        }
    }

    fn sample_thread(id: &str, preview: &str) -> types::Thread {
        types::Thread {
            id: id.to_string(),
            preview: preview.to_string(),
            ephemeral: false,
            model_provider: "openai".to_string(),
            created_at: 1,
            updated_at: 2,
            status: types::ThreadStatus::Idle,
            path: None,
            cwd: "/tmp".to_string(),
            cli_version: "0.1.0".to_string(),
            source: None,
            name: Some(format!("Thread {id}")),
            turns: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }

    async fn recv_request(server: &mut ScriptedServer) -> JsonRpcRequest {
        match server.recv_message().await {
            JsonRpcMessage::Request(request) => request,
            other => panic!("expected request, got {other:?}"),
        }
    }

    async fn recv_notification(server: &mut ScriptedServer) -> JsonRpcNotification {
        match server.recv_message().await {
            JsonRpcMessage::Notification(notification) => notification,
            other => panic!("expected notification, got {other:?}"),
        }
    }

    async fn recv_connection_event(events: &mut EventSubscription) -> String {
        loop {
            let event = events.recv().await.expect("connection event");
            if let OrcasEvent::ConnectionStateChanged(state) = event.event {
                return state.status;
            }
        }
    }

    async fn complete_initialize(server: &mut ScriptedServer) -> RequestId {
        let initialize = recv_request(server).await;
        assert_eq!(initialize.method, methods::INITIALIZE);
        let request_id = initialize.id.clone();
        server
            .send_message(JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request_id.clone(),
                result: serde_json::to_value(types::InitializeResponse {
                    server_info: Some(types::ServerInfo {
                        name: Some("codex".to_string()),
                        version: Some("1.0.0".to_string()),
                    }),
                    user_agent: Some("codex-tests".to_string()),
                    platform_family: None,
                    platform_os: None,
                })
                .expect("serialize initialize response"),
            }))
            .await;
        let initialized = recv_notification(server).await;
        assert_eq!(initialized.method, methods::INITIALIZED);
        request_id
    }

    #[tokio::test]
    async fn initialize_handshake_only_happens_once_after_explicit_initialize() {
        let (connection, mut server) = scripted_connection();
        let client = CodexClient::new(
            Arc::new(ScriptedTransport::new(vec![Ok(connection)])),
            reconnect_policy(Some(0)),
            Arc::new(StaticApprovalRouter {
                decision: ApprovalDecision::Result(json!({"approved": true})),
            }),
        );

        client.connect().await.expect("connect client");
        let response_task = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.initialize(initialize_params()).await })
        };
        let initialize_id = complete_initialize(&mut server).await;
        let response = response_task
            .await
            .expect("join initialize task")
            .expect("initialize response");
        assert_eq!(
            response.server_info.and_then(|info| info.name),
            Some("codex".to_string())
        );
        assert_eq!(initialize_id, RequestId::Integer(1));

        let model_list_task = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.model_list(types::ModelListParams::default()).await })
        };
        let model_request = recv_request(&mut server).await;
        assert_eq!(model_request.method, methods::MODEL_LIST);
        server
            .send_message(JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: model_request.id,
                result: serde_json::to_value(types::ModelListResponse {
                    data: vec![types::Model {
                        id: "gpt-5.4".to_string(),
                        model: "gpt-5.4".to_string(),
                        display_name: "GPT-5.4".to_string(),
                        description: "test".to_string(),
                        hidden: false,
                        is_default: true,
                    }],
                    next_cursor: None,
                })
                .expect("serialize model list response"),
            }))
            .await;
        let models = model_list_task
            .await
            .expect("join model list task")
            .expect("model list response");
        assert_eq!(models.data.len(), 1);
    }

    #[tokio::test]
    async fn concurrent_requests_resolve_by_request_id() {
        let (connection, mut server) = scripted_connection();
        let client = CodexClient::new(
            Arc::new(ScriptedTransport::new(vec![Ok(connection)])),
            reconnect_policy(Some(0)),
            Arc::new(StaticApprovalRouter {
                decision: ApprovalDecision::Result(json!({"approved": true})),
            }),
        );

        client.connect().await.expect("connect client");
        let initialize_task = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.initialize(initialize_params()).await })
        };
        complete_initialize(&mut server).await;
        initialize_task
            .await
            .expect("join initialize task")
            .expect("initialize succeeds");

        let model_list = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.model_list(types::ModelListParams::default()).await })
        };
        let thread_list = {
            let client = Arc::clone(&client);
            tokio::spawn(
                async move { client.thread_list(types::ThreadListParams::default()).await },
            )
        };

        let first = recv_request(&mut server).await;
        let second = recv_request(&mut server).await;
        assert_ne!(first.id, second.id);

        for request in [&second, &first] {
            let response = match request.method.as_str() {
                methods::MODEL_LIST => JsonRpcMessage::Response(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: serde_json::to_value(types::ModelListResponse {
                        data: vec![types::Model {
                            id: "gpt-5.4-mini".to_string(),
                            model: "gpt-5.4-mini".to_string(),
                            display_name: "GPT-5.4 Mini".to_string(),
                            description: "test".to_string(),
                            hidden: false,
                            is_default: false,
                        }],
                        next_cursor: None,
                    })
                    .expect("serialize model response"),
                }),
                methods::THREAD_LIST => JsonRpcMessage::Response(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id.clone(),
                    result: serde_json::to_value(types::ThreadListResponse {
                        data: vec![sample_thread("thread-1", "hello")],
                        next_cursor: None,
                    })
                    .expect("serialize thread response"),
                }),
                other => panic!("unexpected request method {other}"),
            };
            server.send_message(response).await;
        }

        let models = model_list
            .await
            .expect("join model list")
            .expect("model list succeeds");
        let threads = thread_list
            .await
            .expect("join thread list")
            .expect("thread list succeeds");

        assert_eq!(models.data[0].id, "gpt-5.4-mini");
        assert_eq!(threads.data[0].id, "thread-1");
    }

    #[tokio::test(start_paused = true)]
    async fn request_timeout_cleans_up_pending_and_allows_future_requests() {
        let (connection, mut server) = scripted_connection();
        let client = CodexClient::new(
            Arc::new(ScriptedTransport::new(vec![Ok(connection)])),
            reconnect_policy(Some(0)),
            Arc::new(StaticApprovalRouter {
                decision: ApprovalDecision::Result(json!({"approved": true})),
            }),
        );

        client.connect().await.expect("connect client");
        let initialize_task = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.initialize(initialize_params()).await })
        };
        complete_initialize(&mut server).await;
        initialize_task
            .await
            .expect("join initialize task")
            .expect("initialize succeeds");

        let timed_out_request = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.model_list(types::ModelListParams::default()).await })
        };
        let model_request = recv_request(&mut server).await;
        assert_eq!(model_request.method, methods::MODEL_LIST);

        tokio::time::advance(CodexClient::REQUEST_TIMEOUT + Duration::from_secs(1)).await;
        let error = timed_out_request
            .await
            .expect("join timed out request")
            .expect_err("request should time out");
        assert!(
            matches!(error, OrcasError::Transport(message) if message.contains("timed out waiting for `model/list` response"))
        );

        let thread_list = {
            let client = Arc::clone(&client);
            tokio::spawn(
                async move { client.thread_list(types::ThreadListParams::default()).await },
            )
        };
        let thread_request = recv_request(&mut server).await;
        assert_eq!(thread_request.method, methods::THREAD_LIST);
        server
            .send_message(JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: thread_request.id,
                result: serde_json::to_value(types::ThreadListResponse {
                    data: vec![sample_thread("thread-after-timeout", "recovered")],
                    next_cursor: None,
                })
                .expect("serialize thread list response"),
            }))
            .await;
        let response = thread_list
            .await
            .expect("join recovery request")
            .expect("recovery request succeeds");
        assert_eq!(response.data[0].id, "thread-after-timeout");
    }

    #[tokio::test]
    async fn reconnect_reinitializes_before_followup_requests() {
        let (connection_one, mut server_one) = scripted_connection();
        let (connection_two, mut server_two) = scripted_connection();
        let client = CodexClient::new(
            Arc::new(ScriptedTransport::new(vec![
                Ok(connection_one),
                Ok(connection_two),
            ])),
            reconnect_policy(Some(1)),
            Arc::new(StaticApprovalRouter {
                decision: ApprovalDecision::Result(json!({"approved": true})),
            }),
        );

        let mut events = client.subscribe();
        client.connect().await.expect("connect client");
        assert_eq!(recv_connection_event(&mut events).await, "connecting");
        assert_eq!(recv_connection_event(&mut events).await, "connected");

        let initialize_task = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.initialize(initialize_params()).await })
        };
        complete_initialize(&mut server_one).await;
        initialize_task
            .await
            .expect("join initialize task")
            .expect("initialize succeeds");

        server_one.close();
        assert_eq!(recv_connection_event(&mut events).await, "disconnected");
        assert_eq!(recv_connection_event(&mut events).await, "connecting");
        assert_eq!(recv_connection_event(&mut events).await, "connected");

        let model_list = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.model_list(types::ModelListParams::default()).await })
        };
        complete_initialize(&mut server_two).await;
        let request = recv_request(&mut server_two).await;
        assert_eq!(request.method, methods::MODEL_LIST);
        server_two
            .send_message(JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: serde_json::to_value(types::ModelListResponse {
                    data: vec![types::Model {
                        id: "gpt-reconnected".to_string(),
                        model: "gpt-reconnected".to_string(),
                        display_name: "GPT Reconnected".to_string(),
                        description: "test".to_string(),
                        hidden: false,
                        is_default: false,
                    }],
                    next_cursor: None,
                })
                .expect("serialize model list response"),
            }))
            .await;

        let response = model_list
            .await
            .expect("join reconnect request")
            .expect("request succeeds after reconnect");
        assert_eq!(response.data[0].id, "gpt-reconnected");
    }

    #[tokio::test]
    async fn notifications_and_server_requests_are_mapped_to_events_and_responses() {
        let (connection, mut server) = scripted_connection();
        let client = CodexClient::new(
            Arc::new(ScriptedTransport::new(vec![Ok(connection)])),
            reconnect_policy(Some(0)),
            Arc::new(StaticApprovalRouter {
                decision: ApprovalDecision::Result(json!({"approved": true})),
            }),
        );

        let mut events = client.subscribe();
        client.connect().await.expect("connect client");
        assert_eq!(recv_connection_event(&mut events).await, "connecting");
        assert_eq!(recv_connection_event(&mut events).await, "connected");

        server.send_raw("not-json").await;
        match events.recv().await.expect("warning event").event {
            OrcasEvent::Warning { message } => {
                assert!(message.contains("failed to decode JSON-RPC message"));
            }
            other => panic!("expected warning event, got {other:?}"),
        }

        let thread = sample_thread("thread-42", "preview");
        server
            .send_message(JsonRpcMessage::Notification(JsonRpcNotification::new(
                methods::THREAD_STARTED,
                Some(
                    serde_json::to_value(types::ThreadStartedNotification {
                        thread: thread.clone(),
                    })
                    .expect("serialize thread started notification"),
                ),
            )))
            .await;
        match events.recv().await.expect("thread started event").event {
            OrcasEvent::ThreadStarted { thread_id, preview } => {
                assert_eq!(thread_id, "thread-42");
                assert_eq!(preview, "preview");
            }
            other => panic!("expected thread started event, got {other:?}"),
        }
        assert_eq!(
            client
                .snapshot_thread("thread-42")
                .await
                .expect("thread snapshot cached")
                .preview,
            "preview"
        );

        server
            .send_message(JsonRpcMessage::Request(JsonRpcRequest::new(
                RequestId::Integer(77),
                "approval/request",
                Some(json!({ "path": "src/lib.rs" })),
            )))
            .await;
        match events.recv().await.expect("server request event").event {
            OrcasEvent::ServerRequest { method } => {
                assert_eq!(method, "approval/request");
            }
            other => panic!("expected server request event, got {other:?}"),
        }
        match server.recv_message().await {
            JsonRpcMessage::Response(JsonRpcResponse { id, result, .. }) => {
                assert_eq!(id, RequestId::Integer(77));
                assert_eq!(result, json!({"approved": true}));
            }
            other => panic!("expected approval response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn disconnect_fails_pending_request() {
        let (connection, mut server) = scripted_connection();
        let client = CodexClient::new(
            Arc::new(ScriptedTransport::new(vec![Ok(connection)])),
            reconnect_policy(Some(0)),
            Arc::new(StaticApprovalRouter {
                decision: ApprovalDecision::Result(json!({"approved": true})),
            }),
        );

        client.connect().await.expect("connect client");
        let initialize_task = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.initialize(initialize_params()).await })
        };
        complete_initialize(&mut server).await;
        initialize_task
            .await
            .expect("join initialize task")
            .expect("initialize succeeds");

        let pending = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.model_list(types::ModelListParams::default()).await })
        };
        let request = recv_request(&mut server).await;
        assert_eq!(request.method, methods::MODEL_LIST);
        server.close();

        let error = pending
            .await
            .expect("join pending request")
            .expect_err("disconnect should fail pending request");
        assert!(
            matches!(error, OrcasError::Transport(message) if message.contains("transport disconnected"))
        );
    }
}
