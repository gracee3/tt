#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_variables))]

use orcas_core::ipc::BrowserPushNotificationRoute;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushOpenContext {
    pub notification_id: String,
    pub route: BrowserPushNotificationRoute,
}

pub fn current_push_open_context() -> Option<PushOpenContext> {
    #[cfg(target_arch = "wasm32")]
    {
        let href = web_sys::window()?.location().href().ok()?;
        return push_open_context_from_url(&href);
    }

    None
}

pub fn push_open_context_from_url(url: &str) -> Option<PushOpenContext> {
    let parsed = Url::parse(url).ok()?;
    let mut notification_id = None;
    let mut push_requested = false;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "notification_id" if !value.trim().is_empty() => {
                notification_id = Some(value.into_owned())
            }
            "push" if value == "1" || value.eq_ignore_ascii_case("true") => push_requested = true,
            _ => {}
        }
    }
    if !push_requested {
        return None;
    }
    let notification_id = notification_id?;
    let route = route_from_url(&parsed, &notification_id)?;
    Some(PushOpenContext {
        notification_id,
        route,
    })
}

pub fn push_context_summary(context: &PushOpenContext) -> String {
    match &context.route {
        BrowserPushNotificationRoute::InboxItem {
            origin_node_id,
            item_id,
            candidate_id,
        } => format!(
            "Opened from browser notification {} for mirrored inbox item {} (candidate {}, origin {}).",
            context.notification_id, item_id, candidate_id, origin_node_id
        ),
        BrowserPushNotificationRoute::RemoteActionRequest {
            origin_node_id,
            request_id,
        } => format!(
            "Opened from browser notification {} for remote action request {} (origin {}).",
            context.notification_id, request_id, origin_node_id
        ),
        BrowserPushNotificationRoute::Notifications { origin_node_id } => format!(
            "Opened from browser notification {} on the notifications view (origin {}).",
            context.notification_id, origin_node_id
        ),
        BrowserPushNotificationRoute::Deliveries { origin_node_id } => format!(
            "Opened from browser notification {} on the deliveries view (origin {}).",
            context.notification_id, origin_node_id
        ),
    }
}

fn route_from_url(parsed: &Url, _notification_id: &str) -> Option<BrowserPushNotificationRoute> {
    let path = parsed.path().trim_matches('/');
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    match (segments.next(), segments.next(), segments.next()) {
        (Some("inbox"), Some(item_id), None) => {
            let mut origin_node_id = None;
            let mut candidate_id = None;
            for (key, value) in parsed.query_pairs() {
                match key.as_ref() {
                    "origin_node_id" if !value.trim().is_empty() => {
                        origin_node_id = Some(value.into_owned())
                    }
                    "candidate_id" if !value.trim().is_empty() => {
                        candidate_id = Some(value.into_owned())
                    }
                    _ => {}
                }
            }
            Some(BrowserPushNotificationRoute::InboxItem {
                origin_node_id: origin_node_id?,
                item_id: item_id.to_string(),
                candidate_id: candidate_id?,
            })
        }
        (Some("actions"), Some(request_id), None) => {
            let origin_node_id = parsed.query_pairs().find_map(|(key, value)| {
                (key == "origin_node_id" && !value.trim().is_empty()).then(|| value.into_owned())
            })?;
            Some(BrowserPushNotificationRoute::RemoteActionRequest {
                origin_node_id,
                request_id: request_id.to_string(),
            })
        }
        (Some("notifications"), None, None) => {
            let origin_node_id = parsed.query_pairs().find_map(|(key, value)| {
                (key == "origin_node_id" && !value.trim().is_empty()).then(|| value.into_owned())
            })?;
            Some(BrowserPushNotificationRoute::Notifications { origin_node_id })
        }
        (Some("deliveries"), None, None) => {
            let origin_node_id = parsed.query_pairs().find_map(|(key, value)| {
                (key == "origin_node_id" && !value.trim().is_empty()).then(|| value.into_owned())
            })?;
            Some(BrowserPushNotificationRoute::Deliveries { origin_node_id })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inbox_push_context_from_url() {
        let context = push_open_context_from_url(
            "https://operator.test/inbox/item-1?origin_node_id=origin-a&candidate_id=candidate-1&notification_id=note-1&push=1",
        )
        .expect("context");
        assert_eq!(context.notification_id, "note-1");
        assert!(matches!(
            context.route,
            BrowserPushNotificationRoute::InboxItem {
                origin_node_id,
                item_id,
                candidate_id,
            } if origin_node_id == "origin-a" && item_id == "item-1" && candidate_id == "candidate-1"
        ));
    }

    #[test]
    fn parses_action_push_context_from_url() {
        let context = push_open_context_from_url(
            "https://operator.test/actions/request-1?origin_node_id=origin-a&notification_id=note-1&push=true",
        )
        .expect("context");
        assert!(matches!(
            context.route,
            BrowserPushNotificationRoute::RemoteActionRequest {
                origin_node_id,
                request_id,
            } if origin_node_id == "origin-a" && request_id == "request-1"
        ));
    }

    #[test]
    fn ignores_urls_without_push_marker() {
        assert!(push_open_context_from_url(
            "https://operator.test/inbox/item-1?origin_node_id=origin-a&candidate_id=candidate-1&notification_id=note-1"
        )
        .is_none());
    }
}
