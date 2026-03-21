use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::time::{Duration, Instant, sleep};
use tracing::info;

use crate::delivery::{LogNotificationDeliveryTransport, MockNotificationDeliveryTransport};
use orcas_core::ipc::{
    NotificationDeliveryJobGetRequest, NotificationDeliveryJobGetResponse,
    NotificationDeliveryJobListRequest, NotificationDeliveryJobListResponse,
    NotificationDeliveryRunPendingRequest, NotificationDeliveryRunPendingResponse,
    NotificationRecipientListRequest, NotificationRecipientListResponse,
    NotificationRecipientUpsertRequest, NotificationRecipientUpsertResponse,
    NotificationSubscriptionListRequest, NotificationSubscriptionListResponse,
    NotificationSubscriptionSetEnabledRequest, NotificationSubscriptionSetEnabledResponse,
    NotificationSubscriptionUpsertRequest, NotificationSubscriptionUpsertResponse,
    NotificationTransportKind, OperatorInboxMirrorApplyRequest, OperatorInboxMirrorApplyResponse,
    OperatorInboxMirrorCheckpointQueryRequest, OperatorInboxMirrorCheckpointQueryResponse,
    OperatorInboxMirrorGetResponse, OperatorInboxMirrorListResponse,
    OperatorInboxWaitForCheckpointRequest, OperatorInboxWaitForCheckpointResponse,
    OperatorNotificationAckRequest, OperatorNotificationAckResponse,
    OperatorNotificationGetRequest, OperatorNotificationGetResponse,
    OperatorNotificationListRequest, OperatorNotificationListResponse,
    OperatorNotificationSuppressRequest, OperatorNotificationSuppressResponse,
    OperatorRemoteActionClaimRequest, OperatorRemoteActionClaimResponse,
    OperatorRemoteActionCompleteRequest, OperatorRemoteActionCompleteResponse,
    OperatorRemoteActionCreateRequest, OperatorRemoteActionCreateResponse,
    OperatorRemoteActionFailRequest, OperatorRemoteActionFailResponse,
    OperatorRemoteActionGetRequest, OperatorRemoteActionGetResponse,
    OperatorRemoteActionListRequest, OperatorRemoteActionListResponse,
    OperatorRemoteActionWaitRequest, OperatorRemoteActionWaitResponse,
};
use orcas_core::{AppPaths, OrcasResult};

use crate::store::InboxMirrorStore;

#[derive(Debug, Clone)]
pub struct InboxMirrorServerConfig {
    pub bind_addr: SocketAddr,
    pub data_dir: PathBuf,
    pub operator_api_token: Option<String>,
}

#[derive(Clone)]
struct InboxMirrorServerState {
    store: Arc<InboxMirrorStore>,
    operator_api_token: Option<String>,
}

#[derive(Clone)]
pub struct InboxMirrorServer {
    state: Arc<InboxMirrorServerState>,
}

impl InboxMirrorServer {
    pub fn new(store: InboxMirrorStore) -> Self {
        Self::with_operator_api_token(store, None)
    }

    pub fn with_operator_api_token(
        store: InboxMirrorStore,
        operator_api_token: Option<String>,
    ) -> Self {
        Self {
            state: Arc::new(InboxMirrorServerState {
                store: Arc::new(store),
                operator_api_token,
            }),
        }
    }

    pub async fn serve(self, bind_addr: SocketAddr) -> OrcasResult<()> {
        let listener = tokio::net::TcpListener::bind(bind_addr).await?;
        self.serve_with_listener(listener).await
    }

