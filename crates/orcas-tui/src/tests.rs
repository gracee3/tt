use chrono::Utc;

use crate::app::{BannerLevel, UserAction};
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

fn sample_snapshot() -> ipc::StateSnapshot {
    ipc::StateSnapshot {
        daemon: ipc::DaemonStatusResponse {
            socket_path: "/tmp/orcasd.sock".to_string(),
            codex_endpoint: "ws://127.0.0.1:4500".to_string(),
            codex_binary_path: "/home/emmy/git/codex/codex-rs/target/debug/codex".to_string(),
            upstream: ConnectionState {
                endpoint: "ws://127.0.0.1:4500".to_string(),
                status: "connected".to_string(),
                detail: None,
            },
            client_count: 1,
            known_threads: 2,
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
async fn thread_selection_loads_detail() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_thread(sample_thread_view("thread-2", "later", "second output"))
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
                    id: "turn-9".to_string(),
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
    assert_eq!(banner.level, BannerLevel::Error);
    assert!(banner.message.contains("snapshot failed"));
}
