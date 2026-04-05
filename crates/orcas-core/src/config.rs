use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{OrcasError, OrcasResult};
use crate::paths::AppPaths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub codex: CodexDaemonConfig,
    #[serde(default)]
    pub supervisor: SupervisorConfig,
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub inbox_mirror: InboxMirrorConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            codex: CodexDaemonConfig::default(),
            supervisor: SupervisorConfig::default(),
            defaults: DefaultsConfig::default(),
            inbox_mirror: InboxMirrorConfig::default(),
        }
    }
}

impl AppConfig {
    pub async fn load_or_default(paths: &AppPaths) -> OrcasResult<Self> {
        if tokio::fs::try_exists(&paths.config_file).await? {
            let raw = tokio::fs::read_to_string(&paths.config_file).await?;
            Ok(Self::normalize_loaded_config(toml::from_str::<Self>(&raw)?))
        } else {
            Ok(Self::default())
        }
    }

    pub async fn write_default_if_missing(paths: &AppPaths) -> OrcasResult<Self> {
        paths.ensure().await?;
        let config = Self::load_or_default(paths).await?;
        if !tokio::fs::try_exists(&paths.config_file).await? {
            let raw = config.render_default_toml();
            tokio::fs::write(&paths.config_file, raw).await?;
        }
        Ok(config)
    }

    pub fn resolve_codex_bin(&self) -> OrcasResult<PathBuf> {
        if self.codex.binary_path.as_os_str().is_empty() {
            return Err(OrcasError::Config(
                "codex.binary_path must be set to a concrete local build path".to_string(),
            ));
        }
        Ok(self.codex.binary_path.clone())
    }

    fn normalize_loaded_config(mut config: Self) -> Self {
        let default_listen_url = default_codex_listen_url();
        if config.codex.listen_url.trim().is_empty()
            || (config.codex.listen_url == default_listen_url
                && !config.codex.app_server.default.listen_url.trim().is_empty()
                && config.codex.app_server.default.listen_url != default_listen_url)
        {
            config.codex.listen_url = config.codex.app_server.default.listen_url.clone();
        }
        if config.codex.app_server.default.listen_url.trim().is_empty() {
            config.codex.app_server.default.listen_url = config.codex.listen_url.clone();
        }
        config
    }

    fn render_default_toml(&self) -> String {
        format!(
            r#"# Orcas configuration
#
# Orcas runs one host/home daemon and connects it to one shared Codex app-server.
# The recommended lifecycle is `orcas app-server ...`, with Codex/OpenAI auth
# coming from the host environment unless explicitly overridden.

[codex]
binary_path = "{binary_path}"
connection_mode = "connect_only"
config_overrides = []

[codex.reconnect]
initial_delay_ms = {initial_delay_ms}
max_delay_ms = {max_delay_ms}
multiplier = {multiplier}

[codex.app_server.default]
enabled = {app_server_enabled}
owner = "{app_server_owner}"
transport = "{app_server_transport}"
listen_url = "{listen_url}"

[codex.responses]
base_url = "https://api.openai.com/v1"

[codex.direct_api]
# auth_file = "~/.codex/auth.json"

[codex.profiles.local]
model_provider = "vllm"
model = "local-model"

[codex.model_providers.vllm]
name = "vLLM"
base_url = "http://127.0.0.1:8000/v1"
wire_api = "responses"

[supervisor]
base_url = "{supervisor_base_url}"
api_key_env = "{supervisor_api_key_env}"
model = "{supervisor_model}"
reasoning_effort = "{supervisor_reasoning_effort}"
max_output_tokens = {supervisor_max_output_tokens}

[supervisor.proposals]
auto_create_on_report_recorded = {auto_create_on_report_recorded}

[defaults]
# cwd = "/path/to/default/repo"
model = "{default_model}"
"#,
            binary_path = self.codex.binary_path.display(),
            initial_delay_ms = self.codex.reconnect.initial_delay_ms,
            max_delay_ms = self.codex.reconnect.max_delay_ms,
            multiplier = self.codex.reconnect.multiplier,
            app_server_enabled = self.codex.effective_app_server().enabled,
            app_server_owner = self.codex.effective_app_server().owner.as_str(),
            app_server_transport = self.codex.effective_app_server().transport.as_str(),
            listen_url = self.codex.effective_listen_url(),
            supervisor_base_url = self.supervisor.base_url,
            supervisor_api_key_env = self.supervisor.api_key_env,
            supervisor_model = self.supervisor.model,
            supervisor_reasoning_effort = self.supervisor.reasoning_effort,
            supervisor_max_output_tokens = self.supervisor.max_output_tokens,
            auto_create_on_report_recorded = self.supervisor.proposals.auto_create_on_report_recorded,
            default_model = self.defaults.model.as_deref().unwrap_or("gpt-5"),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexDaemonConfig {
    pub binary_path: PathBuf,
    #[serde(default = "default_codex_listen_url")]
    pub listen_url: String,
    #[serde(default)]
    pub connection_mode: CodexConnectionMode,
    #[serde(default)]
    pub reconnect: ReconnectPolicy,
    #[serde(default)]
    pub config_overrides: Vec<String>,
    #[serde(default)]
    pub app_server: CodexAppServerConfig,
    #[serde(default)]
    pub responses: CodexResponsesConfig,
    #[serde(default)]
    pub direct_api: CodexDirectApiConfig,
    #[serde(default)]
    pub profiles: BTreeMap<String, CodexProfileConfig>,
    #[serde(default)]
    pub model_providers: BTreeMap<String, CodexModelProviderConfig>,
}

impl Default for CodexDaemonConfig {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::from("/home/emmy/git/codex/codex-rs/target/debug/codex"),
            listen_url: default_codex_listen_url(),
            connection_mode: CodexConnectionMode::ConnectOnly,
            reconnect: ReconnectPolicy::default(),
            config_overrides: Vec::new(),
            app_server: CodexAppServerConfig::default(),
            responses: CodexResponsesConfig::default(),
            direct_api: CodexDirectApiConfig::default(),
            profiles: BTreeMap::new(),
            model_providers: BTreeMap::new(),
        }
    }
}

impl CodexDaemonConfig {
    pub fn effective_app_server(&self) -> &CodexNamedAppServerConfig {
        &self.app_server.default
    }

