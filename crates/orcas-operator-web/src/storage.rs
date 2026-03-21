#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_variables))]

use orcas_operator_core::OperatorServerSettings;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const STORAGE_KEY: &str = "orcas.operator.web.settings.v1";
const BROWSER_PUSH_IDENTITY_KEY: &str = "orcas.operator.web.browser_push_identity.v1";

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

    OperatorServerSettings::default()
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
