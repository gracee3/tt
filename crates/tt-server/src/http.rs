use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::{Duration, Instant, sleep};
use tower_http::cors::CorsLayer;
use tracing::info;

use crate::delivery::WebPushNotificationDeliveryTransport;
use crate::delivery::{LogNotificationDeliveryTransport, MockNotificationDeliveryTransport};
use tt_core::ipc::{
    AssignmentStartRequest, AssignmentStartResponse, AuthorityDeletePlanRequest,
    AuthorityDeletePlanResponse, AuthorityHierarchyGetRequest, AuthorityHierarchyGetResponse,
    AuthorityTrackedThreadCreateRequest, AuthorityTrackedThreadCreateResponse,
    AuthorityTrackedThreadDeleteRequest, AuthorityTrackedThreadDeleteResponse,
    AuthorityTrackedThreadEditRequest, AuthorityTrackedThreadEditResponse,
    AuthorityTrackedThreadGetRequest, AuthorityTrackedThreadGetResponse,
    AuthorityWorkstreamCreateRequest, AuthorityWorkstreamCreateResponse,
    AuthorityWorkstreamDeleteRequest, AuthorityWorkstreamDeleteResponse,
    AuthorityWorkstreamEditRequest, AuthorityWorkstreamEditResponse,
    AuthorityWorkunitCreateRequest, AuthorityWorkunitCreateResponse,
    AuthorityWorkunitDeleteRequest, AuthorityWorkunitDeleteResponse, AuthorityWorkunitEditRequest,
    AuthorityWorkunitEditResponse, AuthorityWorkunitGetRequest, AuthorityWorkunitGetResponse,
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
    OperatorReadModelCheckpointQueryRequest, OperatorReadModelCheckpointQueryResponse,
    OperatorReadModelWaitForCheckpointRequest, OperatorReadModelWaitForCheckpointResponse,
    OperatorRemoteActionClaimRequest, OperatorRemoteActionClaimResponse,
    OperatorRemoteActionCompleteRequest, OperatorRemoteActionCompleteResponse,
    OperatorRemoteActionCreateRequest, OperatorRemoteActionCreateResponse,
    OperatorRemoteActionFailRequest, OperatorRemoteActionFailResponse,
    OperatorRemoteActionGetRequest, OperatorRemoteActionGetResponse,
    OperatorRemoteActionListRequest, OperatorRemoteActionListResponse,
    OperatorRemoteActionWaitRequest, OperatorRemoteActionWaitResponse,
    PlanningSessionApproveRequest, PlanningSessionApproveResponse, PlanningSessionCreateRequest,
    PlanningSessionCreateResponse, PlanningSessionListRequest, PlanningSessionListResponse,
    PlanningSessionMarkReadyForReviewRequest, PlanningSessionMarkReadyForReviewResponse,
    PlanningSessionRejectRequest, PlanningSessionRejectResponse,
    PlanningSessionRequestResearchRequest, PlanningSessionRequestResearchResponse,
    PlanningSessionRequestSupervisorContextRequest,
    PlanningSessionRequestSupervisorContextResponse, ProposalApproveRequest,
    ProposalApproveResponse, ProposalArtifactDetailGetRequest, ProposalArtifactDetailGetResponse,
    ProposalCreateRequest, ProposalCreateResponse, ProposalGetRequest, ProposalGetResponse,
    ProposalRejectRequest, ProposalRejectResponse, StateGetRequest, StateGetResponse,
    TTAssignmentPauseRequest, TTAssignmentPauseResponse, TTAssignmentResumeRequest,
    TTAssignmentResumeResponse, ThreadGetRequest, ThreadGetResponse,
};
use tt_core::jsonrpc::{JsonRpcMessage, JsonRpcRequest, RequestId};
use tt_core::{AppPaths, TTResult};

use crate::store::InboxMirrorStore;

#[derive(Debug, Clone)]
pub struct InboxMirrorServerConfig {
    pub bind_addr: SocketAddr,
    pub data_dir: PathBuf,
    pub daemon_socket_file: Option<PathBuf>,
    pub operator_api_token: Option<String>,
    pub push_vapid_private_key_base64: Option<String>,
    pub push_vapid_subject: Option<String>,
}

