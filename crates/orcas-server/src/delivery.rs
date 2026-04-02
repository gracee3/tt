use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Mutex;

use isahc::prelude::*;
use serde_json::Value;
use web_push::{ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushMessageBuilder};

use orcas_core::ipc::{
    BrowserPushNotificationPayload, BrowserPushNotificationRoute, NotificationDeliveryJob,
    NotificationDeliveryJobStatus, NotificationRecipient, NotificationSubscription,
    NotificationTransportKind, OperatorNotificationCandidate,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationDeliveryOutcome {
    pub status: NotificationDeliveryJobStatus,
    pub receipt: Option<Value>,
    pub error: Option<String>,
}

impl NotificationDeliveryOutcome {
    pub fn delivered(receipt: Option<Value>) -> Self {
        Self {
            status: NotificationDeliveryJobStatus::Delivered,
            receipt,
            error: None,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            status: NotificationDeliveryJobStatus::Failed,
            receipt: None,
            error: Some(error.into()),
        }
    }
}

pub struct NotificationDeliveryContext<'a> {
    pub job: &'a NotificationDeliveryJob,
    pub candidate: &'a OperatorNotificationCandidate,
    pub recipient: &'a NotificationRecipient,
    pub subscription: &'a NotificationSubscription,
}

pub trait NotificationDeliveryTransport: Send + Sync {
    fn kind(&self) -> NotificationTransportKind;
    fn dispatch(&self, context: &NotificationDeliveryContext<'_>) -> NotificationDeliveryOutcome;
}

#[derive(Debug, Clone)]
pub struct WebPushNotificationDeliveryTransport {
    vapid_private_key_base64: String,
    vapid_subject: String,
}

impl WebPushNotificationDeliveryTransport {
    pub fn new(
        vapid_private_key_base64: impl Into<String>,
        vapid_subject: impl Into<String>,
    ) -> Self {
        Self {
            vapid_private_key_base64: vapid_private_key_base64.into(),
            vapid_subject: vapid_subject.into(),
        }
    }
}

#[derive(Debug, Default)]
pub struct LogNotificationDeliveryTransport;

impl NotificationDeliveryTransport for LogNotificationDeliveryTransport {
    fn kind(&self) -> NotificationTransportKind {
        NotificationTransportKind::Log
    }

    fn dispatch(&self, context: &NotificationDeliveryContext<'_>) -> NotificationDeliveryOutcome {
        NotificationDeliveryOutcome::delivered(Some(serde_json::json!({
            "job_id": context.job.job_id,
            "candidate_id": context.job.candidate_id,
            "subscription_id": context.job.subscription_id,
            "recipient_id": context.job.recipient_id,
            "transport_kind": context.job.transport_kind,
            "candidate_status": context.candidate.status,
            "recipient_enabled": context.recipient.enabled,
            "subscription_enabled": context.subscription.enabled,
        })))
    }
}

fn browser_push_payload(
    context: &NotificationDeliveryContext<'_>,
) -> BrowserPushNotificationPayload {
    let item = &context.candidate.item;
    let body = item
        .rationale
        .clone()
        .or_else(|| item.provenance.clone())
        .unwrap_or_else(|| item.summary.clone());
    BrowserPushNotificationPayload {
        notification_id: context.job.job_id.clone(),
        title: item.title.clone(),
        body,
        route: BrowserPushNotificationRoute::InboxItem {
            origin_node_id: context.job.origin_node_id.clone(),
            item_id: item.id.clone(),
            candidate_id: context.job.candidate_id.clone(),
        },
        source_kind: Some(item.source_kind),
        candidate_status: Some(context.candidate.status),
        item_status: Some(item.status),
        icon: Some("/icon-192.svg".to_string()),
        badge: Some("/icon-192.svg".to_string()),
    }
}

#[derive(Debug, Default)]
pub struct MockNotificationDeliveryTransport {
    outcomes: Mutex<BTreeMap<String, NotificationDeliveryOutcome>>,
}

impl MockNotificationDeliveryTransport {
    pub fn with_job_outcome(
        job_id: impl Into<String>,
        outcome: NotificationDeliveryOutcome,
    ) -> Self {
        let mut outcomes = BTreeMap::new();
        outcomes.insert(job_id.into(), outcome);
        Self {
            outcomes: Mutex::new(outcomes),
        }
    }

