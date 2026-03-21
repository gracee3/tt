use tracing::debug;

use orcas_core::ipc::{
    NotificationDeliveryJobGetRequest, NotificationDeliveryJobGetResponse,
    NotificationDeliveryJobListRequest, NotificationDeliveryJobListResponse,
    NotificationDeliveryRunPendingRequest, NotificationDeliveryRunPendingResponse,
    OperatorInboxMirrorCheckpointQueryResponse, OperatorInboxMirrorGetResponse,
    OperatorInboxMirrorListResponse, OperatorInboxWaitForCheckpointRequest,
    OperatorInboxWaitForCheckpointResponse, OperatorNotificationAckRequest,
    OperatorNotificationAckResponse,
    OperatorNotificationGetRequest, OperatorNotificationGetResponse, OperatorNotificationListRequest,
    OperatorNotificationListResponse, OperatorNotificationSuppressRequest,
    OperatorNotificationSuppressResponse, OperatorRemoteActionClaimRequest,
    OperatorRemoteActionClaimResponse, OperatorRemoteActionCompleteRequest,
    OperatorRemoteActionCompleteResponse, OperatorRemoteActionCreateRequest,
    OperatorRemoteActionCreateResponse, OperatorRemoteActionFailRequest,
    OperatorRemoteActionFailResponse, OperatorRemoteActionGetRequest,
    OperatorRemoteActionGetResponse, OperatorRemoteActionListRequest,
    OperatorRemoteActionListResponse, OperatorRemoteActionWaitRequest,
    OperatorRemoteActionWaitResponse,
};
use orcas_core::{OrcasError, OrcasResult};
use uuid::Uuid;

#[cfg(not(target_arch = "wasm32"))]
use reqwest::header::AUTHORIZATION;

#[derive(Debug, Clone)]
pub struct OrcasServerClient {
    base_url: String,
    operator_api_token: Option<String>,
}