#[derive(Clone)]
struct InboxMirrorServerState {
    store: Arc<InboxMirrorStore>,
    daemon_socket_file: Option<PathBuf>,
    operator_api_token: Option<String>,
    web_push_delivery: Option<WebPushNotificationDeliveryTransport>,
}

#[derive(Clone)]
pub struct InboxMirrorServer {
    state: Arc<InboxMirrorServerState>,
}

impl InboxMirrorServer {
    pub fn new(store: InboxMirrorStore) -> Self {
        Self::with_operator_api_token_and_web_push(store, None, None)
    }

    pub fn from_config(store: InboxMirrorStore, config: InboxMirrorServerConfig) -> Self {
        let web_push_delivery = match (
            config.push_vapid_private_key_base64,
            config.push_vapid_subject,
        ) {
            (Some(private_key), Some(subject)) => Some(WebPushNotificationDeliveryTransport::new(
                private_key,
                subject,
            )),
            _ => None,
        };
        Self {
            state: Arc::new(InboxMirrorServerState {
                store: Arc::new(store),
                daemon_socket_file: config.daemon_socket_file,
                operator_api_token: config.operator_api_token,
                web_push_delivery,
            }),
        }
    }

    pub fn with_operator_api_token(
        store: InboxMirrorStore,
        operator_api_token: Option<String>,
    ) -> Self {
        Self::with_operator_api_token_and_web_push(store, operator_api_token, None)
    }

    pub fn with_operator_api_token_and_web_push(
        store: InboxMirrorStore,
        operator_api_token: Option<String>,
        web_push_delivery: Option<WebPushNotificationDeliveryTransport>,
    ) -> Self {
        Self {
            state: Arc::new(InboxMirrorServerState {
                store: Arc::new(store),
                daemon_socket_file: None,
                operator_api_token,
                web_push_delivery,
            }),
        }
    }

    pub async fn serve(self, bind_addr: SocketAddr) -> TTResult<()> {
        let listener = tokio::net::TcpListener::bind(bind_addr).await?;
        self.serve_with_listener(listener).await
    }

