use crate::app::{AppState, BannerLevel, DaemonConnectionPhase};

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
pub struct ThreadRowViewModel {
    pub id: String,
    pub status: String,
    pub turn_badge: Option<String>,
    pub preview: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadListViewModel {
    pub rows: Vec<ThreadRowViewModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLogViewModel {
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptBoxViewModel {
    pub text: String,
    pub active: bool,
    pub in_flight: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusBannerViewModel {
    pub level: BannerLevel,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDetailViewModel {
    pub title: String,
    pub lines: Vec<String>,
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

pub fn thread_list(state: &AppState) -> ThreadListViewModel {
    ThreadListViewModel {
        rows: state
            .threads
            .iter()
            .map(|thread| ThreadRowViewModel {
                id: thread.id.clone(),
                status: thread_status_label(state, thread),
                turn_badge: thread_turn_badge(state, &thread.id),
                preview: thread.preview.replace('\n', " "),
                selected: state.selected_thread_id.as_deref() == Some(thread.id.as_str()),
            })
            .collect(),
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

pub fn prompt_box(state: &AppState) -> PromptBoxViewModel {
    PromptBoxViewModel {
        text: state.prompt_input.clone(),
        active: state.prompt_mode,
        in_flight: state.prompt_in_flight,
    }
}

pub fn status_banner(state: &AppState) -> Option<StatusBannerViewModel> {
    state.banner.as_ref().map(|banner| StatusBannerViewModel {
        level: banner.level,
        message: banner.message.clone(),
    })
}

pub fn thread_detail(state: &AppState) -> ThreadDetailViewModel {
    let Some(thread_id) = state.selected_thread_id.as_ref() else {
        return ThreadDetailViewModel {
            title: "Thread".to_string(),
            lines: vec!["No thread selected.".to_string()],
        };
    };

    let Some(thread) = state.thread_details.get(thread_id) else {
        return ThreadDetailViewModel {
            title: format!("Thread {thread_id}"),
            lines: vec!["Loading thread details...".to_string()],
        };
    };

    let mut lines = Vec::new();
    lines.push(format!("status: {}", thread.summary.status));
    lines.push(format!("cwd: {}", thread.summary.cwd));
    if let Some(turn_state) = latest_turn_state_for_thread(state, thread_id) {
        lines.push(format!(
            "turn_state: {}  attachable={}  live_stream={}",
            lifecycle_label(&turn_state.lifecycle),
            turn_state.attachable,
            turn_state.live_stream
        ));
        if let Some(event) = turn_state.recent_event.as_ref() {
            lines.push(format!("turn_event: {event}"));
        }
        if let Some(output) = turn_state.recent_output.as_ref() {
            lines.push(format!("turn_output: {output}"));
        }
    }
    lines.push(format!(
        "preview: {}",
        thread.summary.preview.replace('\n', " ")
    ));
    lines.push(String::new());

    if thread.turns.is_empty() {
        lines.push("No turns loaded.".to_string());
    } else {
        for turn in &thread.turns {
            lines.push(format!("turn {} [{}]", turn.id, turn.status));
            for item in &turn.items {
                let status = item.status.clone().unwrap_or_else(|| "unknown".to_string());
                let text = item.text.clone().unwrap_or_default().replace('\n', "\\n");
                lines.push(format!("  {} {} {}", item.item_type, status, text));
            }
        }
    }

    ThreadDetailViewModel {
        title: format!("Thread {}", thread.summary.id),
        lines,
    }
}

fn thread_status_label(state: &AppState, thread: &orcas_core::ipc::ThreadSummary) -> String {
    latest_turn_state_for_thread(state, &thread.id)
        .map(|turn| lifecycle_label(&turn.lifecycle).to_string())
        .unwrap_or_else(|| thread.status.clone())
}

fn thread_turn_badge(state: &AppState, thread_id: &str) -> Option<String> {
    latest_turn_state_for_thread(state, thread_id).map(|turn| {
        if turn.attachable && turn.live_stream {
            format!("{} attachable", lifecycle_label(&turn.lifecycle))
        } else {
            lifecycle_label(&turn.lifecycle).to_string()
        }
    })
}

fn latest_turn_state_for_thread<'a>(
    state: &'a AppState,
    thread_id: &str,
) -> Option<&'a orcas_core::ipc::TurnStateView> {
    state
        .turn_states
        .values()
        .filter(|turn| turn.thread_id == thread_id)
        .max_by(|left, right| left.updated_at.cmp(&right.updated_at))
}

fn lifecycle_label(lifecycle: &orcas_core::ipc::TurnLifecycleState) -> &'static str {
    match lifecycle {
        orcas_core::ipc::TurnLifecycleState::Active => "active",
        orcas_core::ipc::TurnLifecycleState::Completed => "completed",
        orcas_core::ipc::TurnLifecycleState::Failed => "failed",
        orcas_core::ipc::TurnLifecycleState::Interrupted => "interrupted",
        orcas_core::ipc::TurnLifecycleState::Lost => "lost",
        orcas_core::ipc::TurnLifecycleState::Unknown => "unknown",
    }
}
