#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_variables))]

use serde::{Deserialize, Serialize};
use tt_operator_core::OperatorServerSettings;
use uuid::Uuid;

use chrono::{DateTime, Utc};
use tt_core::ipc::OperatorInboxActionKind;

use crate::workspace::WorkspaceState;

const STORAGE_KEY: &str = "tt.operator.web.settings.v1";
const BROWSER_PUSH_IDENTITY_KEY: &str = "tt.operator.web.browser_push_identity.v1";
const WORKSPACE_STATE_KEY: &str = "tt.operator.web.workspace_state.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPushIdentity {
    pub browser_instance_id: String,
}

pub fn load_settings() -> OperatorServerSettings {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                if let Ok(Some(raw)) = storage.get_item(STORAGE_KEY) {
                    if let Ok(settings) = serde_json::from_str::<OperatorServerSettings>(&raw) {
                        return settings;
                    }
                }
            }
        }
    }

    OperatorServerSettings {
        server_url: "http://127.0.0.1:3000".to_string(),
        ..OperatorServerSettings::default()
    }
}

pub fn save_settings(_settings: &OperatorServerSettings) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                if let Ok(raw) = serde_json::to_string(_settings) {
                    let _ = storage.set_item(STORAGE_KEY, &raw);
                }
            }
        }
    }
}

pub fn settings_ready(settings: &OperatorServerSettings) -> bool {
    !settings.server_url.trim().is_empty() && !settings.origin_node_id.trim().is_empty()
}

pub fn load_browser_push_identity() -> BrowserPushIdentity {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                if let Ok(Some(raw)) = storage.get_item(BROWSER_PUSH_IDENTITY_KEY) {
                    if let Ok(identity) = serde_json::from_str::<BrowserPushIdentity>(&raw) {
                        return identity;
                    }
                }
            }
        }
    }

    BrowserPushIdentity {
        browser_instance_id: Uuid::now_v7().to_string(),
    }
}

pub fn save_browser_push_identity(_identity: &BrowserPushIdentity) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                if let Ok(raw) = serde_json::to_string(_identity) {
                    let _ = storage.set_item(BROWSER_PUSH_IDENTITY_KEY, &raw);
                }
            }
        }
    }
}

pub fn browser_push_recipient_id(origin_node_id: &str, identity: &BrowserPushIdentity) -> String {
    format!(
        "browser::{origin_node_id}::{}",
        identity.browser_instance_id
    )
}

pub fn browser_push_subscription_id(
    origin_node_id: &str,
    identity: &BrowserPushIdentity,
) -> String {
    format!(
        "browser::{origin_node_id}::{}::webpush",
        identity.browser_instance_id
    )
}

pub fn remote_action_idempotency_key(
    origin_node_id: &str,
    item_id: &str,
    action_kind: OperatorInboxActionKind,
    item_updated_at: DateTime<Utc>,
) -> String {
    format!(
        "tt-operator-web::{origin_node_id}::{item_id}::{action_kind:?}::{}",
        item_updated_at.to_rfc3339()
    )
}

pub fn load_workspace_state() -> WorkspaceState {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                if let Ok(Some(raw)) = storage.get_item(WORKSPACE_STATE_KEY) {
                    if let Ok(state) = serde_json::from_str::<WorkspaceState>(&raw) {
                        return state;
                    }
                }
            }
        }
    }

    WorkspaceState::default()
}

pub fn save_workspace_state(_state: &WorkspaceState) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                if let Ok(raw) = serde_json::to_string(_state) {
                    let _ = storage.set_item(WORKSPACE_STATE_KEY, &raw);
                }
            }
        }
    }
}