    pub async fn serve_with_listener(self, listener: tokio::net::TcpListener) -> TTResult<()> {
        let state = self.state.clone();
        if let Some(transport) = state.web_push_delivery.clone() {
            let store = state.store.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(15));
                loop {
                    interval.tick().await;
                    let store = store.clone();
                    let transport = transport.clone();
                    match tokio::task::spawn_blocking(move || {
                        store.dispatch_pending_notification_delivery_jobs(&transport, Some(32))
                    })
                    .await
                    {
                        Ok(Ok(result)) => {
                            if !result.jobs.is_empty() {
                                info!(
                                    jobs = result.jobs.len(),
                                    "browser push delivery loop dispatched pending jobs"
                                );
                            }
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "browser push delivery loop failed");
                        }
                        Err(error) => {
                            tracing::warn!(%error, "browser push delivery loop join failed");
                        }
                    }
                }
            });
        }
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
                "/operator-notifications/checkpoint",
                post(notification_checkpoint),
            )
            .route(
                "/operator-notifications/wait_for_checkpoint",
                post(wait_for_notification_checkpoint),
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
                "/operator-notifications/delivery/checkpoint",
                post(delivery_checkpoint),
            )
            .route(
                "/operator-notifications/delivery/wait_for_checkpoint",
                post(wait_for_delivery_checkpoint),
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
            .route("/operator-runtime/state/get", post(state_get))
            .route(
                "/operator-runtime/assignments/start",
                post(assignment_start),
            )
            .route("/operator-runtime/proposals/create", post(proposal_create))
            .route("/operator-runtime/proposals/get", post(proposal_get))
            .route(
                "/operator-runtime/proposals/artifact-detail",
                post(proposal_artifact_detail_get),
            )
            .route(
                "/operator-runtime/proposals/approve",
                post(proposal_approve),
            )
            .route("/operator-runtime/proposals/reject", post(proposal_reject))
            .route(
                "/operator-authority/hierarchy/get",
                post(authority_hierarchy_get),
            )
            .route(
                "/operator-authority/delete-plan",
                post(authority_delete_plan),
            )
            .route(
                "/operator-authority/workstreams/create",
                post(authority_workstream_create),
            )
            .route(
                "/operator-authority/workstreams/edit",
                post(authority_workstream_edit),
            )
            .route(
                "/operator-authority/workstreams/delete",
                post(authority_workstream_delete),
            )
            .route(
                "/operator-authority/workunits/get",
                post(authority_workunit_get),
            )
            .route(
                "/operator-authority/workunits/create",
                post(authority_workunit_create),
            )
            .route(
                "/operator-authority/workunits/edit",
                post(authority_workunit_edit),
            )
            .route(
                "/operator-authority/workunits/delete",
                post(authority_workunit_delete),
            )
            .route(
                "/operator-authority/tracked-threads/create",
                post(authority_tracked_thread_create),
            )
            .route(
                "/operator-authority/tracked-threads/edit",
                post(authority_tracked_thread_edit),
            )
            .route(
                "/operator-authority/tracked-threads/delete",
                post(authority_tracked_thread_delete),
            )
            .route(
                "/operator-authority/tracked-threads/get",
                post(authority_tracked_thread_get),
            )
            .route(
                "/operator-runtime/planning-sessions/list",
                post(planning_session_list),
            )
            .route(
                "/operator-runtime/planning-sessions/create",
                post(planning_session_create),
            )
            .route(
                "/operator-runtime/planning-sessions/request-supervisor-context",
                post(planning_session_request_supervisor_context),
            )
            .route(
                "/operator-runtime/planning-sessions/request-research",
                post(planning_session_request_research),
            )
            .route(
                "/operator-runtime/planning-sessions/mark-ready",
                post(planning_session_mark_ready_for_review),
            )
            .route(
                "/operator-runtime/planning-sessions/approve",
                post(planning_session_approve),
            )
            .route(
                "/operator-runtime/planning-sessions/reject",
                post(planning_session_reject),
            )
            .route("/operator-runtime/threads/get", post(thread_get))
            .route(
                "/operator-runtime/tt-assignments/pause",
                post(tt_assignment_pause),
            )
            .route(
                "/operator-runtime/tt-assignments/resume",
                post(tt_assignment_resume),
            )
            // Trunk serves the browser app from a different port during local
            // development, so allow cross-origin operator requests.
            .layer(CorsLayer::permissive())
            .layer(middleware::from_fn_with_state(state.clone(), operator_auth))
            .with_state(state);
        let bind_addr = listener.local_addr()?;
        info!(%bind_addr, "tt-server listening");
        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn operator_auth(
    State(state): State<Arc<InboxMirrorServerState>>,
    request: Request<Body>,
    next: Next,
) -> Result<axum::response::Response, StatusCode> {
    if request.method() == axum::http::Method::OPTIONS {
        return Ok(next.run(request).await);
    }
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

async fn daemon_request<P, T>(
    state: &InboxMirrorServerState,
    method: &str,
    params: &P,
) -> Result<T, String>
where
    P: Serialize,
    T: DeserializeOwned,
{
    let socket_path = state
        .daemon_socket_file
        .as_ref()
        .ok_or_else(|| "daemon socket is not configured for this server".to_string())?;
    let mut stream = UnixStream::connect(socket_path).await.map_err(|error| {
        format!(
            "failed to connect to daemon socket {}: {error}",
            socket_path.display()
        )
    })?;
    let payload = serde_json::to_value(params).map_err(|error| error.to_string())?;
    let request = JsonRpcRequest::new(RequestId::Integer(1), method, Some(payload));
    let mut line = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    line.push(b'\n');
    stream
        .write_all(&line)
        .await
        .map_err(|error| format!("failed to write daemon request: {error}"))?;
    stream
        .flush()
        .await
        .map_err(|error| format!("failed to flush daemon request: {error}"))?;
    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    let bytes = reader
        .read_line(&mut response_line)
        .await
        .map_err(|error| format!("failed to read daemon response: {error}"))?;
    if bytes == 0 {
        return Err("daemon closed the socket before sending a response".to_string());
    }
    let message: JsonRpcMessage =
        serde_json::from_str(&response_line).map_err(|error| error.to_string())?;
    match message {
        JsonRpcMessage::Response(response) => {
            serde_json::from_value(response.result).map_err(|error| error.to_string())
        }
        JsonRpcMessage::Error(error) => Err(error.error.message),
        JsonRpcMessage::Notification(notification) => Err(format!(
            "unexpected daemon notification `{}` while waiting for `{method}`",
            notification.method
        )),
        JsonRpcMessage::Request(request) => Err(format!(
            "unexpected daemon request `{}` while waiting for `{method}`",
            request.method
        )),
    }
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

async fn notification_checkpoint(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorReadModelCheckpointQueryRequest>,
) -> Result<Json<OperatorReadModelCheckpointQueryResponse>, String> {
    let response = state
        .store
        .notification_checkpoint(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn wait_for_notification_checkpoint(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorReadModelWaitForCheckpointRequest>,
) -> Result<Json<OperatorReadModelWaitForCheckpointResponse>, String> {
    let response = state
        .store
        .wait_for_notification_checkpoint(&request)
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

async fn delivery_checkpoint(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorReadModelCheckpointQueryRequest>,
) -> Result<Json<OperatorReadModelCheckpointQueryResponse>, String> {
    let response = state
        .store
        .delivery_checkpoint(&request)
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
}

async fn wait_for_delivery_checkpoint(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<OperatorReadModelWaitForCheckpointRequest>,
) -> Result<Json<OperatorReadModelWaitForCheckpointResponse>, String> {
    let response = state
        .store
        .wait_for_delivery_checkpoint(&request)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Json(response))
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
        NotificationTransportKind::WebPush => {
            let Some(transport) = state.web_push_delivery.as_ref() else {
                return Err("browser push delivery is not configured on this server".to_string());
            };
            state
                .store
                .dispatch_pending_notification_delivery_jobs(transport, request.limit)
                .map_err(|error| error.to_string())?
        }
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

async fn state_get(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<StateGetRequest>,
) -> Result<Json<StateGetResponse>, String> {
    Ok(Json(
        daemon_request(&state, tt_core::ipc::methods::STATE_GET, &request).await?,
    ))
}

async fn assignment_start(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AssignmentStartRequest>,
) -> Result<Json<AssignmentStartResponse>, String> {
    Ok(Json(
        daemon_request(&state, tt_core::ipc::methods::ASSIGNMENT_START, &request).await?,
    ))
}

async fn proposal_create(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<ProposalCreateRequest>,
) -> Result<Json<ProposalCreateResponse>, String> {
    Ok(Json(
        daemon_request(&state, tt_core::ipc::methods::PROPOSAL_CREATE, &request).await?,
    ))
}

async fn proposal_get(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<ProposalGetRequest>,
) -> Result<Json<ProposalGetResponse>, String> {
    Ok(Json(
        daemon_request(&state, tt_core::ipc::methods::PROPOSAL_GET, &request).await?,
    ))
}

async fn proposal_artifact_detail_get(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<ProposalArtifactDetailGetRequest>,
) -> Result<Json<ProposalArtifactDetailGetResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::PROPOSAL_ARTIFACT_DETAIL_GET,
            &request,
        )
        .await?,
    ))
}

async fn proposal_approve(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<ProposalApproveRequest>,
) -> Result<Json<ProposalApproveResponse>, String> {
    Ok(Json(
        daemon_request(&state, tt_core::ipc::methods::PROPOSAL_APPROVE, &request).await?,
    ))
}

async fn proposal_reject(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<ProposalRejectRequest>,
) -> Result<Json<ProposalRejectResponse>, String> {
    Ok(Json(
        daemon_request(&state, tt_core::ipc::methods::PROPOSAL_REJECT, &request).await?,
    ))
}

async fn authority_hierarchy_get(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityHierarchyGetRequest>,
) -> Result<Json<AuthorityHierarchyGetResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_HIERARCHY_GET,
            &request,
        )
        .await?,
    ))
}

