use orcas_operator_core::OperatorServerSettings;

const STORAGE_KEY: &str = "orcas.operator.web.settings.v1";

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
                if let Ok(raw) = serde_json::to_string(settings) {
                    let _ = storage.set_item(STORAGE_KEY, &raw);
                }
            }
        }
    }
}

pub fn settings_ready(settings: &OperatorServerSettings) -> bool {
    !settings.server_url.trim().is_empty() && !settings.origin_node_id.trim().is_empty()
}
