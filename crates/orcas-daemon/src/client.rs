use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};

use orcas_core::ipc;
use orcas_core::jsonrpc::{
    JsonRpcError, JsonRpcErrorObject, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, RequestId,
};
use orcas_core::{AppPaths, OrcasError, OrcasResult};

type PendingResponse = oneshot::Sender<OrcasResult<Value>>;
pub type EventSubscription = broadcast::Receiver<ipc::DaemonEventEnvelope>;

pub struct OrcasIpcClient {
    pending: Mutex<HashMap<RequestId, PendingResponse>>,
    outbound: mpsc::Sender<String>,
    event_tx: broadcast::Sender<ipc::DaemonEventEnvelope>,
    next_request_id: AtomicI64,
}

impl OrcasIpcClient {
    pub async fn connect(paths: &AppPaths) -> OrcasResult<Arc<Self>> {
        let stream = UnixStream::connect(&paths.socket_file)
            .await
            .map_err(|error| {
                OrcasError::Transport(format!(
                    "failed to connect to Orcas daemon at {}: {error}",
                    paths.socket_file.display()
                ))
            })?;
        Self::from_stream(stream)
    }

    pub fn subscribe(&self) -> EventSubscription {
        self.event_tx.subscribe()
    }

    pub async fn daemon_status(&self) -> OrcasResult<ipc::DaemonStatusResponse> {
        self.request(ipc::methods::DAEMON_STATUS, &ipc::Empty::default())
            .await
    }

    pub async fn daemon_connect(&self) -> OrcasResult<ipc::DaemonConnectResponse> {
        self.request(
            ipc::methods::DAEMON_CONNECT,
            &ipc::DaemonConnectRequest::default(),
        )
        .await
    }

    pub async fn state_get(&self) -> OrcasResult<ipc::StateGetResponse> {
        self.request(ipc::methods::STATE_GET, &ipc::StateGetRequest::default())
            .await
    }

    pub async fn session_get_active(&self) -> OrcasResult<ipc::SessionGetActiveResponse> {
        self.request(
            ipc::methods::SESSION_GET_ACTIVE,
            &ipc::SessionGetActiveRequest::default(),
        )
        .await
    }

    pub async fn models_list(&self) -> OrcasResult<ipc::ModelsListResponse> {
        self.request(ipc::methods::MODELS_LIST, &ipc::Empty::default())
            .await
    }

    pub async fn threads_list(&self) -> OrcasResult<ipc::ThreadsListResponse> {
        self.request(
            ipc::methods::THREADS_LIST,
            &ipc::ThreadsListRequest::default(),
        )
        .await
    }

    pub async fn threads_list_scoped(&self) -> OrcasResult<ipc::ThreadsListResponse> {
        self.request(
            ipc::methods::THREADS_LIST_SCOPED,
            &ipc::ThreadsListScopedRequest::default(),
        )
        .await
    }

    pub async fn thread_start(
        &self,
        params: &ipc::ThreadStartRequest,
    ) -> OrcasResult<ipc::ThreadStartResponse> {
        self.request(ipc::methods::THREAD_START, params).await
    }

    pub async fn thread_read(
        &self,
        params: &ipc::ThreadReadRequest,
    ) -> OrcasResult<ipc::ThreadReadResponse> {
        self.request(ipc::methods::THREAD_READ, params).await
    }

    pub async fn thread_get(
        &self,
        params: &ipc::ThreadGetRequest,
    ) -> OrcasResult<ipc::ThreadGetResponse> {
        self.request(ipc::methods::THREAD_GET, params).await
    }

    pub async fn thread_resume(
        &self,
        params: &ipc::ThreadResumeRequest,
    ) -> OrcasResult<ipc::ThreadResumeResponse> {
        self.request(ipc::methods::THREAD_RESUME, params).await
    }

    pub async fn turns_recent(
        &self,
        params: &ipc::TurnsRecentRequest,
    ) -> OrcasResult<ipc::TurnsRecentResponse> {
        self.request(ipc::methods::TURNS_RECENT, params).await
    }