impl OrcasServerClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            operator_api_token: None,
        }
    }

    pub fn with_operator_api_token(
        base_url: impl Into<String>,
        operator_api_token: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            operator_api_token: Some(operator_api_token.into()),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    fn auth_header_value(&self) -> Option<String> {
        self.operator_api_token
            .as_ref()
            .map(|token| format!("Bearer {token}"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> OrcasResult<T> {
        let mut request = reqwest::Client::new().get(self.url(path));
        if let Some(value) = self.auth_header_value() {
            let header_value = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|error| OrcasError::Transport(error.to_string()))?;
            request = request.header(AUTHORIZATION, header_value);
        }
        let response = request
            .send()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .json::<T>()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?;
        Ok(response)
    }

    #[cfg(target_arch = "wasm32")]
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> OrcasResult<T> {
        let mut request = gloo_net::http::Request::get(&self.url(path));
        if let Some(value) = self.auth_header_value() {
            request = request.header("Authorization", &value);
        }
        let response = request
            .send()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?;
        if !response.ok() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(OrcasError::Transport(format!(
                "http {status} while requesting {}: {body}",
                self.url(path)
            )));
        }
        let response = response
            .json::<T>()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?;
        Ok(response)
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn post_json<T: serde::de::DeserializeOwned, U: serde::Serialize>(
        &self,
        path: &str,
        request: &U,
    ) -> OrcasResult<T> {
        let mut builder = reqwest::Client::new().post(self.url(path));
        if let Some(value) = self.auth_header_value() {
            let header_value = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|error| OrcasError::Transport(error.to_string()))?;
            builder = builder.header(AUTHORIZATION, header_value);
        }
        let response = builder
            .json(request)
            .send()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .json::<T>()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?;
        Ok(response)
    }

    #[cfg(target_arch = "wasm32")]
    async fn post_json<T: serde::de::DeserializeOwned, U: serde::Serialize>(
        &self,
        path: &str,
        request: &U,
    ) -> OrcasResult<T> {
        let mut builder = gloo_net::http::Request::post(&self.url(path));
        if let Some(value) = self.auth_header_value() {
            builder = builder.header("Authorization", &value);
        }
        let response = builder
            .json(request)
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .send()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?;
        if !response.ok() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(OrcasError::Transport(format!(
                "http {status} while requesting {}: {body}",
                self.url(path)
            )));
        }
        let response = response
            .json::<T>()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?;
        Ok(response)
    }

    pub async fn operator_inbox_list(
        &self,
        origin_node_id: &str,
    ) -> OrcasResult<OperatorInboxMirrorListResponse> {
        self.get_json(&format!("operator-inbox/{origin_node_id}/items"))
            .await
    }

    pub async fn operator_inbox_get(
        &self,
        origin_node_id: &str,
        item_id: &str,
    ) -> OrcasResult<OperatorInboxMirrorGetResponse> {
        self.get_json(&format!("operator-inbox/{origin_node_id}/items/{item_id}"))
            .await
    }

    pub async fn notification_list(
        &self,
        request: &OperatorNotificationListRequest,
    ) -> OrcasResult<OperatorNotificationListResponse> {
        self.post_json("operator-notifications/list", request)
            .await
    }

    pub async fn notification_get(
        &self,
        request: &OperatorNotificationGetRequest,
    ) -> OrcasResult<OperatorNotificationGetResponse> {
        self.post_json("operator-notifications/get", request)
            .await
    }

    pub async fn notification_ack(
        &self,
        request: &OperatorNotificationAckRequest,
    ) -> OrcasResult<OperatorNotificationAckResponse> {
        self.post_json("operator-notifications/ack", request)
            .await
    }

    pub async fn notification_suppress(
        &self,
        request: &OperatorNotificationSuppressRequest,
    ) -> OrcasResult<OperatorNotificationSuppressResponse> {
        self.post_json("operator-notifications/suppress", request)
        .await
    }

    pub async fn delivery_job_list(
        &self,
        request: &NotificationDeliveryJobListRequest,
    ) -> OrcasResult<NotificationDeliveryJobListResponse> {
        self.post_json("operator-notifications/delivery-jobs/list", request)
        .await
    }

    pub async fn delivery_job_get(
        &self,
        request: &NotificationDeliveryJobGetRequest,
    ) -> OrcasResult<NotificationDeliveryJobGetResponse> {
        self.post_json("operator-notifications/delivery-jobs/get", request)
        .await
    }

    pub async fn delivery_run_pending(
        &self,
        request: &NotificationDeliveryRunPendingRequest,
    ) -> OrcasResult<NotificationDeliveryRunPendingResponse> {
        self.post_json("operator-notifications/delivery/run_pending", request)
        .await
    }

    pub async fn remote_action_create(
        &self,
        mut request: OperatorRemoteActionCreateRequest,
    ) -> OrcasResult<OperatorRemoteActionCreateResponse> {
        if request.idempotency_key.is_none() {
            request.idempotency_key = Some(Uuid::now_v7().to_string());
        }
        self.post_json("operator-actions/request", &request)
            .await
    }

    pub async fn remote_action_list(
        &self,
        request: &OperatorRemoteActionListRequest,
    ) -> OrcasResult<OperatorRemoteActionListResponse> {
        self.post_json("operator-actions/list", request)
            .await
    }

    pub async fn remote_action_get(
        &self,
        request: &OperatorRemoteActionGetRequest,
    ) -> OrcasResult<OperatorRemoteActionGetResponse> {
        self.post_json("operator-actions/get", request)
            .await
    }

    pub async fn remote_action_claim(
        &self,
        request: &OperatorRemoteActionClaimRequest,
    ) -> OrcasResult<OperatorRemoteActionClaimResponse> {
        self.post_json("operator-actions/claim", request).await
    }

    pub async fn remote_action_complete(
        &self,
        request: &OperatorRemoteActionCompleteRequest,
    ) -> OrcasResult<OperatorRemoteActionCompleteResponse> {
        self.post_json("operator-actions/complete", request)
            .await
    }

    pub async fn remote_action_fail(
        &self,
        request: &OperatorRemoteActionFailRequest,
    ) -> OrcasResult<OperatorRemoteActionFailResponse> {
        self.post_json("operator-actions/fail", request).await
    }

    pub async fn remote_action_wait(
        &self,
        request: &OperatorRemoteActionWaitRequest,
    ) -> OrcasResult<OperatorRemoteActionWaitResponse> {
        self.post_json("operator-actions/wait", request)
            .await
    }

    pub async fn inbox_checkpoint(
        &self,
        origin_node_id: &str,
    ) -> OrcasResult<OperatorInboxMirrorCheckpointQueryResponse> {
        self.get_json(&format!("operator-inbox/{origin_node_id}/checkpoint"))
            .await
    }

    pub async fn inbox_wait_for_checkpoint(
        &self,
        request: &OperatorInboxWaitForCheckpointRequest,
    ) -> OrcasResult<OperatorInboxWaitForCheckpointResponse> {
        self.post_json("operator-inbox/wait_for_checkpoint", request)
            .await
    }

    pub async fn wait_for_remote_action_update(
        &self,
        request: &OperatorRemoteActionWaitRequest,
    ) -> OrcasResult<OperatorRemoteActionWaitResponse> {
        let response = self.remote_action_wait(request).await?;
        debug!(
            origin_node_id = %response.origin_node_id,
            timed_out = response.timed_out,
            "remote action wait resolved"
        );
        Ok(response)
    }
}

