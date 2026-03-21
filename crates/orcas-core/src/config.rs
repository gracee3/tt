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
            Ok(toml::from_str(&raw)?)
        } else {
            Ok(Self::default())
        }
    }

    pub async fn write_default_if_missing(paths: &AppPaths) -> OrcasResult<Self> {
        paths.ensure().await?;
        let config = Self::load_or_default(paths).await?;
        if !tokio::fs::try_exists(&paths.config_file).await? {
            let raw = toml::to_string_pretty(&config)?;
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexDaemonConfig {
    pub binary_path: PathBuf,
    pub listen_url: String,
    pub connection_mode: CodexConnectionMode,
    pub reconnect: ReconnectPolicy,
    pub config_overrides: Vec<String>,
}

impl Default for CodexDaemonConfig {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::from("/home/emmy/git/codex/codex-rs/target/debug/codex"),
            listen_url: "ws://127.0.0.1:4500".to_string(),
            connection_mode: CodexConnectionMode::SpawnIfNeeded,
            reconnect: ReconnectPolicy::default(),
            config_overrides: Vec::new(),
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

    use super::{
        AppConfig, CodexConnectionMode, DefaultsConfig, ReconnectPolicy, SupervisorConfig,
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
        assert_eq!(config.supervisor.base_url, "https://api.openai.com/v1");
        assert_eq!(config.supervisor.model, "gpt-5.4");
        assert_eq!(config.defaults.model.as_deref(), Some("gpt-5"));
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
