//! Codex integration layer for TT v2.
//!
//! This crate owns Codex home discovery and lightweight catalog access for
//! TT. It does not reimplement Codex runtime behavior.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::SystemTime;

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
pub const CODEX_AUTH_FILE_NAME: &str = "auth.json";
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
const TURN_WAIT_TIMEOUT_SECS_ENV: &str = "TT_CODEX_TURN_WAIT_TIMEOUT_SECS";
const TURN_SOFT_SILENCE_SECS_ENV: &str = "TT_CODEX_TURN_SOFT_SILENCE_SECS";
const TURN_HARD_CEILING_SECS_ENV: &str = "TT_CODEX_TURN_HARD_CEILING_SECS";
const DEFAULT_TURN_SOFT_SILENCE_SECS: u64 = 900;
const DEFAULT_TURN_HARD_CEILING_SECS: u64 = 7_200;

#[derive(Debug, Default)]
struct RepoSettingsEnv {
    values: BTreeMap<String, String>,
}

static REPO_SETTINGS_ENV: OnceLock<StdMutex<RepoSettingsEnv>> = OnceLock::new();

fn repo_settings_env() -> &'static StdMutex<RepoSettingsEnv> {
    REPO_SETTINGS_ENV.get_or_init(|| StdMutex::new(RepoSettingsEnv::default()))
}

pub fn load_repo_settings_env(cwd: impl AsRef<Path>) -> Result<()> {
    let cwd = cwd.as_ref();
    let settings_path = cwd
        .ancestors()
        .map(|ancestor| ancestor.join(".tt").join("settings.env"))
        .find(|path| path.is_file());

    let mut overlay = RepoSettingsEnv::default();
    if let Some(path) = settings_path {
        let repo_root = path
            .parent()
            .and_then(|parent| parent.parent())
            .context("invalid TT settings.env path")?;
        overlay.values = parse_repo_settings_env(&path, repo_root)?;
    }

    *repo_settings_env().lock().expect("repo settings env mutex") = overlay;
    Ok(())
}

pub fn repo_env_var_os(key: &str) -> Option<OsString> {
    merge_repo_env_value(
        env::var_os(key),
        repo_settings_env()
            .lock()
            .expect("repo settings env mutex")
            .values
            .get(key)
            .map(String::as_str),
    )
}

pub fn repo_env_var(key: &str) -> Option<String> {
    repo_env_var_os(key).and_then(|value| value.into_string().ok())
}

pub fn apply_repo_settings_env(command: &mut Command) {
    let overlay = repo_settings_env()
        .lock()
        .expect("repo settings env mutex")
        .values
        .clone();
    for (key, value) in overlay {
        if env::var_os(&key).is_none() {
            command.env(key, value);
        }
    }
}

fn merge_repo_env_value(current: Option<OsString>, overlay: Option<&str>) -> Option<OsString> {
    current.or_else(|| overlay.map(OsString::from))
}

fn parse_repo_settings_env(path: &Path, repo_root: &Path) -> Result<BTreeMap<String, String>> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("read TT settings env {}", path.display()))?;
    let mut values = BTreeMap::new();
    for (line_number, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = trimmed.split_once('=') else {
            anyhow::bail!(
                "invalid TT settings env line {} in {}: expected KEY=VALUE",
                line_number + 1,
                path.display()
            );
        };
        let key = key.trim();
        if key.is_empty() {
            anyhow::bail!(
                "invalid TT settings env line {} in {}: empty key",
                line_number + 1,
                path.display()
            );
        }
        let value = normalize_repo_settings_env_value(repo_root, key, raw_value.trim())?;
        values.insert(key.to_string(), value);
    }
    Ok(values)
}