pub mod prelude {
    pub use super::OrcasServerClient;
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::TempDir;
    use tokio::net::TcpListener;

    use orcas_core::ipc::{
        NotificationDeliveryJobListRequest, NotificationDeliveryJobStatus,
        NotificationRecipientUpsertRequest, NotificationSubscriptionUpsertRequest,
        NotificationTransportKind, OperatorInboxActionKind, OperatorInboxChange,
        OperatorInboxChangeKind, OperatorInboxCheckpoint, OperatorInboxItem, OperatorInboxItemStatus,
        OperatorInboxSourceKind, OperatorNotificationCandidateStatus, OperatorRemoteActionClaimRequest,
        OperatorRemoteActionCompleteRequest, OperatorRemoteActionCreateRequest,
        OperatorRemoteActionGetRequest, OperatorRemoteActionRequestStatus,
        OperatorRemoteActionWaitRequest, OperatorNotificationListRequest,
        OperatorRemoteActionListRequest,
    };
    use crate::OrcasServerClient;
    use orcas_server::InboxMirrorServer;
    use orcas_server::InboxMirrorStore;

    fn actionable_item(id: &str, sequence: u64, title: &str) -> OperatorInboxItem {
        let now = Utc::now();
        OperatorInboxItem {
            id: id.to_string(),
            sequence,
            source_kind: OperatorInboxSourceKind::SupervisorProposal,
            actionable_object_id: id.to_string(),
            workstream_id: Some("workstream-1".to_string()),
            work_unit_id: Some("work-unit-1".to_string()),
            title: title.to_string(),
            summary: format!("summary {title}"),
            status: OperatorInboxItemStatus::Open,
            available_actions: vec![OperatorInboxActionKind::Approve, OperatorInboxActionKind::Reject],
            created_at: now,
            updated_at: now,
            resolved_at: None,
            rationale: Some("please review".to_string()),
            provenance: Some("source=proposal".to_string()),
        }
    }

    fn resolved_item(id: &str, sequence: u64, title: &str) -> OperatorInboxItem {
        let now = Utc::now();
        OperatorInboxItem {
            status: OperatorInboxItemStatus::Resolved,
            available_actions: Vec::new(),
            resolved_at: Some(now),
            ..actionable_item(id, sequence, title)
        }
    }

    async fn start_server() -> (TempDir, String, String, tokio::task::JoinHandle<()>) {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let db_path = tempdir.path().join("server.db");
        let token = format!("token-{}", uuid::Uuid::new_v4());
        let store = InboxMirrorStore::open(&db_path).expect("store");
        let server = InboxMirrorServer::with_operator_api_token(store, Some(token.clone()));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let server_url = format!("http://{}", listener.local_addr().expect("addr"));
        let handle = tokio::spawn(async move {
            server
                .serve_with_listener(listener)
                .await
                .expect("server");
        });
        (tempdir, server_url, token, handle)
    }

    fn seed_actionable_inbox(store: &InboxMirrorStore, origin_node_id: &str) {
        store
            .apply_batch(
                origin_node_id,
                OperatorInboxCheckpoint::default(),
                &[OperatorInboxChange {
                    sequence: 1,
                    kind: OperatorInboxChangeKind::Upsert,
                    item: actionable_item("proposal-1", 1, "review me"),
                    changed_at: Utc::now(),
                }],
            )
            .expect("seed inbox");
    }