async fn authority_delete_plan(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityDeletePlanRequest>,
) -> Result<Json<AuthorityDeletePlanResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_DELETE_PLAN,
            &request,
        )
        .await?,
    ))
}

async fn authority_workstream_create(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityWorkstreamCreateRequest>,
) -> Result<Json<AuthorityWorkstreamCreateResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_WORKSTREAM_CREATE,
            &request,
        )
        .await?,
    ))
}

async fn authority_workstream_edit(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityWorkstreamEditRequest>,
) -> Result<Json<AuthorityWorkstreamEditResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_WORKSTREAM_EDIT,
            &request,
        )
        .await?,
    ))
}

async fn authority_workstream_delete(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityWorkstreamDeleteRequest>,
) -> Result<Json<AuthorityWorkstreamDeleteResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_WORKSTREAM_DELETE,
            &request,
        )
        .await?,
    ))
}

async fn authority_workunit_get(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityWorkunitGetRequest>,
) -> Result<Json<AuthorityWorkunitGetResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_WORKUNIT_GET,
            &request,
        )
        .await?,
    ))
}

async fn authority_workunit_create(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityWorkunitCreateRequest>,
) -> Result<Json<AuthorityWorkunitCreateResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_WORKUNIT_CREATE,
            &request,
        )
        .await?,
    ))
}

