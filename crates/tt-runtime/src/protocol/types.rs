use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn default_persist_extended_history() -> bool {
    true
}

pub(crate) fn normalize_label(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len() + 8);
    let mut previous_was_separator = false;
    for ch in raw.chars() {
        if ch == '-' || ch == ' ' {
            if !normalized.is_empty() && !previous_was_separator {
                normalized.push('_');
            }
            previous_was_separator = true;
            continue;
        }
        if ch.is_uppercase() {
            if !normalized.is_empty() && !previous_was_separator {
                normalized.push('_');
            }
            for lower in ch.to_lowercase() {
                normalized.push(lower);
            }
            previous_was_separator = false;
            continue;
        }
        normalized.push(ch);
        previous_was_separator = ch == '_';
    }
    normalized
}

fn map_string(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).map(ToOwned::to_owned)
}

fn map_string_vec(map: &Map<String, Value>, key: &str) -> Vec<String> {
    map.get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn map_value_vec(map: &Map<String, Value>, key: &str) -> Vec<Value> {
    map.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn map_i64(map: &Map<String, Value>, key: &str) -> Option<i64> {
    map.get(key).and_then(Value::as_i64)
}

fn map_i32(map: &Map<String, Value>, key: &str) -> Option<i32> {
    map_i64(map, key).and_then(|value| i32::try_from(value).ok())
}

fn normalized_status(value: Option<&str>) -> Option<String> {
    value.map(normalize_label)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub title: Option<String>,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeCapabilities {
    pub experimental_api: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opt_out_notification_methods: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<InitializeCapabilities>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    #[serde(default)]
    pub server_info: Option<ServerInfo>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub platform_family: Option<String>,
    #[serde(default)]
    pub platform_os: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<AskForApproval>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    #[serde(default)]
    pub experimental_raw_events: bool,
    #[serde(default = "default_persist_extended_history")]
    pub persist_extended_history: bool,
}

impl Default for ThreadStartParams {
    fn default() -> Self {
        Self {
            model: None,
            model_provider: None,
            cwd: None,
            approval_policy: Some(AskForApproval::default()),
            approvals_reviewer: None,
            sandbox: None,
            config: None,
            service_name: Some("tt".to_string()),
            base_instructions: None,
            developer_instructions: None,
            ephemeral: Some(false),
            experimental_raw_events: false,
            persist_extended_history: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<AskForApproval>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    #[serde(default = "default_persist_extended_history")]
    pub persist_extended_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadParams {
    pub thread_id: String,
    pub include_turns: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_providers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_kinds: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_term: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_hidden: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<AskForApproval>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_policy: Option<SandboxPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
    pub expected_turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UserInput {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default)]
        text_elements: Vec<TextElement>,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TextElement {
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AskForApproval {
    Mode(ApprovalMode),
    Granular { granular: GranularApproval },
}

impl Default for AskForApproval {
    fn default() -> Self {
        Self::Mode(ApprovalMode::Never)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    Untrusted,
    OnFailure,
    OnRequest,
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GranularApproval {
    pub sandbox_approval: bool,
    pub rules: bool,
    pub skill_approval: bool,
    pub request_permissions: bool,
    pub mcp_elicitations: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalsReviewer {
    User,
    GuardianSubagent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SandboxPolicy {
    DangerFullAccess,
    ReadOnly {
        #[serde(default)]
        access: Value,
        network_access: bool,
    },
    WorkspaceWrite {
        #[serde(default)]
        writable_roots: Vec<String>,
        #[serde(default)]
        read_only_access: Value,
        network_access: bool,
        exclude_tmpdir_env_var: bool,
        exclude_slash_tmp: bool,
    },
    ExternalSandbox {
        #[serde(default)]
        network_access: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartResponse {
    pub thread: Thread,
    pub model: String,
    pub model_provider: String,
    #[serde(default)]
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeResponse {
    pub thread: Thread,
    pub model: String,
    pub model_provider: String,
    #[serde(default)]
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadReadResponse {
    pub thread: Thread,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListResponse {
    pub data: Vec<Thread>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartResponse {
    pub turn: Turn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerResponse {
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnInterruptResponse {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListResponse {
    pub data: Vec<Model>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartedNotification {
    pub thread: Thread,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStatusChangedNotification {
    pub thread_id: String,
    pub status: ThreadStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedNotification {
    pub thread_id: String,
    pub turn: Turn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedNotification {
    pub thread_id: String,
    pub turn: Turn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemStartedNotification {
    pub item: ThreadItem,
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemCompletedNotification {
    pub item: ThreadItem,
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningSummaryTextDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
    pub summary_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningSummaryPartAddedNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub summary_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningTextDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
    pub content_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandExecutionOutputDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeOutputDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallProgressNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnDiffUpdatedNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanUpdatedNotification {
    pub thread_id: String,
    pub turn_id: String,
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub plan: Vec<TurnPlanStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPlanStep {
    pub step: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTokenUsageUpdatedNotification {
    pub thread_id: String,
    pub token_usage: ThreadTokenUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTokenUsage {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub ephemeral: bool,
    #[serde(default)]
    pub model_provider: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    pub status: ThreadStatus,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub cli_version: String,
    #[serde(default)]
    pub source: Option<Value>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub turns: Vec<Turn>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub id: String,
    #[serde(default)]
    pub items: Vec<ThreadItem>,
    pub status: TurnStatus,
    #[serde(default)]
    pub error: Option<TurnError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnError {
    pub message: String,
    #[serde(default)]
    pub additional_details: Option<String>,
    #[serde(default)]
    pub tt_error_info: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadItemDetail {
    UserMessage {
        content: Vec<Value>,
    },
    HookPrompt {
        fragments: Vec<Value>,
    },
    AgentMessage {
        text: String,
        phase: Option<String>,
        memory_citation: Option<Value>,
    },
    Plan {
        text: String,
    },
    Reasoning {
        summary: Vec<String>,
        content: Vec<String>,
    },
    CommandExecution {
        command: String,
        cwd: String,
        process_id: Option<String>,
        source: Option<String>,
        status: Option<String>,
        command_actions: Vec<Value>,
        aggregated_output: Option<String>,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
    },
    FileChange {
        changes: Vec<Value>,
        status: Option<String>,
    },
    McpToolCall {
        server: String,
        tool: String,
        status: Option<String>,
        arguments: Option<Value>,
        result: Option<Value>,
        error: Option<Value>,
        duration_ms: Option<i64>,
    },
    DynamicToolCall {
        tool: String,
        arguments: Option<Value>,
        status: Option<String>,
        content_items: Vec<Value>,
        success: Option<bool>,
        duration_ms: Option<i64>,
    },
    CollabAgentToolCall {
        tool: Option<String>,
        status: Option<String>,
        sender_thread_id: Option<String>,
        receiver_thread_ids: Vec<String>,
        prompt: Option<String>,
        model: Option<String>,
        reasoning_effort: Option<String>,
        agents_states: Option<Value>,
    },
    WebSearch {
        query: String,
        action: Option<Value>,
    },
    ImageView {
        path: String,
    },
    ImageGeneration {
        status: Option<String>,
        revised_prompt: Option<String>,
        result: Option<String>,
        saved_path: Option<String>,
    },
    EnteredReviewMode {
        review: String,
    },
    ExitedReviewMode {
        review: String,
    },
    ContextCompaction,
    Unknown {
        raw: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadItem {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ThreadItem {
    pub fn text(&self) -> Option<&str> {
        self.extra.get("text").and_then(Value::as_str)
    }

    pub fn normalized_item_type(&self) -> String {
        normalize_label(&self.item_type)
    }

    pub fn item_status(&self) -> Option<String> {
        normalized_status(self.extra.get("status").and_then(Value::as_str))
    }

    pub fn parsed_detail(&self) -> ThreadItemDetail {
        let kind = self.normalized_item_type();
        match kind.as_str() {
            "user_message" => ThreadItemDetail::UserMessage {
                content: map_value_vec(&self.extra, "content"),
            },
            "hook_prompt" => ThreadItemDetail::HookPrompt {
                fragments: map_value_vec(&self.extra, "fragments"),
            },
            "agent_message" => ThreadItemDetail::AgentMessage {
                text: self.text().unwrap_or_default().to_string(),
                phase: normalized_status(self.extra.get("phase").and_then(Value::as_str)),
                memory_citation: self.extra.get("memoryCitation").cloned(),
            },
            "plan" => ThreadItemDetail::Plan {
                text: self.text().unwrap_or_default().to_string(),
            },
            "reasoning" => ThreadItemDetail::Reasoning {
                summary: map_string_vec(&self.extra, "summary"),
                content: map_string_vec(&self.extra, "content"),
            },
            "command_execution" => ThreadItemDetail::CommandExecution {
                command: map_string(&self.extra, "command").unwrap_or_default(),
                cwd: map_string(&self.extra, "cwd").unwrap_or_default(),
                process_id: map_string(&self.extra, "processId"),
                source: normalized_status(self.extra.get("source").and_then(Value::as_str)),
                status: self.item_status(),
                command_actions: map_value_vec(&self.extra, "commandActions"),
                aggregated_output: map_string(&self.extra, "aggregatedOutput"),
                exit_code: map_i32(&self.extra, "exitCode"),
                duration_ms: map_i64(&self.extra, "durationMs"),
            },
            "file_change" => ThreadItemDetail::FileChange {
                changes: map_value_vec(&self.extra, "changes"),
                status: self.item_status(),
            },
            "mcp_tool_call" => ThreadItemDetail::McpToolCall {
                server: map_string(&self.extra, "server").unwrap_or_default(),
                tool: map_string(&self.extra, "tool").unwrap_or_default(),
                status: self.item_status(),
                arguments: self.extra.get("arguments").cloned(),
                result: self.extra.get("result").cloned(),
                error: self.extra.get("error").cloned(),
                duration_ms: map_i64(&self.extra, "durationMs"),
            },
            "dynamic_tool_call" => ThreadItemDetail::DynamicToolCall {
                tool: map_string(&self.extra, "tool").unwrap_or_default(),
                arguments: self.extra.get("arguments").cloned(),
                status: self.item_status(),
                content_items: map_value_vec(&self.extra, "contentItems"),
                success: self.extra.get("success").and_then(Value::as_bool),
                duration_ms: map_i64(&self.extra, "durationMs"),
            },
            "collab_agent_tool_call" => ThreadItemDetail::CollabAgentToolCall {
                tool: map_string(&self.extra, "tool"),
                status: self.item_status(),
                sender_thread_id: map_string(&self.extra, "senderThreadId"),
                receiver_thread_ids: map_string_vec(&self.extra, "receiverThreadIds"),
                prompt: map_string(&self.extra, "prompt"),
                model: map_string(&self.extra, "model"),
                reasoning_effort: normalized_status(
                    self.extra.get("reasoningEffort").and_then(Value::as_str),
                ),
                agents_states: self.extra.get("agentsStates").cloned(),
            },
            "web_search" => ThreadItemDetail::WebSearch {
                query: map_string(&self.extra, "query").unwrap_or_default(),
                action: self.extra.get("action").cloned(),
            },
            "image_view" => ThreadItemDetail::ImageView {
                path: map_string(&self.extra, "path").unwrap_or_default(),
            },
            "image_generation" => ThreadItemDetail::ImageGeneration {
                status: self.item_status(),
                revised_prompt: map_string(&self.extra, "revisedPrompt"),
                result: map_string(&self.extra, "result"),
                saved_path: map_string(&self.extra, "savedPath"),
            },
            "entered_review_mode" => ThreadItemDetail::EnteredReviewMode {
                review: map_string(&self.extra, "review").unwrap_or_default(),
            },
            "exited_review_mode" => ThreadItemDetail::ExitedReviewMode {
                review: map_string(&self.extra, "review").unwrap_or_default(),
            },
            "context_compaction" => ThreadItemDetail::ContextCompaction,
            _ => ThreadItemDetail::Unknown {
                raw: Value::Object(self.extra.clone()),
            },
        }
    }

    pub fn detail_kind(&self) -> String {
        self.normalized_item_type()
    }

    pub fn detail_json(&self) -> Option<Value> {
        serde_json::to_value(self.parsed_detail()).ok()
    }

    pub fn display_text(&self) -> Option<String> {
        match self.parsed_detail() {
            ThreadItemDetail::AgentMessage { text, .. } | ThreadItemDetail::Plan { text } => {
                (!text.trim().is_empty()).then_some(text)
            }
            ThreadItemDetail::Reasoning { summary, content } => {
                let combined = summary
                    .into_iter()
                    .chain(content)
                    .collect::<Vec<_>>()
                    .join("\n");
                (!combined.trim().is_empty()).then_some(combined)
            }
            ThreadItemDetail::CommandExecution {
                aggregated_output,
                command,
                ..
            } => aggregated_output
                .filter(|output| !output.trim().is_empty())
                .or_else(|| (!command.trim().is_empty()).then_some(command)),
            ThreadItemDetail::EnteredReviewMode { review }
            | ThreadItemDetail::ExitedReviewMode { review } => {
                (!review.trim().is_empty()).then_some(review)
            }
            ThreadItemDetail::WebSearch { query, .. } => {
                (!query.trim().is_empty()).then_some(query)
            }
            ThreadItemDetail::ImageGeneration { result, .. } => {
                result.filter(|value| !value.trim().is_empty())
            }
            _ => self.text().map(ToOwned::to_owned),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub model: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ThreadStatus {
    #[serde(rename = "notLoaded")]
    NotLoaded,
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "systemError")]
    SystemError,
    #[serde(rename = "active")]
    Active {
        #[serde(default)]
        active_flags: Vec<String>,
    },
}

impl ThreadStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::NotLoaded => "notLoaded",
            Self::Idle => "idle",
            Self::SystemError => "systemError",
            Self::Active { .. } => "active",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
}

impl TurnStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
            Self::InProgress => "inProgress",
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AskForApproval, ClientInfo, GranularApproval, InitializeParams, InitializeResponse,
        ItemCompletedNotification, ModelListResponse, SandboxPolicy, TextElement, Thread,
        ThreadItem, ThreadListParams, ThreadReadResponse, ThreadResumeParams, ThreadStartParams,
        ThreadStartedNotification, ThreadStatus, ThreadStatusChangedNotification, Turn,
        TurnCompletedNotification, TurnError, TurnStatus, UserInput,
    };

    fn sample_thread() -> Thread {
        Thread {
            id: "thread-1".to_string(),
            preview: "Preview".to_string(),
            ephemeral: true,
            model_provider: "openai".to_string(),
            created_at: 1_700_000_000,
            updated_at: 1_700_000_100,
            status: ThreadStatus::Active {
                active_flags: vec!["turn_running".to_string()],
            },
            path: Some("/repo/session.json".to_string()),
            cwd: "/repo".to_string(),
            cli_version: "0.1.0".to_string(),
            source: Some(json!({"kind":"cli"})),
            name: Some("Thread".to_string()),
            turns: vec![Turn {
                id: "turn-1".to_string(),
                items: vec![ThreadItem {
                    id: "item-1".to_string(),
                    item_type: "message".to_string(),
                    extra: serde_json::from_value(json!({
                        "text": "hello",
                        "role": "assistant"
                    }))
                    .expect("thread item extra map"),
                }],
                status: TurnStatus::Completed,
                error: Some(TurnError {
                    message: "done".to_string(),
                    additional_details: Some("details".to_string()),
                    tt_error_info: Some(json!({"code":"done"})),
                }),
            }],
            extra: serde_json::from_value(json!({
                "serviceName": "tt",
                "custom": true
            }))
            .expect("thread extra map"),
        }
    }

    #[test]
    fn initialize_params_omits_optional_capabilities_when_absent() {
        let params = InitializeParams {
            client_info: ClientInfo {
                name: "tt".to_string(),
                title: None,
                version: "0.1.0".to_string(),
            },
            capabilities: None,
        };

        let value = serde_json::to_value(&params).expect("serialize initialize params");
        assert_eq!(value["clientInfo"]["name"], "tt");
        assert!(value.get("capabilities").is_none());

        let round_trip: InitializeParams =
            serde_json::from_value(value).expect("deserialize initialize params");
        assert!(round_trip.capabilities.is_none());
    }

    #[test]
    fn initialize_response_defaults_missing_optional_fields_to_none() {
        let response =
            serde_json::from_value::<InitializeResponse>(json!({})).expect("deserialize response");

        assert!(response.server_info.is_none());
        assert!(response.user_agent.is_none());
        assert!(response.platform_family.is_none());
        assert!(response.platform_os.is_none());
    }

    #[test]
    fn thread_start_params_default_matches_expected_operator_contract() {
        let params = ThreadStartParams::default();
        let value = serde_json::to_value(&params).expect("serialize thread start params");

        assert_eq!(value["serviceName"], "tt");
        assert_eq!(value["approvalPolicy"], "never");
        assert_eq!(value["ephemeral"], false);
        assert_eq!(value["experimentalRawEvents"], false);
        assert_eq!(value["persistExtendedHistory"], true);
    }

    #[test]
    fn thread_start_params_deserialize_missing_booleans_with_defaults() {
        let params = serde_json::from_value::<ThreadStartParams>(json!({}))
            .expect("deserialize thread start params");

        assert!(!params.experimental_raw_events);
        assert!(params.persist_extended_history);
        assert!(matches!(params.approval_policy, None));
        assert!(params.ephemeral.is_none());
    }

    #[test]
    fn sandbox_policy_uses_stable_tagged_shape() {
        let policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: vec!["/repo".to_string()],
            read_only_access: json!({"kind":"minimal"}),
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: true,
        };

        let value = serde_json::to_value(&policy).expect("serialize sandbox policy");
        assert_eq!(value["type"], "workspaceWrite");
        assert_eq!(value["writable_roots"][0], "/repo");

        let round_trip: SandboxPolicy =
            serde_json::from_value(value).expect("deserialize sandbox policy");
        match round_trip {
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                network_access,
                exclude_tmpdir_env_var,
                exclude_slash_tmp,
                ..
            } => {
                assert_eq!(writable_roots, vec!["/repo".to_string()]);
                assert!(network_access);
                assert!(!exclude_tmpdir_env_var);
                assert!(exclude_slash_tmp);
            }
            other => panic!("unexpected sandbox policy: {other:?}"),
        }
    }

    #[test]
    fn user_input_text_preserves_tag_and_default_text_elements() {
        let input = serde_json::from_value::<UserInput>(json!({
            "type": "text",
            "text": "hello"
        }))
        .expect("deserialize user input");

        match input {
            UserInput::Text {
                text,
                text_elements,
            } => {
                assert_eq!(text, "hello");
                assert!(text_elements.is_empty());
            }
        }

        let serialized = serde_json::to_value(&UserInput::Text {
            text: "hello".to_string(),
            text_elements: vec![TextElement::default()],
        })
        .expect("serialize user input");
        assert_eq!(serialized["type"], "text");
        assert_eq!(serialized["text"], "hello");
    }

    #[test]
    fn thread_status_active_defaults_active_flags_when_missing() {
        let status = serde_json::from_value::<ThreadStatus>(json!({
            "type": "active"
        }))
        .expect("deserialize thread status");

        match status {
            ThreadStatus::Active { active_flags } => assert!(active_flags.is_empty()),
            other => panic!("unexpected thread status: {other:?}"),
        }
    }

    #[test]
    fn turn_status_serializes_with_camel_case_variant_names() {
        let value = serde_json::to_value(TurnStatus::InProgress).expect("serialize turn status");
        assert_eq!(value, json!("inProgress"));

        let round_trip: TurnStatus =
            serde_json::from_value(value).expect("deserialize turn status");
        assert!(matches!(round_trip, TurnStatus::InProgress));
    }

    #[test]
    fn ask_for_approval_granular_uses_untagged_granular_shape() {
        let approval = AskForApproval::Granular {
            granular: GranularApproval {
                sandbox_approval: true,
                rules: false,
                skill_approval: true,
                request_permissions: false,
                mcp_elicitations: true,
            },
        };

        let value = serde_json::to_value(&approval).expect("serialize approval");
        assert_eq!(
            value,
            json!({
                "granular": {
                    "sandbox_approval": true,
                    "rules": false,
                    "skill_approval": true,
                    "request_permissions": false,
                    "mcp_elicitations": true
                }
            })
        );

        let round_trip: AskForApproval =
            serde_json::from_value(value).expect("deserialize approval");
        match round_trip {
            AskForApproval::Granular { granular } => {
                assert!(granular.sandbox_approval);
                assert!(granular.skill_approval);
                assert!(granular.mcp_elicitations);
                assert!(!granular.rules);
                assert!(!granular.request_permissions);
            }
            other => panic!("unexpected approval shape: {other:?}"),
        }
    }

    #[test]
    fn thread_resume_params_omit_absent_fields_but_preserve_history_default() {
        let params = ThreadResumeParams {
            thread_id: "thread-1".to_string(),
            model: None,
            cwd: None,
            approval_policy: None,
            approvals_reviewer: None,
            sandbox: None,
            config: None,
            base_instructions: None,
            developer_instructions: None,
            persist_extended_history: true,
        };

        let value = serde_json::to_value(&params).expect("serialize resume params");
        assert_eq!(value["threadId"], "thread-1");
        assert_eq!(value["persistExtendedHistory"], true);
        assert!(value.get("model").is_none());
        assert!(value.get("config").is_none());

        let round_trip: ThreadResumeParams = serde_json::from_value(json!({"threadId":"thread-1"}))
            .expect("deserialize sparse resume params");
        assert_eq!(round_trip.thread_id, "thread-1");
        assert!(round_trip.persist_extended_history);
        assert!(round_trip.model.is_none());
        assert!(round_trip.approval_policy.is_none());
    }

    #[test]
    fn thread_list_params_round_trip_sparse_filter_vectors_and_flags() {
        let params = ThreadListParams {
            cursor: Some("cursor-1".to_string()),
            limit: Some(25),
            sort_key: Some("updated_at".to_string()),
            model_providers: Some(vec!["openai".to_string()]),
            source_kinds: Some(vec!["cli".to_string(), "daemon".to_string()]),
            archived: Some(false),
            cwd: Some("/repo".to_string()),
            search_term: Some("bugfix".to_string()),
        };

        let value = serde_json::to_value(&params).expect("serialize list params");
        assert_eq!(value["modelProviders"][0], "openai");
        assert_eq!(value["sourceKinds"][1], "daemon");
        assert_eq!(value["archived"], false);

        let round_trip: ThreadListParams =
            serde_json::from_value(value).expect("deserialize list params");
        assert_eq!(round_trip.limit, Some(25));
        assert_eq!(round_trip.cwd.as_deref(), Some("/repo"));
        assert_eq!(round_trip.search_term.as_deref(), Some("bugfix"));
    }

    #[test]
    fn thread_started_notification_defaults_sparse_thread_fields_and_preserves_extras() {
        let notification = serde_json::from_value::<ThreadStartedNotification>(json!({
            "thread": {
                "id": "thread-1",
                "status": {"type":"idle"},
                "serviceName": "tt",
                "custom": true
            }
        }))
        .expect("deserialize started notification");

        assert_eq!(notification.thread.id, "thread-1");
        assert_eq!(notification.thread.preview, "");
        assert!(!notification.thread.ephemeral);
        assert_eq!(notification.thread.cwd, "");
        assert!(notification.thread.turns.is_empty());
        assert_eq!(
            notification.thread.extra.get("serviceName"),
            Some(&json!("tt"))
        );
        assert_eq!(notification.thread.extra.get("custom"), Some(&json!(true)));
    }

    #[test]
    fn thread_status_changed_notification_preserves_tagged_active_status() {
        let notification = serde_json::from_value::<ThreadStatusChangedNotification>(json!({
            "threadId": "thread-1",
            "status": {
                "type": "active",
                "active_flags": ["turn_running", "streaming"]
            }
        }))
        .expect("deserialize status changed notification");

        assert_eq!(notification.thread_id, "thread-1");
        match notification.status {
            ThreadStatus::Active { active_flags } => {
                assert_eq!(
                    active_flags,
                    vec!["turn_running".to_string(), "streaming".to_string()]
                );
            }
            other => panic!("unexpected thread status: {other:?}"),
        }
    }

    #[test]
    fn turn_completed_notification_defaults_sparse_turn_fields() {
        let notification = serde_json::from_value::<TurnCompletedNotification>(json!({
            "threadId": "thread-1",
            "turn": {
                "id": "turn-1",
                "status": "completed"
            }
        }))
        .expect("deserialize completed notification");

        assert_eq!(notification.thread_id, "thread-1");
        assert_eq!(notification.turn.id, "turn-1");
        assert!(notification.turn.items.is_empty());
        assert!(notification.turn.error.is_none());
    }

    #[test]
    fn item_completed_notification_preserves_flattened_thread_item_payload() {
        let notification = serde_json::from_value::<ItemCompletedNotification>(json!({
            "threadId": "thread-1",
            "turnId": "turn-1",
            "item": {
                "id": "item-1",
                "type": "message",
                "text": "hello",
                "role": "assistant"
            }
        }))
        .expect("deserialize item notification");

        assert_eq!(notification.thread_id, "thread-1");
        assert_eq!(notification.turn_id, "turn-1");
        assert_eq!(notification.item.text(), Some("hello"));
        assert_eq!(
            notification.item.extra.get("role"),
            Some(&json!("assistant"))
        );
    }

    #[test]
    fn thread_read_response_round_trips_nested_turns_and_extra_fields() {
        let response = ThreadReadResponse {
            thread: sample_thread(),
        };

        let value = serde_json::to_value(&response).expect("serialize read response");
        assert_eq!(value["thread"]["status"]["type"], "active");
        assert_eq!(value["thread"]["turns"][0]["items"][0]["type"], "message");
        assert_eq!(value["thread"]["serviceName"], "tt");

        let round_trip: ThreadReadResponse =
            serde_json::from_value(value).expect("deserialize read response");
        assert_eq!(round_trip.thread.id, "thread-1");
        assert_eq!(round_trip.thread.turns.len(), 1);
        assert_eq!(round_trip.thread.turns[0].items[0].text(), Some("hello"));
        assert_eq!(round_trip.thread.extra.get("custom"), Some(&json!(true)));
    }

    #[test]
    fn model_list_response_defaults_display_and_visibility_fields_when_sparse() {
        let response = serde_json::from_value::<ModelListResponse>(json!({
            "data": [
                {
                    "id": "model-1",
                    "model": "gpt-5.4"
                }
            ],
            "nextCursor": null
        }))
        .expect("deserialize model list response");

        assert_eq!(response.data.len(), 1);
        let model = &response.data[0];
        assert_eq!(model.id, "model-1");
        assert_eq!(model.model, "gpt-5.4");
        assert_eq!(model.display_name, "");
        assert_eq!(model.description, "");
        assert!(!model.hidden);
        assert!(!model.is_default);
    }
}