    fn seed_resolved_inbox(store: &InboxMirrorStore, origin_node_id: &str) {
        store
            .apply_batch(
                origin_node_id,
                OperatorInboxCheckpoint::default(),
                &[OperatorInboxChange {
                    sequence: 1,
                    kind: OperatorInboxChangeKind::Upsert,
                    item: resolved_item("proposal-1", 1, "resolved"),
                    changed_at: Utc::now(),
                }],
            )
            .expect("seed resolved inbox");
    }

    fn seed_delivery_subscription(store: &InboxMirrorStore) {
        store
            .upsert_notification_recipient(&NotificationRecipientUpsertRequest {
                recipient_id: "recipient-1".to_string(),
                display_name: "Recipient 1".to_string(),
                enabled: true,
            })
            .expect("recipient");
        store
            .upsert_notification_subscription(&NotificationSubscriptionUpsertRequest {
                subscription_id: "subscription-1".to_string(),
                recipient_id: "recipient-1".to_string(),
                transport_kind: NotificationTransportKind::Mock,
                endpoint: serde_json::json!({"endpoint": "https://example.invalid/subscription-1"}),
                enabled: true,
            })
            .expect("subscription");
    }

    #[tokio::test]
    async fn auth_header_is_required_for_operator_routes() {
        let (tempdir, server_url, token, handle) = start_server().await;
        let origin_node_id = "origin-a";
        let db_path = tempdir.path().join("server.db");
        let store = InboxMirrorStore::open(&db_path).expect("reopen");
        seed_actionable_inbox(&store, origin_node_id);

        let unauthorized = OrcasServerClient::new(server_url.clone())
            .operator_inbox_list(origin_node_id)
            .await;
        let unauthorized = unauthorized.expect_err("unauthorized error");
        assert!(unauthorized.to_string().contains("401"), "expected auth failure");

        let client = OrcasServerClient::with_operator_api_token(server_url, token);
        let response = client
            .operator_inbox_list(origin_node_id)
            .await
            .expect("authorized list");
        assert_eq!(response.items.len(), 1);

        handle.abort();
    }

    #[tokio::test]
    async fn typed_read_models_decode_for_inbox_notifications_deliveries_and_actions() {
        let (tempdir, server_url, token, handle) = start_server().await;
        let origin_node_id = "origin-a";
        let db_path = tempdir.path().join("server.db");
        let store = InboxMirrorStore::open(&db_path).expect("reopen");
        seed_delivery_subscription(&store);
        seed_actionable_inbox(&store, origin_node_id);

        let client = OrcasServerClient::with_operator_api_token(server_url.clone(), token.clone());
        let inbox = client
            .operator_inbox_list(origin_node_id)
            .await
            .expect("inbox list");
        assert_eq!(inbox.items[0].id, "proposal-1");
        let inbox_item = client
            .operator_inbox_get(origin_node_id, "proposal-1")
            .await
            .expect("inbox get");
        assert_eq!(inbox_item.item.expect("inbox item").status, OperatorInboxItemStatus::Open);

        let notifications = client
            .notification_list(&OperatorNotificationListRequest {
                origin_node_id: origin_node_id.to_string(),
                pending_only: true,
                actionable_only: true,
                ..Default::default()
            })
            .await
            .expect("notification list");
        assert_eq!(
            notifications.candidates[0].status,
            OperatorNotificationCandidateStatus::Pending
        );
        let delivery_jobs = client
            .delivery_job_list(&NotificationDeliveryJobListRequest {
                origin_node_id: Some(origin_node_id.to_string()),
                ..Default::default()
            })
            .await
            .expect("delivery list");
        assert!(delivery_jobs
            .jobs
            .iter()
            .all(|job| job.status == NotificationDeliveryJobStatus::Pending));

        let create = client
            .remote_action_create(OperatorRemoteActionCreateRequest {
                origin_node_id: origin_node_id.to_string(),
                item_id: "proposal-1".to_string(),
                action_kind: OperatorInboxActionKind::Approve,
                idempotency_key: Some("client-idempotency-key".to_string()),
                requested_by: Some("operator".to_string()),
                request_note: Some("approve".to_string()),
            })
            .await
            .expect("remote action create");
        assert_eq!(create.request.status, OperatorRemoteActionRequestStatus::Pending);
        let listed = client
            .remote_action_list(&OperatorRemoteActionListRequest {
                origin_node_id: origin_node_id.to_string(),
                pending_only: true,
                ..Default::default()
            })
            .await
            .expect("remote action list");
        assert_eq!(listed.requests.len(), 1);
        let got = client
            .remote_action_get(&OperatorRemoteActionGetRequest {
                origin_node_id: origin_node_id.to_string(),
                request_id: create.request.request_id.clone(),
            })
            .await
            .expect("remote action get");
        assert_eq!(got.request.expect("request").request_id, create.request.request_id);

        handle.abort();
    }

