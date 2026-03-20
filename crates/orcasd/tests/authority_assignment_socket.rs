#![allow(unused_crate_dependencies)]

mod harness;

use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde_json::{Map, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant, sleep};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};

use harness::TestDaemon;
use orcas_codex::protocol::{
    jsonrpc::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RequestId},
    methods, types,
};
use orcas_core::authority::{self, CommandActor, CommandMetadata, CorrelationId, OriginNodeId};
use orcas_core::{AssignmentStatus, WorkUnitStatus, WorkstreamStatus, ipc};

struct AuthorityFixture {
    origin_node_id: OriginNodeId,
    actor: CommandActor,
}

impl AuthorityFixture {
    fn new() -> Self {
        Self {
            origin_node_id: OriginNodeId::new(),
            actor: CommandActor::parse("integration_test").expect("command actor"),
        }
    }

    fn metadata(&self, label: &str) -> CommandMetadata {
        CommandMetadata {
            command_id: authority::CommandId::new(),
            issued_at: Utc::now(),
            origin_node_id: self.origin_node_id.clone(),
            actor: self.actor.clone(),
            correlation_id: Some(
                CorrelationId::parse(format!("corr-{label}")).expect("correlation id"),
            ),
        }
    }
}

struct FakeCodexAppServer {
    endpoint: String,
    task: JoinHandle<()>,
}

