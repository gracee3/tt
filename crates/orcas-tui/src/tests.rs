use chrono::Utc;

use crate::app::{BannerLevel, DaemonConnectionPhase, UserAction};
use crate::backend::BackendCommand;
use crate::test_harness::AppHarness;
use orcas_core::{ConnectionState, ipc};

fn sample_thread_summary(id: &str, preview: &str, updated_at: i64) -> ipc::ThreadSummary {
    ipc::ThreadSummary {
        id: id.to_string(),
        preview: preview.to_string(),
        name: None,
        model_provider: "openai".to_string(),
        cwd: "/tmp/orcas".to_string(),
        status: "idle".to_string(),
        created_at: updated_at - 10,
        updated_at,
        scope: "orcas_managed".to_string(),
        recent_output: Some(preview.to_string()),
        recent_event: Some("thread idle".to_string()),
        turn_in_flight: false,
    }
}

fn sample_thread_view(id: &str, preview: &str, output: &str) -> ipc::ThreadView {
    ipc::ThreadView {
        summary: sample_thread_summary(id, preview, 200),
        turns: vec![ipc::TurnView {
            id: "turn-1".to_string(),
            status: "completed".to_string(),
            error_message: None,
            items: vec![ipc::ItemView {
                id: "item-1".to_string(),
                item_type: "agent_message".to_string(),
                status: Some("completed".to_string()),
                text: Some(output.to_string()),
            }],
        }],
    }
}

fn sample_turn_state(
    thread_id: &str,
    turn_id: &str,
    lifecycle: ipc::TurnLifecycleState,
    status: &str,
    attachable: bool,
) -> ipc::TurnStateView {
    ipc::TurnStateView {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
        lifecycle,
        status: status.to_string(),
        attachable,
        live_stream: attachable,
        terminal: !matches!(lifecycle, ipc::TurnLifecycleState::Active),
        recent_output: Some("turn output".to_string()),
        recent_event: Some(format!("turn {status}")),
        updated_at: Utc::now(),
        error_message: None,
    }
}

fn sample_snapshot() -> ipc::StateSnapshot {
    ipc::StateSnapshot {
        daemon: ipc::DaemonStatusResponse {
            socket_path: "/tmp/orcasd.sock".to_string(),
            metadata_path: "/tmp/orcasd.json".to_string(),
            codex_endpoint: "ws://127.0.0.1:4500".to_string(),
            codex_binary_path: "/home/emmy/git/codex/codex-rs/target/debug/codex".to_string(),
            upstream: ConnectionState {
                endpoint: "ws://127.0.0.1:4500".to_string(),
                status: "connected".to_string(),
                detail: None,
            },
            client_count: 1,
            known_threads: 2,
            runtime: ipc::DaemonRuntimeMetadata {
                pid: 4242,
                started_at: Utc::now(),
                version: "0.1.0".to_string(),
                build_fingerprint: "abc123".to_string(),
                binary_path: "/tmp/orcasd".to_string(),
                socket_path: "/tmp/orcasd.sock".to_string(),
                metadata_path: "/tmp/orcasd.json".to_string(),
                git_commit: None,
            },
        },
        session: ipc::SessionState {
            active_thread_id: Some("thread-1".to_string()),
            active_turns: Vec::new(),
        },
        threads: vec![
            sample_thread_summary("thread-1", "hello", 200),
            sample_thread_summary("thread-2", "later", 150),
        ],
        active_thread: Some(sample_thread_view("thread-1", "hello", "world")),
        recent_events: vec![ipc::EventSummary {
            timestamp: Utc::now(),
            kind: "thread".to_string(),
            message: "loaded thread-1".to_string(),
            thread_id: Some("thread-1".to_string()),
            turn_id: None,
        }],
    }
}

#[tokio::test]
async fn initial_snapshot_load_populates_state() {
    let harness = AppHarness::new(sample_snapshot()).await.unwrap();
    let connection = harness.connection_vm();
    let threads = harness.thread_list_vm();

    assert_eq!(connection.daemon_phase, DaemonConnectionPhase::Connected);
    assert_eq!(connection.upstream_status, "connected");
    assert_eq!(threads.rows.len(), 2);
    assert!(threads.rows[0].selected);
}

#[tokio::test]
async fn event_stream_updates_connection_state() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::UpstreamStatusChanged {
                upstream: ConnectionState {
                    endpoint: "ws://127.0.0.1:4500".to_string(),
                    status: "connect_failed".to_string(),
                    detail: Some("boom".to_string()),
                },
            },
        ))
        .await
        .unwrap();

    let connection = harness.connection_vm();
    assert_eq!(connection.upstream_status, "connect_failed");
    assert_eq!(connection.upstream_detail.as_deref(), Some("boom"));
}