    #[tokio::test]
    async fn remote_action_submission_is_idempotent_with_the_same_key() {
        let (tempdir, server_url, token, handle) = start_server().await;
        let origin_node_id = "origin-a";
        let db_path = tempdir.path().join("server.db");
        let store = InboxMirrorStore::open(&db_path).expect("reopen");
        seed_actionable_inbox(&store, origin_node_id);
        let client = OrcasServerClient::with_operator_api_token(server_url, token);

        let first = client
            .remote_action_create(OperatorRemoteActionCreateRequest {
                origin_node_id: origin_node_id.to_string(),
                item_id: "proposal-1".to_string(),
                action_kind: OperatorInboxActionKind::Approve,
                idempotency_key: Some("retry-key".to_string()),
                requested_by: Some("operator".to_string()),
                request_note: Some("first submit".to_string()),
            })
            .await
            .expect("first");
        let second = client
            .remote_action_create(OperatorRemoteActionCreateRequest {
                origin_node_id: origin_node_id.to_string(),
                item_id: "proposal-1".to_string(),
                action_kind: OperatorInboxActionKind::Approve,
                idempotency_key: Some("retry-key".to_string()),
                requested_by: Some("operator".to_string()),
                request_note: Some("retry submit".to_string()),
            })
            .await
            .expect("second");

        assert_eq!(first.request.request_id, second.request.request_id);
        assert_eq!(first.request.idempotency_key.as_deref(), Some("retry-key"));
        assert_eq!(second.request.idempotency_key.as_deref(), Some("retry-key"));

        handle.abort();
    }

    #[tokio::test]
    async fn remote_action_wait_and_status_changes_are_visible() {
        let (tempdir, server_url, token, handle) = start_server().await;
        let origin_node_id = "origin-a";
        let db_path = tempdir.path().join("server.db");
        let store = InboxMirrorStore::open(&db_path).expect("reopen");
        seed_actionable_inbox(&store, origin_node_id);
        let client = OrcasServerClient::with_operator_api_token(server_url, token);

        let created = client
            .remote_action_create(OperatorRemoteActionCreateRequest {
                origin_node_id: origin_node_id.to_string(),
                item_id: "proposal-1".to_string(),
                action_kind: OperatorInboxActionKind::Approve,
                idempotency_key: Some("watch-key".to_string()),
                requested_by: Some("operator".to_string()),
                request_note: Some("watch".to_string()),
            })
            .await
            .expect("create");
        let initial = client
            .remote_action_get(&OperatorRemoteActionGetRequest {
                origin_node_id: origin_node_id.to_string(),
                request_id: created.request.request_id.clone(),
            })
            .await
            .expect("get initial")
            .request
            .expect("request");
        let wait_handle = {
            let client = client.clone();
            let origin_node_id = origin_node_id.to_string();
            let request_id = created.request.request_id.clone();
            let after_updated_at = initial.updated_at;
            tokio::spawn(async move {
                client
                    .wait_for_remote_action_update(&OperatorRemoteActionWaitRequest {
                        origin_node_id,
                        request_id,
                        after_updated_at: Some(after_updated_at),
                        timeout_ms: Some(10_000),
                    })
                    .await
            })
        };

        let claimed = client
            .remote_action_claim(&OperatorRemoteActionClaimRequest {
                origin_node_id: origin_node_id.to_string(),
                worker_id: "worker-1".to_string(),
                limit: Some(1),
                lease_ms: Some(60_000),
            })
            .await
            .expect("claim");
        let claimed_request = claimed.requests.first().expect("claimed request");
        assert_eq!(claimed_request.request.status, OperatorRemoteActionRequestStatus::Claimed);
        let waited = wait_handle.await.expect("wait task").expect("wait result");
        assert_eq!(
            waited
                .request
                .as_ref()
                .expect("waited request")
                .status,
            OperatorRemoteActionRequestStatus::Claimed
        );

        let wait_after_claim = {
            let client = client.clone();
            let origin_node_id = origin_node_id.to_string();
            let request_id = created.request.request_id.clone();
            let after_updated_at = waited.request.as_ref().expect("claimed request").updated_at;
            tokio::spawn(async move {
                client
                    .wait_for_remote_action_update(&OperatorRemoteActionWaitRequest {
                        origin_node_id,
                        request_id,
                        after_updated_at: Some(after_updated_at),
                        timeout_ms: Some(10_000),
                    })
                    .await
            })
        };

        let completed = client
            .remote_action_complete(&OperatorRemoteActionCompleteRequest {
                origin_node_id: origin_node_id.to_string(),
                request_id: created.request.request_id.clone(),
                claim_token: claimed_request.claim_token.clone(),
                result: serde_json::json!({"status": "ok"}),
            })
            .await
            .expect("complete");
        assert_eq!(completed.request.status, OperatorRemoteActionRequestStatus::Completed);
        let final_wait = wait_after_claim.await.expect("wait task").expect("wait result");
        assert_eq!(
            final_wait
                .request
                .as_ref()
                .expect("final request")
                .status,
            OperatorRemoteActionRequestStatus::Completed
        );

        handle.abort();
    }

