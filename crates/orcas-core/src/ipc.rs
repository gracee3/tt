use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::events::ConnectionState;

pub mod methods {
    pub const DAEMON_STATUS: &str = "daemon/status";
    pub const DAEMON_CONNECT: &str = "daemon/connect";
    pub const DAEMON_DISCONNECT: &str = "daemon/disconnect";
    pub const STATE_GET: &str = "state/get";
    pub const SESSION_GET_ACTIVE: &str = "session/get_active";
    pub const MODELS_LIST: &str = "models/list";
    pub const THREADS_LIST: &str = "threads/list";
    pub const THREADS_LIST_SCOPED: &str = "threads/list_scoped";
    pub const THREAD_START: &str = "thread/start";
    pub const THREAD_READ: &str = "thread/read";
    pub const THREAD_GET: &str = "thread/get";
    pub const THREAD_RESUME: &str = "thread/resume";
    pub const TURNS_RECENT: &str = "turns/recent";
    pub const TURN_START: &str = "turn/start";
    pub const TURN_INTERRUPT: &str = "turn/interrupt";
    pub const EVENTS_SUBSCRIBE: &str = "events/subscribe";
    pub const EVENTS_NOTIFICATION: &str = "events/notification";
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Empty {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatusResponse {
    pub socket_path: String,
    pub codex_endpoint: String,
    pub codex_binary_path: String,
    pub upstream: ConnectionState,
    pub client_count: usize,
    pub known_threads: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaemonConnectRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConnectResponse {
    pub status: DaemonStatusResponse,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateGetRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateGetResponse {
    pub snapshot: StateSnapshot,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionGetActiveRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionGetActiveResponse {
    pub session: SessionState,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventsSubscribeRequest {
    pub include_snapshot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsSubscribeResponse {
    pub subscribed: bool,
    pub snapshot: Option<StateSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsNotification {
    pub event: DaemonEventEnvelope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub daemon: DaemonStatusResponse,
    pub session: SessionState,
    pub threads: Vec<ThreadSummary>,
    pub active_thread: Option<ThreadView>,
    pub recent_events: Vec<EventSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub active_thread_id: Option<String>,
    pub active_turns: Vec<ActiveTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveTurn {
    pub thread_id: String,
    pub turn_id: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub message: String,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonEventEnvelope {
    pub emitted_at: DateTime<Utc>,
    pub event: DaemonEvent,
}

impl DaemonEventEnvelope {
    pub fn new(event: DaemonEvent) -> Self {
        Self {
            emitted_at: Utc::now(),
            event,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonEvent {
    UpstreamStatusChanged {
        upstream: ConnectionState,
    },
    SessionChanged {
        session: SessionState,
    },
    ThreadUpdated {
        thread: ThreadSummary,
    },
    TurnUpdated {
        thread_id: String,
        turn: TurnView,
    },
    ItemUpdated {
        thread_id: String,
        turn_id: String,
        item: ItemView,
    },
    OutputDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    Warning {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsListResponse {
    pub data: Vec<ModelSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSummary {
    pub id: String,
    pub display_name: String,
    pub hidden: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadsListRequest {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadsListScopedRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadsListResponse {
    pub data: Vec<ThreadSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub id: String,
    pub preview: String,
    pub name: Option<String>,
    pub model_provider: String,
    pub cwd: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadView {
    pub summary: ThreadSummary,
    pub turns: Vec<TurnView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnView {
    pub id: String,
    pub status: String,
    pub error_message: Option<String>,
    pub items: Vec<ItemView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemView {
    pub id: String,
    pub item_type: String,
    pub status: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadStartRequest {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub ephemeral: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadStartResponse {
    pub thread: ThreadSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadResumeRequest {
    pub thread_id: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadResumeResponse {
    pub thread: ThreadSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadReadRequest {
    pub thread_id: String,
    pub include_turns: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadReadResponse {
    pub thread: ThreadView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadGetRequest {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadGetResponse {
    pub thread: ThreadView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnsRecentRequest {
    pub thread_id: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnsRecentResponse {
    pub thread_id: String,
    pub turns: Vec<TurnView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartRequest {
    pub thread_id: String,
    pub text: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartResponse {
    pub turn_id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnInterruptRequest {
    pub thread_id: String,
    pub turn_id: String,
}
