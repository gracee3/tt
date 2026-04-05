#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_variables))]

use serde::{Deserialize, Serialize};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

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
        let window =
            web_sys::window().ok_or_else(|| "browser window is unavailable".to_string())?;
        let navigator = window.navigator();
        let service_worker = navigator.service_worker();
        let registration = wasm_bindgen_futures::JsFuture::from(service_worker.register("/sw.js"))
            .await
            .map_err(js_error)?;
        return Ok(!registration.is_undefined() && !registration.is_null());
    }

    Err("service worker registration is only available in the browser".to_string())
}

pub async fn inspect_browser_push_state() -> Result<BrowserPushState, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let window =
            web_sys::window().ok_or_else(|| "browser window is unavailable".to_string())?;
        let navigator = window.navigator();
        let permission = browser_notification_permission();
        let service_worker = navigator.service_worker();
        let document_url = window.location().href().map_err(js_error)?;
        let registration_value: wasm_bindgen::JsValue = wasm_bindgen_futures::JsFuture::from(
            service_worker.get_registration_with_document_url(&document_url),
        )
        .await
        .map_err(js_error)?;
        let service_worker_registered =
            !registration_value.is_undefined() && !registration_value.is_null();
        let subscription = if service_worker_registered {
            let registration: web_sys::ServiceWorkerRegistration = registration_value
                .dyn_into::<web_sys::ServiceWorkerRegistration>()
                .map_err(js_error)?;
            let push_manager = registration.push_manager().map_err(js_error)?;
            let subscription_value: wasm_bindgen::JsValue = wasm_bindgen_futures::JsFuture::from(
                push_manager.get_subscription().map_err(js_error)?,
            )
            .await
            .map_err(js_error)?;
            if subscription_value.is_undefined() || subscription_value.is_null() {
                None
            } else {
                let subscription: web_sys::PushSubscription =
                    subscription_value.dyn_into().map_err(js_error)?;
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
    application_server_key: Option<String>,
) -> Result<BrowserPushState, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let window =
            web_sys::window().ok_or_else(|| "browser window is unavailable".to_string())?;
        let navigator = window.navigator();
        let service_worker = navigator.service_worker();
        let _ = wasm_bindgen_futures::JsFuture::from(service_worker.register("/sw.js"))
            .await
            .map_err(js_error)?;

        let permission = match web_sys::Notification::permission() {
            web_sys::NotificationPermission::Granted => BrowserNotificationPermission::Granted,
            web_sys::NotificationPermission::Denied => BrowserNotificationPermission::Denied,
            web_sys::NotificationPermission::Default => {
                let value = wasm_bindgen_futures::JsFuture::from(
                    web_sys::Notification::request_permission().map_err(js_error)?,
                )
                .await
                .map_err(js_error)?;
                match value.as_string().as_deref() {
                    Some("granted") => BrowserNotificationPermission::Granted,
                    Some("denied") => BrowserNotificationPermission::Denied,
                    _ => BrowserNotificationPermission::Default,
                }
            }
            _ => BrowserNotificationPermission::Default,
        };

        if !matches!(permission, BrowserNotificationPermission::Granted) {
            return Ok(BrowserPushState {
                service_worker_registered: true,
                notification_permission: permission,
                subscription: None,
            });
        }

        let registration_value: wasm_bindgen::JsValue = wasm_bindgen_futures::JsFuture::from(
            service_worker
                .get_registration_with_document_url(&window.location().href().map_err(js_error)?),
        )
        .await
        .map_err(js_error)?;
        let registration: web_sys::ServiceWorkerRegistration = registration_value
            .dyn_into::<web_sys::ServiceWorkerRegistration>()
            .map_err(js_error)?;
        let push_manager = registration.push_manager().map_err(js_error)?;
        let existing: wasm_bindgen::JsValue = wasm_bindgen_futures::JsFuture::from(
            push_manager.get_subscription().map_err(js_error)?,
        )
        .await
        .map_err(js_error)?;
        let subscription = if existing.is_undefined() || existing.is_null() {
            let Some(application_server_key) = application_server_key.as_deref() else {
                return Err(
                    "an application server key is required to create a browser push subscription"
                        .to_string(),
                );
            };
            let options = web_sys::PushSubscriptionOptionsInit::new();
            options.set_user_visible_only(true);
            options.set_application_server_key_opt_str(Some(application_server_key));
            let subscription_value: wasm_bindgen::JsValue = wasm_bindgen_futures::JsFuture::from(
                push_manager
                    .subscribe_with_options(&options)
                    .map_err(js_error)?,
            )
            .await
            .map_err(js_error)?;
            let subscription: web_sys::PushSubscription = subscription_value
                .dyn_into::<web_sys::PushSubscription>()
                .map_err(js_error)?;
            Some(push_subscription_snapshot(&subscription)?)
        } else {
            let subscription: web_sys::PushSubscription = existing
                .dyn_into::<web_sys::PushSubscription>()
                .map_err(js_error)?;
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
        let window =
            web_sys::window().ok_or_else(|| "browser window is unavailable".to_string())?;
        let navigator = window.navigator();
        let service_worker = navigator.service_worker();
        let registration_value: wasm_bindgen::JsValue = wasm_bindgen_futures::JsFuture::from(
            service_worker
                .get_registration_with_document_url(&window.location().href().map_err(js_error)?),
        )
        .await
        .map_err(js_error)?;
        if registration_value.is_undefined() || registration_value.is_null() {
            return Ok(BrowserPushState {
                service_worker_registered: false,
                notification_permission: browser_notification_permission(),
                subscription: None,
            });
        }

        let registration: web_sys::ServiceWorkerRegistration = registration_value
            .dyn_into::<web_sys::ServiceWorkerRegistration>()
            .map_err(js_error)?;
        let push_manager = registration.push_manager().map_err(js_error)?;
        let existing: wasm_bindgen::JsValue = wasm_bindgen_futures::JsFuture::from(
            push_manager.get_subscription().map_err(js_error)?,
        )
        .await
        .map_err(js_error)?;
        if !existing.is_undefined() && !existing.is_null() {
            let subscription: web_sys::PushSubscription = existing
                .dyn_into::<web_sys::PushSubscription>()
                .map_err(js_error)?;
            let _ =
                wasm_bindgen_futures::JsFuture::from(subscription.unsubscribe().map_err(js_error)?)
                    .await
                    .map_err(js_error)?;
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
fn browser_notification_permission() -> BrowserNotificationPermission {
    match web_sys::Notification::permission() {
        web_sys::NotificationPermission::Default => BrowserNotificationPermission::Default,
        web_sys::NotificationPermission::Denied => BrowserNotificationPermission::Denied,
        web_sys::NotificationPermission::Granted => BrowserNotificationPermission::Granted,
        _ => BrowserNotificationPermission::Default,
    }
}

#[cfg(target_arch = "wasm32")]
fn js_error(error: impl core::fmt::Debug) -> String {
    format!("{error:?}")
}

#[cfg(target_arch = "wasm32")]
fn push_subscription_snapshot(
    subscription: &web_sys::PushSubscription,
) -> Result<BrowserPushSubscriptionSnapshot, String> {
    let endpoint = subscription.endpoint();
    let auth_buffer = subscription
        .get_key(web_sys::PushEncryptionKeyName::Auth)
        .map_err(js_error)?
        .ok_or_else(|| "push subscription missing auth key".to_string())?;
    let p256dh_buffer = subscription
        .get_key(web_sys::PushEncryptionKeyName::P256dh)
        .map_err(js_error)?
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
    use super::base64_url_encode;

    #[test]
    fn base64_url_encode_matches_expected_output() {
        assert_eq!(base64_url_encode(b""), "");
        assert_eq!(base64_url_encode(b"f"), "Zg");
        assert_eq!(base64_url_encode(b"fo"), "Zm8");
        assert_eq!(base64_url_encode(b"foo"), "Zm9v");
        assert_eq!(base64_url_encode(b"foobar"), "Zm9vYmFy");
    }
}
