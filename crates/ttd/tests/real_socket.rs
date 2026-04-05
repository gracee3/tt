#![allow(warnings)]

mod harness;

use chrono::Utc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::broadcast;
use tokio::time::{Duration, timeout};
use tt_core::{JsonRpcMessage, JsonRpcRequest, RequestId, WorkstreamStatus, authority, ipc};

use harness::TestDaemon;

struct AuthorityFixture {
    origin_node_id: authority::OriginNodeId,
    actor: authority::CommandActor,
}

impl AuthorityFixture {
    fn new() -> Self {
        Self {
            origin_node_id: authority::OriginNodeId::new(),
            actor: authority::CommandActor::parse("real_socket_test").expect("command actor"),
        }
    }

    fn metadata(&self, label: &str) -> authority::CommandMetadata {
        authority::CommandMetadata {
            command_id: authority::CommandId::new(),
            issued_at: Utc::now(),
            origin_node_id: self.origin_node_id.clone(),
            actor: self.actor.clone(),
            correlation_id: Some(
                authority::CorrelationId::parse(format!("real-socket-{label}"))
                    .expect("correlation id"),
            ),
        }
    }
}

async fn create_authority_workstream(
    daemon: &TestDaemon,
    fixture: &AuthorityFixture,
    workstream_id: &str,
    title: &str,
) -> authority::WorkstreamRecord {
    daemon
        .connect()
        .await
        .authority_workstream_create(&ipc::AuthorityWorkstreamCreateRequest {
            command: authority::CreateWorkstream {
                metadata: fixture.metadata("ws-create"),
                workstream_id: authority::WorkstreamId::parse(workstream_id)
                    .expect("workstream id"),
                title: title.to_string(),
                objective: format!("Objective for {title}"),
                status: WorkstreamStatus::Active,
                priority: "high".to_string(),
                execution_scope: None,
            },
        })
        .await
        .expect("create authority workstream")
        .workstream
}

async fn call_raw_method(daemon: &TestDaemon, method: &str) -> JsonRpcMessage {
    let mut stream = UnixStream::connect(&daemon.paths.socket_file)
        .await
        .expect("connect raw socket");
    let request = JsonRpcRequest::new(RequestId::Integer(41), method, Some(serde_json::json!({})));
    let raw = serde_json::to_string(&request).expect("serialize request");
    stream
        .write_all(format!("{raw}\n").as_bytes())
        .await
        .expect("write raw request");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .expect("read raw response");
    serde_json::from_str(&line).expect("decode jsonrpc response")
}

#[tokio::test]
async fn state_get_over_real_socket_returns_expected_basics() {
    let mut daemon = TestDaemon::spawn("state-get").await;

    let client = daemon.connect().await;
    let state = client
        .state_get()
        .await
        .expect("state/get over real socket");

    assert_eq!(
        state.snapshot.daemon.socket_path,
        daemon.paths.socket_file.display().to_string()
    );
    assert_eq!(
        state.snapshot.daemon.metadata_path,
        daemon.paths.daemon_metadata_file.display().to_string()
    );
    assert!(state.snapshot.daemon.runtime.pid > 0);
    assert!(state.snapshot.daemon.client_count >= 1);
    assert!(state.snapshot.daemon.known_threads >= state.snapshot.threads.len());
    assert!(!state.snapshot.daemon.upstream.status.is_empty());
    assert!(state.snapshot.collaboration.workstreams.is_empty());

    daemon.stop().await;
}

#[tokio::test]
async fn subscribe_events_with_snapshot_over_real_socket() {
    let mut daemon = TestDaemon::spawn("subscribe-events").await;
    let fixture = AuthorityFixture::new();

    let client = daemon.connect().await;
    let (mut events, snapshot) = client
        .subscribe_events(true)
        .await
        .expect("subscribe to daemon events");
    let snapshot = snapshot.expect("include_snapshot should return snapshot");
    assert_eq!(
        snapshot.daemon.socket_path,
        daemon.paths.socket_file.display().to_string()
    );
    assert!(snapshot.collaboration.workstreams.is_empty());

    let response = create_authority_workstream(
        &daemon,
        &fixture,
        &authority::WorkstreamId::new().to_string(),
        "Socket event workstream",
    )
    .await;

    let event = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && workstream.id == response.id.as_str()
        )
    })
    .await;

    match event.event {
        ipc::DaemonEvent::WorkstreamLifecycle { action, workstream } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Created);
            assert_eq!(workstream.id, response.id.as_str());
            assert_eq!(workstream.title, "Socket event workstream");
            assert_eq!(
                workstream.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected workstream lifecycle event, got {other:?}"),
    }

    daemon.stop().await;
}