async fn authority_workunit_edit(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityWorkunitEditRequest>,
) -> Result<Json<AuthorityWorkunitEditResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_WORKUNIT_EDIT,
            &request,
        )
        .await?,
    ))
}

async fn authority_workunit_delete(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityWorkunitDeleteRequest>,
) -> Result<Json<AuthorityWorkunitDeleteResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_WORKUNIT_DELETE,
            &request,
        )
        .await?,
    ))
}

async fn authority_tracked_thread_create(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityTrackedThreadCreateRequest>,
) -> Result<Json<AuthorityTrackedThreadCreateResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_TRACKED_THREAD_CREATE,
            &request,
        )
        .await?,
    ))
}

async fn authority_tracked_thread_edit(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityTrackedThreadEditRequest>,
) -> Result<Json<AuthorityTrackedThreadEditResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_TRACKED_THREAD_EDIT,
            &request,
        )
        .await?,
    ))
}

async fn authority_tracked_thread_delete(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityTrackedThreadDeleteRequest>,
) -> Result<Json<AuthorityTrackedThreadDeleteResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_TRACKED_THREAD_DELETE,
            &request,
        )
        .await?,
    ))
}

async fn authority_tracked_thread_get(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<AuthorityTrackedThreadGetRequest>,
) -> Result<Json<AuthorityTrackedThreadGetResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::AUTHORITY_TRACKED_THREAD_GET,
            &request,
        )
        .await?,
    ))
}

async fn planning_session_create(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<PlanningSessionCreateRequest>,
) -> Result<Json<PlanningSessionCreateResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::PLANNING_SESSION_CREATE,
            &request,
        )
        .await?,
    ))
}

async fn planning_session_list(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<PlanningSessionListRequest>,
) -> Result<Json<PlanningSessionListResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::PLANNING_SESSION_LIST,
            &request,
        )
        .await?,
    ))
}

async fn planning_session_request_supervisor_context(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<PlanningSessionRequestSupervisorContextRequest>,
) -> Result<Json<PlanningSessionRequestSupervisorContextResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::PLANNING_SESSION_REQUEST_SUPERVISOR_CONTEXT,
            &request,
        )
        .await?,
    ))
}

async fn planning_session_request_research(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<PlanningSessionRequestResearchRequest>,
) -> Result<Json<PlanningSessionRequestResearchResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::PLANNING_SESSION_REQUEST_RESEARCH,
            &request,
        )
        .await?,
    ))
}

async fn planning_session_mark_ready_for_review(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<PlanningSessionMarkReadyForReviewRequest>,
) -> Result<Json<PlanningSessionMarkReadyForReviewResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::PLANNING_SESSION_MARK_READY_FOR_REVIEW,
            &request,
        )
        .await?,
    ))
}

async fn planning_session_approve(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<PlanningSessionApproveRequest>,
) -> Result<Json<PlanningSessionApproveResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::PLANNING_SESSION_APPROVE,
            &request,
        )
        .await?,
    ))
}

async fn planning_session_reject(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<PlanningSessionRejectRequest>,
) -> Result<Json<PlanningSessionRejectResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::PLANNING_SESSION_REJECT,
            &request,
        )
        .await?,
    ))
}

