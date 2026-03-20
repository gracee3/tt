use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde_json::{Map, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};

use orcas_codex::protocol::{
    jsonrpc::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RequestId},
    methods, types,
};

pub struct FakeCodexAppServer {
    pub endpoint: String,
    task: JoinHandle<()>,
}

impl FakeCodexAppServer {
    pub async fn spawn() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind fake Codex app server");
        let endpoint = format!(
            "ws://127.0.0.1:{}",
            listener
                .local_addr()
                .expect("fake Codex listener address")
                .port()
        );
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.expect("accept fake Codex client");
                match accept_async(stream).await {
                    Ok(socket) => {
                        Self::serve(socket).await;
                    }
                    Err(_) => continue,
                }
            }
        });
        Self { endpoint, task }
    }

    async fn serve(mut socket: WebSocketStream<TcpStream>) {
        let mut thread: Option<types::Thread> = None;

        while let Some(frame) = socket.next().await {
            let Ok(Message::Text(raw)) = frame else {
                continue;
            };
            let message: JsonRpcMessage =
                serde_json::from_str(&raw).expect("decode fake Codex JSON-RPC");
            match message {
                JsonRpcMessage::Notification(JsonRpcNotification { .. }) => {}
                JsonRpcMessage::Request(request) => {
                    Self::handle_request(&mut socket, &mut thread, request).await;
                }
                JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_) => {}
            }
        }
    }

    async fn handle_request(
        socket: &mut WebSocketStream<TcpStream>,
        thread: &mut Option<types::Thread>,
        request: JsonRpcRequest,
    ) {
        match request.method.as_str() {
            methods::INITIALIZE => {
                Self::send_response(
                    socket,
                    request.id,
                    &types::InitializeResponse {
                        server_info: Some(types::ServerInfo {
                            name: Some("fake-codex".to_string()),
                            version: Some("0.1.0".to_string()),
                        }),
                        user_agent: Some("fake-codex-tests".to_string()),
                        platform_family: None,
                        platform_os: None,
                    },
                )
                .await;
            }
            methods::THREAD_LIST => {
                Self::send_response(
                    socket,
                    request.id,
                    &types::ThreadListResponse {
                        data: thread.iter().cloned().collect(),
                        next_cursor: None,
                    },
                )
                .await;
            }
            methods::THREAD_START => {
                let now = Utc::now().timestamp();
                let created = types::Thread {
                    id: "thread-authority-assignment".to_string(),
                    preview: "Projected authority assignment".to_string(),
                    ephemeral: false,
                    model_provider: "openai".to_string(),
                    created_at: now,
                    updated_at: now,
                    status: types::ThreadStatus::Idle,
                    path: None,
                    cwd: "/tmp".to_string(),
                    cli_version: "0.1.0".to_string(),
                    source: None,
                    name: Some("Projected authority assignment".to_string()),
                    turns: Vec::new(),
                    extra: Map::new(),
                };
                *thread = Some(created.clone());
                Self::send_response(
                    socket,
                    request.id,
                    &types::ThreadStartResponse {
                        thread: created,
                        model: "gpt-5.4".to_string(),
                        model_provider: "openai".to_string(),
                        cwd: "/tmp".to_string(),
                    },
                )
                .await;
            }
            methods::THREAD_READ => {
                let params: types::ThreadReadParams =
                    serde_json::from_value(request.params.unwrap_or(Value::Null))
                        .expect("thread/read params");
                let current = thread
                    .clone()
                    .expect("fake Codex thread should exist before thread/read");
                assert_eq!(current.id, params.thread_id);
                Self::send_response(
                    socket,
                    request.id,
                    &types::ThreadReadResponse { thread: current },
                )
                .await;
            }
            methods::TURN_START => {
                let params: types::TurnStartParams =
                    serde_json::from_value(request.params.unwrap_or(Value::Null))
                        .expect("turn/start params");
                let active_turn = types::Turn {
                    id: "turn-authority-assignment".to_string(),
                    items: Vec::new(),
                    status: types::TurnStatus::InProgress,
                    error: None,
                };
                let updated_thread = {
                    let thread = thread
                        .as_mut()
                        .expect("fake Codex thread should exist before turn/start");
                    assert_eq!(thread.id, params.thread_id);
                    thread.status = types::ThreadStatus::Active {
                        active_flags: vec!["turn_running".to_string()],
                    };
                    thread.updated_at = Utc::now().timestamp();
                    thread.turns.push(active_turn.clone());
                    thread.clone()
                };
                Self::send_response(
                    socket,
                    request.id,
                    &types::TurnStartResponse {
                        turn: active_turn.clone(),
                    },
                )
                .await;
                Self::send_notification(
                    socket,
                    methods::TURN_STARTED,
                    &types::TurnStartedNotification {
                        thread_id: updated_thread.id.clone(),
                        turn: active_turn,
                    },
                )
                .await;

                let completed_turn = types::Turn {
                    id: "turn-authority-assignment".to_string(),
                    items: vec![types::ThreadItem {
                        id: "item-authority-assignment".to_string(),
                        item_type: "agent_message".to_string(),
                        extra: Map::from_iter([(
                            "text".to_string(),
                            Value::String(
                                "bounded authority-backed assignment run completed".to_string(),
                            ),
                        )]),
                    }],
                    status: types::TurnStatus::Completed,
                    error: None,
                };
                if let Some(thread) = thread.as_mut() {
                    if let Some(existing_turn) = thread
                        .turns
                        .iter_mut()
                        .find(|existing| existing.id == completed_turn.id)
                    {
                        *existing_turn = completed_turn.clone();
                    }
                    thread.status = types::ThreadStatus::Idle;
                    thread.updated_at = Utc::now().timestamp();
                }
                Self::send_notification(
                    socket,
                    methods::TURN_COMPLETED,
                    &types::TurnCompletedNotification {
                        thread_id: params.thread_id,
                        turn: completed_turn,
                    },
                )
                .await;
            }
            other => panic!("unexpected fake Codex request method {other}"),
        }
    }

    async fn send_response(
        socket: &mut WebSocketStream<TcpStream>,
        id: RequestId,
        result: &impl serde::Serialize,
    ) {
        let message = JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: serde_json::to_value(result).expect("serialize fake Codex response"),
        });
        Self::send_message(socket, message).await;
    }

    async fn send_notification(
        socket: &mut WebSocketStream<TcpStream>,
        method: &str,
        params: &impl serde::Serialize,
    ) {
        let message = JsonRpcMessage::Notification(JsonRpcNotification::new(
            method,
            Some(serde_json::to_value(params).expect("serialize fake Codex notification")),
        ));
        Self::send_message(socket, message).await;
    }

    async fn send_message(socket: &mut WebSocketStream<TcpStream>, message: JsonRpcMessage) {
        socket
            .send(Message::Text(
                serde_json::to_string(&message)
                    .expect("serialize fake Codex message")
                    .into(),
            ))
            .await
            .expect("send fake Codex websocket message");
    }
}

impl Drop for FakeCodexAppServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
