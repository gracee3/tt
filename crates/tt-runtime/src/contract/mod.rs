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
    pub tt_repo_root: String,
    pub tt_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TTContractSnapshot {
    pub source: ContractSource,
    pub config: config::ConfigContract,
    pub cli: cli::CliContract,
    pub protocol: protocol::ProtocolContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TTContractIndex {
    pub source: ContractSource,
    pub config_paths: Vec<String>,
    pub cli_command_paths: Vec<String>,
    pub cli_arg_paths: Vec<String>,
    pub protocol_methods: Vec<String>,
}

pub fn default_tt_repo_root() -> PathBuf {
    tt_repo_root_from_manifest_dir()
}

pub fn discover_tt_source_root(start: impl AsRef<Path>) -> Option<PathBuf> {
    let start = start.as_ref();
    for ancestor in start.ancestors().take(4) {
        if let Some(root) = search_for_tt_repo_root(ancestor, 2) {
            return Some(root);
        }
    }
    None
}

fn search_for_tt_repo_root(dir: &Path, remaining_depth: usize) -> Option<PathBuf> {
    if is_tt_repo_root(dir) {
        return Some(dir.to_path_buf());
    }
    if remaining_depth == 0 {
        return None;
    }
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(root) = search_for_tt_repo_root(&path, remaining_depth - 1) {
                return Some(root);
            }
        }
    }
    None
}

fn is_tt_repo_root(dir: &Path) -> bool {
    dir.join("core/config.schema.json").is_file() && dir.join("cli/src/main.rs").is_file()
}

fn tt_repo_root_from_manifest_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or(manifest_dir)
}

impl TTContractSnapshot {
    pub fn load_from_tt_repo(root: impl AsRef<Path>) -> Result<Self, ContractError> {
        let root = root.as_ref();
        let load_root = discover_tt_source_root(root).unwrap_or_else(|| root.to_path_buf());
        let source = ContractSource {
            tt_repo_root: root.display().to_string(),
            tt_commit: git_commit(root),
        };
        Ok(Self {
            source,
            config: config::load_contract(&load_root)?,
            cli: cli::load_contract(&load_root)?,
            protocol: protocol::load_contract(&load_root)?,
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

    pub fn inventory(&self) -> TTContractIndex {
        TTContractIndex {
            source: self.source.clone(),
            config_paths: self.config.key_paths(),
            cli_command_paths: self.cli.command_paths(),
            cli_arg_paths: self.cli.arg_paths(),
            protocol_methods: self.protocol.method_names(),
        }
    }
}

impl TTContractIndex {
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

    fn tt_repo_root() -> PathBuf {
        std::env::var_os("RUNTIME_REPO_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(default_tt_repo_root)
    }

    fn tt_source_root() -> PathBuf {
        discover_tt_source_root(tt_repo_root()).unwrap_or_else(tt_repo_root)
    }

    #[test]
    fn contract_index_matches_current_tt_checkout() {
        let root = tt_repo_root();
        let source_root = tt_source_root();
        assert!(
            source_root.join("core/config.schema.json").exists(),
            "expected TT contract source checkout at {}",
            source_root.display()
        );

        let snapshot = TTContractSnapshot::load_from_tt_repo(&root).expect("load live TT contract");
        let expected: TTContractIndex =
            serde_json::from_str(include_str!("../../contracts/tt-contract-index.json"))
                .expect("parse checked-in contract index");

        assert_eq!(expected, snapshot.inventory());
    }

    #[test]
    fn contract_index_contains_key_paths() {
        let root = tt_repo_root();
        let snapshot = TTContractSnapshot::load_from_tt_repo(&root).expect("load live TT contract");
        let index = snapshot.inventory();

        assert!(
            index
                .cli_command_paths
                .contains(&"tt exec resume".to_string())
        );
        assert!(
            index
                .cli_command_paths
                .contains(&"tt app-server generate-json-schema".to_string())
        );
        assert!(
            index
                .config_paths
                .contains(&"profiles.analytics.enabled".to_string())
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