fn normalize_repo_settings_env_value(repo_root: &Path, key: &str, value: &str) -> Result<String> {
    let unquoted = strip_matching_quotes(value);
    if key == "HOME" || !is_path_like_repo_settings_key(key) {
        return Ok(unquoted.to_string());
    }

    if let Some(expanded) = expand_home_dir(unquoted) {
        return Ok(expanded);
    }

    let path = Path::new(unquoted);
    if path.is_absolute() {
        return Ok(path.display().to_string());
    }
    if is_path_like_literal(unquoted) {
        return Ok(repo_root.join(path).display().to_string());
    }
    Ok(unquoted.to_string())
}

fn is_path_like_repo_settings_key(key: &str) -> bool {
    key == "TT_RUNTIME_BIN"
        || key.ends_with("_BIN")
        || key.ends_with("_PATH")
        || key.ends_with("_FILE")
}

fn is_path_like_literal(value: &str) -> bool {
    value.starts_with("./")
        || value.starts_with("../")
        || value == "."
        || value == ".."
        || value.contains('/')
}

fn expand_home_dir(value: &str) -> Option<String> {
    if value == "~" || value.starts_with("~/") {
        let home = dirs::home_dir()?;
        let suffix = value.strip_prefix("~/").unwrap_or("");
        return Some(home.join(suffix).display().to_string());
    }
    None
}

fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == 34 && bytes[bytes.len() - 1] == 34)
            || (bytes[0] == 39 && bytes[bytes.len() - 1] == 39))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRuntimeContract {
    codex_bin: PathBuf,
    app_server_bin: PathBuf,
    auth_json: PathBuf,
}

impl CodexRuntimeContract {
    pub fn codex_bin(&self) -> &Path {
        &self.codex_bin
    }

    pub fn app_server_bin(&self) -> &Path {
        &self.app_server_bin
    }

