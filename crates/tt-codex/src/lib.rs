//! Codex integration layer for TT v2.
//!
//! This crate owns Codex home discovery and lightweight catalog access for
//! TT. It does not reimplement Codex runtime behavior.

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use codex_app_server_protocol as protocol;
use codex_protocol::openai_models::ReasoningEffort;
use futures::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep, timeout};
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;

pub const CODEX_HOME_ENV: &str = "CODEX_HOME";
pub const CODEX_SQLITE_HOME_ENV: &str = "CODEX_SQLITE_HOME";
pub const TT_CODEX_BIN_ENV: &str = "TT_CODEX_BIN";
pub const TT_CODEX_APP_SERVER_BIN_ENV: &str = "TT_CODEX_APP_SERVER_BIN";
pub const CODEX_BIN_FILENAME: &str = "codex";
pub const CODEX_APP_SERVER_BIN_FILENAME: &str = "codex-app-server";
pub const SESSION_INDEX_FILE: &str = "session_index.jsonl";
pub const CODEX_STATE_DB_FILENAME: &str = "state_5.sqlite";
pub const CODEX_LOGS_DB_FILENAME: &str = "logs_1.sqlite";
pub const CODEX_APP_SERVER_LISTEN_URL_ENV: &str = "CODEX_APP_SERVER_LISTEN_URL";
pub const DEFAULT_APP_SERVER_LISTEN_URL: &str = "ws://127.0.0.1:4500";
const APP_SERVER_CONNECT_ATTEMPTS: usize = 5;
const APP_SERVER_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const APP_SERVER_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(10);
const APP_SERVER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const TURN_POLL_INTERVAL: Duration = Duration::from_millis(500);
const TURN_WAIT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRuntimeContract {
    codex_bin: PathBuf,
    app_server_bin: PathBuf,
}

impl CodexRuntimeContract {
    pub fn codex_bin(&self) -> &Path {
        &self.codex_bin
    }

