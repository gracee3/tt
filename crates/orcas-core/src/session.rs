use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThreadRegistry {
    pub threads: BTreeMap<String, ThreadMetadata>,
    pub last_connected_endpoint: Option<String>,
}

impl ThreadRegistry {
    pub fn upsert(&mut self, metadata: ThreadMetadata) {
        self.threads.insert(metadata.id.clone(), metadata);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMetadata {
    pub id: String,
    pub name: Option<String>,
    pub preview: String,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub cwd: Option<PathBuf>,
    pub endpoint: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub recent_output: Option<String>,
    #[serde(default)]
    pub recent_event: Option<String>,
    #[serde(default)]
    pub turn_in_flight: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadDescriptor {
    pub id: String,
    pub model: Option<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnDescriptor {
    pub thread_id: String,
    pub turn_id: String,
    pub status: String,
}