    pub fn auth_json(&self) -> &Path {
        &self.auth_json
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexHome {
    root: PathBuf,
}

impl CodexHome {
    pub fn discover() -> Result<Self> {
        load_repo_settings_env(env::current_dir()?)?;
        Self::discover_in(env::current_dir()?)
    }

    pub fn discover_in(cwd: impl AsRef<Path>) -> Result<Self> {
        let cwd = cwd.as_ref();
        load_repo_settings_env(cwd)?;
        let codex_dir = managed_project_codex_home(cwd);
        if codex_dir.is_dir() {
            return Ok(Self::from_path(codex_dir));
        }
        Self::discover_from(repo_env_var_os(CODEX_HOME_ENV), dirs::home_dir())
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

    pub fn auth_json_path(&self) -> PathBuf {
        self.root.join(CODEX_AUTH_FILE_NAME)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnWatchdogConfig {
    pub soft_silence: Duration,
    pub hard_ceiling: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnWatchdogState {
    Healthy,
    Quiet,
    Suspect,
    Stalled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnWatchdogObservation {
    pub state: TurnWatchdogState,
    pub elapsed_seconds: u64,
    pub silent_seconds: u64,
    pub thread_updated_at: i64,
    pub turn_count: usize,
    pub turn_status: Option<String>,
    pub turn_items: usize,
    pub progress_signal: Option<String>,
    pub app_server_log_modified_at: Option<i64>,
    pub app_server_log_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnWatchdogSnapshot {
    thread_updated_at: i64,
    turn_count: usize,
    turn_status: Option<String>,
    turn_items: usize,
    app_server_log_modified_at: Option<i64>,
    app_server_log_size: Option<u64>,
    progress_signal: Option<String>,
}

impl TurnWatchdogSnapshot {
    fn from_thread_and_turn(
        thread: &protocol::Thread,
        turn: Option<&protocol::Turn>,
        app_server_log_path: Option<&Path>,
    ) -> Self {
        let (app_server_log_modified_at, app_server_log_size) = app_server_log_path
            .and_then(progress_marker)
            .unwrap_or((None, None));
        Self {
            thread_updated_at: thread.updated_at,
            turn_count: thread.turns.len(),
            turn_status: turn.map(|turn| format!("{:?}", turn.status)),
            turn_items: turn.map(|turn| turn.items.len()).unwrap_or(0),
            app_server_log_modified_at,
            app_server_log_size,
            progress_signal: None,
        }
    }

    fn progress_signal(&self, previous: &Option<Self>) -> Option<String> {
        let Some(previous) = previous.as_ref() else {
            return Some("first observation".to_string());
        };
        if self.thread_updated_at != previous.thread_updated_at {
            return Some("thread.updated_at changed".to_string());
        }
        if self.turn_count != previous.turn_count {
            return Some("thread.turn_count changed".to_string());
        }
        if self.turn_status != previous.turn_status {
            return Some("turn.status changed".to_string());
        }
        if self.turn_items != previous.turn_items {
            return Some("turn.items changed".to_string());
        }
        if self.app_server_log_modified_at != previous.app_server_log_modified_at {
            return Some("app-server log changed".to_string());
        }
        if self.app_server_log_size != previous.app_server_log_size {
            return Some("app-server log size changed".to_string());
        }
        None
    }

    fn state(
        &self,
        last_progress_at: &std::time::Instant,
        soft_silence: Duration,
        deadline: std::time::Instant,
    ) -> TurnWatchdogState {
        let now = std::time::Instant::now();
        if now >= deadline {
            return TurnWatchdogState::Stalled;
        }
        let silent_for = now.duration_since(*last_progress_at);
        if silent_for.as_secs() >= soft_silence.as_secs().saturating_mul(2) {
            TurnWatchdogState::Suspect
        } else if silent_for >= soft_silence {
            TurnWatchdogState::Quiet
        } else {
            TurnWatchdogState::Healthy
        }
    }
}

impl CodexRuntimeClient {
    pub fn open(cwd: impl AsRef<Path>) -> Result<Self> {
        let cwd = cwd.as_ref();
        load_repo_settings_env(cwd)?;
        let contract = validate_runtime_contract(cwd)?;
        let codex_home = CodexHome::discover_in(cwd)?;
        let runtime = Runtime::new().context("create tokio runtime for Codex client")?;
        let listen_url = resolve_app_server_listen_url();
        let connection = match runtime
            .block_on(async { CodexAppServerConnection::connect(&listen_url).await })
        {
            Ok(connection) => connection,
            Err(_first_error) => {
                start_codex_app_server_if_needed(cwd, &contract, &listen_url)?;
                runtime.block_on(async { CodexAppServerConnection::connect(&listen_url).await })?
            }
        };
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
        self.wait_for_turn_completion_with_watchdog(
            thread_id,
            turn_id,
            turn_watchdog_config(),
            |_| {},
        )
    }

    pub fn wait_for_turn_completion_with_watchdog<F>(
        &self,
        thread_id: &str,
        turn_id: &str,
        watchdog: TurnWatchdogConfig,
        mut on_progress: F,
    ) -> Result<protocol::Turn>
    where
        F: FnMut(&TurnWatchdogObservation),
    {
        let started_at = std::time::Instant::now();
        let deadline = std::time::Instant::now() + watchdog.hard_ceiling;
        let mut last_progress_at = std::time::Instant::now();
        let mut last_snapshot: Option<TurnWatchdogSnapshot> = None;
        let mut last_reported_state: Option<TurnWatchdogState> = None;
        let app_server_log_path = env::var_os("TT_CODEX_APP_SERVER_LOG_PATH").map(PathBuf::from);
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
                            watchdog.hard_ceiling
                        );
                    }
                    std::thread::sleep(TURN_POLL_INTERVAL);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let current_turn = thread.turns.iter().find(|turn| turn.id == turn_id);
            let current_snapshot = TurnWatchdogSnapshot::from_thread_and_turn(
                &thread,
                current_turn,
                app_server_log_path.as_deref(),
            );
            let progress_signal = current_snapshot.progress_signal(&last_snapshot);
            let mut state =
                current_snapshot.state(&last_progress_at, watchdog.soft_silence, deadline);
            if progress_signal.is_some() {
                last_progress_at = std::time::Instant::now();
                state = TurnWatchdogState::Healthy;
            }
            let silent_seconds = std::time::Instant::now()
                .duration_since(last_progress_at)
                .as_secs();
            let observation = TurnWatchdogObservation {
                state,
                elapsed_seconds: started_at.elapsed().as_secs(),
                silent_seconds,
                thread_updated_at: thread.updated_at,
                turn_count: thread.turns.len(),
                turn_status: current_turn.map(|turn| format!("{:?}", turn.status)),
                turn_items: current_turn.map(|turn| turn.items.len()).unwrap_or(0),
                progress_signal,
                app_server_log_modified_at: current_snapshot.app_server_log_modified_at,
                app_server_log_size: current_snapshot.app_server_log_size,
            };
            if last_reported_state != Some(observation.state)
                || observation.progress_signal.is_some()
            {
                on_progress(&observation);
                last_reported_state = Some(observation.state);
            }

            let Some(turn) = current_turn.cloned() else {
                if std::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "timed out waiting for turn `{}` in thread `{}` after {:?}",
                        turn_id,
                        thread_id,
                        watchdog.hard_ceiling
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

            last_snapshot = Some(current_snapshot);
            if state == TurnWatchdogState::Stalled {
                anyhow::bail!(
                    "watchdog stalled waiting for turn `{}` in thread `{}` after {:?} (soft={}s hard={}s, last_signal={})",
                    turn_id,
                    thread_id,
                    watchdog.hard_ceiling,
                    watchdog.soft_silence.as_secs(),
                    watchdog.hard_ceiling.as_secs(),
                    last_snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.progress_signal.as_deref())
                        .unwrap_or("<none>")
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
        let turn_wait_timeout = turn_wait_timeout();
        let deadline = std::time::Instant::now() + turn_wait_timeout;
        let mut last_seen = None;
        loop {
            let thread = match self.read_thread_full(selector, true) {
                Ok(thread) => thread,
                Err(error) if can_retry_after_turn_completion(&error) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(error);
                    }
                    std::thread::sleep(TURN_POLL_INTERVAL);
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Some(thread) = thread {
                if let Some(turn) = thread.turns.into_iter().find(|turn| turn.id == turn_id) {
                    if !turn.items.is_empty() {
                        return Ok(Some(turn));
                    }
                    last_seen = Some(turn);
                }
            }

            let thread = match self.resume_thread_full(selector, cwd, model.clone()) {
                Ok(thread) => thread,
                Err(error) if can_retry_after_turn_completion(&error) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(error);
                    }
                    std::thread::sleep(TURN_POLL_INTERVAL);
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Some(thread) = thread {
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

fn can_retry_after_turn_completion(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("failed to load rollout") && message.contains("empty session file")
    })
}

fn turn_watchdog_config() -> TurnWatchdogConfig {
    let soft_silence = env::var(TURN_SOFT_SILENCE_SECS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TURN_SOFT_SILENCE_SECS);
    let hard_ceiling = env::var(TURN_HARD_CEILING_SECS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            env::var(TURN_WAIT_TIMEOUT_SECS_ENV)
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_TURN_HARD_CEILING_SECS)
        });
    TurnWatchdogConfig {
        soft_silence: Duration::from_secs(soft_silence),
        hard_ceiling: Duration::from_secs(hard_ceiling),
    }
}

fn turn_wait_timeout() -> Duration {
    turn_watchdog_config().hard_ceiling
}

fn progress_marker(path: &Path) -> Option<(Option<i64>, Option<u64>)> {
    let metadata = fs::metadata(path).ok()?;
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|value: SystemTime| value.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration: std::time::Duration| duration.as_secs() as i64);
    Some((modified_at, Some(metadata.len())))
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
    repo_env_var(CODEX_APP_SERVER_LISTEN_URL_ENV)
        .or_else(|| repo_env_var("TT_APP_SERVER_LISTEN_URL"))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_APP_SERVER_LISTEN_URL.to_string())
}

pub fn configured_app_server_listen_url() -> String {
    resolve_app_server_listen_url()
}

fn resolve_required_binary(
    _cwd: &Path,
    env_key: &str,
    binary_name: &str,
    label: &str,
) -> Result<PathBuf> {
    let path = if let Some(value) = repo_env_var_os(env_key) {
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

pub fn validate_runtime_contract(cwd: impl AsRef<Path>) -> Result<CodexRuntimeContract> {
    let cwd = cwd.as_ref();
    let codex_bin =
        resolve_required_binary(cwd, TT_CODEX_BIN_ENV, CODEX_BIN_FILENAME, "Codex CLI")?;
    let app_server_bin = resolve_required_binary(
        cwd,
        TT_CODEX_APP_SERVER_BIN_ENV,
        CODEX_APP_SERVER_BIN_FILENAME,
        "Codex app-server",
    )?;
    let mut contract = validate_runtime_contract_paths(codex_bin, app_server_bin)?;
    contract.auth_json = managed_project_auth_json_path(cwd);
    Ok(contract)
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
        auth_json: PathBuf::new(),
    })
}

pub fn managed_project_codex_home(cwd: impl AsRef<Path>) -> PathBuf {
    let cwd = cwd.as_ref();
    cwd.ancestors()
        .find(|ancestor| ancestor.join(".tt").is_dir())
        .unwrap_or(cwd)
        .join(".codex")
}

pub fn managed_project_auth_json_path(cwd: impl AsRef<Path>) -> PathBuf {
    managed_project_codex_home(cwd).join(CODEX_AUTH_FILE_NAME)
}

pub fn managed_project_auth_is_present(cwd: impl AsRef<Path>) -> bool {
    ensure_auth_file(managed_project_auth_json_path(cwd)).is_ok()
}

fn start_codex_app_server_if_needed(
    cwd: &Path,
    contract: &CodexRuntimeContract,
    listen_url: &str,
) -> Result<()> {
    if codex_app_server_is_reachable(listen_url)? {
        return Ok(());
    }

    let repo_root = cwd
        .ancestors()
        .find(|ancestor| ancestor.join(".tt").is_dir())
        .unwrap_or(cwd);
    let log_path = repo_root.join(".tt").join("codex-app-server.log");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "create Codex app-server runtime directory {}",
                parent.display()
            )
        })?;
    }
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open Codex app-server log {}", log_path.display()))?;
    let stderr = log_file
        .try_clone()
        .with_context(|| format!("clone Codex app-server log {}", log_path.display()))?;

    let codex_home = managed_project_codex_home(repo_root);
    fs::create_dir_all(&codex_home)
        .with_context(|| format!("create Codex home {}", codex_home.display()))?;

    let mut command = Command::new(contract.app_server_bin());
    command
        .arg("--listen")
        .arg(listen_url)
        .env("CODEX_HOME", &codex_home)
        .env("TT_RUNTIME_LISTEN_URL", listen_url)
        .env("TT_APP_SERVER_LISTEN_URL", listen_url)
        .env("CODEX_APP_SERVER_LISTEN_URL", listen_url)
        .env("TT_CODEX_APP_SERVER_LOG_PATH", &log_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr));

    command.spawn().with_context(|| {
        format!(
            "start Codex app-server `{}`",
            contract.app_server_bin().display()
        )
    })?;

    Ok(())
}

fn codex_app_server_is_reachable(listen_url: &str) -> Result<bool> {
    let url = Url::parse(listen_url)
        .with_context(|| format!("invalid Codex app-server URL `{listen_url}`"))?;
    let Some(host) = url.host_str() else {
        anyhow::bail!("Codex app-server URL `{listen_url}` has no host");
    };
    let Some(port) = url.port_or_known_default() else {
        anyhow::bail!("Codex app-server URL `{listen_url}` has no port");
    };
    let socket_addr = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))
        .with_context(|| format!("resolve `{host}:{port}` for `{listen_url}`"))?
        .next();
    let Some(socket_addr) = socket_addr else {
        anyhow::bail!("no socket addresses resolved for `{listen_url}`");
    };
    Ok(std::net::TcpStream::connect_timeout(&socket_addr, Duration::from_millis(250)).is_ok())
}

