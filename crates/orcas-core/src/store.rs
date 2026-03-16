use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::CollaborationState;
use crate::config::AppConfig;
use crate::error::OrcasResult;
use crate::paths::AppPaths;
use crate::session::{ThreadMetadata, ThreadRegistry};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredState {
    pub registry: ThreadRegistry,
    #[serde(default)]
    pub collaboration: CollaborationState,
}

#[async_trait]
pub trait OrcasSessionStore: Send + Sync {
    async fn load(&self) -> OrcasResult<StoredState>;
    async fn save(&self, state: &StoredState) -> OrcasResult<()>;
    async fn upsert_thread(&self, metadata: ThreadMetadata) -> OrcasResult<()>;
}

#[derive(Debug, Clone)]
pub struct JsonSessionStore {
    paths: AppPaths,
    #[allow(dead_code)]
    config: AppConfig,
}

impl JsonSessionStore {
    pub fn new(paths: AppPaths, config: AppConfig) -> Self {
        Self { paths, config }
    }
}

#[async_trait]
impl OrcasSessionStore for JsonSessionStore {
    async fn load(&self) -> OrcasResult<StoredState> {
        self.paths.ensure().await?;
        if tokio::fs::try_exists(&self.paths.state_file).await? {
            let raw = tokio::fs::read_to_string(&self.paths.state_file).await?;
            Ok(serde_json::from_str(&raw)?)
        } else {
            Ok(StoredState::default())
        }
    }

    async fn save(&self, state: &StoredState) -> OrcasResult<()> {
        self.paths.ensure().await?;
        let raw = serde_json::to_string_pretty(state)?;
        tokio::fs::write(&self.paths.state_file, raw).await?;
        Ok(())
    }

    async fn upsert_thread(&self, metadata: ThreadMetadata) -> OrcasResult<()> {
        let mut state = self.load().await?;
        state.registry.upsert(metadata);
        self.save(&state).await
    }
}
