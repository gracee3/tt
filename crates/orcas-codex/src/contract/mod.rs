use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod cli;
pub mod config;
pub mod protocol;
mod schema;

pub use cli::{CliArg, CliArgKind, CliCommand, CliCommandKind, CliContract};
pub use config::ConfigContract;
pub use protocol::{ProtocolContract, ProtocolMethod};
pub use schema::{SchemaContract, SchemaDefinition, SchemaKind, SchemaNode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractSource {
    pub codex_repo_root: String,
    pub codex_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexContractSnapshot {
    pub source: ContractSource,
    pub config: config::ConfigContract,
    pub cli: cli::CliContract,
    pub protocol: protocol::ProtocolContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexContractIndex {
    pub source: ContractSource,
    pub config_paths: Vec<String>,
    pub cli_command_paths: Vec<String>,
    pub cli_arg_paths: Vec<String>,
    pub protocol_methods: Vec<String>,
}

impl CodexContractSnapshot {
    pub fn load_from_codex_repo(root: impl AsRef<Path>) -> Result<Self, ContractError> {
        let root = root.as_ref();
        let source = ContractSource {
            codex_repo_root: root.display().to_string(),
            codex_commit: git_commit(root),
        };
        Ok(Self {
            source,
            config: config::load_contract(root)?,
            cli: cli::load_contract(root)?,
            protocol: protocol::load_contract(root)?,
        })
    }

    pub fn to_pretty_json(&self) -> Result<String, ContractError> {
        serde_json::to_string_pretty(self).map_err(ContractError::from)
    }

    pub fn write_pretty_json(&self, path: impl AsRef<Path>) -> Result<(), ContractError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ContractError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(path, self.to_pretty_json()?).map_err(|source| ContractError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    pub fn inventory(&self) -> CodexContractIndex {
        CodexContractIndex {
            source: self.source.clone(),
            config_paths: self.config.key_paths(),
            cli_command_paths: self.cli.command_paths(),
            cli_arg_paths: self.cli.arg_paths(),
            protocol_methods: self.protocol.method_names(),
        }
    }
}

impl CodexContractIndex {
    pub fn to_pretty_json(&self) -> Result<String, ContractError> {
        serde_json::to_string_pretty(self).map_err(ContractError::from)
    }

    pub fn write_pretty_json(&self, path: impl AsRef<Path>) -> Result<(), ContractError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ContractError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(path, self.to_pretty_json()?).map_err(|source| ContractError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn codex_repo_root() -> PathBuf {
        std::env::var_os("CODEX_REPO_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/home/emmy/openai/codex/codex-rs"))
    }

    #[test]
    fn contract_index_matches_current_codex_checkout() {
        let root = codex_repo_root();
        assert!(
            root.join("core/config.schema.json").exists(),
            "expected Codex checkout at {}",
            root.display()
        );

        let snapshot =
            CodexContractSnapshot::load_from_codex_repo(&root).expect("load live Codex contract");
        let expected: CodexContractIndex =
            serde_json::from_str(include_str!("../../contracts/codex-contract-index.json"))
                .expect("parse checked-in contract index");

        assert_eq!(expected, snapshot.inventory());
    }

    #[test]
    fn contract_index_contains_key_paths() {
        let root = codex_repo_root();
        let snapshot =
            CodexContractSnapshot::load_from_codex_repo(&root).expect("load live Codex contract");
        let index = snapshot.inventory();

        assert!(
            index
                .cli_command_paths
                .contains(&"codex exec resume".to_string())
        );
        assert!(
            index
                .cli_command_paths
                .contains(&"codex app-server generate-json-schema".to_string())
        );
        assert!(
            index
                .config_paths
                .contains(&"analytics.enabled".to_string())
        );
        assert!(
            index
                .protocol_methods
                .contains(&"client_request:thread/start".to_string())
        );
        assert!(
            index
                .protocol_methods
                .contains(&"client_notification:initialized".to_string())
        );
    }
}

fn git_commit(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!commit.is_empty()).then_some(commit)
}

#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse json schema {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to serialize contract json: {source}")]
    Serialize {
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to parse Rust source {path}: {source}")]
    Syn {
        path: PathBuf,
        #[source]
        source: syn::Error,
    },
    #[error("unsupported source file {0}")]
    Unsupported(String),
}

pub(crate) fn read_to_string(path: impl AsRef<Path>) -> Result<String, ContractError> {
    let path = path.as_ref();
    fs::read_to_string(path).map_err(|source| ContractError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn read_json(path: impl AsRef<Path>) -> Result<serde_json::Value, ContractError> {
    let path = path.as_ref();
    let raw = read_to_string(path)?;
    serde_json::from_str(&raw).map_err(|source| ContractError::Json {
        path: path.to_path_buf(),
        source,
    })
}

impl From<serde_json::Error> for ContractError {
    fn from(source: serde_json::Error) -> Self {
        ContractError::Serialize { source }
    }
}