    pub fn effective_listen_url(&self) -> &str {
        if self.app_server.default.listen_url.trim().is_empty() {
            &self.listen_url
        } else {
            &self.app_server.default.listen_url
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexConnectionMode {
    ConnectOnly,
    SpawnIfNeeded,
    SpawnAlways,
}

impl Default for CodexConnectionMode {
    fn default() -> Self {
        Self::ConnectOnly
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAppServerConfig {
    #[serde(default)]
    pub default: CodexNamedAppServerConfig,
}

impl Default for CodexAppServerConfig {
    fn default() -> Self {
        Self {
            default: CodexNamedAppServerConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexNamedAppServerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub owner: CodexAppServerOwner,
    #[serde(default)]
    pub transport: CodexAppServerTransport,
    pub listen_url: String,
}

impl Default for CodexNamedAppServerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            owner: CodexAppServerOwner::default(),
            transport: CodexAppServerTransport::default(),
            listen_url: default_codex_listen_url(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexAppServerOwner {
    #[default]
    Orcas,
    Systemd,
    External,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodexAppServerTransport {
    Stdio,
    #[default]
    Websocket,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodexResponsesConfig {
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodexDirectApiConfig {
    #[serde(default)]
    pub auth_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CodexProfileConfig {
    #[serde(default)]
    pub model_provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub wire_api: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CodexModelProviderConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub wire_api: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_codex_listen_url() -> String {
    "ws://127.0.0.1:4500".to_string()
}

impl CodexAppServerOwner {
    fn as_str(self) -> &'static str {
        match self {
            Self::Orcas => "orcas",
            Self::Systemd => "systemd",
            Self::External => "external",
        }
    }
}

impl CodexAppServerTransport {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Websocket => "websocket",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectPolicy {
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub multiplier: f64,
    pub max_attempts: Option<u32>,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay_ms: 150,
            max_delay_ms: 5_000,
            multiplier: 2.0,
            max_attempts: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsConfig {
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            cwd: None,
            model: Some("gpt-5".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxMirrorConfig {
    pub server_url: Option<String>,
    #[serde(default)]
    pub operator_api_token: Option<String>,
}

impl Default for InboxMirrorConfig {
    fn default() -> Self {
        Self {
            server_url: None,
            operator_api_token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorConfig {
    pub base_url: String,
    pub api_key_env: String,
    pub model: String,
    pub reasoning_effort: String,
    #[serde(default)]
    pub temperature: Option<f64>,
    pub max_output_tokens: u32,
    #[serde(default)]
    pub proposals: SupervisorProposalConfig,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key_env: "OPENAI_API_KEY".to_string(),
            model: "gpt-5.4".to_string(),
            reasoning_effort: "high".to_string(),
            temperature: None,
            max_output_tokens: 2_000,
            proposals: SupervisorProposalConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SupervisorProposalConfig {
    #[serde(default)]
    pub auto_create_on_report_recorded: bool,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;
    use uuid::Uuid;

    use crate::AppPaths;

    use super::{
        AppConfig, CodexAppServerOwner, CodexConnectionMode, CodexModelProviderConfig,
        CodexProfileConfig, DefaultsConfig, ReconnectPolicy, SupervisorConfig,
        SupervisorProposalConfig,
    };

    #[test]
    fn app_config_sparse_toml_uses_defaults_for_missing_sections() {
        let config = toml::from_str::<AppConfig>(
            r#"
            [codex]
            binary_path = "/tmp/codex"
            listen_url = "ws://127.0.0.1:4500"
            connection_mode = "connect_only"
            config_overrides = []

            [codex.reconnect]
            initial_delay_ms = 150
            max_delay_ms = 5000
            multiplier = 2.0
            "#,
        )
        .expect("deserialize sparse app config");

        assert_eq!(config.codex.binary_path, PathBuf::from("/tmp/codex"));
        assert_eq!(
            config.codex.connection_mode,
            CodexConnectionMode::ConnectOnly
        );
        assert_eq!(
            config.codex.app_server.default.owner,
            CodexAppServerOwner::Orcas
        );
        assert_eq!(config.supervisor.base_url, "https://api.openai.com/v1");
        assert_eq!(config.supervisor.model, "gpt-5.4");
        assert_eq!(config.defaults.model.as_deref(), Some("gpt-5"));
    }

    #[tokio::test]
    async fn write_default_if_missing_emits_nested_shared_runtime_config() {
        let root = std::env::temp_dir().join(format!("orcas-config-test-{}", Uuid::new_v4()));
        let paths = AppPaths::from_home(root.clone());

        let config = AppConfig::write_default_if_missing(&paths)
            .await
            .expect("write default config");
        let raw = tokio::fs::read_to_string(&paths.config_file)
            .await
            .expect("read config file");

        assert_eq!(config.codex.connection_mode, CodexConnectionMode::ConnectOnly);
        assert!(raw.contains("[codex.app_server.default]"));
        assert!(raw.contains("connection_mode = \"connect_only\""));
        assert!(raw.contains("owner = \"orcas\""));
        assert!(raw.contains("[codex.profiles.local]"));
        assert!(!raw.contains("spawn_if_needed"));

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn load_or_default_normalizes_flat_listen_url_into_nested_app_server() {
        let root = std::env::temp_dir().join(format!("orcas-config-test-{}", Uuid::new_v4()));
        let paths = AppPaths::from_home(root.clone());
        paths.ensure().await.expect("create config dirs");
        tokio::fs::write(
            &paths.config_file,
            r#"
            [codex]
            binary_path = "/tmp/codex"
            listen_url = "ws://127.0.0.1:4900"
            connection_mode = "connect_only"
            config_overrides = []

            [codex.reconnect]
            initial_delay_ms = 150
            max_delay_ms = 5000
            multiplier = 2.0

            [codex.app_server.default]
            enabled = true
            owner = "orcas"
            transport = "websocket"
            listen_url = ""
            "#,
        )
        .await
        .expect("write flat compatibility config");

        let config = AppConfig::load_or_default(&paths)
            .await
            .expect("load config");
        assert_eq!(config.codex.listen_url, "ws://127.0.0.1:4900");
        assert_eq!(
            config.codex.app_server.default.listen_url,
            "ws://127.0.0.1:4900"
        );
        assert_eq!(config.codex.effective_listen_url(), "ws://127.0.0.1:4900");

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn codex_nested_profiles_round_trip() {
        let config = toml::from_str::<AppConfig>(
            r#"
            [codex]
            binary_path = "/tmp/codex"
            listen_url = "ws://127.0.0.1:4500"
            connection_mode = "connect_only"
            config_overrides = []

            [codex.app_server.default]
            owner = "external"
            transport = "websocket"
            listen_url = "ws://127.0.0.1:4600"

            [codex.profiles.local]
            model_provider = "vllm"
            model = "local-model"

            [codex.model_providers.vllm]
            name = "vLLM"
            base_url = "http://127.0.0.1:8000/v1"
            wire_api = "responses"
            "#,
        )
        .expect("deserialize nested codex config");

        assert_eq!(config.codex.effective_listen_url(), "ws://127.0.0.1:4600");
        assert_eq!(
            config.codex.profiles.get("local"),
            Some(&CodexProfileConfig {
                model_provider: Some("vllm".to_string()),
                model: Some("local-model".to_string()),
                base_url: None,
                wire_api: None,
            })
        );
        assert_eq!(
            config.codex.model_providers.get("vllm"),
            Some(&CodexModelProviderConfig {
                name: Some("vLLM".to_string()),
                base_url: Some("http://127.0.0.1:8000/v1".to_string()),
                wire_api: Some("responses".to_string()),
            })
        );
    }

    #[test]
    fn nested_public_shape_can_omit_flat_listen_url() {
        let config = AppConfig::normalize_loaded_config(
            toml::from_str::<AppConfig>(
                r#"
                [codex]
                binary_path = "/tmp/codex"
                connection_mode = "connect_only"
                config_overrides = []

                [codex.reconnect]
                initial_delay_ms = 150
                max_delay_ms = 5000
                multiplier = 2.0

                [codex.app_server.default]
                owner = "orcas"
                transport = "websocket"
                listen_url = "ws://127.0.0.1:4700"
                "#,
            )
            .expect("deserialize nested public shape"),
        );

        assert_eq!(config.codex.listen_url, "ws://127.0.0.1:4700");
        assert_eq!(config.codex.effective_listen_url(), "ws://127.0.0.1:4700");
    }

    #[test]
    fn reconnect_policy_round_trips_optional_attempt_limit() {
        let policy = ReconnectPolicy {
            initial_delay_ms: 250,
            max_delay_ms: 9_000,
            multiplier: 1.5,
            max_attempts: Some(7),
        };

        let value = serde_json::to_value(&policy).expect("serialize reconnect policy");
        assert_eq!(value["initial_delay_ms"], 250);
        assert_eq!(value["max_attempts"], 7);

        let round_trip =
            serde_json::from_value::<ReconnectPolicy>(value).expect("deserialize reconnect policy");
        assert_eq!(round_trip.initial_delay_ms, 250);
        assert_eq!(round_trip.max_delay_ms, 9_000);
        assert_eq!(round_trip.multiplier, 1.5);
        assert_eq!(round_trip.max_attempts, Some(7));
    }

    #[test]
    fn defaults_config_round_trips_optional_path_and_model() {
        let defaults = DefaultsConfig {
            cwd: Some(PathBuf::from("/repo")),
            model: Some("gpt-5.4-mini".to_string()),
        };

        let value = serde_json::to_value(&defaults).expect("serialize defaults config");
        assert_eq!(value["cwd"], "/repo");
        assert_eq!(value["model"], "gpt-5.4-mini");

        let round_trip =
            serde_json::from_value::<DefaultsConfig>(value).expect("deserialize defaults config");
        assert_eq!(round_trip.cwd, Some(PathBuf::from("/repo")));
        assert_eq!(round_trip.model.as_deref(), Some("gpt-5.4-mini"));
    }

    #[test]
    fn supervisor_config_defaults_nested_proposals_when_omitted() {
        let config = serde_json::from_value::<SupervisorConfig>(json!({
            "base_url": "https://example.invalid/v1",
            "api_key_env": "EXAMPLE_API_KEY",
            "model": "gpt-test",
            "reasoning_effort": "medium",
            "temperature": 0.2,
            "max_output_tokens": 512
        }))
        .expect("deserialize supervisor config");

        assert_eq!(config.base_url, "https://example.invalid/v1");
        assert_eq!(config.proposals.auto_create_on_report_recorded, false);
    }

    #[test]
    fn supervisor_proposal_config_defaults_auto_create_to_false() {
        let config = serde_json::from_value::<SupervisorProposalConfig>(json!({}))
            .expect("deserialize proposal config");
        assert!(!config.auto_create_on_report_recorded);
    }
}
