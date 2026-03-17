use crate::app::{AppState, BannerLevel, CollaborationFocus, DaemonConnectionPhase};
use orcas_core::ipc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelViewModel {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionStatusViewModel {
    pub socket_path: String,
    pub daemon_phase: DaemonConnectionPhase,
    pub upstream_status: String,
    pub upstream_detail: Option<String>,
    pub client_count: usize,
    pub known_threads: usize,
    pub reconnect_attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLogViewModel {
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusBannerViewModel {
    pub level: BannerLevel,
    pub message: String,
}

pub fn collaboration_focus_label(focus: CollaborationFocus) -> &'static str {
    match focus {
        CollaborationFocus::Workstreams => "workstreams",
        CollaborationFocus::WorkUnits => "work_units",
    }
}

pub fn connection_status(state: &AppState) -> ConnectionStatusViewModel {
    let daemon = state.daemon.as_ref();
    ConnectionStatusViewModel {
        socket_path: daemon
            .map(|status| status.socket_path.clone())
            .unwrap_or_else(|| "unavailable".to_string()),
        daemon_phase: state.daemon_phase,
        upstream_status: daemon
            .map(|status| status.upstream.status.clone())
            .unwrap_or_else(|| "disconnected".to_string()),
        upstream_detail: daemon.and_then(|status| status.upstream.detail.clone()),
        client_count: daemon.map_or(0, |status| status.client_count),
        known_threads: daemon.map_or(state.threads.len(), |status| status.known_threads),
        reconnect_attempt: state.reconnect_attempt,
    }
}

pub fn event_log(state: &AppState) -> EventLogViewModel {
    EventLogViewModel {
        lines: state
            .recent_events
            .iter()
            .map(|event| match (&event.thread_id, &event.turn_id) {
                (Some(thread_id), Some(turn_id)) => {
                    format!("[{}] {thread_id}/{turn_id} {}", event.kind, event.message)
                }
                (Some(thread_id), None) => {
                    format!("[{}] {thread_id} {}", event.kind, event.message)
                }
                _ => format!("[{}] {}", event.kind, event.message),
            })
            .collect(),
    }
}

pub fn status_banner(state: &AppState) -> Option<StatusBannerViewModel> {
    state.banner.as_ref().map(|banner| StatusBannerViewModel {
        level: banner.level,
        message: banner.message.clone(),
    })
}

pub(crate) fn daemon_phase_label(phase: DaemonConnectionPhase) -> &'static str {
    match phase {
        DaemonConnectionPhase::Connected => "connected",
        DaemonConnectionPhase::Reconnecting => "reconnecting",
        DaemonConnectionPhase::Disconnected => "disconnected",
    }
}

pub(crate) fn lifecycle_label(lifecycle: &ipc::TurnLifecycleState) -> &'static str {
    match lifecycle {
        ipc::TurnLifecycleState::Active => "active",
        ipc::TurnLifecycleState::Completed => "completed",
        ipc::TurnLifecycleState::Failed => "failed",
        ipc::TurnLifecycleState::Interrupted => "interrupted",
        ipc::TurnLifecycleState::Lost => "lost",
        ipc::TurnLifecycleState::Unknown => "unknown",
    }
}

pub(crate) fn compact_line(text: &str) -> String {
    text.replace('\n', " ")
}

pub(crate) fn abbreviate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{truncated}…")
}

pub(crate) fn short_id(id: &str) -> String {
    if id.len() <= 18 {
        id.to_string()
    } else {
        format!("{}…", &id[..18])
    }
}

pub(crate) fn timestamp_label(timestamp: chrono::DateTime<chrono::Utc>) -> String {
    timestamp.format("%H:%M:%S").to_string()
}
