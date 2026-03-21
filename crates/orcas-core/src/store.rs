use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::CollaborationState;
use crate::config::AppConfig;
use crate::error::OrcasResult;
use crate::ipc::{OperatorInboxMirrorCheckpoint, OperatorInboxState, ThreadView, TurnStateView};
use crate::paths::AppPaths;
use crate::session::{ThreadMetadata, ThreadRegistry};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredState {
    pub registry: ThreadRegistry,
    #[serde(default)]
    pub thread_views: BTreeMap<String, ThreadView>,
    #[serde(default)]
    pub turn_states: BTreeMap<String, TurnStateView>,
    #[serde(default)]
    pub collaboration: CollaborationState,
    #[serde(default)]
    pub operator_inbox: OperatorInboxState,
    #[serde(default)]
    pub operator_inbox_mirrors: BTreeMap<String, OperatorInboxMirrorCheckpoint>,
}

#[async_trait]
pub trait OrcasSessionStore: Send + Sync {
    async fn load(&self) -> OrcasResult<StoredState>;
    async fn save(&self, state: &StoredState) -> OrcasResult<()>;
    async fn upsert_thread(&self, metadata: ThreadMetadata) -> OrcasResult<()>;
    async fn upsert_thread_view(&self, thread: ThreadView) -> OrcasResult<()>;
    async fn upsert_turn_state(&self, turn: TurnStateView) -> OrcasResult<()>;
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

    async fn upsert_thread_view(&self, thread: ThreadView) -> OrcasResult<()> {
        let mut state = self.load().await?;
        state.thread_views.insert(thread.summary.id.clone(), thread);
        self.save(&state).await
    }

    async fn upsert_turn_state(&self, turn: TurnStateView) -> OrcasResult<()> {
        let mut state = self.load().await?;
        state
            .turn_states
            .insert(format!("{}::{}", turn.thread_id, turn.turn_id), turn);
        self.save(&state).await
    }
}
