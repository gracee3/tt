use std::collections::{HashMap, VecDeque};

use orcas_core::{ConnectionState, ipc};

const MAX_LOG_ENTRIES: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub daemon: Option<ipc::DaemonStatusResponse>,
    pub session: ipc::SessionState,
    pub threads: Vec<ipc::ThreadSummary>,
    pub thread_details: HashMap<String, ipc::ThreadView>,
    pub selected_thread_id: Option<String>,
    pub recent_events: VecDeque<ipc::EventSummary>,
    pub prompt_input: String,
    pub prompt_mode: bool,
    pub prompt_in_flight: bool,
    pub banner: Option<StatusBanner>,
    pub show_help: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusBanner {
    pub level: BannerLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub enum Action {
    Start,
    User(UserAction),
    Event(UiEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserAction {
    Refresh,
    ToggleHelp,
    SelectNextThread,
    SelectPreviousThread,
    SelectThread(String),
    EnterPromptMode,
    ExitPromptMode,
    PromptAppend(char),
    PromptBackspace,
    SubmitPrompt,
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    SnapshotLoaded(ipc::StateSnapshot),
    ThreadLoaded(ipc::ThreadView),
    PromptStarted {
        thread_id: String,
        turn_id: String,
    },
    UpstreamChanged(ConnectionState),
    SessionChanged(ipc::SessionState),
    ThreadUpdated(ipc::ThreadSummary),
    TurnUpdated {
        thread_id: String,
        turn: ipc::TurnView,
    },
    ItemUpdated {
        thread_id: String,
        turn_id: String,
        item: ipc::ItemView,
    },
    OutputDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    Warning(String),
    Error(String),
}

impl UiEvent {
    pub fn from_daemon(event: ipc::DaemonEventEnvelope) -> Self {
        match event.event {
            ipc::DaemonEvent::UpstreamStatusChanged { upstream } => Self::UpstreamChanged(upstream),
            ipc::DaemonEvent::SessionChanged { session } => Self::SessionChanged(session),
            ipc::DaemonEvent::ThreadUpdated { thread } => Self::ThreadUpdated(thread),
            ipc::DaemonEvent::TurnUpdated { thread_id, turn } => {
                Self::TurnUpdated { thread_id, turn }
            }
            ipc::DaemonEvent::ItemUpdated {
                thread_id,
                turn_id,
                item,
            } => Self::ItemUpdated {
                thread_id,
                turn_id,
                item,
            },
            ipc::DaemonEvent::OutputDelta {
                thread_id,
                turn_id,
                item_id,
                delta,
            } => Self::OutputDelta {
                thread_id,
                turn_id,
                item_id,
                delta,
            },
            ipc::DaemonEvent::Warning { message } => Self::Warning(message),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Effect {
    SubscribeEvents,
    RefreshSnapshot,
    LoadThread { thread_id: String },
    SubmitPrompt { thread_id: String, text: String },
}

pub fn reduce(state: &mut AppState, action: Action) -> Vec<Effect> {
    match action {
        Action::Start => vec![Effect::SubscribeEvents, Effect::RefreshSnapshot],
        Action::User(user_action) => reduce_user_action(state, user_action),
        Action::Event(event) => reduce_event(state, event),
    }
}

fn reduce_user_action(state: &mut AppState, action: UserAction) -> Vec<Effect> {
    match action {
        UserAction::Refresh => vec![Effect::RefreshSnapshot],
        UserAction::ToggleHelp => {
            state.show_help = !state.show_help;
            Vec::new()
        }
        UserAction::SelectNextThread => select_relative_thread(state, 1),
        UserAction::SelectPreviousThread => select_relative_thread(state, -1),
        UserAction::SelectThread(thread_id) => select_thread(state, thread_id),
        UserAction::EnterPromptMode => {
            state.prompt_mode = true;
            Vec::new()
        }
        UserAction::ExitPromptMode => {
            state.prompt_mode = false;
            Vec::new()
        }
        UserAction::PromptAppend(ch) => {
            if state.prompt_mode {
                state.prompt_input.push(ch);
            }
            Vec::new()
        }
        UserAction::PromptBackspace => {
            if state.prompt_mode {
                state.prompt_input.pop();
            }
            Vec::new()
        }
        UserAction::SubmitPrompt => {
            let Some(thread_id) = state.selected_thread_id.clone() else {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Error,
                    message: "Select a thread before submitting a prompt.".to_string(),
                });
                return Vec::new();
            };
            let text = state.prompt_input.trim().to_string();
            if text.is_empty() {
                state.banner = Some(StatusBanner {
                    level: BannerLevel::Error,
                    message: "Prompt input is empty.".to_string(),
                });
                return Vec::new();
            }
            state.prompt_mode = false;
            state.prompt_in_flight = true;
            state.prompt_input.clear();
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: format!("Submitting prompt to thread {thread_id}."),
            });
            vec![Effect::SubmitPrompt { thread_id, text }]
        }
    }
}

fn reduce_event(state: &mut AppState, event: UiEvent) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Some(summary) = event_summary_from_ui_event(&event) {
        push_event_summary(state, summary);
    }

    match event {
        UiEvent::SnapshotLoaded(snapshot) => {
            state.daemon = Some(snapshot.daemon);
            state.session = snapshot.session;
            state.threads = snapshot.threads;
            state.recent_events = snapshot.recent_events.into_iter().collect();
            if let Some(thread) = snapshot.active_thread {
                state.selected_thread_id = Some(thread.summary.id.clone());
                state
                    .thread_details
                    .insert(thread.summary.id.clone(), thread);
            }
            if state.selected_thread_id.is_none() {
                state.selected_thread_id = state
                    .session
                    .active_thread_id
                    .clone()
                    .or_else(|| state.threads.first().map(|thread| thread.id.clone()));
            }
            if let Some(thread_id) = state.selected_thread_id.clone()
                && !state.thread_details.contains_key(&thread_id)
            {
                effects.push(Effect::LoadThread { thread_id });
            }
            state.banner = None;
        }
        UiEvent::ThreadLoaded(thread) => {
            let thread_id = thread.summary.id.clone();
            upsert_thread_summary(&mut state.threads, thread.summary.clone());
            state.thread_details.insert(thread_id.clone(), thread);
            if state.selected_thread_id.is_none() {
                state.selected_thread_id = Some(thread_id);
            }
            state.banner = None;
        }
        UiEvent::PromptStarted { thread_id, .. } => {
            state.prompt_in_flight = true;
            state.selected_thread_id = Some(thread_id);
            state.banner = Some(StatusBanner {
                level: BannerLevel::Info,
                message: "Prompt submitted.".to_string(),
            });
        }
        UiEvent::UpstreamChanged(upstream) => {
            if let Some(daemon) = state.daemon.as_mut() {
                daemon.upstream = upstream.clone();
            }
            if upstream.status != "connected" {
                state.prompt_in_flight = false;
            }
        }
        UiEvent::SessionChanged(session) => {
            let active_thread_id = session.active_thread_id.clone();
            state.session = session;
            if let Some(thread_id) = active_thread_id {
                if state.selected_thread_id.is_none() {
                    state.selected_thread_id = Some(thread_id.clone());
                }
                if !state.thread_details.contains_key(&thread_id) {
                    effects.push(Effect::LoadThread { thread_id });
                }
            }
        }
        UiEvent::ThreadUpdated(thread) => {
            let thread_id = thread.id.clone();
            upsert_thread_summary(&mut state.threads, thread.clone());
            if let Some(detail) = state.thread_details.get_mut(&thread_id) {
                detail.summary = thread;
            }
            if state.selected_thread_id.is_none() {
                state.selected_thread_id = Some(thread_id.clone());
            }
            if state.selected_thread_id.as_deref() == Some(thread_id.as_str())
                && !state.thread_details.contains_key(&thread_id)
            {
                effects.push(Effect::LoadThread { thread_id });
            }
        }
        UiEvent::TurnUpdated { thread_id, turn } => {
            ensure_thread_detail(state, &thread_id);
            if let Some(detail) = state.thread_details.get_mut(&thread_id) {
                upsert_turn(detail, turn.clone());
                if is_terminal_status(&turn.status) {
                    state.prompt_in_flight = false;
                }
            }
        }
        UiEvent::ItemUpdated {
            thread_id,
            turn_id,
            item,
        } => {
            ensure_thread_detail(state, &thread_id);
            if let Some(detail) = state.thread_details.get_mut(&thread_id) {
                let turn = ensure_turn(detail, &turn_id);
                upsert_item(turn, item);
            }
        }
        UiEvent::OutputDelta {
            thread_id,
            turn_id,
            item_id,
            delta,
        } => {
            ensure_thread_detail(state, &thread_id);
            if let Some(detail) = state.thread_details.get_mut(&thread_id) {
                let turn = ensure_turn(detail, &turn_id);
                let item = ensure_item(turn, &item_id);
                item.status = Some("streaming".to_string());
                item.text.get_or_insert_with(String::new).push_str(&delta);
            }
        }
        UiEvent::Warning(message) => {
            state.banner = Some(StatusBanner {
                level: BannerLevel::Warning,
                message,
            });
        }
        UiEvent::Error(message) => {
            state.prompt_in_flight = false;
            state.banner = Some(StatusBanner {
                level: BannerLevel::Error,
                message,
            });
        }
    }

    effects
}

fn select_relative_thread(state: &mut AppState, delta: isize) -> Vec<Effect> {
    if state.threads.is_empty() {
        return Vec::new();
    }
    let current_index = state
        .selected_thread_id
        .as_ref()
        .and_then(|thread_id| {
            state
                .threads
                .iter()
                .position(|thread| thread.id == *thread_id)
        })
        .unwrap_or(0);
    let next_index = if delta.is_negative() {
        current_index.saturating_sub(delta.unsigned_abs())
    } else {
        (current_index + delta as usize).min(state.threads.len().saturating_sub(1))
    };
    select_thread(state, state.threads[next_index].id.clone())
}

fn select_thread(state: &mut AppState, thread_id: String) -> Vec<Effect> {
    state.selected_thread_id = Some(thread_id.clone());
    if state.thread_details.contains_key(&thread_id) {
        Vec::new()
    } else {
        vec![Effect::LoadThread { thread_id }]
    }
}

fn ensure_thread_detail(state: &mut AppState, thread_id: &str) {
    if state.thread_details.contains_key(thread_id) {
        return;
    }
    let summary = state
        .threads
        .iter()
        .find(|thread| thread.id == thread_id)
        .cloned()
        .unwrap_or_else(|| ipc::ThreadSummary {
            id: thread_id.to_string(),
            preview: String::new(),
            name: None,
            model_provider: String::new(),
            cwd: String::new(),
            status: "pending".to_string(),
            created_at: 0,
            updated_at: 0,
        });
    state.thread_details.insert(
        thread_id.to_string(),
        ipc::ThreadView {
            summary,
            turns: Vec::new(),
        },
    );
}

fn ensure_turn<'a>(thread: &'a mut ipc::ThreadView, turn_id: &str) -> &'a mut ipc::TurnView {
    if let Some(index) = thread.turns.iter().position(|turn| turn.id == turn_id) {
        return &mut thread.turns[index];
    }
    thread.turns.push(ipc::TurnView {
        id: turn_id.to_string(),
        status: "in_progress".to_string(),
        error_message: None,
        items: Vec::new(),
    });
    let index = thread.turns.len() - 1;
    &mut thread.turns[index]
}