    pub async fn serve_with_listener(self, listener: tokio::net::TcpListener) -> OrcasResult<()> {
        let state = self.state.clone();
        let app = Router::new()
            .route("/operator-inbox/mirror/apply", post(apply))
            .route(
                "/operator-inbox/{origin_node_id}/checkpoint",
                get(checkpoint),
            )
            .route("/operator-inbox/{origin_node_id}/items", get(list_items))
            .route(
                "/operator-inbox/{origin_node_id}/items/{item_id}",
                get(get_item),
            )
            .route(
                "/operator-inbox/wait_for_checkpoint",
                post(wait_for_inbox_checkpoint),
            )
            .route(
                "/operator-notifications/list",
                post(list_notification_candidates),
            )
            .route(
                "/operator-notifications/get",
                post(get_notification_candidate),
            )
            .route(
                "/operator-notifications/ack",
                post(ack_notification_candidate),
            )
            .route(
                "/operator-notifications/suppress",
                post(suppress_notification_candidate),
            )
            .route(
                "/operator-notifications/recipients/upsert",
                post(upsert_notification_recipient),
            )
            .route(
                "/operator-notifications/recipients/list",
                post(list_notification_recipients),
            )
            .route(
                "/operator-notifications/subscriptions/upsert",
                post(upsert_notification_subscription),
            )
            .route(
                "/operator-notifications/subscriptions/list",
                post(list_notification_subscriptions),
            )
            .route(
                "/operator-notifications/subscriptions/set_enabled",
                post(set_notification_subscription_enabled),
            )
            .route(
                "/operator-notifications/delivery-jobs/list",
                post(list_notification_delivery_jobs),
            )
            .route(
                "/operator-notifications/delivery-jobs/get",
                post(get_notification_delivery_job),
            )
            .route(
                "/operator-notifications/delivery/run_pending",
                post(run_pending_notification_delivery_jobs),
            )
            .route(
                "/operator-actions/request",
                post(create_remote_action_request),
            )
            .route("/operator-actions/list", post(list_remote_action_requests))
            .route("/operator-actions/get", post(get_remote_action_request))
            .route(
                "/operator-actions/claim",
                post(claim_remote_action_requests),
            )
            .route("/operator-actions/wait", post(wait_remote_action_request))
            .route(
                "/operator-actions/complete",
                post(complete_remote_action_request),
            )
            .route("/operator-actions/fail", post(fail_remote_action_request))
            .layer(middleware::from_fn_with_state(state.clone(), operator_auth))
            .with_state(state);
        let bind_addr = listener.local_addr()?;
        info!(%bind_addr, "orcas-server listening");
        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn operator_auth(
    State(state): State<Arc<InboxMirrorServerState>>,
    request: Request<Body>,
    next: Next,
) -> Result<axum::response::Response, StatusCode> {
    let Some(expected) = state.operator_api_token.as_deref() else {
        return Ok(next.run(request).await);
    };
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected);
    if !authorized {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

async fn apply(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorInboxMirrorApplyRequest>,
) -> Result<Json<OperatorInboxMirrorApplyResponse>, String> {
    let result = state
        .store
        .apply_batch(
            request.origin_node_id.as_str(),
            request.checkpoint.clone(),
            &request.changes,
        )
        .map_err(|error| error.to_string())?;
    Ok(Json(OperatorInboxMirrorApplyResponse {
        origin_node_id: request.origin_node_id,
        checkpoint: result.checkpoint,
        mirror_checkpoint: result.mirror_checkpoint,
        applied_changes: result.applied_changes,
        skipped_changes: result.skipped_changes,
    }))
}

async fn checkpoint(
    State(state): State<Arc<InboxMirrorServerState>>,
    Path(OperatorInboxMirrorCheckpointQueryRequest { origin_node_id }): Path<
        OperatorInboxMirrorCheckpointQueryRequest,
    >,
) -> Result<Json<OperatorInboxMirrorCheckpointQueryResponse>, String> {
    let checkpoint = state
        .store
        .checkpoint(origin_node_id.as_str())
        .map_err(|error| error.to_string())?;
    Ok(Json(OperatorInboxMirrorCheckpointQueryResponse {
        origin_node_id,
        checkpoint,
    }))
}

async fn list_items(
    State(state): State<Arc<InboxMirrorServerState>>,
    Path(origin_node_id): Path<String>,
) -> Result<Json<OperatorInboxMirrorListResponse>, String> {
    let response = state
        .store
        .list(origin_node_id.as_str(), None)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn get_item(
    State(state): State<Arc<InboxMirrorServerState>>,
    Path((origin_node_id, item_id)): Path<(String, String)>,
) -> Result<Json<OperatorInboxMirrorGetResponse>, String> {
    let item = state
        .store
        .get(origin_node_id.as_str(), item_id.as_str())
        .map_err(|error| error.to_string())?;
    Ok(Json(OperatorInboxMirrorGetResponse {
        origin_node_id,
        item,
    }))
}

async fn wait_for_inbox_checkpoint(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorInboxWaitForCheckpointRequest>,
) -> Result<Json<OperatorInboxWaitForCheckpointResponse>, String> {
    let response = state
        .store
        .wait_for_checkpoint(&request)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn list_notification_candidates(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorNotificationListRequest>,
) -> Result<Json<OperatorNotificationListResponse>, String> {
    let response = state
        .store
        .notification_candidates(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn get_notification_candidate(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorNotificationGetRequest>,
) -> Result<Json<OperatorNotificationGetResponse>, String> {
    let candidate = state
        .store
        .notification_candidate(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(OperatorNotificationGetResponse {
        origin_node_id: request.origin_node_id,
        candidate,
    }))
}

async fn ack_notification_candidate(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorNotificationAckRequest>,
) -> Result<Json<OperatorNotificationAckResponse>, String> {
    let response = state
        .store
        .acknowledge_notification_candidate(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn suppress_notification_candidate(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorNotificationSuppressRequest>,
) -> Result<Json<OperatorNotificationSuppressResponse>, String> {
    let response = state
        .store
        .suppress_notification_candidate(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn upsert_notification_recipient(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<NotificationRecipientUpsertRequest>,
) -> Result<Json<NotificationRecipientUpsertResponse>, String> {
    let response = state
        .store
        .upsert_notification_recipient(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn list_notification_recipients(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<NotificationRecipientListRequest>,
) -> Result<Json<NotificationRecipientListResponse>, String> {
    let response = state
        .store
        .list_notification_recipients(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn upsert_notification_subscription(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<NotificationSubscriptionUpsertRequest>,
) -> Result<Json<NotificationSubscriptionUpsertResponse>, String> {
    let response = state
        .store
        .upsert_notification_subscription(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn list_notification_subscriptions(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<NotificationSubscriptionListRequest>,
) -> Result<Json<NotificationSubscriptionListResponse>, String> {
    let response = state
        .store
        .list_notification_subscriptions(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn set_notification_subscription_enabled(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<NotificationSubscriptionSetEnabledRequest>,
) -> Result<Json<NotificationSubscriptionSetEnabledResponse>, String> {
    let response = state
        .store
        .set_notification_subscription_enabled(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn list_notification_delivery_jobs(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<NotificationDeliveryJobListRequest>,
) -> Result<Json<NotificationDeliveryJobListResponse>, String> {
    let response = state
        .store
        .list_notification_delivery_jobs(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn get_notification_delivery_job(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<NotificationDeliveryJobGetRequest>,
) -> Result<Json<NotificationDeliveryJobGetResponse>, String> {
    let job = state
        .store
        .get_notification_delivery_job(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(NotificationDeliveryJobGetResponse { job }))
}

async fn run_pending_notification_delivery_jobs(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<NotificationDeliveryRunPendingRequest>,
) -> Result<Json<NotificationDeliveryRunPendingResponse>, String> {
    let response = match request
        .transport_kind
        .unwrap_or(NotificationTransportKind::Log)
    {
        NotificationTransportKind::Log => state
            .store
            .dispatch_pending_notification_delivery_jobs(
                &LogNotificationDeliveryTransport,
                request.limit,
            )
            .map_err(|error| error.to_string())?,
        NotificationTransportKind::Mock => state
            .store
            .dispatch_pending_notification_delivery_jobs(
                &MockNotificationDeliveryTransport::default(),
                request.limit,
            )
            .map_err(|error| error.to_string())?,
        other => {
            return Err(format!(
                "notification delivery transport kind `{other:?}` is not supported by the local server"
            ));
        }
    };
    Ok(Json(response))
}

async fn create_remote_action_request(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorRemoteActionCreateRequest>,
) -> Result<Json<OperatorRemoteActionCreateResponse>, String> {
    let response = state
        .store
        .create_remote_action_request(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn list_remote_action_requests(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorRemoteActionListRequest>,
) -> Result<Json<OperatorRemoteActionListResponse>, String> {
    let response = state
        .store
        .list_remote_action_requests(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn get_remote_action_request(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorRemoteActionGetRequest>,
) -> Result<Json<OperatorRemoteActionGetResponse>, String> {
    let response = state
        .store
        .get_remote_action_request(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn wait_remote_action_request(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorRemoteActionWaitRequest>,
) -> Result<Json<OperatorRemoteActionWaitResponse>, String> {
    let start = Instant::now();
    let timeout = Duration::from_millis(request.timeout_ms.unwrap_or(30_000).max(1));
    loop {
        let response = state
            .store
            .get_remote_action_request(&OperatorRemoteActionGetRequest {
                origin_node_id: request.origin_node_id.clone(),
                request_id: request.request_id.clone(),
            })
            .map_err(|error| error.to_string())?;
        if let Some(ref current) = response.request {
            if request
                .after_updated_at
                .is_none_or(|after| current.updated_at > after)
            {
                return Ok(Json(OperatorRemoteActionWaitResponse {
                    origin_node_id: request.origin_node_id,
                    request: response.request.clone(),
                    timed_out: false,
                }));
            }
        }
        if start.elapsed() >= timeout {
            return Ok(Json(OperatorRemoteActionWaitResponse {
                origin_node_id: request.origin_node_id,
                request: response.request,
                timed_out: true,
            }));
        }
        sleep(Duration::from_millis(200)).await;
    }
}

async fn claim_remote_action_requests(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorRemoteActionClaimRequest>,
) -> Result<Json<OperatorRemoteActionClaimResponse>, String> {
    let response = state
        .store
        .claim_remote_action_requests(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn complete_remote_action_request(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorRemoteActionCompleteRequest>,
) -> Result<Json<OperatorRemoteActionCompleteResponse>, String> {
    let response = state
        .store
        .complete_remote_action_request(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn fail_remote_action_request(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorRemoteActionFailRequest>,
) -> Result<Json<OperatorRemoteActionFailResponse>, String> {
    let response = state
        .store
        .fail_remote_action_request(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

pub fn app(store: InboxMirrorStore) -> Router {
    app_with_operator_api_token(store, None)
}

pub fn app_with_operator_api_token(
    store: InboxMirrorStore,
    operator_api_token: Option<String>,
) -> Router {
    let state = Arc::new(InboxMirrorServerState {
        store: Arc::new(store),
        operator_api_token,
    });
    Router::new()
        .route("/operator-inbox/mirror/apply", post(apply))
        .route(
            "/operator-inbox/{origin_node_id}/checkpoint",
            get(checkpoint),
        )
        .route("/operator-inbox/{origin_node_id}/items", get(list_items))
        .route(
            "/operator-inbox/{origin_node_id}/items/{item_id}",
            get(get_item),
        )
        .route(
            "/operator-notifications/list",
            post(list_notification_candidates),
        )
        .route(
            "/operator-notifications/get",
            post(get_notification_candidate),
        )
        .route(
            "/operator-notifications/ack",
            post(ack_notification_candidate),
        )
        .route(
            "/operator-notifications/suppress",
            post(suppress_notification_candidate),
        )
        .route(
            "/operator-notifications/recipients/upsert",
            post(upsert_notification_recipient),
        )
        .route(
            "/operator-notifications/recipients/list",
            post(list_notification_recipients),
        )
        .route(
            "/operator-notifications/subscriptions/upsert",
            post(upsert_notification_subscription),
        )
        .route(
            "/operator-notifications/subscriptions/list",
            post(list_notification_subscriptions),
        )
        .route(
            "/operator-notifications/subscriptions/set_enabled",
            post(set_notification_subscription_enabled),
        )
        .route(
            "/operator-notifications/delivery-jobs/list",
            post(list_notification_delivery_jobs),
        )
        .route(
            "/operator-notifications/delivery-jobs/get",
            post(get_notification_delivery_job),
        )
        .route(
            "/operator-notifications/delivery/run_pending",
            post(run_pending_notification_delivery_jobs),
        )
        .route(
            "/operator-actions/request",
            post(create_remote_action_request),
        )
        .route("/operator-actions/list", post(list_remote_action_requests))
        .route("/operator-actions/get", post(get_remote_action_request))
        .route(
            "/operator-actions/claim",
            post(claim_remote_action_requests),
        )
        .route("/operator-actions/wait", post(wait_remote_action_request))
        .route(
            "/operator-actions/complete",
            post(complete_remote_action_request),
        )
        .route("/operator-actions/fail", post(fail_remote_action_request))
        .layer(middleware::from_fn_with_state(state.clone(), operator_auth))
        .with_state(state)
}

#[allow(dead_code)]
pub async fn serve_from_paths(paths: AppPaths, bind_addr: SocketAddr) -> OrcasResult<()> {
    let db_path = paths.data_dir.join("server_inbox.db");
    let store = InboxMirrorStore::open(db_path)?;
    InboxMirrorServer::new(store).serve(bind_addr).await
}
