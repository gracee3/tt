use crate::app::{AppState, BannerLevel};

use super::shared::{
    PanelViewModel, connection_status, daemon_phase_label, event_log, lifecycle_label,
    status_banner, timestamp_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewViewModel {
    pub connection: PanelViewModel,
    pub active_work: PanelViewModel,
    pub warnings: PanelViewModel,
    pub recent_events: PanelViewModel,
}

pub fn overview_view(state: &AppState) -> OverviewViewModel {
    let connection = connection_status(state);
    let mut connection_lines = vec![
        format!(
            "daemon: {}  upstream: {}",
            daemon_phase_label(connection.daemon_phase),
            connection.upstream_status
        ),
        format!(
            "clients: {}  known_threads: {}  reconnect_attempt: {}",
            connection.client_count, connection.known_threads, connection.reconnect_attempt
        ),
        format!("socket: {}", connection.socket_path),
    ];
    if let Some(detail) = connection.upstream_detail {
        connection_lines.push(format!("detail: {detail}"));
    }

    let active_turn_lines = active_turn_lines(state);
    let mut warning_lines = warning_lines(state);
    if warning_lines.is_empty() {
        warning_lines.push("No recent warnings.".to_string());
    }

    let mut event_lines = event_log(state)
        .lines
        .into_iter()
        .rev()
        .take(6)
        .collect::<Vec<_>>();
    event_lines.reverse();
    if event_lines.is_empty() {
        event_lines.push("No recent events.".to_string());
    }

    OverviewViewModel {
        connection: PanelViewModel {
            title: "Connection".to_string(),
            lines: connection_lines,
        },
        active_work: PanelViewModel {
            title: "Active Work".to_string(),
            lines: active_turn_lines,
        },
        warnings: PanelViewModel {
            title: "Recent Warnings".to_string(),
            lines: warning_lines,
        },
        recent_events: PanelViewModel {
            title: "Recent Events".to_string(),
            lines: event_lines,
        },
    }
}

fn active_turn_lines(state: &AppState) -> Vec<String> {
    let mut lines = Vec::new();
    let mut active_turns = state
        .session
        .active_turns
        .iter()
        .map(|turn| {
            format!(
                "{} / {} [{}] @ {}",
                turn.thread_id,
                turn.turn_id,
                turn.status,
                timestamp_label(turn.updated_at)
            )
        })
        .collect::<Vec<_>>();

    if active_turns.is_empty() {
        let mut derived = state
            .turn_states
            .values()
            .filter(|turn| matches!(turn.lifecycle, orcas_core::ipc::TurnLifecycleState::Active))
            .collect::<Vec<_>>();
        derived.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        active_turns = derived
            .into_iter()
            .map(|turn| {
                format!(
                    "{} / {} [{}] attachable={} @ {}",
                    turn.thread_id,
                    turn.turn_id,
                    lifecycle_label(&turn.lifecycle),
                    turn.attachable,
                    timestamp_label(turn.updated_at)
                )
            })
            .collect();
    }

    lines.push(format!("active turns: {}", active_turns.len()));
    if let Some(thread_id) = state.session.active_thread_id.as_deref() {
        lines.push(format!("session thread: {thread_id}"));
    }
    if active_turns.is_empty() {
        lines.push("No active turns reported.".to_string());
        if let Some(thread_id) = state.selected_thread_id.as_deref() {
            lines.push(format!("selected thread: {thread_id}"));
        }
    } else {
        lines.extend(active_turns.into_iter().take(4));
    }
    if state.prompt_in_flight && lines.iter().all(|line| !line.contains('/')) {
        lines.push("live activity is still settling after the last refresh.".to_string());
    }
    lines
}

fn warning_lines(state: &AppState) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(banner) = status_banner(state)
        && matches!(banner.level, BannerLevel::Warning | BannerLevel::Error)
    {
        lines.push(format!("banner: {}", banner.message));
    }
    for event in state
        .recent_events
        .iter()
        .rev()
        .filter(|event| {
            matches!(
                event.kind.as_str(),
                "warning" | "error" | "disconnect" | "reconnect"
            )
        })
        .take(4)
    {
        lines.push(format!("[{}] {}", event.kind, event.message));
    }
    lines
}