async fn thread_get(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<ThreadGetRequest>,
) -> Result<Json<ThreadGetResponse>, String> {
    Ok(Json(
        daemon_request(&state, tt_core::ipc::methods::THREAD_GET, &request).await?,
    ))
}

async fn tt_assignment_pause(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<TTAssignmentPauseRequest>,
) -> Result<Json<TTAssignmentPauseResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::RUNTIME_ASSIGNMENT_PAUSE,
            &request,
        )
        .await?,
    ))
}

async fn tt_assignment_resume(
    State(state): State<Arc<InboxMirrorServerState>>,
    Json(request): Json<TTAssignmentResumeRequest>,
) -> Result<Json<TTAssignmentResumeResponse>, String> {
    Ok(Json(
        daemon_request(
            &state,
            tt_core::ipc::methods::RUNTIME_ASSIGNMENT_RESUME,
            &request,
        )
        .await?,
    ))
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
        daemon_socket_file: None,
        operator_api_token,
        web_push_delivery: None,
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
        .route("/operator-runtime/state/get", post(state_get))
        .route(
            "/operator-runtime/assignments/start",
            post(assignment_start),
        )
        .route("/operator-runtime/proposals/create", post(proposal_create))
        .route(
            "/operator-runtime/proposals/approve",
            post(proposal_approve),
        )
        .route("/operator-runtime/proposals/reject", post(proposal_reject))
        .route(
            "/operator-authority/hierarchy/get",
            post(authority_hierarchy_get),
        )
        .route(
            "/operator-authority/delete-plan",
            post(authority_delete_plan),
        )
        .route(
            "/operator-authority/workstreams/create",
            post(authority_workstream_create),
        )
        .route(
            "/operator-authority/workstreams/edit",
            post(authority_workstream_edit),
        )
        .route(
            "/operator-authority/workstreams/delete",
            post(authority_workstream_delete),
        )
        .route(
            "/operator-authority/workunits/get",
            post(authority_workunit_get),
        )
        .route(
            "/operator-authority/workunits/create",
            post(authority_workunit_create),
        )
        .route(
            "/operator-authority/workunits/edit",
            post(authority_workunit_edit),
        )
        .route(
            "/operator-authority/workunits/delete",
            post(authority_workunit_delete),
        )
        .route(
            "/operator-authority/tracked-threads/create",
            post(authority_tracked_thread_create),
        )
        .route(
            "/operator-authority/tracked-threads/edit",
            post(authority_tracked_thread_edit),
        )
        .route(
            "/operator-authority/tracked-threads/delete",
            post(authority_tracked_thread_delete),
        )
        .route(
            "/operator-authority/tracked-threads/get",
            post(authority_tracked_thread_get),
        )
        .route(
            "/operator-runtime/planning-sessions/list",
            post(planning_session_list),
        )
        .route(
            "/operator-runtime/planning-sessions/create",
            post(planning_session_create),
        )
        .route(
            "/operator-runtime/planning-sessions/request-supervisor-context",
            post(planning_session_request_supervisor_context),
        )
        .route(
            "/operator-runtime/planning-sessions/request-research",
            post(planning_session_request_research),
        )
        .route(
            "/operator-runtime/planning-sessions/mark-ready",
            post(planning_session_mark_ready_for_review),
        )
        .route(
            "/operator-runtime/planning-sessions/approve",
            post(planning_session_approve),
        )
        .route(
            "/operator-runtime/planning-sessions/reject",
            post(planning_session_reject),
        )
        .route("/operator-runtime/threads/get", post(thread_get))
        .route(
            "/operator-runtime/tt-assignments/pause",
            post(tt_assignment_pause),
        )
        .route(
            "/operator-runtime/tt-assignments/resume",
            post(tt_assignment_resume),
        )
        .layer(middleware::from_fn_with_state(state.clone(), operator_auth))
        .with_state(state)
}

#[allow(dead_code)]
pub async fn serve_from_paths(paths: AppPaths, bind_addr: SocketAddr) -> TTResult<()> {
    let db_path = paths.data_dir.join("server_inbox.db");
    let store = InboxMirrorStore::open(db_path)?;
    InboxMirrorServer::new(store).serve(bind_addr).await
}