    #[tokio::test]
    async fn request_and_auth_errors_are_surfaces_cleanly() {
        let (tempdir, server_url, token, handle) = start_server().await;
        let origin_node_id = "origin-a";
        let db_path = tempdir.path().join("server.db");
        let store = InboxMirrorStore::open(&db_path).expect("reopen");
        seed_resolved_inbox(&store, origin_node_id);

        let unauthorized = OrcasServerClient::new(server_url.clone())
            .remote_action_list(&orcas_core::ipc::OperatorRemoteActionListRequest {
                origin_node_id: origin_node_id.to_string(),
                ..Default::default()
            })
            .await;
        assert!(unauthorized
            .expect_err("unauthorized")
            .to_string()
            .contains("401"));

        let client = OrcasServerClient::with_operator_api_token(server_url, token);
        let err = client
            .remote_action_create(OperatorRemoteActionCreateRequest {
                origin_node_id: origin_node_id.to_string(),
                item_id: "proposal-1".to_string(),
                action_kind: OperatorInboxActionKind::Approve,
                idempotency_key: Some("fail-key".to_string()),
                requested_by: Some("operator".to_string()),
                request_note: Some("should fail".to_string()),
            })
            .await
            .expect_err("expected request failure");
        assert!(err.to_string().contains("transport error"));

        handle.abort();
    }

    #[tokio::test]
    async fn execution_failure_is_reported_clearly() {
        let (tempdir, server_url, token, handle) = start_server().await;
        let origin_node_id = "origin-a";
        let db_path = tempdir.path().join("server.db");
        let store = InboxMirrorStore::open(&db_path).expect("reopen");
        seed_actionable_inbox(&store, origin_node_id);
        let client = OrcasServerClient::with_operator_api_token(server_url, token);

        let created = client
            .remote_action_create(OperatorRemoteActionCreateRequest {
                origin_node_id: origin_node_id.to_string(),
                item_id: "proposal-1".to_string(),
                action_kind: OperatorInboxActionKind::Approve,
                idempotency_key: Some("execution-failure-key".to_string()),
                requested_by: Some("operator".to_string()),
                request_note: Some("exercise failure".to_string()),
            })
            .await
            .expect("create");
        let claimed = client
            .remote_action_claim(&OperatorRemoteActionClaimRequest {
                origin_node_id: origin_node_id.to_string(),
                worker_id: "worker-1".to_string(),
                limit: Some(1),
                lease_ms: Some(60_000),
            })
            .await
            .expect("claim");
        let claim_token = claimed.requests.first().expect("claimed").claim_token.clone();
        let err = client
            .remote_action_complete(&OperatorRemoteActionCompleteRequest {
                origin_node_id: origin_node_id.to_string(),
                request_id: created.request.request_id.clone(),
                claim_token: format!("{claim_token}-wrong"),
                result: serde_json::json!({"status": "ok"}),
            })
            .await
            .expect_err("expected completion failure");
        assert!(err.to_string().contains("transport error"));

        handle.abort();
    }
}
