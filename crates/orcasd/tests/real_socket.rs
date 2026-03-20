#![allow(unused_crate_dependencies)]

mod harness;

use orcas_core::ipc;
use tokio::sync::broadcast;
use tokio::time::{Duration, timeout};

use harness::TestDaemon;

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

    let response = client
        .workstream_create(&ipc::WorkstreamCreateRequest {
            title: "Socket event workstream".to_string(),
            objective: "Exercise event subscription".to_string(),
            priority: Some("high".to_string()),
        })
        .await
        .expect("create workstream through real ipc");

    let event = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && workstream.id == response.workstream.id
        )
    })
    .await;

    match event.event {
        ipc::DaemonEvent::WorkstreamLifecycle { action, workstream } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Created);
            assert_eq!(workstream.id, response.workstream.id);
            assert_eq!(workstream.title, "Socket event workstream");
        }
        other => panic!("expected workstream lifecycle event, got {other:?}"),
    }

    daemon.stop().await;
}

#[tokio::test]
async fn workstream_mutation_is_visible_via_state_and_events() {
    let mut daemon = TestDaemon::spawn("workstream-mutation").await;

    let client = daemon.connect().await;
    let (mut events, _) = client
        .subscribe_events(false)
        .await
        .expect("subscribe without snapshot");

    let response = client
        .workstream_create(&ipc::WorkstreamCreateRequest {
            title: "Persisted workstream".to_string(),
            objective: "Visible in state and events".to_string(),
            priority: None,
        })
        .await
        .expect("create workstream");

    let state = client.state_get().await.expect("state/get after mutation");
    let summary = state
        .snapshot
        .collaboration
        .workstreams
        .iter()
        .find(|workstream| workstream.id == response.workstream.id)
        .expect("created workstream should appear in state snapshot");
    assert_eq!(summary.title, "Persisted workstream");

    let event = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && workstream.id == response.workstream.id
        )
    })
    .await;
    match event.event {
        ipc::DaemonEvent::WorkstreamLifecycle { workstream, .. } => {
            assert_eq!(workstream.title, "Persisted workstream");
        }
        other => panic!("expected workstream lifecycle event, got {other:?}"),
    }

    daemon.stop().await;
}

#[tokio::test]
async fn restart_preserves_state_and_allows_reconnect() {
    let mut daemon = TestDaemon::spawn("restart-reconnect").await;

    let first_client = daemon.connect().await;
    let created = first_client
        .workstream_create(&ipc::WorkstreamCreateRequest {
            title: "Restarted workstream".to_string(),
            objective: "Persist across daemon restart".to_string(),
            priority: Some("normal".to_string()),
        })
        .await
        .expect("create workstream before restart")
        .workstream;

    daemon.restart().await;

    let second_client = daemon.connect().await;
    let state = second_client
        .state_get()
        .await
        .expect("state/get after restart");
    let summary = state
        .snapshot
        .collaboration
        .workstreams
        .iter()
        .find(|workstream| workstream.id == created.id)
        .expect("workstream should persist across restart");
    assert_eq!(summary.title, "Restarted workstream");

    let (mut events, _) = second_client
        .subscribe_events(false)
        .await
        .expect("re-subscribe after restart");
    let follow_up = second_client
        .workstream_create(&ipc::WorkstreamCreateRequest {
            title: "Post-restart workstream".to_string(),
            objective: "Exercise fresh client after restart".to_string(),
            priority: None,
        })
        .await
        .expect("create workstream after restart");

    let event = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && workstream.id == follow_up.workstream.id
        )
    })
    .await;
    match event.event {
        ipc::DaemonEvent::WorkstreamLifecycle { workstream, .. } => {
            assert_eq!(workstream.title, "Post-restart workstream");
        }
        other => panic!("expected post-restart workstream lifecycle event, got {other:?}"),
    }

    daemon.stop().await;
}

#[tokio::test]
async fn restart_closes_old_event_subscription_and_requires_fresh_resubscribe() {
    let mut daemon = TestDaemon::spawn("restart-event-subscription").await;

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
    let follow_up = second_client
        .workstream_create(&ipc::WorkstreamCreateRequest {
            title: "Fresh subscription".to_string(),
            objective: "Verify restart requires a new subscription".to_string(),
            priority: None,
        })
        .await
        .expect("create post-restart workstream");

    let event = TestDaemon::next_event_matching(&mut new_events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && workstream.id == follow_up.workstream.id
        )
    })
    .await;
    match event.event {
        ipc::DaemonEvent::WorkstreamLifecycle { workstream, .. } => {
            assert_eq!(workstream.title, "Fresh subscription");
        }
        other => panic!("expected workstream lifecycle event, got {other:?}"),
    }

    daemon.stop().await;
}