    pub fn app_server_bin(&self) -> &Path {
        &self.app_server_bin
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexHome {
    root: PathBuf,
}

impl CodexHome {
    pub fn discover() -> Result<Self> {
        validate_runtime_contract()?;
        Self::discover_from(env::var_os(CODEX_HOME_ENV), dirs::home_dir())
    }

    pub fn discover_in(cwd: impl AsRef<Path>) -> Result<Self> {
        validate_runtime_contract()?;
        let codex_dir = cwd.as_ref().join(".codex");
        if codex_dir.is_dir() {
            return Ok(Self::from_path(codex_dir));
        }
        Self::discover()
    }

    pub fn from_path(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    pub fn state_db_path(&self) -> PathBuf {
        self.root.join(CODEX_STATE_DB_FILENAME)
    }

    pub fn logs_db_path(&self) -> PathBuf {
        self.root.join(CODEX_LOGS_DB_FILENAME)
    }

    pub fn session_index_path(&self) -> PathBuf {
        self.root.join(SESSION_INDEX_FILE)
    }

    pub fn session_catalog(&self) -> Result<CodexSessionCatalog> {
        CodexSessionCatalog::load(self.root())
    }

    fn discover_from(env_value: Option<OsString>, home_dir: Option<PathBuf>) -> Result<Self> {
        if let Some(value) = env_value {
            let root = PathBuf::from(value);
            if root.is_dir() {
                return Ok(Self { root });
            }
            anyhow::bail!("{} is set but is not a directory", CODEX_HOME_ENV);
        }

        let Some(home) = home_dir else {
            anyhow::bail!("could not resolve a home directory for Codex");
        };
        Ok(Self {
            root: home.join(".codex"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIndexEntry {
    pub id: String,
    pub thread_name: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexThreadRecord {
    pub thread_id: String,
    pub thread_name: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSessionCatalog {
    pub codex_home: CodexHome,
    pub threads: Vec<CodexThreadRecord>,
}

impl CodexSessionCatalog {
    pub fn load(root: &Path) -> Result<Self> {
        let codex_home = CodexHome::from_path(root);
        let path = codex_home.session_index_path();
        let mut threads = Vec::new();

        if path.exists() {
            let file = File::open(&path)
                .with_context(|| format!("open Codex session index {}", path.display()))?;
            for line in BufReader::new(file).lines() {
                let line = line?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(entry) = serde_json::from_str::<SessionIndexEntry>(trimmed) else {
                    continue;
                };
                threads.push(CodexThreadRecord {
                    thread_id: entry.id,
                    thread_name: (!entry.thread_name.trim().is_empty())
                        .then_some(entry.thread_name),
                    updated_at: DateTime::parse_from_rfc3339(&entry.updated_at)
                        .map(|value| value.with_timezone(&Utc))
                        .ok(),
                });
            }
        }

        threads.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.thread_id.cmp(&left.thread_id))
        });

        Ok(Self {
            codex_home,
            threads,
        })
    }

    pub fn find_thread_by_id(&self, thread_id: &str) -> Option<&CodexThreadRecord> {
        self.threads
            .iter()
            .find(|record| record.thread_id == thread_id)
    }

    pub fn find_thread_by_name(&self, thread_name: &str) -> Option<&CodexThreadRecord> {
        self.threads
            .iter()
            .find(|record| record.thread_name.as_deref() == Some(thread_name))
    }

    pub fn resolve_thread(&self, selector: &str) -> Option<&CodexThreadRecord> {
        self.find_thread_by_id(selector)
            .or_else(|| self.find_thread_by_name(selector))
            .or_else(|| {
                self.threads.iter().find(|record| {
                    record
                        .thread_id
                        .split_once(':')
                        .is_some_and(|(_, suffix)| suffix == selector)
                })
            })
    }

    pub fn recent_threads(&self, limit: usize) -> Vec<CodexThreadRecord> {
        self.threads.iter().take(limit).cloned().collect()
    }

    pub fn all_threads(&self) -> &[CodexThreadRecord] {
        &self.threads
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexThreadRuntimeSnapshot {
    pub thread_id: String,
    pub thread_name: Option<String>,
    pub preview: String,
    pub status: String,
    pub cwd: String,
    pub model_provider: String,
    pub ephemeral: bool,
    pub updated_at: i64,
    pub turn_count: usize,
    pub latest_turn_id: Option<String>,
    pub path: Option<String>,
}

pub struct CodexRuntimeClient {
    runtime: Runtime,
    connection: Arc<Mutex<CodexAppServerConnection>>,
    codex_home: CodexHome,
    listen_url: String,
}

impl CodexRuntimeClient {
    pub fn open(cwd: impl AsRef<Path>) -> Result<Self> {
        validate_runtime_contract()?;
        let codex_home = CodexHome::discover_in(cwd.as_ref())?;
        let runtime = Runtime::new().context("create tokio runtime for Codex client")?;
        let listen_url = resolve_app_server_listen_url();
        let connection =
            runtime.block_on(async { CodexAppServerConnection::connect(&listen_url).await })?;
        Ok(Self {
            runtime,
            connection: Arc::new(Mutex::new(connection)),
            codex_home,
            listen_url,
        })
    }

    pub fn codex_home(&self) -> &CodexHome {
        &self.codex_home
    }

    pub fn listen_url(&self) -> &str {
        &self.listen_url
    }

    pub fn catalog(&self) -> Result<CodexSessionCatalog> {
        self.codex_home.session_catalog()
    }

    pub fn list_threads(
        &self,
        cwd: &Path,
        limit: Option<usize>,
    ) -> Result<Vec<CodexThreadRuntimeSnapshot>> {
        let cwd = cwd.to_path_buf();
        let connection = Arc::clone(&self.connection);
        self.runtime.block_on(async {
            let mut connection = connection.lock().await;
            let request_id = connection.next_request_id();
            let response: protocol::ThreadListResponse = connection
                .request_typed(protocol::ClientRequest::ThreadList {
                    request_id,
                    params: protocol::ThreadListParams {
                        cursor: None,
                        limit: limit.map(|value| value as u32),
                        sort_key: None,
                        model_providers: None,
                        source_kinds: None,
                        archived: None,
                        cwd: Some(cwd.display().to_string()),
                        search_term: None,
                    },
                })
                .await?;
            Ok(response.data.into_iter().map(thread_to_snapshot).collect())
        })
    }

    pub fn read_thread(
        &self,
        selector: &str,
        include_turns: bool,
    ) -> Result<Option<CodexThreadRuntimeSnapshot>> {
        let Some(thread_id) = self.resolve_selector(selector)? else {
            return Ok(None);
        };
        let connection = Arc::clone(&self.connection);
        self.runtime.block_on(async {
            let mut connection = connection.lock().await;
            let request_id = connection.next_request_id();
            let response: protocol::ThreadReadResponse = connection
                .request_typed(protocol::ClientRequest::ThreadRead {
                    request_id,
                    params: protocol::ThreadReadParams {
                        thread_id: thread_id.to_string(),
                        include_turns,
                    },
                })
                .await?;
            Ok(Some(thread_to_snapshot(response.thread)))
        })
    }

    pub fn read_thread_full(
        &self,
        selector: &str,
        include_turns: bool,
    ) -> Result<Option<protocol::Thread>> {
        let Some(thread_id) = self.resolve_selector(selector)? else {
            return Ok(None);
        };
        let connection = Arc::clone(&self.connection);
        self.runtime.block_on(async {
            let mut connection = connection.lock().await;
            let request_id = connection.next_request_id();
            let response: protocol::ThreadReadResponse = connection
                .request_typed(protocol::ClientRequest::ThreadRead {
                    request_id,
                    params: protocol::ThreadReadParams {
                        thread_id: thread_id.to_string(),
                        include_turns,
                    },
                })
                .await?;
            Ok(Some(response.thread))
        })
    }

    pub fn start_thread(
        &self,
        cwd: &Path,
        model: Option<String>,
        ephemeral: bool,
    ) -> Result<CodexThreadRuntimeSnapshot> {
        self.start_thread_with_params(protocol::ThreadStartParams {
            cwd: Some(cwd.display().to_string()),
            model,
            sandbox: Some(protocol::SandboxMode::WorkspaceWrite),
            approval_policy: Some(protocol::AskForApproval::Never),
            service_name: Some("tt".to_string()),
            ephemeral: Some(ephemeral),
            persist_extended_history: true,
            ..protocol::ThreadStartParams::default()
        })
    }

    pub fn start_thread_with_params(
        &self,
        params: protocol::ThreadStartParams,
    ) -> Result<CodexThreadRuntimeSnapshot> {
        let connection = Arc::clone(&self.connection);
        self.runtime.block_on(async {
            let mut connection = connection.lock().await;
            let request_id = connection.next_request_id();
            let response: protocol::ThreadStartResponse = connection
                .request_typed(protocol::ClientRequest::ThreadStart { request_id, params })
                .await
                .map_err(anyhow::Error::from)?;
            Ok(thread_to_snapshot(response.thread))
        })
    }

    pub fn resume_thread(
        &self,
        selector: &str,
        cwd: Option<&Path>,
        model: Option<String>,
    ) -> Result<Option<CodexThreadRuntimeSnapshot>> {
        let Some(thread_id) = self.resolve_selector(selector)? else {
            return Ok(None);
        };
        let cwd = cwd.map(|path| path.display().to_string());
        let connection = Arc::clone(&self.connection);
        self.runtime.block_on(async {
            let mut connection = connection.lock().await;
            let request_id = connection.next_request_id();
            let response: protocol::ThreadResumeResponse = connection
                .request_typed(protocol::ClientRequest::ThreadResume {
                    request_id,
                    params: protocol::ThreadResumeParams {
                        thread_id: thread_id.to_string(),
                        model,
                        model_provider: None,
                        history: None,
                        path: None,
                        service_tier: None,
                        cwd,
                        approval_policy: Some(protocol::AskForApproval::Never),
                        approvals_reviewer: None,
                        sandbox: Some(protocol::SandboxMode::WorkspaceWrite),
                        config: None,
                        base_instructions: None,
                        developer_instructions: None,
                        personality: None,
                        persist_extended_history: true,
                    },
                })
                .await
                .map_err(anyhow::Error::from)?;
            Ok(Some(thread_to_snapshot(response.thread)))
        })
    }

    pub fn resume_thread_full(
        &self,
        selector: &str,
        cwd: Option<&Path>,
        model: Option<String>,
    ) -> Result<Option<protocol::Thread>> {
        let Some(thread_id) = self.resolve_selector(selector)? else {
            return Ok(None);
        };
        let cwd = cwd.map(|path| path.display().to_string());
        let connection = Arc::clone(&self.connection);
        self.runtime.block_on(async {
            let mut connection = connection.lock().await;
            let request_id = connection.next_request_id();
            let response: protocol::ThreadResumeResponse = connection
                .request_typed(protocol::ClientRequest::ThreadResume {
                    request_id,
                    params: protocol::ThreadResumeParams {
                        thread_id: thread_id.to_string(),
                        model,
                        model_provider: None,
                        history: None,
                        path: None,
                        service_tier: None,
                        cwd,
                        approval_policy: Some(protocol::AskForApproval::Never),
                        approvals_reviewer: None,
                        sandbox: Some(protocol::SandboxMode::WorkspaceWrite),
                        config: None,
                        base_instructions: None,
                        developer_instructions: None,
                        personality: None,
                        persist_extended_history: true,
                    },
                })
                .await
                .map_err(anyhow::Error::from)?;
            Ok(Some(response.thread))
        })
    }

    pub fn start_turn(
        &self,
        thread_id: &str,
        prompt: &str,
        cwd: Option<&Path>,
        model: Option<String>,
        effort: Option<ReasoningEffort>,
        output_schema: Option<JsonValue>,
    ) -> Result<protocol::Turn> {
        let connection = Arc::clone(&self.connection);
        let thread_id = thread_id.to_string();
        let cwd = cwd.map(|path| path.to_path_buf());
        let prompt = prompt.to_string();
        self.runtime.block_on(async {
            let mut connection = connection.lock().await;
            let request_id = connection.next_request_id();
            let response: protocol::TurnStartResponse = connection
                .request_typed(protocol::ClientRequest::TurnStart {
                    request_id,
                    params: protocol::TurnStartParams {
                        thread_id,
                        input: vec![protocol::UserInput::Text {
                            text: prompt,
                            text_elements: Vec::new(),
                        }],
                        cwd,
                        model,
                        effort,
                        approval_policy: Some(protocol::AskForApproval::Never),
                        output_schema,
                        ..protocol::TurnStartParams::default()
                    },
                })
                .await?;
            Ok(response.turn)
        })
    }

    pub fn wait_for_turn_completion(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<protocol::Turn> {
        let deadline = std::time::Instant::now() + TURN_WAIT_TIMEOUT;
        loop {
            let thread = match self.read_thread_full(thread_id, true) {
                Ok(Some(thread)) => thread,
                Ok(None) => anyhow::bail!("codex thread `{thread_id}` disappeared"),
                Err(error) if can_retry_without_turns(&error) => {
                    if std::time::Instant::now() >= deadline {
                        anyhow::bail!(
                            "timed out waiting for turn `{}` in thread `{}` after {:?}",
                            turn_id,
                            thread_id,
                            TURN_WAIT_TIMEOUT
                        );
                    }
                    std::thread::sleep(TURN_POLL_INTERVAL);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let Some(turn) = thread.turns.into_iter().find(|turn| turn.id == turn_id) else {
                if std::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "timed out waiting for turn `{}` in thread `{}` after {:?}",
                        turn_id,
                        thread_id,
                        TURN_WAIT_TIMEOUT
                    );
                }
                std::thread::sleep(TURN_POLL_INTERVAL);
                continue;
            };
            match turn.status {
                protocol::TurnStatus::Completed
                | protocol::TurnStatus::Failed
                | protocol::TurnStatus::Interrupted => return Ok(turn),
                protocol::TurnStatus::InProgress => {}
            }

            if std::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "timed out waiting for turn `{}` in thread `{}` after {:?}",
                    turn_id,
                    thread_id,
                    TURN_WAIT_TIMEOUT
                );
            }
            std::thread::sleep(TURN_POLL_INTERVAL);
        }
    }

    pub fn load_completed_turn_with_history(
        &self,
        selector: &str,
        turn_id: &str,
        cwd: Option<&Path>,
        model: Option<String>,
    ) -> Result<Option<protocol::Turn>> {
        let deadline = std::time::Instant::now() + TURN_WAIT_TIMEOUT;
        let mut last_seen = None;
        loop {
            if let Some(thread) = self.read_thread_full(selector, true)? {
                if let Some(turn) = thread.turns.into_iter().find(|turn| turn.id == turn_id) {
                    if !turn.items.is_empty() {
                        return Ok(Some(turn));
                    }
                    last_seen = Some(turn);
                }
            }

            if let Some(thread) = self.resume_thread_full(selector, cwd, model.clone())? {
                if let Some(turn) = thread.turns.into_iter().find(|turn| turn.id == turn_id) {
                    if !turn.items.is_empty() {
                        return Ok(Some(turn));
                    }
                    last_seen = Some(turn);
                }
            }

            if std::time::Instant::now() >= deadline {
                return Ok(last_seen);
            }
            std::thread::sleep(TURN_POLL_INTERVAL);
        }
    }

    fn resolve_selector(&self, selector: &str) -> Result<Option<String>> {
        if uuid::Uuid::parse_str(selector).is_ok() {
            return Ok(Some(selector.to_string()));
        }
        let catalog = self.catalog()?;
        Ok(catalog
            .resolve_thread(selector)
            .map(|thread| thread.thread_id.clone()))
    }
}

fn can_retry_without_turns(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("includeTurns is unavailable before first user message")
            || message.contains("ephemeral threads do not support includeTurns")
    })
}

struct CodexAppServerConnection {
    stream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    next_request_id: i64,
    listen_url: String,
}

impl CodexAppServerConnection {
    async fn connect(listen_url: &str) -> Result<Self> {
        let url = Url::parse(listen_url)
            .with_context(|| format!("invalid Codex app-server URL `{listen_url}`"))?;
        let mut last_error = None;

        for attempt in 1..=APP_SERVER_CONNECT_ATTEMPTS {
            match timeout(
                APP_SERVER_CONNECT_TIMEOUT,
                Self::connect_once(&url, listen_url),
            )
            .await
            {
                Ok(Ok(connection)) => return Ok(connection),
                Ok(Err(error)) => last_error = Some(error),
                Err(_) => {
                    last_error = Some(anyhow::anyhow!(
                        "timed out connecting to Codex app-server `{listen_url}` after {:?}",
                        APP_SERVER_CONNECT_TIMEOUT
                    ));
                }
            }

            if attempt < APP_SERVER_CONNECT_ATTEMPTS {
                sleep(connect_backoff_delay(attempt)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("failed to connect to Codex app-server `{listen_url}`")
        }))
    }

    async fn connect_once(url: &Url, listen_url: &str) -> Result<Self> {
        let request = url
            .as_str()
            .into_client_request()
            .with_context(|| format!("prepare request for `{listen_url}`"))?;
        let (mut stream, _) = connect_async(request)
            .await
            .with_context(|| format!("connect to Codex app-server `{listen_url}`"))?;

        let initialize_request_id = protocol::RequestId::String("initialize".to_string());
        Self::send_jsonrpc_message(
            &mut stream,
            protocol::JSONRPCMessage::Request(protocol::JSONRPCRequest {
                method: "initialize".to_string(),
                params: Some(serde_json::to_value(protocol::InitializeParams {
                    client_info: protocol::ClientInfo {
                        name: "tt-codex".to_string(),
                        title: Some("TT Codex Adapter".to_string()),
                        version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                    capabilities: Some(protocol::InitializeCapabilities {
                        experimental_api: true,
                        opt_out_notification_methods: None,
                    }),
                })?),
                id: initialize_request_id.clone(),
                trace: None,
            }),
            listen_url,
        )
        .await?;

        timeout(APP_SERVER_INITIALIZE_TIMEOUT, async {
            loop {
                let Some(message) = stream.next().await else {
                    anyhow::bail!("Codex app-server `{listen_url}` closed during initialize");
                };
                let message = message.with_context(|| {
                    format!("Codex app-server `{listen_url}` sent invalid websocket frame")
                })?;
                let Message::Text(text) = message else {
                    continue;
                };
                let jsonrpc = serde_json::from_str::<protocol::JSONRPCMessage>(&text).with_context(
                    || format!("Codex app-server `{listen_url}` sent invalid JSON-RPC"),
                )?;
                match jsonrpc {
                    protocol::JSONRPCMessage::Response(response)
                        if response.id == initialize_request_id =>
                    {
                        break;
                    }
                    protocol::JSONRPCMessage::Error(error)
                        if error.id == initialize_request_id =>
                    {
                        anyhow::bail!(
                            "Codex app-server `{listen_url}` rejected initialize: {}",
                            error.error.message
                        );
                    }
                    protocol::JSONRPCMessage::Notification(_notification) => {}
                    protocol::JSONRPCMessage::Request(request) => {
                        Self::reject_server_request(&mut stream, request).await?;
                    }
                    protocol::JSONRPCMessage::Response(_) | protocol::JSONRPCMessage::Error(_) => {}
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .with_context(|| {
            format!(
                "timed out waiting for Codex app-server `{listen_url}` initialize response after {:?}",
                APP_SERVER_INITIALIZE_TIMEOUT
            )
        })??;

        Self::send_jsonrpc_message(
            &mut stream,
            protocol::JSONRPCMessage::Notification(jsonrpc_notification_from_client_notification(
                protocol::ClientNotification::Initialized,
            )),
            listen_url,
        )
        .await?;

        Ok(Self {
            stream,
            next_request_id: 1,
            listen_url: listen_url.to_string(),
        })
    }

    fn next_request_id(&mut self) -> protocol::RequestId {
        let id = self.next_request_id;
        self.next_request_id += 1;
        protocol::RequestId::Integer(id)
    }

    async fn request_typed<T>(&mut self, request: protocol::ClientRequest) -> Result<T>
    where
        T: DeserializeOwned,
    {
        timeout(APP_SERVER_REQUEST_TIMEOUT, self.request_typed_impl(request))
            .await
            .with_context(|| {
                format!(
                    "timed out waiting for Codex app-server `{}` after {:?}",
                    self.listen_url, APP_SERVER_REQUEST_TIMEOUT
                )
            })?
    }

    async fn request_typed_impl<T>(&mut self, request: protocol::ClientRequest) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let request_id = request_id_from_client_request(&request);
        let request_message =
            protocol::JSONRPCMessage::Request(jsonrpc_request_from_client_request(request));
        Self::send_jsonrpc_message(&mut self.stream, request_message, &self.listen_url).await?;

        loop {
            let Some(message) = self.stream.next().await else {
                anyhow::bail!(
                    "Codex app-server `{}` closed while waiting for response",
                    self.listen_url
                );
            };
            let message = message.context("read websocket message from Codex app-server")?;
            let Message::Text(text) = message else {
                continue;
            };
            let jsonrpc =
                serde_json::from_str::<protocol::JSONRPCMessage>(&text).with_context(|| {
                    format!(
                        "parse JSON-RPC message from Codex app-server `{}`",
                        self.listen_url
                    )
                })?;
            match jsonrpc {
                protocol::JSONRPCMessage::Response(response) if response.id == request_id => {
                    return serde_json::from_value(response.result)
                        .context("decode Codex app-server response");
                }
                protocol::JSONRPCMessage::Error(error) if error.id == request_id => {
                    anyhow::bail!(
                        "Codex app-server `{}` request failed: {}",
                        self.listen_url,
                        error.error.message
                    );
                }
                protocol::JSONRPCMessage::Notification(_notification) => {}
                protocol::JSONRPCMessage::Request(request) => {
                    Self::reject_server_request(&mut self.stream, request).await?;
                }
                protocol::JSONRPCMessage::Response(_) | protocol::JSONRPCMessage::Error(_) => {}
            }
        }
    }

    async fn send_jsonrpc_message(
        stream: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        message: protocol::JSONRPCMessage,
        listen_url: &str,
    ) -> Result<()> {
        let payload = serde_json::to_string(&message).context("serialize JSON-RPC message")?;
        stream
            .send(Message::Text(payload.into()))
            .await
            .with_context(|| format!("write websocket message to `{listen_url}`"))?;
        Ok(())
    }

    async fn reject_server_request(
        stream: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        request: protocol::JSONRPCRequest,
    ) -> Result<()> {
        let id = request.id.clone();
        Self::send_jsonrpc_message(
            stream,
            protocol::JSONRPCMessage::Error(protocol::JSONRPCError {
                error: protocol::JSONRPCErrorError {
                    code: -32601,
                    message: format!("unsupported Codex app-server request `{}`", request.method),
                    data: None,
                },
                id,
            }),
            "<codex-app-server>",
        )
        .await
    }
}

fn request_id_from_client_request(request: &protocol::ClientRequest) -> protocol::RequestId {
    jsonrpc_request_from_client_request(request.clone()).id
}

fn jsonrpc_request_from_client_request(
    request: protocol::ClientRequest,
) -> protocol::JSONRPCRequest {
    let value = serde_json::to_value(request).expect("client request should serialize");
    serde_json::from_value(value).expect("client request should encode as JSON-RPC request")
}

fn jsonrpc_notification_from_client_notification(
    notification: protocol::ClientNotification,
) -> protocol::JSONRPCNotification {
    let value = serde_json::to_value(notification).expect("client notification should serialize");
    serde_json::from_value(value)
        .expect("client notification should encode as JSON-RPC notification")
}

fn resolve_app_server_listen_url() -> String {
    std::env::var(CODEX_APP_SERVER_LISTEN_URL_ENV)
        .or_else(|_| std::env::var("TT_APP_SERVER_LISTEN_URL"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_APP_SERVER_LISTEN_URL.to_string())
}

fn resolve_required_binary(env_key: &str, binary_name: &str, label: &str) -> Result<PathBuf> {
    let path = if let Some(value) = env::var_os(env_key) {
        PathBuf::from(value)
    } else {
        expected_local_user_bin(binary_name)?
    };
    ensure_executable_file(&path, label)
}

fn expected_local_user_bin(binary_name: &str) -> Result<PathBuf> {
    let Some(home_dir) = dirs::home_dir() else {
        anyhow::bail!(
            "could not resolve a home directory while validating the TT/Codex runtime contract"
        );
    };
    Ok(home_dir.join(".local").join("bin").join(binary_name))
}

fn ensure_executable_file(path: &Path, label: &str) -> Result<PathBuf> {
    let metadata = std::fs::metadata(path).with_context(|| {
        format!(
            "{label} is required by the TT/Codex runtime contract but was not found at {}",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        anyhow::bail!(
            "{label} is required by the TT/Codex runtime contract but {} is not a file",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o111 == 0 {
            anyhow::bail!(
                "{label} is required by the TT/Codex runtime contract but {} is not executable",
                path.display()
            );
        }
    }
    Ok(path.to_path_buf())
}

fn connect_backoff_delay(attempt: usize) -> Duration {
    let millis = 100_u64.saturating_mul(2_u64.saturating_pow((attempt.saturating_sub(1)) as u32));
    Duration::from_millis(millis.min(2_000))
}

fn thread_to_snapshot(thread: protocol::Thread) -> CodexThreadRuntimeSnapshot {
    CodexThreadRuntimeSnapshot {
        thread_id: thread.id,
        thread_name: thread.name,
        preview: thread.preview,
        status: format_thread_status(&thread.status),
        cwd: thread.cwd.display().to_string(),
        model_provider: thread.model_provider,
        ephemeral: thread.ephemeral,
        updated_at: thread.updated_at,
        turn_count: thread.turns.len(),
        latest_turn_id: thread.turns.last().map(|turn| turn.id.clone()),
        path: thread.path.map(|path| path.display().to_string()),
    }
}

fn format_thread_status(status: &protocol::ThreadStatus) -> String {
    match status {
        protocol::ThreadStatus::NotLoaded => "notLoaded".to_string(),
        protocol::ThreadStatus::Idle => "idle".to_string(),
        protocol::ThreadStatus::SystemError => "systemError".to_string(),
        protocol::ThreadStatus::Active { .. } => "active".to_string(),
    }
}

pub fn discover_codex_home() -> Result<CodexHome> {
    CodexHome::discover()
}

pub fn validate_runtime_contract() -> Result<CodexRuntimeContract> {
    let codex_bin = resolve_required_binary(TT_CODEX_BIN_ENV, CODEX_BIN_FILENAME, "Codex CLI")?;
    let app_server_bin = resolve_required_binary(
        TT_CODEX_APP_SERVER_BIN_ENV,
        CODEX_APP_SERVER_BIN_FILENAME,
        "Codex app-server",
    )?;
    validate_runtime_contract_paths(codex_bin, app_server_bin)
}

fn validate_runtime_contract_paths(
    codex_bin: PathBuf,
    app_server_bin: PathBuf,
) -> Result<CodexRuntimeContract> {
    let codex_bin = ensure_executable_file(&codex_bin, "Codex CLI")?;
    let app_server_bin = ensure_executable_file(&app_server_bin, "Codex app-server")?;
    Ok(CodexRuntimeContract {
        codex_bin,
        app_server_bin,
    })
}

pub fn codex_state_db_path(codex_home: &Path) -> PathBuf {
    codex_home.join(CODEX_STATE_DB_FILENAME)
}

pub fn codex_logs_db_path(codex_home: &Path) -> PathBuf {
    codex_home.join(CODEX_LOGS_DB_FILENAME)
}

pub fn codex_session_index_path(codex_home: &Path) -> PathBuf {
    codex_home.join(SESSION_INDEX_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn discover_uses_environment_override() {
        let dir = tempdir().expect("tempdir");
        let discovered = CodexHome::discover_from(
            Some(dir.path().as_os_str().to_os_string()),
            Some(PathBuf::from("/tmp/fallback")),
        )
        .expect("discover codex home");

        assert_eq!(discovered.root(), dir.path());
    }

    #[test]
    fn catalog_loads_session_index() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join(SESSION_INDEX_FILE);
        std::fs::write(
            &path,
            concat!(
                "{\"id\":\"a\",\"thread_name\":\"alpha\",\"updated_at\":\"2026-04-08T12:00:00Z\"}\n",
                "{\"id\":\"b\",\"thread_name\":\"\",\"updated_at\":\"2026-04-08T12:01:00Z\"}\n"
            ),
        )
        .expect("write session index");

        let catalog = CodexSessionCatalog::load(dir.path()).expect("load catalog");

        assert_eq!(catalog.threads.len(), 2);
        assert_eq!(
            catalog
                .find_thread_by_id("a")
                .and_then(|record| record.thread_name.as_deref()),
            Some("alpha")
        );
        assert!(catalog.find_thread_by_name("alpha").is_some());
    }

    #[test]
    fn runtime_contract_accepts_env_override_bins() {
        let dir = tempdir().expect("tempdir");
        let codex_bin = dir.path().join(CODEX_BIN_FILENAME);
        let app_server_bin = dir.path().join(CODEX_APP_SERVER_BIN_FILENAME);
        std::fs::write(&codex_bin, "#!/bin/sh\n").expect("write codex bin");
        std::fs::write(&app_server_bin, "#!/bin/sh\n").expect("write app-server bin");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&codex_bin, std::fs::Permissions::from_mode(0o755))
                .expect("chmod codex");
            std::fs::set_permissions(&app_server_bin, std::fs::Permissions::from_mode(0o755))
                .expect("chmod app-server");
        }

        let contract = validate_runtime_contract_paths(codex_bin.clone(), app_server_bin.clone())
            .expect("validate contract");

        assert_eq!(contract.codex_bin(), codex_bin.as_path());
        assert_eq!(contract.app_server_bin(), app_server_bin.as_path());
    }

    #[test]
    fn runtime_contract_reports_missing_binary() {
        let dir = tempdir().expect("tempdir");
        let codex_bin = dir.path().join(CODEX_BIN_FILENAME);
        let missing_app_server = dir.path().join(CODEX_APP_SERVER_BIN_FILENAME);
        std::fs::write(&codex_bin, "#!/bin/sh\n").expect("write codex bin");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&codex_bin, std::fs::Permissions::from_mode(0o755))
                .expect("chmod codex");
        }

        let error = validate_runtime_contract_paths(codex_bin.clone(), missing_app_server.clone())
            .expect_err("contract should fail");

        let message = format!("{error:#}");
        assert!(message.contains("Codex app-server"));
        assert!(message.contains(&missing_app_server.display().to_string()));
    }
}
