#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_variables))]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserNotificationPermission {
    Default,
    Denied,
    Granted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPushSubscriptionSnapshot {
    pub endpoint: String,
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub p256dh: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPushState {
    pub service_worker_registered: bool,
    pub notification_permission: BrowserNotificationPermission,
    #[serde(default)]
    pub subscription: Option<BrowserPushSubscriptionSnapshot>,
}

pub fn register_service_worker() {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = ensure_service_worker_registration().await;
        });
    }
}

pub async fn ensure_service_worker_registration() -> Result<bool, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return Err("browser window is unavailable".to_string());
        };
        let navigator = window.navigator();
        let Some(service_worker) = navigator.service_worker() else {
            return Ok(false);
        };
        let registration = wasm_bindgen_futures::JsFuture::from(service_worker.register("/sw.js"))
            .await
            .map_err(|error| error.to_string())?;
        let registered = !registration.is_undefined() && !registration.is_null();
        return Ok(registered);
    }

    Err("service worker registration is only available in the browser".to_string())
}

pub async fn inspect_browser_push_state() -> Result<BrowserPushState, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return Err("browser window is unavailable".to_string());
        };
        let navigator = window.navigator();
        let permission = match web_sys::Notification::permission() {
            web_sys::NotificationPermission::Default => BrowserNotificationPermission::Default,
            web_sys::NotificationPermission::Denied => BrowserNotificationPermission::Denied,
            web_sys::NotificationPermission::Granted => BrowserNotificationPermission::Granted,
        };
        let Some(service_worker) = navigator.service_worker() else {
            return Ok(BrowserPushState {
                service_worker_registered: false,
                notification_permission: permission,
                subscription: None,
            });
        };
        let document_url = window
            .location()
            .href()
            .map_err(|error| error.to_string())?;
        let registration_value = wasm_bindgen_futures::JsFuture::from(
            service_worker.get_registration_with_document_url(&document_url),
        )
        .await
        .map_err(|error| error.to_string())?;
        let service_worker_registered =
            !registration_value.is_undefined() && !registration_value.is_null();
        let subscription = if service_worker_registered {
            let registration: web_sys::ServiceWorkerRegistration = registration_value
                .dyn_into::<web_sys::ServiceWorkerRegistration>()
                .map_err(|error| error.to_string())?;
            let push_manager = registration
                .push_manager()
                .map_err(|error| error.to_string())?;
            let subscription_value = wasm_bindgen_futures::JsFuture::from(
                push_manager
                    .get_subscription()
                    .map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?;
            if subscription_value.is_undefined() || subscription_value.is_null() {
                None
            } else {
                let subscription: web_sys::PushSubscription = subscription_value
                    .dyn_into()
                    .map_err(|error| error.to_string())?;
                Some(push_subscription_snapshot(&subscription)?)
            }
        } else {
            None
        };

        return Ok(BrowserPushState {
            service_worker_registered,
            notification_permission: permission,
            subscription,
        });
    }

    Err("browser push inspection is only available in the browser".to_string())
}

pub async fn register_browser_push_subscription(
    _application_server_key: Option<String>,
) -> Result<BrowserPushState, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return Err("browser window is unavailable".to_string());
        };
        let navigator = window.navigator();
        let Some(service_worker) = navigator.service_worker() else {
            return Err("service workers are unavailable in this browser".to_string());
        };
        let _ = wasm_bindgen_futures::JsFuture::from(service_worker.register("/sw.js"))
            .await
            .map_err(|error| error.to_string())?;

        let permission = match web_sys::Notification::permission() {
            web_sys::NotificationPermission::Granted => BrowserNotificationPermission::Granted,
            web_sys::NotificationPermission::Denied => BrowserNotificationPermission::Denied,
            web_sys::NotificationPermission::Default => {
                let value = wasm_bindgen_futures::JsFuture::from(
                    web_sys::Notification::request_permission()
                        .map_err(|error| error.to_string())?,
                )
                .await
                .map_err(|error| error.to_string())?;
                match value.as_string().as_deref() {
                    Some("granted") => BrowserNotificationPermission::Granted,
                    Some("denied") => BrowserNotificationPermission::Denied,
                    _ => BrowserNotificationPermission::Default,
                }
            }
        };

        if !matches!(permission, BrowserNotificationPermission::Granted) {
            return Ok(BrowserPushState {
                service_worker_registered: true,
                notification_permission: permission,
                subscription: None,
            });
        }

        let registration_value = wasm_bindgen_futures::JsFuture::from(
            service_worker.get_registration_with_document_url(
                &window
                    .location()
                    .href()
                    .map_err(|error| error.to_string())?,
            ),
        )
        .await
        .map_err(|error| error.to_string())?;
        let registration: web_sys::ServiceWorkerRegistration = registration_value
            .dyn_into::<web_sys::ServiceWorkerRegistration>()
            .map_err(|error| error.to_string())?;
        let push_manager = registration
            .push_manager()
            .map_err(|error| error.to_string())?;
        let existing = wasm_bindgen_futures::JsFuture::from(
            push_manager
                .get_subscription()
                .map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;
        let subscription = if existing.is_undefined() || existing.is_null() {
            let Some(application_server_key) = _application_server_key.as_deref() else {
                return Err(
                    "an application server key is required to create a browser push subscription"
                        .to_string(),
                );
            };
            let options = web_sys::PushSubscriptionOptionsInit::new();
            options.set_user_visible_only(true);
            options.set_application_server_key_opt_str(Some(application_server_key));
            let subscription_value = wasm_bindgen_futures::JsFuture::from(
                push_manager
                    .subscribe_with_options(&options)
                    .map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?;
            let subscription: web_sys::PushSubscription = subscription_value
                .dyn_into::<web_sys::PushSubscription>()
                .map_err(|error| error.to_string())?;
            Some(push_subscription_snapshot(&subscription)?)
        } else {
            let subscription: web_sys::PushSubscription = existing
                .dyn_into::<web_sys::PushSubscription>()
                .map_err(|error| error.to_string())?;
            Some(push_subscription_snapshot(&subscription)?)
        };

        return Ok(BrowserPushState {
            service_worker_registered: true,
            notification_permission: permission,
            subscription,
        });
    }

    Err("browser push registration is only available in the browser".to_string())
}