fn ensure_auth_file(path: PathBuf) -> Result<PathBuf> {
    if !path.exists() {
        anyhow::bail!("Codex auth file is missing: {}", path.display());
    }
    if !path.is_file() {
        anyhow::bail!("Codex auth path is not a file: {}", path.display());
    }
    Ok(path)
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
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;
    use tempfile::tempdir;

    static SETTINGS_ENV_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn watchdog_snapshot(
        thread_updated_at: i64,
        turn_count: usize,
        turn_status: Option<&str>,
        turn_items: usize,
        app_server_log_modified_at: Option<i64>,
        app_server_log_size: Option<u64>,
    ) -> TurnWatchdogSnapshot {
        TurnWatchdogSnapshot {
            thread_updated_at,
            turn_count,
            turn_status: turn_status.map(std::string::ToString::to_string),
            turn_items,
            app_server_log_modified_at,
            app_server_log_size,
            progress_signal: None,
        }
    }

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
    fn repo_settings_env_loads_and_resolves_paths() {
        let _guard = SETTINGS_ENV_TEST_LOCK.lock().expect("settings env lock");
        let repo = tempdir().expect("tempdir");
        std::fs::create_dir_all(repo.path().join(".tt")).expect("create tt dir");
        std::fs::write(
            repo.path().join(".tt/settings.env"),
            format!(
                "TT_RUNTIME_BIN=./target/debug/tt-cli\nTT_CUSTOM_BIN=./target/debug/custom\nTT_CUSTOM_PATH=./target/debug/custom-path\nTT_REPO_SETTINGS_SENTINEL=loaded\n",
            ),
        )
        .expect("write settings env");

        load_repo_settings_env(repo.path()).expect("load settings env");
        let tt_runtime_bin = repo.path().join("./target/debug/tt-cli");
        let custom_bin = repo.path().join("./target/debug/custom");
        let custom_path = repo.path().join("./target/debug/custom-path");
        let tt_runtime_bin_str = tt_runtime_bin.to_string_lossy().to_string();
        let custom_bin_str = custom_bin.to_string_lossy().to_string();
        let custom_path_str = custom_path.to_string_lossy().to_string();

        assert_eq!(
            repo_env_var("TT_CUSTOM_BIN").as_deref(),
            Some(custom_bin_str.as_str())
        );
        assert_eq!(
            repo_env_var("TT_CUSTOM_PATH").as_deref(),
            Some(custom_path_str.as_str())
        );
        assert_eq!(
            repo_env_var("TT_RUNTIME_BIN").as_deref(),
            Some(tt_runtime_bin_str.as_str())
        );
        assert_eq!(
            repo_env_var("TT_REPO_SETTINGS_SENTINEL").as_deref(),
            Some("loaded")
        );
    }

    #[test]
    fn repo_settings_env_injects_child_commands() {
        let _guard = SETTINGS_ENV_TEST_LOCK.lock().expect("settings env lock");
        let repo = tempdir().expect("tempdir");
        std::fs::create_dir_all(repo.path().join(".tt")).expect("create tt dir");
        std::fs::write(
            repo.path().join(".tt/settings.env"),
            "TT_REPO_SETTINGS_SENTINEL=loaded\n",
        )
        .expect("write settings env");

        load_repo_settings_env(repo.path()).expect("load settings env");

        let mut command = Command::new("sh");
        command.args(["-c", "printf %s \"$TT_REPO_SETTINGS_SENTINEL\""]);
        apply_repo_settings_env(&mut command);
        let output = command.output().expect("run child");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "loaded");
    }

    #[test]
    fn repo_settings_env_overlay_does_not_override_shell_env() {
        assert_eq!(
            merge_repo_env_value(Some(OsString::from("shell")), Some("overlay"))
                .and_then(|value| value.into_string().ok()),
            Some("shell".to_string())
        );
        assert_eq!(
            merge_repo_env_value(None, Some("overlay")).and_then(|value| value.into_string().ok()),
            Some("overlay".to_string())
        );
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
        std::fs::create_dir_all(dir.path().join(".tt")).expect("create tt dir");
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
    fn discover_in_prefers_repo_local_codex_home_without_auth() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".tt")).expect("create tt dir");
        std::fs::create_dir_all(dir.path().join(".codex")).expect("create codex dir");

        let discovered = CodexHome::discover_in(dir.path()).expect("discover codex home");

        assert_eq!(discovered.root(), dir.path().join(".codex").as_path());
        assert!(!managed_project_auth_is_present(dir.path()));
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

    #[test]
    fn retry_after_turn_completion_matches_empty_session_rollout() {
        let error = anyhow::anyhow!(
            "Codex app-server `ws://127.0.0.1:4500` request failed: failed to load rollout `/home/me/.codex/sessions/2026/04/09/rollout.jsonl` for thread 123: empty session file"
        );
        assert!(can_retry_after_turn_completion(&error));
    }

    #[test]
    fn retry_after_turn_completion_rejects_other_failures() {
        let error = anyhow::anyhow!(
            "Codex app-server `ws://127.0.0.1:4500` request failed: model provider rejected request"
        );
        assert!(!can_retry_after_turn_completion(&error));
    }

    #[test]
    fn watchdog_progress_signal_detects_thread_update() {
        let previous = watchdog_snapshot(1, 1, Some("InProgress"), 0, Some(10), Some(100));
        let current = watchdog_snapshot(2, 1, Some("InProgress"), 0, Some(10), Some(100));

        assert_eq!(
            current.progress_signal(&Some(previous)),
            Some("thread.updated_at changed".to_string())
        );
    }

    #[test]
    fn watchdog_progress_signal_detects_log_growth() {
        let previous = watchdog_snapshot(1, 1, Some("InProgress"), 0, Some(10), Some(100));
        let current = watchdog_snapshot(1, 1, Some("InProgress"), 0, Some(11), Some(100));

        assert_eq!(
            current.progress_signal(&Some(previous)),
            Some("app-server log changed".to_string())
        );
    }

    #[test]
    fn watchdog_state_transitions_through_quiet_suspect_and_stalled() {
        let snapshot = watchdog_snapshot(1, 1, Some("InProgress"), 0, Some(10), Some(100));
        let soft = Duration::from_secs(10);
        let healthy_last_progress = std::time::Instant::now();
        let quiet_last_progress = std::time::Instant::now() - Duration::from_secs(12);
        let suspect_last_progress = std::time::Instant::now() - Duration::from_secs(25);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let stalled_deadline = std::time::Instant::now() - Duration::from_secs(1);

        assert_eq!(
            snapshot.state(&healthy_last_progress, soft, deadline),
            TurnWatchdogState::Healthy
        );
        assert_eq!(
            snapshot.state(&quiet_last_progress, soft, deadline),
            TurnWatchdogState::Quiet
        );
        assert_eq!(
            snapshot.state(&suspect_last_progress, soft, deadline),
            TurnWatchdogState::Suspect
        );
        assert_eq!(
            snapshot.state(&healthy_last_progress, soft, stalled_deadline),
            TurnWatchdogState::Stalled
        );
    }
}
