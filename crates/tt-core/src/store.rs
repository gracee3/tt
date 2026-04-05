use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::CollaborationState;
use crate::config::AppConfig;
use crate::error::TTResult;
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

impl StoredState {
    pub fn from_json_str_with_normalization(raw: &str) -> TTResult<(Self, bool)> {
        let parsed = serde_json::from_str::<Value>(raw)?;
        let state = serde_json::from_value::<StoredState>(parsed.clone())?;
        let normalized = serde_json::to_value(&state)?;
        Ok((state, parsed != normalized))
    }

    pub fn from_json_str(raw: &str) -> TTResult<Self> {
        Ok(Self::from_json_str_with_normalization(raw)?.0)
    }

    pub fn to_pretty_json(&self) -> TTResult<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

#[async_trait]
pub trait TTSessionStore: Send + Sync {
    async fn load(&self) -> TTResult<StoredState>;
    async fn save(&self, state: &StoredState) -> TTResult<()>;
    async fn upsert_thread(&self, metadata: ThreadMetadata) -> TTResult<()>;
    async fn upsert_thread_view(&self, thread: ThreadView) -> TTResult<()>;
    async fn upsert_turn_state(&self, turn: TurnStateView) -> TTResult<()>;
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

    pub async fn load_with_normalization_flag(&self) -> TTResult<(StoredState, bool)> {
        self.paths.ensure().await?;
        if tokio::fs::try_exists(&self.paths.state_file).await? {
            let raw = tokio::fs::read_to_string(&self.paths.state_file).await?;
            StoredState::from_json_str_with_normalization(&raw)
        } else {
            Ok((StoredState::default(), false))
        }
    }
}

#[async_trait]
impl TTSessionStore for JsonSessionStore {
    async fn load(&self) -> TTResult<StoredState> {
        Ok(self.load_with_normalization_flag().await?.0)
    }

    async fn save(&self, state: &StoredState) -> TTResult<()> {
        self.paths.ensure().await?;
        let mut raw = state.to_pretty_json()?;
        raw.push('\n');
        tokio::fs::write(&self.paths.state_file, raw).await?;
        Ok(())
    }

    async fn upsert_thread(&self, metadata: ThreadMetadata) -> TTResult<()> {
        let mut state = self.load().await?;
        state.registry.upsert(metadata);
        self.save(&state).await
    }

    async fn upsert_thread_view(&self, thread: ThreadView) -> TTResult<()> {
        let mut state = self.load().await?;
        state.thread_views.insert(thread.summary.id.clone(), thread);
        self.save(&state).await
    }

    async fn upsert_turn_state(&self, turn: TurnStateView) -> TTResult<()> {
        let mut state = self.load().await?;
        state
            .turn_states
            .insert(format!("{}::{}", turn.thread_id, turn.turn_id), turn);
        self.save(&state).await
    }
}

#[cfg(test)]
mod tests {
    use super::StoredState;

    #[test]
    fn stored_state_loader_defaults_missing_sections() {
        let (state, needs_normalization) = StoredState::from_json_str_with_normalization(
            r#"{
  "registry": {
    "threads": {},
    "last_connected_endpoint": null
  }
}"#,
        )
        .expect("stored state should deserialize");

        assert!(needs_normalization);
        assert!(state.thread_views.is_empty());
        assert!(state.turn_states.is_empty());
        assert!(state.collaboration.workstreams.is_empty());
        assert!(state.operator_inbox_mirrors.is_empty());
    }

    #[test]
    fn stored_state_json_round_trips_through_shared_helpers() {
        let state = StoredState::from_json_str(
            r#"{
  "registry": {
    "threads": {},
    "last_connected_endpoint": null
  },
  "collaboration": {
    "workstreams": {
      "ws-1": {
        "id": "ws-1",
        "title": "Seeded",
        "objective": "Round trip",
        "status": "active",
        "priority": "high",
        "created_at": "2026-03-21T01:00:00Z",
        "updated_at": "2026-03-21T01:00:00Z"
      }
    }
  }
}"#,
        )
        .expect("stored state should deserialize");

        let encoded = state
            .to_pretty_json()
            .expect("stored state should serialize");
        let decoded =
            StoredState::from_json_str(&encoded).expect("serialized state should deserialize");

        assert_eq!(decoded.collaboration.workstreams.len(), 1);
        assert!(decoded.thread_views.is_empty());
    }

    #[test]
    fn canonical_state_json_does_not_need_normalization() {
        let canonical = StoredState::default()
            .to_pretty_json()
            .expect("stored state should serialize");
        let (_, needs_normalization) = StoredState::from_json_str_with_normalization(&canonical)
            .expect("stored state should deserialize");

        assert!(!needs_normalization);
    }
}
