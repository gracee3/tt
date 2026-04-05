use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ipc::{ThreadLoadedStatus, ThreadManagementState, ThreadMonitorState};

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
    #[serde(default)]
    pub runtime_workstream_id: Option<String>,
    #[serde(default)]
    pub owner_workstream_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub loaded_status: ThreadLoadedStatus,
    #[serde(default)]
    pub active_flags: Vec<String>,
    #[serde(default)]
    pub active_turn_id: Option<String>,
    #[serde(default)]
    pub last_seen_turn_id: Option<String>,
    #[serde(default)]
    pub recent_output: Option<String>,
    #[serde(default)]
    pub recent_event: Option<String>,
    #[serde(default)]
    pub turn_in_flight: bool,
    #[serde(default)]
    pub monitor_state: ThreadMonitorState,
    #[serde(default = "Utc::now")]
    pub last_sync_at: DateTime<Utc>,
    #[serde(default)]
    pub management_state: ThreadManagementState,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub raw_summary: Option<Value>,
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::{ThreadDescriptor, ThreadMetadata, ThreadRegistry, TurnDescriptor};
    use crate::ipc::{ThreadLoadedStatus, ThreadManagementState, ThreadMonitorState};

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 6, 7, 8, 9, 10)
            .single()
            .expect("valid timestamp")
    }

    #[test]
    fn thread_registry_defaults_missing_threads_and_endpoint() {
        let registry = serde_json::from_value::<ThreadRegistry>(json!({
            "threads": {}
        }))
        .expect("deserialize registry");

        assert!(registry.threads.is_empty());
        assert!(registry.last_connected_endpoint.is_none());
    }

    #[test]
    fn thread_metadata_defaults_additive_fields_when_missing() {
        let metadata = serde_json::from_value::<ThreadMetadata>(json!({
            "id": "thread-1",
            "name": "Thread",
            "preview": "Preview",
            "model": "gpt-5",
            "model_provider": "openai",
            "cwd": "/repo",
            "endpoint": "ws://127.0.0.1:4500",
            "created_at": fixed_now(),
            "updated_at": fixed_now(),
            "status": "idle"
        }))
        .expect("deserialize thread metadata");

        assert_eq!(metadata.scope, "");
        assert!(!metadata.archived);
        assert_eq!(metadata.loaded_status, ThreadLoadedStatus::Unknown);
        assert!(metadata.active_flags.is_empty());
        assert!(metadata.active_turn_id.is_none());
        assert!(metadata.last_seen_turn_id.is_none());
        assert!(metadata.recent_output.is_none());
        assert!(metadata.recent_event.is_none());
        assert!(!metadata.turn_in_flight);
        assert_eq!(metadata.monitor_state, ThreadMonitorState::Detached);
        assert_eq!(
            metadata.management_state,
            ThreadManagementState::ObservedUnmanaged
        );
        assert!(metadata.owner_workstream_id.is_none());
        assert!(metadata.source_kind.is_none());
        assert!(metadata.raw_summary.is_none());
    }

    #[test]
    fn thread_metadata_round_trips_nested_optional_fields() {
        let metadata = ThreadMetadata {
            id: "thread-1".to_string(),
            name: Some("Thread".to_string()),
            preview: "Preview".to_string(),
            model: Some("gpt-5".to_string()),
            model_provider: Some("openai".to_string()),
            cwd: Some(PathBuf::from("/repo")),
            endpoint: Some("ws://127.0.0.1:4500".to_string()),
            runtime_workstream_id: Some("runtime-ws".to_string()),
            owner_workstream_id: Some("owner-ws".to_string()),
            created_at: fixed_now(),
            updated_at: fixed_now(),
            status: "active".to_string(),
            scope: "workspace".to_string(),
            archived: true,
            loaded_status: ThreadLoadedStatus::Active,
            active_flags: vec!["turn_running".to_string()],
            active_turn_id: Some("turn-1".to_string()),
            last_seen_turn_id: Some("turn-0".to_string()),
            recent_output: Some("delta".to_string()),
            recent_event: Some("turn_updated".to_string()),
            turn_in_flight: true,
            monitor_state: ThreadMonitorState::Attached,
            last_sync_at: fixed_now(),
            management_state: ThreadManagementState::Managed,
            source_kind: Some("cli".to_string()),
            raw_summary: Some(json!({"extra": true})),
        };

        let value = serde_json::to_value(&metadata).expect("serialize metadata");
        assert_eq!(value["loaded_status"], "active");
        assert_eq!(value["monitor_state"], "attached");
        assert_eq!(value["active_flags"][0], "turn_running");
        assert_eq!(value["raw_summary"]["extra"], true);

        let round_trip =
            serde_json::from_value::<ThreadMetadata>(value).expect("deserialize metadata");
        assert_eq!(round_trip.loaded_status, ThreadLoadedStatus::Active);
        assert_eq!(round_trip.monitor_state, ThreadMonitorState::Attached);
        assert_eq!(round_trip.management_state, ThreadManagementState::Managed);
        assert_eq!(round_trip.cwd, Some(PathBuf::from("/repo")));
        assert_eq!(
            round_trip.runtime_workstream_id.as_deref(),
            Some("runtime-ws")
        );
        assert_eq!(round_trip.owner_workstream_id.as_deref(), Some("owner-ws"));
        assert_eq!(round_trip.source_kind.as_deref(), Some("cli"));
        assert_eq!(round_trip.raw_summary, Some(json!({"extra": true})));
    }

    #[test]
    fn thread_and_turn_descriptors_round_trip_optional_path_shape() {
        let thread = ThreadDescriptor {
            id: "thread-1".to_string(),
            model: Some("gpt-5".to_string()),
            cwd: Some(PathBuf::from("/repo")),
        };
        let turn = TurnDescriptor {
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
            status: "completed".to_string(),
        };

        let thread_value = serde_json::to_value(&thread).expect("serialize thread descriptor");
        assert_eq!(thread_value["cwd"], "/repo");
        let turn_value = serde_json::to_value(&turn).expect("serialize turn descriptor");
        assert_eq!(turn_value["status"], "completed");

        let thread_round_trip = serde_json::from_value::<ThreadDescriptor>(thread_value)
            .expect("deserialize thread descriptor");
        let turn_round_trip = serde_json::from_value::<TurnDescriptor>(turn_value)
            .expect("deserialize turn descriptor");
        assert_eq!(thread_round_trip.cwd, Some(PathBuf::from("/repo")));
        assert_eq!(turn_round_trip.turn_id, "turn-1");
    }
}