fn ensure_item<'a>(turn: &'a mut ipc::TurnView, item_id: &str) -> &'a mut ipc::ItemView {
    if let Some(index) = turn.items.iter().position(|item| item.id == item_id) {
        return &mut turn.items[index];
    }
    turn.items.push(ipc::ItemView {
        id: item_id.to_string(),
        item_type: "agent_message".to_string(),
        status: None,
        text: None,
    });
    let index = turn.items.len() - 1;
    &mut turn.items[index]
}

fn upsert_turn(thread: &mut ipc::ThreadView, turn: ipc::TurnView) {
    if let Some(existing) = thread
        .turns
        .iter_mut()
        .find(|existing| existing.id == turn.id)
    {
        existing.status = turn.status;
        existing.error_message = turn.error_message;
        for item in turn.items {
            upsert_item(existing, item);
        }
        return;
    }
    thread.turns.push(turn);
}

fn upsert_item(turn: &mut ipc::TurnView, item: ipc::ItemView) {
    if let Some(existing) = turn
        .items
        .iter_mut()
        .find(|existing| existing.id == item.id)
    {
        existing.item_type = item.item_type;
        if item.status.is_some() {
            existing.status = item.status;
        }
        if item.text.is_some() {
            existing.text = item.text;
        }
        return;
    }
    turn.items.push(item);
}