    pub fn set_job_outcome(&self, job_id: impl Into<String>, outcome: NotificationDeliveryOutcome) {
        if let Ok(mut outcomes) = self.outcomes.lock() {
            outcomes.insert(job_id.into(), outcome);
        }
    }
}

impl NotificationDeliveryTransport for WebPushNotificationDeliveryTransport {
    fn kind(&self) -> NotificationTransportKind {
        NotificationTransportKind::WebPush
    }

    fn dispatch(&self, context: &NotificationDeliveryContext<'_>) -> NotificationDeliveryOutcome {
        let payload = browser_push_payload(context);
        let payload_bytes = match serde_json::to_vec(&payload) {
            Ok(payload) => payload,
            Err(error) => {
                return NotificationDeliveryOutcome::failed(format!(
                    "failed to serialize browser push payload: {error}"
                ));
            }
        };
        let subscription_info: SubscriptionInfo =
            match serde_json::from_value(context.subscription.endpoint.clone()) {
                Ok(subscription) => subscription,
                Err(error) => {
                    return NotificationDeliveryOutcome::failed(format!(
                        "invalid browser push subscription payload: {error}"
                    ));
                }
            };
        let mut signature_builder = match VapidSignatureBuilder::from_base64(
            self.vapid_private_key_base64.as_str(),
            &subscription_info,
        ) {
            Ok(builder) => builder,
            Err(error) => {
                return NotificationDeliveryOutcome::failed(format!(
                    "failed to load browser push VAPID key: {error}"
                ));
            }
        };
        signature_builder.add_claim("sub", self.vapid_subject.as_str());
        let signature = match signature_builder.build() {
            Ok(signature) => signature,
            Err(error) => {
                return NotificationDeliveryOutcome::failed(format!(
                    "failed to build browser push VAPID signature: {error}"
                ));
            }
        };
        let mut builder = WebPushMessageBuilder::new(&subscription_info);
        builder.set_ttl(300);
        builder.set_payload(ContentEncoding::Aes128Gcm, payload_bytes.as_slice());
        builder.set_vapid_signature(signature);
        let message = match builder.build() {
            Ok(message) => message,
            Err(error) => {
                return NotificationDeliveryOutcome::failed(format!(
                    "browser push message build failed: {error}"
                ));
            }
        };
        let request = web_push::request_builder::build_request::<isahc::Body>(message);
        let response = match request.send() {
            Ok(response) => response,
            Err(error) => {
                return NotificationDeliveryOutcome::failed(format!(
                    "browser push dispatch failed: {error}"
                ));
            }
        };
        let status = response.status();
        let mut body = Vec::new();
        let mut body_reader = response.into_body();
        if let Err(error) = body_reader.read_to_end(&mut body) {
            return NotificationDeliveryOutcome::failed(format!(
                "browser push response read failed: {error}"
            ));
        }
        match web_push::request_builder::parse_response(status, body) {
            Ok(()) => NotificationDeliveryOutcome::delivered(Some(serde_json::json!({
                "notification_id": payload.notification_id,
                "route": payload.route.route_path(payload.notification_id.as_str()),
                "status": status.as_u16(),
                "transport_kind": "web_push",
            }))),
            Err(error) => NotificationDeliveryOutcome::failed(error.to_string()),
        }
    }
}

impl NotificationDeliveryTransport for MockNotificationDeliveryTransport {
    fn kind(&self) -> NotificationTransportKind {
        NotificationTransportKind::Mock
    }