pub async fn disable_browser_push_subscription() -> Result<BrowserPushState, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return Err("browser window is unavailable".to_string());
        };
        let navigator = window.navigator();
        let Some(service_worker) = navigator.service_worker() else {
            return Err("service workers are unavailable in this browser".to_string());
        };
        let registration_value = wasm_bindgen_futures::JsFuture::from(
            service_worker.get_registration_with_document_url(
                &window
                    .location()
                    .href()
                    .map_err(|error| error.to_string())?,
            ),
        )
        .await
        .map_err(|error| error.to_string())?;
        if registration_value.is_undefined() || registration_value.is_null() {
            return Ok(BrowserPushState {
                service_worker_registered: false,
                notification_permission: match web_sys::Notification::permission() {
                    web_sys::NotificationPermission::Default => {
                        BrowserNotificationPermission::Default
                    }
                    web_sys::NotificationPermission::Denied => {
                        BrowserNotificationPermission::Denied
                    }
                    web_sys::NotificationPermission::Granted => {
                        BrowserNotificationPermission::Granted
                    }
                },
                subscription: None,
            });
        }

        let registration: web_sys::ServiceWorkerRegistration = registration_value
            .dyn_into::<web_sys::ServiceWorkerRegistration>()
            .map_err(|error| error.to_string())?;
        let push_manager = registration
            .push_manager()
            .map_err(|error| error.to_string())?;
        let existing = wasm_bindgen_futures::JsFuture::from(
            push_manager
                .get_subscription()
                .map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;
        if !existing.is_undefined() && !existing.is_null() {
            let subscription: web_sys::PushSubscription = existing
                .dyn_into::<web_sys::PushSubscription>()
                .map_err(|error| error.to_string())?;
            let _ = wasm_bindgen_futures::JsFuture::from(
                subscription
                    .unsubscribe()
                    .map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?;
        }

        return inspect_browser_push_state().await;
    }

    Err("browser push disable is only available in the browser".to_string())
}

pub fn browser_notification_permission_label(
    permission: BrowserNotificationPermission,
) -> &'static str {
    match permission {
        BrowserNotificationPermission::Default => "permission default",
        BrowserNotificationPermission::Denied => "permission denied",
        BrowserNotificationPermission::Granted => "permission granted",
    }
}

#[cfg(target_arch = "wasm32")]
fn push_subscription_snapshot(
    subscription: &web_sys::PushSubscription,
) -> Result<BrowserPushSubscriptionSnapshot, String> {
    let endpoint = subscription.endpoint();
    let auth_buffer = subscription
        .get_key(web_sys::PushEncryptionKeyName::Auth)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "push subscription missing auth key".to_string())?;
    let p256dh_buffer = subscription
        .get_key(web_sys::PushEncryptionKeyName::P256dh)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "push subscription missing p256dh key".to_string())?;
    Ok(BrowserPushSubscriptionSnapshot {
        endpoint,
        auth: Some(base64_url_encode(
            &js_sys::Uint8Array::new(&auth_buffer).to_vec(),
        )),
        p256dh: Some(base64_url_encode(
            &js_sys::Uint8Array::new(&p256dh_buffer).to_vec(),
        )),
    })
}

fn base64_url_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    let mut index = 0usize;
    while index + 3 <= bytes.len() {
        let chunk = ((bytes[index] as u32) << 16)
            | ((bytes[index + 1] as u32) << 8)
            | bytes[index + 2] as u32;
        out.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
        out.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
        out.push(TABLE[(chunk & 0x3f) as usize] as char);
        index += 3;
    }
    match bytes.len() - index {
        0 => {}
        1 => {
            let chunk = (bytes[index] as u32) << 16;
            out.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
        }
        2 => {
            let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8);
            out.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            out.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
        }
        _ => unreachable!(),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_url_encode_matches_expected_output() {
        assert_eq!(base64_url_encode(b""), "");
        assert_eq!(base64_url_encode(b"f"), "Zg");
        assert_eq!(base64_url_encode(b"fo"), "Zm8");
        assert_eq!(base64_url_encode(b"foo"), "Zm9v");
        assert_eq!(base64_url_encode(b"foobar"), "Zm9vYmFy");
    }
}