fn upsert_thread_summary(threads: &mut Vec<ipc::ThreadSummary>, summary: ipc::ThreadSummary) {
    if let Some(existing) = threads.iter_mut().find(|thread| thread.id == summary.id) {
        *existing = summary;
    } else {
        threads.push(summary);
    }
    threads.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn push_event_summary(state: &mut AppState, summary: ipc::EventSummary) {
    if state.recent_events.len() >= MAX_LOG_ENTRIES {
        state.recent_events.pop_front();
    }
    state.recent_events.push_back(summary);
}

fn event_summary_from_ui_event(event: &UiEvent) -> Option<ipc::EventSummary> {
    let timestamp = chrono::Utc::now();
    let (kind, message, thread_id, turn_id) = match event {
        UiEvent::SnapshotLoaded(_) => return None,
        UiEvent::ThreadLoaded(thread) => (
            "thread",
            format!("loaded thread {}", thread.summary.id),
            Some(thread.summary.id.clone()),
            None,
        ),
        UiEvent::PromptStarted { thread_id, turn_id } => (
            "prompt",
            format!("submitted turn {turn_id}"),
            Some(thread_id.clone()),
            Some(turn_id.clone()),
        ),
        UiEvent::UpstreamChanged(upstream) => (
            "upstream",
            format!("upstream {}", upstream.status),
            None,
            None,
        ),
        UiEvent::SessionChanged(session) => (
            "session",
            format!("active turns {}", session.active_turns.len()),
            session.active_thread_id.clone(),
            None,
        ),
        UiEvent::ThreadUpdated(thread) => (
            "thread",
            format!("thread {} {}", thread.id, thread.status),
            Some(thread.id.clone()),
            None,
        ),
        UiEvent::TurnUpdated { thread_id, turn } => (
            "turn",
            format!("turn {} {}", turn.id, turn.status),
            Some(thread_id.clone()),
            Some(turn.id.clone()),
        ),
        UiEvent::ItemUpdated {
            thread_id,
            turn_id,
            item,
        } => (
            "item",
            format!(
                "item {} {}",
                item.id,
                item.status.clone().unwrap_or_else(|| "updated".to_string())
            ),
            Some(thread_id.clone()),
            Some(turn_id.clone()),
        ),
        UiEvent::OutputDelta { .. } => return None,
        UiEvent::Warning(message) => ("warning", message.clone(), None, None),
        UiEvent::Error(message) => ("error", message.clone(), None, None),
    };
    Some(ipc::EventSummary {
        timestamp,
        kind: kind.to_string(),
        message,
        thread_id,
        turn_id,
    })
}

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "interrupted")
}