    fn dispatch(&self, context: &NotificationDeliveryContext<'_>) -> NotificationDeliveryOutcome {
        self.outcomes
            .lock()
            .ok()
            .and_then(|outcomes| outcomes.get(&context.job.job_id).cloned())
            .unwrap_or_else(|| {
                NotificationDeliveryOutcome::delivered(Some(serde_json::json!({
                    "mock": true,
                    "job_id": context.job.job_id,
                })))
            })
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use orcas_core::ipc::{
        NotificationDeliveryJob, NotificationDeliveryJobStatus, NotificationRecipient,
        NotificationSubscription, NotificationTransportKind, OperatorInboxActionKind,
        OperatorInboxItem, OperatorInboxItemStatus, OperatorInboxSourceKind,
        OperatorNotificationCandidate, OperatorNotificationCandidateStatus,
    };

    fn notification_context() -> NotificationDeliveryContext<'static> {
        let now = Utc::now();
        let item = OperatorInboxItem {
            id: "item-1".to_string(),
            sequence: 7,
            source_kind: OperatorInboxSourceKind::SupervisorProposal,
            actionable_object_id: "proposal-1".to_string(),
            workstream_id: Some("workstream-1".to_string()),
            work_unit_id: Some("work-unit-1".to_string()),
            title: "Review proposal".to_string(),
            summary: "Summary".to_string(),
            status: OperatorInboxItemStatus::Open,
            available_actions: vec![OperatorInboxActionKind::Approve],
            created_at: now,
            updated_at: now,
            resolved_at: None,
            rationale: Some("Please review".to_string()),
            provenance: Some("source=proposal".to_string()),
        };
        let candidate = Box::leak(Box::new(OperatorNotificationCandidate {
            candidate_id: "candidate-1".to_string(),
            origin_node_id: "origin-1".to_string(),
            item_id: item.id.clone(),
            trigger_sequence: 7,
            status: OperatorNotificationCandidateStatus::Pending,
            item: item.clone(),
            created_at: now,
            updated_at: now,
            acknowledged_at: None,
            suppressed_at: None,
            resolved_at: None,
        }));
        let recipient = Box::leak(Box::new(NotificationRecipient {
            recipient_id: "recipient-1".to_string(),
            display_name: "Recipient".to_string(),
            enabled: true,
            created_at: now,
            updated_at: now,
        }));
        let subscription = Box::leak(Box::new(NotificationSubscription {
            subscription_id: "subscription-1".to_string(),
            recipient_id: recipient.recipient_id.clone(),
            transport_kind: NotificationTransportKind::WebPush,
            endpoint: serde_json::json!({
                "endpoint": "https://example.invalid/push",
                "keys": {
                    "auth": "auth",
                    "p256dh": "p256dh"
                }
            }),
            enabled: true,
            created_at: now,
            updated_at: now,
        }));
        let job = Box::leak(Box::new(NotificationDeliveryJob {
            job_id: "job-1".to_string(),
            origin_node_id: "origin-1".to_string(),
            candidate_id: candidate.candidate_id.clone(),
            trigger_sequence: candidate.trigger_sequence,
            recipient_id: recipient.recipient_id.clone(),
            subscription_id: subscription.subscription_id.clone(),
            transport_kind: NotificationTransportKind::WebPush,
            status: NotificationDeliveryJobStatus::Pending,
            attempt_count: 0,
            created_at: now,
            updated_at: now,
            dispatched_at: None,
            delivered_at: None,
            failed_at: None,
            suppressed_at: None,
            skipped_at: None,
            obsolete_at: None,
            receipt: None,
            error: None,
        }));
        NotificationDeliveryContext {
            job,
            candidate,
            recipient,
            subscription,
        }
    }

    #[test]
    fn browser_push_payload_uses_item_metadata_and_route() {
        let context = notification_context();
        let payload = browser_push_payload(&context);
        assert_eq!(payload.notification_id, "job-1");
        assert_eq!(payload.title, "Review proposal");
        assert_eq!(payload.body, "Please review");
        assert_eq!(
            payload.source_kind,
            Some(OperatorInboxSourceKind::SupervisorProposal)
        );
        assert_eq!(
            payload.route_path(),
            "/inbox/item-1?origin_node_id=origin-1&candidate_id=candidate-1&notification_id=job-1&push=1"
        );
    }

    #[test]
    fn mock_transport_defaults_to_delivered_receipt() {
        let context = notification_context();
        let transport = MockNotificationDeliveryTransport::default();
        let outcome = transport.dispatch(&context);
        assert_eq!(outcome.status, NotificationDeliveryJobStatus::Delivered);
        assert!(outcome.error.is_none());
        assert!(outcome.receipt.is_some());
    }
}