impl FakeCodexAppServer {
    async fn spawn() -> Self {
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
                        break;
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

async fn create_authority_workstream(
    client: &orcasd::OrcasIpcClient,
    fixture: &AuthorityFixture,
    workstream_id: &str,
    title: &str,
) -> authority::WorkstreamRecord {
    client
        .authority_workstream_create(&ipc::AuthorityWorkstreamCreateRequest {
            command: authority::CreateWorkstream {
                metadata: fixture.metadata("assignment-ws-create"),
                workstream_id: authority::WorkstreamId::parse(workstream_id)
                    .expect("workstream id"),
                title: title.to_string(),
                objective: format!("Objective for {title}"),
                status: WorkstreamStatus::Active,
                priority: "high".to_string(),
            },
        })
        .await
        .expect("create authority workstream")
        .workstream
}

async fn create_authority_workunit(
    client: &orcasd::OrcasIpcClient,
    fixture: &AuthorityFixture,
    work_unit_id: &str,
    workstream_id: &authority::WorkstreamId,
    title: &str,
) -> authority::WorkUnitRecord {
    client
        .authority_workunit_create(&ipc::AuthorityWorkunitCreateRequest {
            command: authority::CreateWorkUnit {
                metadata: fixture.metadata("assignment-wu-create"),
                work_unit_id: authority::WorkUnitId::parse(work_unit_id).expect("work unit id"),
                workstream_id: workstream_id.clone(),
                title: title.to_string(),
                task_statement: format!("Task for {title}"),
                status: WorkUnitStatus::Ready,
            },
        })
        .await
        .expect("create authority work unit")
        .work_unit
}

#[tokio::test]
async fn assignment_start_consumes_projected_authority_work_unit_and_updates_state() {
    let fake_codex = FakeCodexAppServer::spawn().await;
    let mut daemon = TestDaemon::spawn_with_env(
        "authority-assignment",
        vec![(
            "ORCAS_CODEX_LISTEN_URL".to_string(),
            fake_codex.endpoint.clone(),
        )],
    )
    .await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();

    let workstream = create_authority_workstream(
        &client,
        &fixture,
        "authority-assignment-ws",
        "Assignment Root",
    )
    .await;
    let work_unit = create_authority_workunit(
        &client,
        &fixture,
        "authority-assignment-wu",
        &workstream.id,
        "Assignment Unit",
    )
    .await;

    let projected_before = client
        .state_get()
        .await
        .expect("state/get before assignment start");
    assert!(
        projected_before
            .snapshot
            .collaboration
            .work_units
            .iter()
            .any(|summary| {
                summary.id == work_unit.id.as_str() && summary.status == WorkUnitStatus::Ready
            })
    );

    let started = client
        .assignment_start(&ipc::AssignmentStartRequest {
            work_unit_id: work_unit.id.to_string(),
            worker_id: "worker-1".to_string(),
            worker_kind: Some("codex".to_string()),
            instructions: Some("Handle the projected authority task".to_string()),
            model: None,
            cwd: None,
        })
        .await
        .expect("assignment start over projected authority work unit");
    assert_eq!(started.assignment.work_unit_id, work_unit.id.as_str());
    assert_eq!(
        started.assignment.status,
        AssignmentStatus::AwaitingDecision
    );
    assert_eq!(started.worker.id, "worker-1");
    assert_eq!(started.report.work_unit_id, work_unit.id.as_str());
    assert_eq!(started.report.assignment_id, started.assignment.id);

    let follow_up_client = daemon.connect().await;
    let assignment_id = started.assignment.id.clone();

    let deadline = Instant::now() + Duration::from_secs(10);
    let settled_snapshot = loop {
        let snapshot = follow_up_client
            .state_get()
            .await
            .expect("state/get while waiting for bounded assignment transition");
        let assignment_summary = snapshot
            .snapshot
            .collaboration
            .assignments
            .iter()
            .find(|summary| summary.id == assignment_id)
            .expect("assignment should remain visible");
        let work_unit_summary = snapshot
            .snapshot
            .collaboration
            .work_units
            .iter()
            .find(|summary| summary.id == work_unit.id.as_str())
            .expect("projected work unit should remain visible");
        if assignment_summary.status == AssignmentStatus::AwaitingDecision
            && work_unit_summary.status == WorkUnitStatus::AwaitingDecision
        {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for assignment to settle into awaiting decision"
        );
        sleep(Duration::from_millis(100)).await;
    };

    let assignment = follow_up_client
        .assignment_get(&ipc::AssignmentGetRequest {
            assignment_id: assignment_id.clone(),
        })
        .await
        .expect("assignment should be queryable after start");
    assert_eq!(assignment.assignment.id, assignment_id);
    assert_eq!(assignment.assignment.work_unit_id, work_unit.id.as_str());
    assert_eq!(
        assignment.assignment.status,
        AssignmentStatus::AwaitingDecision
    );
    let report = assignment
        .report
        .expect("assignment should have produced a persisted report");
    assert_eq!(report.id, started.report.id);
    assert_eq!(report.work_unit_id, work_unit.id.as_str());
    assert_eq!(report.assignment_id, started.assignment.id);

    let settled_assignment = settled_snapshot
        .snapshot
        .collaboration
        .assignments
        .iter()
        .find(|summary| summary.id == assignment_id)
        .expect("assignment should remain visible");
    assert_eq!(
        settled_assignment.status,
        AssignmentStatus::AwaitingDecision
    );
    let projected_work_unit = settled_snapshot
        .snapshot
        .collaboration
        .work_units
        .iter()
        .find(|summary| summary.id == work_unit.id.as_str())
        .expect("projected work unit should remain visible");
    assert_eq!(projected_work_unit.status, WorkUnitStatus::AwaitingDecision);
    assert_eq!(
        projected_work_unit.current_assignment_id.as_deref(),
        Some(assignment_id.as_str())
    );
    assert!(
        settled_snapshot
            .snapshot
            .recent_events
            .iter()
            .any(|event| { event.kind == "assignment" && event.message.contains(&assignment_id) })
    );
    assert!(settled_snapshot.snapshot.recent_events.iter().any(|event| {
        event.kind == "work_unit" && event.message.contains(work_unit.id.as_str())
    }));
    assert!(
        settled_snapshot
            .snapshot
            .recent_events
            .iter()
            .any(|event| { event.kind == "report" && event.message.contains(&started.report.id) })
    );

    daemon.stop().await;
}