    pub async fn turn_start(
        &self,
        params: &ipc::TurnStartRequest,
    ) -> OrcasResult<ipc::TurnStartResponse> {
        self.request(ipc::methods::TURN_START, params).await
    }

    pub async fn turn_interrupt(&self, params: &ipc::TurnInterruptRequest) -> OrcasResult<()> {
        let _: ipc::Empty = self.request(ipc::methods::TURN_INTERRUPT, params).await?;
        Ok(())
    }

    pub async fn subscribe_events(
        &self,
        include_snapshot: bool,
    ) -> OrcasResult<(EventSubscription, Option<ipc::StateSnapshot>)> {
        let events = self.subscribe();
        let response: ipc::EventsSubscribeResponse = self
            .request(
                ipc::methods::EVENTS_SUBSCRIBE,
                &ipc::EventsSubscribeRequest { include_snapshot },
            )
            .await?;
        Ok((events, response.snapshot))
    }

    fn from_stream(stream: UnixStream) -> OrcasResult<Arc<Self>> {
        let (read_half, mut write_half) = stream.into_split();
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<String>(256);
        let (event_tx, _) = broadcast::channel(512);
        let client = Arc::new(Self {
            pending: Mutex::new(HashMap::new()),
            outbound: outbound_tx,
            event_tx,
            next_request_id: AtomicI64::new(1),
        });

        tokio::spawn(async move {
            while let Some(raw) = outbound_rx.recv().await {
                if write_half.write_all(raw.as_bytes()).await.is_err() {
                    break;
                }
                if write_half.write_all(b"\n").await.is_err() {
                    break;
                }
            }
        });

        let client_read = Arc::clone(&client);
        tokio::spawn(async move {
            let mut lines = BufReader::new(read_half).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if let Err(error) = client_read.handle_line(&line).await {
                            client_read.fail_pending(error.to_string().as_str()).await;
                            break;
                        }
                    }
                    Ok(None) => {
                        client_read
                            .fail_pending("Orcas daemon connection closed")
                            .await;
                        break;
                    }
                    Err(error) => {
                        client_read
                            .fail_pending(format!("Orcas daemon read failed: {error}").as_str())
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(client)
    }

    async fn handle_line(&self, raw: &str) -> OrcasResult<()> {
        let message: JsonRpcMessage = serde_json::from_str(raw)?;
        match message {
            JsonRpcMessage::Response(response) => self.resolve_response(response).await,
            JsonRpcMessage::Error(error) => self.resolve_error(error).await,
            JsonRpcMessage::Notification(notification) => {
                self.handle_notification(notification).await
            }
            JsonRpcMessage::Request(request) => {
                let error = JsonRpcMessage::Error(JsonRpcError {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    error: JsonRpcErrorObject {
                        code: -32601,
                        message: "Orcas IPC client does not serve requests".to_string(),
                        data: None,
                    },
                });
                let raw = serde_json::to_string(&error)?;
                self.outbound.send(raw).await.map_err(|send_error| {
                    OrcasError::Transport(format!("failed to reject daemon request: {send_error}"))
                })?;
            }
        }
        Ok(())
    }

    async fn handle_notification(&self, notification: JsonRpcNotification) {
        if notification.method == ipc::methods::EVENTS_NOTIFICATION
            && let Some(params) = notification.params
            && let Ok(event) = serde_json::from_value::<ipc::EventsNotification>(params)
        {
            let _ = self.event_tx.send(event.event);
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

    async fn fail_pending(&self, message: &str) {
        let mut pending = self.pending.lock().await;
        for (_, waiter) in pending.drain() {
            let _ = waiter.send(Err(OrcasError::Transport(message.to_string())));
        }
    }

    async fn request<T>(&self, method: &str, params: &impl Serialize) -> OrcasResult<T>
    where
        T: DeserializeOwned,
    {
        let payload = serde_json::to_value(params)?;
        let request_id = RequestId::Integer(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        let request = JsonRpcRequest::new(request_id.clone(), method, Some(payload));
        let raw = serde_json::to_string(&request)?;
        if let Err(error) = self.outbound.send(raw).await {
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
}