#[tokio::test]
async fn active_turn_state_drives_prompt_in_flight_and_thread_badge() {
    let mut snapshot = sample_snapshot();
    snapshot.session.active_turns = vec![ipc::ActiveTurn {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-7".to_string(),
        status: "in_progress".to_string(),
        updated_at: Utc::now(),
    }];

    let harness = AppHarness::new(snapshot).await.unwrap();
    let prompt = harness.prompt_box_vm();
    let threads = harness.thread_list_vm();

    assert!(prompt.in_flight);
    assert_eq!(threads.rows[0].status, "active");
    assert_eq!(
        threads.rows[0].turn_badge.as_deref(),
        Some("active attachable")
    );
}

#[tokio::test]
async fn thread_selection_loads_detail() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_thread(sample_thread_view("thread-2", "later", "second output"))
        .await;
    harness
        .set_turn(ipc::TurnAttachResponse {
            turn: Some(sample_turn_state(
                "thread-2",
                "turn-1",
                ipc::TurnLifecycleState::Completed,
                "completed",
                false,
            )),
            attached: false,
            reason: Some("turn already completed; only terminal state is queryable".to_string()),
        })
        .await;
    harness.dispatch(UserAction::SelectNextThread).await;

    let threads = harness.thread_list_vm();
    let detail = harness.thread_detail_vm();
    assert!(threads.rows[1].selected);
    assert!(detail.title.contains("thread-2"));
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("turn_state: completed"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("second output"))
    );
}

#[tokio::test]
async fn streamed_deltas_accumulate_in_selected_thread() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::TurnUpdated {
                thread_id: "thread-1".to_string(),
                turn: ipc::TurnView {
                    id: "turn-2".to_string(),
                    status: "in_progress".to_string(),
                    error_message: None,
                    items: Vec::new(),
                },
            },
        ))
        .await
        .unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::OutputDelta {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-2".to_string(),
                item_id: "item-2".to_string(),
                delta: "hello ".to_string(),
            },
        ))
        .await
        .unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::OutputDelta {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-2".to_string(),
                item_id: "item-2".to_string(),
                delta: "world".to_string(),
            },
        ))
        .await
        .unwrap();

    let detail = harness.thread_detail_vm();
    assert!(detail.lines.iter().any(|line| line.contains("hello world")));
}

#[tokio::test]
async fn completed_turn_clears_in_progress_marker() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness.dispatch(UserAction::EnterPromptMode).await;
    for ch in "status".chars() {
        harness.dispatch(UserAction::PromptAppend(ch)).await;
    }
    harness.dispatch(UserAction::SubmitPrompt).await;
    assert!(harness.prompt_box_vm().in_flight);

    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::TurnUpdated {
                thread_id: "thread-1".to_string(),
                turn: ipc::TurnView {
                    id: "turn-1".to_string(),
                    status: "completed".to_string(),
                    error_message: None,
                    items: Vec::new(),
                },
            },
        ))
        .await
        .unwrap();

    assert!(!harness.prompt_box_vm().in_flight);
}

#[tokio::test]
async fn prompt_submission_emits_backend_command() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness.dispatch(UserAction::EnterPromptMode).await;
    for ch in "ship it".chars() {
        harness.dispatch(UserAction::PromptAppend(ch)).await;
    }
    harness.dispatch(UserAction::SubmitPrompt).await;

    let commands = harness.recorded_commands().await;
    assert!(commands.contains(&BackendCommand::SubmitPrompt {
        thread_id: "thread-1".to_string(),
        text: "ship it".to_string(),
    }));
}

#[tokio::test]
async fn backend_failure_surfaces_in_banner_state() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness.fail_snapshot_once("cannot load snapshot").await;
    harness.dispatch(UserAction::Refresh).await;

    let banner = harness.state().banner.clone().unwrap();
    assert_eq!(banner.level, BannerLevel::Warning);
    assert!(banner.message.contains("Reconnecting"));
    assert_eq!(
        harness.state().daemon_phase,
        DaemonConnectionPhase::Reconnecting
    );
}

#[tokio::test]
async fn reconnect_recovers_with_snapshot_then_resubscribe() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    let mut recovered = sample_snapshot();
    recovered.threads = vec![sample_thread_summary("thread-2", "recovered", 300)];
    recovered.session.active_thread_id = Some("thread-2".to_string());
    recovered.active_thread = Some(sample_thread_view("thread-2", "recovered", "after restart"));
    harness.replace_snapshot(recovered).await;

    harness.disconnect_events().await;
    harness.process().await;

    assert_eq!(
        harness.state().daemon_phase,
        DaemonConnectionPhase::Reconnecting
    );
    assert_eq!(harness.snapshot_requests().await, 1);
    assert_eq!(harness.subscribe_requests().await, 1);

    harness.force_reconnect_now();
    harness.process().await;

    let connection = harness.connection_vm();
    let detail = harness.thread_detail_vm();
    assert_eq!(connection.daemon_phase, DaemonConnectionPhase::Connected);
    assert_eq!(harness.snapshot_requests().await, 2);
    assert_eq!(harness.subscribe_requests().await, 2);
    assert_eq!(harness.thread_list_vm().rows.len(), 1);
    assert!(detail.title.contains("thread-2"));
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("after restart"))
    );
}