#[tokio::test]
async fn authority_workstream_mutation_is_visible_via_hierarchy_and_events() {
    let mut daemon = TestDaemon::spawn("authority-workstream-mutation").await;
    let fixture = AuthorityFixture::new();

    let client = daemon.connect().await;
    let (mut events, _) = client
        .subscribe_events(false)
        .await
        .expect("subscribe without snapshot");

    let response = create_authority_workstream(
        &daemon,
        &fixture,
        &authority::WorkstreamId::new().to_string(),
        "Persisted authority workstream",
    )
    .await;

    let state = client.state_get().await.expect("state/get after mutation");
    assert!(
        state
            .snapshot
            .collaboration
            .workstreams
            .iter()
            .all(|workstream| workstream.id != response.id.as_str())
    );

    let hierarchy = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest::default())
        .await
        .expect("authority hierarchy after mutation");
    let summary = hierarchy
        .hierarchy
        .workstreams
        .iter()
        .find(|entry| entry.workstream.id == response.id)
        .expect("created authority workstream should appear in hierarchy");
    assert_eq!(summary.workstream.title, "Persisted authority workstream");

    let event = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && workstream.id == response.id.as_str()
        )
    })
    .await;
    match event.event {
        ipc::DaemonEvent::WorkstreamLifecycle { workstream, .. } => {
            assert_eq!(workstream.title, "Persisted authority workstream");
            assert_eq!(
                workstream.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected workstream lifecycle event, got {other:?}"),
    }

    daemon.stop().await;
}

#[tokio::test]
async fn restart_preserves_authority_hierarchy_and_allows_reconnect() {
    let mut daemon = TestDaemon::spawn("restart-reconnect").await;
    let fixture = AuthorityFixture::new();

    let created = create_authority_workstream(
        &daemon,
        &fixture,
        &authority::WorkstreamId::new().to_string(),
        "Restarted authority workstream",
    )
    .await;

    daemon.restart().await;

    let second_client = daemon.connect().await;
    let state = second_client
        .state_get()
        .await
        .expect("state/get after restart");
    assert!(
        state
            .snapshot
            .collaboration
            .workstreams
            .iter()
            .all(|workstream| workstream.id != created.id.as_str())
    );

    let hierarchy = second_client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest::default())
        .await
        .expect("authority hierarchy after restart");
    let summary = hierarchy
        .hierarchy
        .workstreams
        .iter()
        .find(|entry| entry.workstream.id == created.id)
        .expect("authority workstream should persist across restart");
    assert_eq!(summary.workstream.title, "Restarted authority workstream");

    let (mut events, _) = second_client
        .subscribe_events(false)
        .await
        .expect("re-subscribe after restart");
    let follow_up = create_authority_workstream(
        &daemon,
        &fixture,
        &authority::WorkstreamId::new().to_string(),
        "Post-restart authority workstream",
    )
    .await;

    let event = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && workstream.id == follow_up.id.as_str()
        )
    })
    .await;
    match event.event {
        ipc::DaemonEvent::WorkstreamLifecycle { workstream, .. } => {
            assert_eq!(workstream.title, "Post-restart authority workstream");
            assert_eq!(
                workstream.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected post-restart workstream lifecycle event, got {other:?}"),
    }

    daemon.stop().await;
}

#[tokio::test]
async fn restart_closes_old_event_subscription_and_requires_fresh_resubscribe() {
    let mut daemon = TestDaemon::spawn("restart-event-subscription").await;
    let fixture = AuthorityFixture::new();

    let first_client = daemon.connect().await;
    let (mut old_events, _) = first_client
        .subscribe_events(false)
        .await
        .expect("subscribe before restart");

    daemon.restart().await;

    let closed = timeout(Duration::from_secs(5), old_events.recv())
        .await
        .expect("old subscription should resolve after restart");
    assert!(matches!(closed, Err(broadcast::error::RecvError::Closed)));

    let second_client = daemon.connect().await;
    let (mut new_events, _) = second_client
        .subscribe_events(false)
        .await
        .expect("subscribe after restart");
    let follow_up = create_authority_workstream(
        &daemon,
        &fixture,
        &authority::WorkstreamId::new().to_string(),
        "Fresh subscription",
    )
    .await;

    let event = TestDaemon::next_event_matching(&mut new_events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && workstream.id == follow_up.id.as_str()
        )
    })
    .await;
    match event.event {
        ipc::DaemonEvent::WorkstreamLifecycle { workstream, .. } => {
            assert_eq!(workstream.title, "Fresh subscription");
            assert_eq!(
                workstream.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected workstream lifecycle event, got {other:?}"),
    }

    daemon.stop().await;
}

#[tokio::test]
async fn retired_legacy_planning_methods_stay_unavailable_over_real_socket() {
    let mut daemon = TestDaemon::spawn("retired-legacy-methods").await;

    for method in [
        "workstream/create",
        "workstream/list",
        "workstream/get",
        "workunit/create",
        "workunit/list",
    ] {
        match call_raw_method(&daemon, method).await {
            JsonRpcMessage::Error(error) => {
                assert_eq!(error.id, RequestId::Integer(41));
                assert_eq!(error.error.code, -32601, "unexpected error for {method}");
            }
            other => panic!("expected method-not-found error for {method}, got {other:?}"),
        }
    }

    daemon.stop().await;
}
