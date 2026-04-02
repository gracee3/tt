use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ipc::{ThreadTokenUsageView, TurnPlanView};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub received_at: DateTime<Utc>,
    pub source: String,
    pub event: OrcasEvent,
}

impl EventEnvelope {
    pub fn new(source: impl Into<String>, event: OrcasEvent) -> Self {
        Self {
            received_at: Utc::now(),
            source: source.into(),
            event,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrcasEvent {
    ConnectionStateChanged(ConnectionState),
    ThreadStarted {
        thread_id: String,
        preview: String,
    },
    ThreadStatusChanged {
        thread_id: String,
        status: String,
    },
    TurnStarted {
        thread_id: String,
        turn: CodexTurnEvent,
    },
    TurnCompleted {
        thread_id: String,
        turn: CodexTurnEvent,
    },
    ItemStarted {
        thread_id: String,
        turn_id: String,
        item: CodexItemEvent,
    },
    ItemCompleted {
        thread_id: String,
        turn_id: String,
        item: CodexItemEvent,
    },
    AgentMessageDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    PlanDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    ReasoningSummaryTextDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
        summary_index: i64,
    },
    ReasoningSummaryPartAdded {
        thread_id: String,
        turn_id: String,
        item_id: String,
        summary_index: i64,
    },
    ReasoningTextDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
        content_index: i64,
    },
    CommandExecutionOutputDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    FileChangeOutputDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        delta: String,
    },
    McpToolCallProgress {
        thread_id: String,
        turn_id: String,
        item_id: String,
        message: String,
    },
    TurnDiffUpdated {
        thread_id: String,
        turn_id: String,
        diff: String,
    },
    TurnPlanUpdated {
        thread_id: String,
        turn_id: String,
        plan: TurnPlanView,
    },
    ThreadTokenUsageUpdated {
        thread_id: String,
        token_usage: ThreadTokenUsageView,
    },
    ServerRequest {
        method: String,
    },
    Warning {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionState {
    pub endpoint: String,
    pub status: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexTurnEvent {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub error_summary: Option<String>,
    #[serde(default)]
    pub latest_diff: Option<String>,
    #[serde(default)]
    pub latest_plan_snapshot: Option<Value>,
    #[serde(default)]
    pub token_usage_snapshot: Option<Value>,
    #[serde(default)]
    pub latest_plan: Option<TurnPlanView>,
    #[serde(default)]
    pub token_usage: Option<ThreadTokenUsageView>,
    #[serde(default)]
    pub items: Vec<CodexItemEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexItemEvent {
    pub id: String,
    pub item_type: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub detail_kind: Option<String>,
    #[serde(default)]
    pub detail: Option<Value>,
}
