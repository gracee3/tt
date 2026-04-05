//! Socket-lifetime IPC client for the TT daemon.
//!
//! This layer is intentionally thin: it does not replay missed events, it does
//! not keep subscriptions alive across reconnect, and it does not try to mask a
//! dead daemon socket as a reusable cache. Callers are expected to rebuild the
//! client after disconnect and perform snapshot-first recovery above this layer.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio::time::{Duration, timeout};
use tracing::{debug, info, warn};

use tt_core::ipc;
use tt_core::jsonrpc::{
    JsonRpcError, JsonRpcErrorObject, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, RequestId,
};
use tt_core::{AppPaths, TTError, TTResult};

type PendingResponse = oneshot::Sender<TTResult<Value>>;
/// Event receiver bound to one daemon socket lifetime.
///
/// The stream ends when the client closes the connection, so callers must
/// resubscribe after reconnect instead of assuming missed events were replayed.
pub type EventSubscription = broadcast::Receiver<ipc::DaemonEventEnvelope>;

/// Thin JSON-RPC client for the current daemon socket.
///
/// A closed client is not recoverable. Once the socket dies, outstanding
/// requests fail and event subscriptions terminate so higher layers can rebuild
/// state from a fresh snapshot.
pub struct TTIpcClient {
    pending: Mutex<HashMap<RequestId, PendingResponse>>,
    outbound: mpsc::Sender<String>,
    event_tx: RwLock<Option<broadcast::Sender<ipc::DaemonEventEnvelope>>>,
    closed: std::sync::atomic::AtomicBool,
    next_request_id: AtomicI64,
    socket: String,
}

impl TTIpcClient {
    // Long-form live turns can exceed the default short request window.
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

    pub async fn connect(paths: &AppPaths) -> TTResult<Arc<Self>> {
        let start = Instant::now();
        let socket = paths.socket_file.display().to_string();
        info!(socket, "connecting TT IPC client");
        let stream = UnixStream::connect(&paths.socket_file)
            .await
            .map_err(|error| {
                TTError::Transport(format!(
                    "failed to connect to TT daemon at {}: {error}",
                    paths.socket_file.display()
                ))
            })?;
        let client = Self::from_stream(stream, socket.clone());
        if client.is_ok() {
            info!(
                socket,
                connected = true,
                duration_ms = start.elapsed().as_millis() as u64,
                "TT IPC client connected"
            );
        }
        client
    }

    pub fn subscribe(&self) -> EventSubscription {
        // Subscriptions are per-socket, not per logical daemon identity.
        debug!(
            socket = self.socket.as_str(),
            "subscribing to TT daemon events"
        );
        self.event_tx
            .read()
            .expect("event sender lock poisoned")
            .as_ref()
            .map(broadcast::Sender::subscribe)
            .unwrap_or_else(closed_event_subscription)
    }

    pub async fn daemon_status(&self) -> TTResult<ipc::DaemonStatusResponse> {
        self.request(ipc::methods::DAEMON_STATUS, &ipc::Empty::default())
            .await
    }

    pub async fn daemon_connect(&self) -> TTResult<ipc::DaemonConnectResponse> {
        self.request(
            ipc::methods::DAEMON_CONNECT,
            &ipc::DaemonConnectRequest::default(),
        )
        .await
    }

    pub async fn daemon_stop(&self) -> TTResult<ipc::DaemonStopResponse> {
        self.request(
            ipc::methods::DAEMON_STOP,
            &ipc::DaemonStopRequest::default(),
        )
        .await
    }

    pub async fn state_get(&self) -> TTResult<ipc::StateGetResponse> {
        self.request(ipc::methods::STATE_GET, &ipc::StateGetRequest::default())
            .await
    }

    pub async fn operator_inbox_list(
        &self,
        params: &ipc::OperatorInboxListRequest,
    ) -> TTResult<ipc::OperatorInboxListResponse> {
        self.request(ipc::methods::OPERATOR_INBOX_LIST, params)
            .await
    }

    pub async fn operator_inbox_get(
        &self,
        params: &ipc::OperatorInboxGetRequest,
    ) -> TTResult<ipc::OperatorInboxGetResponse> {
        self.request(ipc::methods::OPERATOR_INBOX_GET, params).await
    }

    pub async fn operator_inbox_checkpoint(
        &self,
        params: &ipc::OperatorInboxCheckpointRequest,
    ) -> TTResult<ipc::OperatorInboxCheckpointResponse> {
        self.request(ipc::methods::OPERATOR_INBOX_CHECKPOINT, params)
            .await
    }

    pub async fn operator_inbox_changes(
        &self,
        params: &ipc::OperatorInboxChangesRequest,
    ) -> TTResult<ipc::OperatorInboxChangesResponse> {
        self.request(ipc::methods::OPERATOR_INBOX_CHANGES, params)
            .await
    }

    pub async fn operator_inbox_action_route(
        &self,
        params: &ipc::OperatorInboxActionRouteRequest,
    ) -> TTResult<ipc::OperatorInboxActionRouteResponse> {
        self.request(ipc::methods::OPERATOR_INBOX_ACTION_ROUTE, params)
            .await
    }

    pub async fn operator_inbox_wait_for_checkpoint(
        &self,
        params: &ipc::OperatorInboxWaitRequest,
    ) -> TTResult<ipc::OperatorInboxWaitResponse> {
        self.request(ipc::methods::OPERATOR_INBOX_WAIT_FOR_CHECKPOINT, params)
            .await
    }

    pub async fn operator_inbox_replay(
        &self,
        params: &ipc::OperatorInboxReplayRequest,
    ) -> TTResult<ipc::OperatorInboxReplayResponse> {
        self.request(ipc::methods::OPERATOR_INBOX_REPLAY, params)
            .await
    }

    pub async fn operator_inbox_export(
        &self,
        params: &ipc::OperatorInboxExportRequest,
    ) -> TTResult<ipc::OperatorInboxExportResponse> {
        self.request(ipc::methods::OPERATOR_INBOX_EXPORT, params)
            .await
    }

    pub async fn operator_inbox_ack(
        &self,
        params: &ipc::OperatorInboxAckRequest,
    ) -> TTResult<ipc::OperatorInboxAckResponse> {
        self.request(ipc::methods::OPERATOR_INBOX_ACK, params).await
    }

    pub async fn operator_inbox_mirror_checkpoint(
        &self,
        params: &ipc::OperatorInboxMirrorCheckpointRequest,
    ) -> TTResult<ipc::OperatorInboxMirrorCheckpointResponse> {
        self.request(ipc::methods::OPERATOR_INBOX_MIRROR_CHECKPOINT, params)
            .await
    }

    pub async fn session_get_active(&self) -> TTResult<ipc::SessionGetActiveResponse> {
        self.request(
            ipc::methods::SESSION_GET_ACTIVE,
            &ipc::SessionGetActiveRequest::default(),
        )
        .await
    }

    pub async fn models_list(
        &self,
        params: &ipc::ModelsListRequest,
    ) -> TTResult<ipc::ModelsListResponse> {
        self.request(ipc::methods::MODELS_LIST, params).await
    }

    pub async fn threads_list(
        &self,
        params: &ipc::ThreadsListRequest,
    ) -> TTResult<ipc::ThreadsListResponse> {
        self.request(ipc::methods::THREADS_LIST, params).await
    }

    pub async fn threads_list_scoped(
        &self,
        params: &ipc::ThreadsListScopedRequest,
    ) -> TTResult<ipc::ThreadsListResponse> {
        self.request(ipc::methods::THREADS_LIST_SCOPED, params)
            .await
    }

    pub async fn threads_list_loaded(
        &self,
        params: &ipc::ThreadsListLoadedRequest,
    ) -> TTResult<ipc::ThreadsListResponse> {
        self.request(ipc::methods::THREADS_LIST_LOADED, params)
            .await
    }

    pub async fn workstream_runtime_list(&self) -> TTResult<ipc::WorkstreamRuntimeListResponse> {
        self.request(
            ipc::methods::WORKSTREAM_RUNTIME_LIST,
            &ipc::Empty::default(),
        )
        .await
    }

    pub async fn workstream_runtime_get(
        &self,
        params: &ipc::WorkstreamRuntimeRefRequest,
    ) -> TTResult<ipc::WorkstreamRuntimeGetResponse> {
        self.request(ipc::methods::WORKSTREAM_RUNTIME_GET, params)
            .await
    }

    pub async fn workstream_runtime_start(
        &self,
        params: &ipc::WorkstreamRuntimeRefRequest,
    ) -> TTResult<ipc::WorkstreamRuntimeControlResponse> {
        self.request(ipc::methods::WORKSTREAM_RUNTIME_START, params)
            .await
    }

    pub async fn workstream_runtime_stop(
        &self,
        params: &ipc::WorkstreamRuntimeRefRequest,
    ) -> TTResult<ipc::WorkstreamRuntimeControlResponse> {
        self.request(ipc::methods::WORKSTREAM_RUNTIME_STOP, params)
            .await
    }

    pub async fn workstream_runtime_restart(
        &self,
        params: &ipc::WorkstreamRuntimeRefRequest,
    ) -> TTResult<ipc::WorkstreamRuntimeControlResponse> {
        self.request(ipc::methods::WORKSTREAM_RUNTIME_RESTART, params)
            .await
    }

    pub async fn thread_start(
        &self,
        params: &ipc::ThreadStartRequest,
    ) -> TTResult<ipc::ThreadStartResponse> {
        self.request(ipc::methods::THREAD_START, params).await
    }

    pub async fn thread_read(
        &self,
        params: &ipc::ThreadReadRequest,
    ) -> TTResult<ipc::ThreadReadResponse> {
        self.request(ipc::methods::THREAD_READ, params).await
    }

    pub async fn thread_read_history(
        &self,
        params: &ipc::ThreadReadHistoryRequest,
    ) -> TTResult<ipc::ThreadReadHistoryResponse> {
        self.request(ipc::methods::THREAD_READ_HISTORY, params)
            .await
    }

    pub async fn thread_get(
        &self,
        params: &ipc::ThreadGetRequest,
    ) -> TTResult<ipc::ThreadGetResponse> {
        self.request(ipc::methods::THREAD_GET, params).await
    }

    pub async fn thread_attach(
        &self,
        params: &ipc::ThreadAttachRequest,
    ) -> TTResult<ipc::ThreadAttachResponse> {
        self.request(ipc::methods::THREAD_ATTACH, params).await
    }

    pub async fn thread_detach(
        &self,
        params: &ipc::ThreadDetachRequest,
    ) -> TTResult<ipc::ThreadDetachResponse> {
        self.request(ipc::methods::THREAD_DETACH, params).await
    }

    pub async fn thread_resume(
        &self,
        params: &ipc::ThreadResumeRequest,
    ) -> TTResult<ipc::ThreadResumeResponse> {
        self.request(ipc::methods::THREAD_RESUME, params).await
    }

    pub async fn turns_recent(
        &self,
        params: &ipc::TurnsRecentRequest,
    ) -> TTResult<ipc::TurnsRecentResponse> {
        self.request(ipc::methods::TURNS_RECENT, params).await
    }

    pub async fn turns_list_active(&self) -> TTResult<ipc::TurnsListActiveResponse> {
        self.request(
            ipc::methods::TURNS_LIST_ACTIVE,
            &ipc::TurnsListActiveRequest::default(),
        )
        .await
    }

    pub async fn turn_get(&self, params: &ipc::TurnGetRequest) -> TTResult<ipc::TurnGetResponse> {
        self.request(ipc::methods::TURN_GET, params).await
    }

    pub async fn turn_attach(
        &self,
        params: &ipc::TurnAttachRequest,
    ) -> TTResult<ipc::TurnAttachResponse> {
        self.request(ipc::methods::TURN_ATTACH, params).await
    }

    pub async fn turn_start(
        &self,
        params: &ipc::TurnStartRequest,
    ) -> TTResult<ipc::TurnStartResponse> {
        self.request(ipc::methods::TURN_START, params).await
    }

    pub async fn turn_steer(
        &self,
        params: &ipc::TurnSteerRequest,
    ) -> TTResult<ipc::TurnSteerResponse> {
        self.request(ipc::methods::TURN_STEER, params).await
    }

    pub async fn turn_interrupt(&self, params: &ipc::TurnInterruptRequest) -> TTResult<()> {
        let _: ipc::Empty = self.request(ipc::methods::TURN_INTERRUPT, params).await?;
        Ok(())
    }

    pub async fn supervisor_decision_list(
        &self,
        params: &ipc::SupervisorDecisionListRequest,
    ) -> TTResult<ipc::SupervisorDecisionListResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_LIST, params)
            .await
    }

    pub async fn supervisor_decision_get(
        &self,
        params: &ipc::SupervisorDecisionGetRequest,
    ) -> TTResult<ipc::SupervisorDecisionGetResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_GET, params)
            .await
    }

    pub async fn supervisor_decision_propose_interrupt(
        &self,
        params: &ipc::SupervisorDecisionProposeInterruptRequest,
    ) -> TTResult<ipc::SupervisorDecisionProposeInterruptResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_PROPOSE_INTERRUPT, params)
            .await
    }

    pub async fn supervisor_decision_record_no_action(
        &self,
        params: &ipc::SupervisorDecisionRecordNoActionRequest,
    ) -> TTResult<ipc::SupervisorDecisionRecordNoActionResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_RECORD_NO_ACTION, params)
            .await
    }

    pub async fn supervisor_decision_manual_refresh(
        &self,
        params: &ipc::SupervisorDecisionManualRefreshRequest,
    ) -> TTResult<ipc::SupervisorDecisionManualRefreshResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_MANUAL_REFRESH, params)
            .await
    }

    pub async fn supervisor_decision_propose_steer(
        &self,
        params: &ipc::SupervisorDecisionProposeSteerRequest,
    ) -> TTResult<ipc::SupervisorDecisionProposeSteerResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_PROPOSE_STEER, params)
            .await
    }

    pub async fn supervisor_decision_replace_pending_steer(
        &self,
        params: &ipc::SupervisorDecisionReplacePendingSteerRequest,
    ) -> TTResult<ipc::SupervisorDecisionReplacePendingSteerResponse> {
        self.request(
            ipc::methods::SUPERVISOR_DECISION_REPLACE_PENDING_STEER,
            params,
        )
        .await
    }

    pub async fn supervisor_decision_approve_and_send(
        &self,
        params: &ipc::SupervisorDecisionApproveAndSendRequest,
    ) -> TTResult<ipc::SupervisorDecisionApproveAndSendResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_APPROVE_AND_SEND, params)
            .await
    }

    pub async fn supervisor_decision_reject(
        &self,
        params: &ipc::SupervisorDecisionRejectRequest,
    ) -> TTResult<ipc::SupervisorDecisionRejectResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_REJECT, params)
            .await
    }

    pub async fn workunit_get(
        &self,
        params: &ipc::WorkunitGetRequest,
    ) -> TTResult<ipc::WorkunitGetResponse> {
        self.request(ipc::methods::WORKUNIT_GET, params).await
    }

    pub async fn planning_session_create(
        &self,
        params: &ipc::PlanningSessionCreateRequest,
    ) -> TTResult<ipc::PlanningSessionCreateResponse> {
        self.request(ipc::methods::PLANNING_SESSION_CREATE, params)
            .await
    }

    pub async fn planning_session_get(
        &self,
        params: &ipc::PlanningSessionGetRequest,
    ) -> TTResult<ipc::PlanningSessionGetResponse> {
        self.request(ipc::methods::PLANNING_SESSION_GET, params)
            .await
    }

    pub async fn planning_session_list(
        &self,
        params: &ipc::PlanningSessionListRequest,
    ) -> TTResult<ipc::PlanningSessionListResponse> {
        self.request(ipc::methods::PLANNING_SESSION_LIST, params)
            .await
    }

    pub async fn planning_session_update_summary(
        &self,
        params: &ipc::PlanningSessionUpdateSummaryRequest,
    ) -> TTResult<ipc::PlanningSessionUpdateSummaryResponse> {
        self.request(ipc::methods::PLANNING_SESSION_UPDATE_SUMMARY, params)
            .await
    }

    pub async fn planning_session_request_supervisor_context(
        &self,
        params: &ipc::PlanningSessionRequestSupervisorContextRequest,
    ) -> TTResult<ipc::PlanningSessionRequestSupervisorContextResponse> {
        self.request(
            ipc::methods::PLANNING_SESSION_REQUEST_SUPERVISOR_CONTEXT,
            params,
        )
        .await
    }

    pub async fn planning_session_request_research(
        &self,
        params: &ipc::PlanningSessionRequestResearchRequest,
    ) -> TTResult<ipc::PlanningSessionRequestResearchResponse> {
        self.request(ipc::methods::PLANNING_SESSION_REQUEST_RESEARCH, params)
            .await
    }

    pub async fn planning_session_mark_ready_for_review(
        &self,
        params: &ipc::PlanningSessionMarkReadyForReviewRequest,
    ) -> TTResult<ipc::PlanningSessionMarkReadyForReviewResponse> {
        self.request(ipc::methods::PLANNING_SESSION_MARK_READY_FOR_REVIEW, params)
            .await
    }

    pub async fn planning_session_abort(
        &self,
        params: &ipc::PlanningSessionAbortRequest,
    ) -> TTResult<ipc::PlanningSessionAbortResponse> {
        self.request(ipc::methods::PLANNING_SESSION_ABORT, params)
            .await
    }

    pub async fn planning_session_approve(
        &self,
        params: &ipc::PlanningSessionApproveRequest,
    ) -> TTResult<ipc::PlanningSessionApproveResponse> {
        self.request(ipc::methods::PLANNING_SESSION_APPROVE, params)
            .await
    }

    pub async fn planning_session_reject(
        &self,
        params: &ipc::PlanningSessionRejectRequest,
    ) -> TTResult<ipc::PlanningSessionRejectResponse> {
        self.request(ipc::methods::PLANNING_SESSION_REJECT, params)
            .await
    }

    pub async fn planning_session_supersede(
        &self,
        params: &ipc::PlanningSessionSupersedeRequest,
    ) -> TTResult<ipc::PlanningSessionSupersedeResponse> {
        self.request(ipc::methods::PLANNING_SESSION_SUPERSEDE, params)
            .await
    }

    pub async fn authority_hierarchy_get(
        &self,
        params: &ipc::AuthorityHierarchyGetRequest,
    ) -> TTResult<ipc::AuthorityHierarchyGetResponse> {
        self.request(ipc::methods::AUTHORITY_HIERARCHY_GET, params)
            .await
    }

    pub async fn authority_delete_plan(
        &self,
        params: &ipc::AuthorityDeletePlanRequest,
    ) -> TTResult<ipc::AuthorityDeletePlanResponse> {
        self.request(ipc::methods::AUTHORITY_DELETE_PLAN, params)
            .await
    }

    pub async fn authority_workstream_create(
        &self,
        params: &ipc::AuthorityWorkstreamCreateRequest,
    ) -> TTResult<ipc::AuthorityWorkstreamCreateResponse> {
        self.request(ipc::methods::AUTHORITY_WORKSTREAM_CREATE, params)
            .await
    }

    pub async fn authority_workstream_edit(
        &self,
        params: &ipc::AuthorityWorkstreamEditRequest,
    ) -> TTResult<ipc::AuthorityWorkstreamEditResponse> {
        self.request(ipc::methods::AUTHORITY_WORKSTREAM_EDIT, params)
            .await
    }

    pub async fn authority_workstream_delete(
        &self,
        params: &ipc::AuthorityWorkstreamDeleteRequest,
    ) -> TTResult<ipc::AuthorityWorkstreamDeleteResponse> {
        self.request(ipc::methods::AUTHORITY_WORKSTREAM_DELETE, params)
            .await
    }

    pub async fn authority_workstream_list(
        &self,
        params: &ipc::AuthorityWorkstreamListRequest,
    ) -> TTResult<ipc::AuthorityWorkstreamListResponse> {
        self.request(ipc::methods::AUTHORITY_WORKSTREAM_LIST, params)
            .await
    }

    pub async fn authority_workstream_get(
        &self,
        params: &ipc::AuthorityWorkstreamGetRequest,
    ) -> TTResult<ipc::AuthorityWorkstreamGetResponse> {
        self.request(ipc::methods::AUTHORITY_WORKSTREAM_GET, params)
            .await
    }

    pub async fn authority_workunit_create(
        &self,
        params: &ipc::AuthorityWorkunitCreateRequest,
    ) -> TTResult<ipc::AuthorityWorkunitCreateResponse> {
        self.request(ipc::methods::AUTHORITY_WORKUNIT_CREATE, params)
            .await
    }

    pub async fn authority_workunit_edit(
        &self,
        params: &ipc::AuthorityWorkunitEditRequest,
    ) -> TTResult<ipc::AuthorityWorkunitEditResponse> {
        self.request(ipc::methods::AUTHORITY_WORKUNIT_EDIT, params)
            .await
    }

    pub async fn authority_workunit_delete(
        &self,
        params: &ipc::AuthorityWorkunitDeleteRequest,
    ) -> TTResult<ipc::AuthorityWorkunitDeleteResponse> {
        self.request(ipc::methods::AUTHORITY_WORKUNIT_DELETE, params)
            .await
    }

    pub async fn authority_workunit_list(
        &self,
        params: &ipc::AuthorityWorkunitListRequest,
    ) -> TTResult<ipc::AuthorityWorkunitListResponse> {
        self.request(ipc::methods::AUTHORITY_WORKUNIT_LIST, params)
            .await
    }

    pub async fn authority_workunit_get(
        &self,
        params: &ipc::AuthorityWorkunitGetRequest,
    ) -> TTResult<ipc::AuthorityWorkunitGetResponse> {
        self.request(ipc::methods::AUTHORITY_WORKUNIT_GET, params)
            .await
    }

    pub async fn authority_tracked_thread_create(
        &self,
        params: &ipc::AuthorityTrackedThreadCreateRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadCreateResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_CREATE, params)
            .await
    }

    pub async fn authority_tracked_thread_edit(
        &self,
        params: &ipc::AuthorityTrackedThreadEditRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadEditResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_EDIT, params)
            .await
    }

    pub async fn authority_tracked_thread_delete(
        &self,
        params: &ipc::AuthorityTrackedThreadDeleteRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadDeleteResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_DELETE, params)
            .await
    }

    pub async fn authority_tracked_thread_list(
        &self,
        params: &ipc::AuthorityTrackedThreadListRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadListResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_LIST, params)
            .await
    }

    pub async fn authority_tracked_thread_get(
        &self,
        params: &ipc::AuthorityTrackedThreadGetRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadGetResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_GET, params)
            .await
    }

    pub async fn authority_events_export(
        &self,
        params: &ipc::AuthorityEventsExportRequest,
    ) -> TTResult<ipc::AuthorityEventsExportResponse> {
        self.request(ipc::methods::AUTHORITY_EVENTS_EXPORT, params)
            .await
    }

    pub async fn authority_events_ack(
        &self,
        params: &ipc::AuthorityEventsAckRequest,
    ) -> TTResult<ipc::AuthorityEventsAckResponse> {
        self.request(ipc::methods::AUTHORITY_EVENTS_ACK, params)
            .await
    }

    pub async fn authority_events_replay(
        &self,
        params: &ipc::AuthorityEventsReplayRequest,
    ) -> TTResult<ipc::AuthorityEventsReplayResponse> {
        self.request(ipc::methods::AUTHORITY_EVENTS_REPLAY, params)
            .await
    }

    pub async fn authority_tracked_thread_prepare_workspace(
        &self,
        params: &ipc::AuthorityTrackedThreadPrepareWorkspaceRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadPrepareWorkspaceResponse> {
        self.request(
            ipc::methods::AUTHORITY_TRACKED_THREAD_PREPARE_WORKSPACE,
            params,
        )
        .await
    }

    pub async fn authority_tracked_thread_refresh_workspace(
        &self,
        params: &ipc::AuthorityTrackedThreadRefreshWorkspaceRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadRefreshWorkspaceResponse> {
        self.request(
            ipc::methods::AUTHORITY_TRACKED_THREAD_REFRESH_WORKSPACE,
            params,
        )
        .await
    }

    pub async fn authority_tracked_thread_merge_prep(
        &self,
        params: &ipc::AuthorityTrackedThreadMergePrepRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadMergePrepResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_MERGE_PREP, params)
            .await
    }

    pub async fn authority_tracked_thread_authorize_merge(
        &self,
        params: &ipc::AuthorityTrackedThreadAuthorizeMergeRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadAuthorizeMergeResponse> {
        self.request(
            ipc::methods::AUTHORITY_TRACKED_THREAD_AUTHORIZE_MERGE,
            params,
        )
        .await
    }

    pub async fn authority_tracked_thread_execute_landing(
        &self,
        params: &ipc::AuthorityTrackedThreadExecuteLandingRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadExecuteLandingResponse> {
        self.request(
            ipc::methods::AUTHORITY_TRACKED_THREAD_EXECUTE_LANDING,
            params,
        )
        .await
    }

    pub async fn authority_tracked_thread_prune_workspace(
        &self,
        params: &ipc::AuthorityTrackedThreadPruneWorkspaceRequest,
    ) -> TTResult<ipc::AuthorityTrackedThreadPruneWorkspaceResponse> {
        self.request(
            ipc::methods::AUTHORITY_TRACKED_THREAD_PRUNE_WORKSPACE,
            params,
        )
        .await
    }

    pub async fn assignment_start(
        &self,
        params: &ipc::AssignmentStartRequest,
    ) -> TTResult<ipc::AssignmentStartResponse> {
        self.request(ipc::methods::ASSIGNMENT_START, params).await
    }

    pub async fn assignment_get(
        &self,
        params: &ipc::AssignmentGetRequest,
    ) -> TTResult<ipc::AssignmentGetResponse> {
        self.request(ipc::methods::ASSIGNMENT_GET, params).await
    }

    pub async fn tt_assignment_create(
        &self,
        params: &ipc::TTAssignmentCreateRequest,
    ) -> TTResult<ipc::TTAssignmentCreateResponse> {
        self.request(ipc::methods::RUNTIME_ASSIGNMENT_CREATE, params)
            .await
    }

    pub async fn tt_assignment_get(
        &self,
        params: &ipc::TTAssignmentGetRequest,
    ) -> TTResult<ipc::TTAssignmentGetResponse> {
        self.request(ipc::methods::RUNTIME_ASSIGNMENT_GET, params)
            .await
    }

    pub async fn tt_assignment_list(
        &self,
        params: &ipc::TTAssignmentListRequest,
    ) -> TTResult<ipc::TTAssignmentListResponse> {
        self.request(ipc::methods::RUNTIME_ASSIGNMENT_LIST, params)
            .await
    }

    pub async fn tt_assignment_pause(
        &self,
        params: &ipc::TTAssignmentPauseRequest,
    ) -> TTResult<ipc::TTAssignmentPauseResponse> {
        self.request(ipc::methods::RUNTIME_ASSIGNMENT_PAUSE, params)
            .await
    }

    pub async fn tt_assignment_resume(
        &self,
        params: &ipc::TTAssignmentResumeRequest,
    ) -> TTResult<ipc::TTAssignmentResumeResponse> {
        self.request(ipc::methods::RUNTIME_ASSIGNMENT_RESUME, params)
            .await
    }

    pub async fn tt_assignment_release(
        &self,
        params: &ipc::TTAssignmentReleaseRequest,
    ) -> TTResult<ipc::TTAssignmentReleaseResponse> {
        self.request(ipc::methods::RUNTIME_ASSIGNMENT_RELEASE, params)
            .await
    }

    pub async fn assignment_communication_get(
        &self,
        params: &ipc::AssignmentCommunicationGetRequest,
    ) -> TTResult<ipc::AssignmentCommunicationGetResponse> {
        self.request(ipc::methods::ASSIGNMENT_COMMUNICATION_GET, params)
            .await
    }

    pub async fn report_get(
        &self,
        params: &ipc::ReportGetRequest,
    ) -> TTResult<ipc::ReportGetResponse> {
        self.request(ipc::methods::REPORT_GET, params).await
    }

    pub async fn report_list_for_workunit(
        &self,
        params: &ipc::ReportListForWorkunitRequest,
    ) -> TTResult<ipc::ReportListForWorkunitResponse> {
        self.request(ipc::methods::REPORT_LIST_FOR_WORKUNIT, params)
            .await
    }

    pub async fn decision_apply(
        &self,
        params: &ipc::DecisionApplyRequest,
    ) -> TTResult<ipc::DecisionApplyResponse> {
        self.request(ipc::methods::DECISION_APPLY, params).await
    }

    pub async fn proposal_create(
        &self,
        params: &ipc::ProposalCreateRequest,
    ) -> TTResult<ipc::ProposalCreateResponse> {
        self.request(ipc::methods::PROPOSAL_CREATE, params).await
    }

    pub async fn proposal_get(
        &self,
        params: &ipc::ProposalGetRequest,
    ) -> TTResult<ipc::ProposalGetResponse> {
        self.request(ipc::methods::PROPOSAL_GET, params).await
    }

    pub async fn proposal_artifact_summary_get(
        &self,
        params: &ipc::ProposalArtifactSummaryGetRequest,
    ) -> TTResult<ipc::ProposalArtifactSummaryGetResponse> {
        self.request(ipc::methods::PROPOSAL_ARTIFACT_SUMMARY_GET, params)
            .await
    }

    pub async fn proposal_artifact_detail_get(
        &self,
        params: &ipc::ProposalArtifactDetailGetRequest,
    ) -> TTResult<ipc::ProposalArtifactDetailGetResponse> {
        self.request(ipc::methods::PROPOSAL_ARTIFACT_DETAIL_GET, params)
            .await
    }

    pub async fn proposal_artifact_export_get(
        &self,
        params: &ipc::ProposalArtifactExportGetRequest,
    ) -> TTResult<ipc::ProposalArtifactExportGetResponse> {
        self.request(ipc::methods::PROPOSAL_ARTIFACT_EXPORT_GET, params)
            .await
    }

    pub async fn proposal_artifact_summary_list_for_workunit(
        &self,
        params: &ipc::ProposalArtifactSummaryListForWorkunitRequest,
    ) -> TTResult<ipc::ProposalArtifactSummaryListForWorkunitResponse> {
        self.request(
            ipc::methods::PROPOSAL_ARTIFACT_SUMMARY_LIST_FOR_WORKUNIT,
            params,
        )
        .await
    }

    pub async fn proposal_list_for_workunit(
        &self,
        params: &ipc::ProposalListForWorkunitRequest,
    ) -> TTResult<ipc::ProposalListForWorkunitResponse> {
        self.request(ipc::methods::PROPOSAL_LIST_FOR_WORKUNIT, params)
            .await
    }

    pub async fn proposal_approve(
        &self,
        params: &ipc::ProposalApproveRequest,
    ) -> TTResult<ipc::ProposalApproveResponse> {
        self.request(ipc::methods::PROPOSAL_APPROVE, params).await
    }

    pub async fn proposal_reject(
        &self,
        params: &ipc::ProposalRejectRequest,
    ) -> TTResult<ipc::ProposalRejectResponse> {
        self.request(ipc::methods::PROPOSAL_REJECT, params).await
    }

    pub async fn subscribe_events(
        &self,
        include_snapshot: bool,
    ) -> TTResult<(EventSubscription, Option<ipc::StateSnapshot>)> {
        // Snapshot-first recovery happens above this layer: request the current
        // snapshot first, then consume incremental events from this connection.
        debug!(
            socket = self.socket.as_str(),
            include_snapshot, "starting TT daemon event subscription"
        );
        let events = self.subscribe();
        let response: ipc::EventsSubscribeResponse = self
            .request(
                ipc::methods::EVENTS_SUBSCRIBE,
                &ipc::EventsSubscribeRequest { include_snapshot },
            )
            .await?;
        info!(
            socket = self.socket.as_str(),
            include_snapshot,
            snapshot_included = response.snapshot.is_some(),
            "TT daemon event subscription ready"
        );
        Ok((events, response.snapshot))
    }

    fn from_stream(stream: UnixStream, socket: String) -> TTResult<Arc<Self>> {
        let (read_half, mut write_half) = stream.into_split();
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<String>(256);
        let (event_tx, _) = broadcast::channel(512);
        let client = Arc::new(Self {
            pending: Mutex::new(HashMap::new()),
            outbound: outbound_tx,
            event_tx: RwLock::new(Some(event_tx)),
            closed: std::sync::atomic::AtomicBool::new(false),
            next_request_id: AtomicI64::new(1),
            socket,
        });

        let client_write = Arc::clone(&client);
        tokio::spawn(async move {
            while let Some(raw) = outbound_rx.recv().await {
                if let Err(error) = write_half.write_all(raw.as_bytes()).await {
                    warn!(
                        socket = client_write.socket.as_str(),
                        error = %error,
                        "TT IPC client write failed"
                    );
                    client_write
                        .close_connection(format!("TT daemon write failed: {error}").as_str())
                        .await;
                    break;
                }
                if let Err(error) = write_half.write_all(b"\n").await {
                    warn!(
                        socket = client_write.socket.as_str(),
                        error = %error,
                        "TT IPC client write framing failed"
                    );
                    client_write
                        .close_connection(format!("TT daemon write failed: {error}").as_str())
                        .await;
                    break;
                }
            }
            debug!(
                socket = client_write.socket.as_str(),
                "TT IPC client write loop stopped"
            );
        });

        let client_read = Arc::clone(&client);
        tokio::spawn(async move {
            let mut lines = BufReader::new(read_half).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if let Err(error) = client_read.handle_line(&line).await {
                            warn!(
                                socket = client_read.socket.as_str(),
                                error = %error,
                                "TT IPC client protocol handling failed"
                            );
                            client_read
                                .close_connection(error.to_string().as_str())
                                .await;
                            break;
                        }
                    }
                    Ok(None) => {
                        info!(
                            socket = client_read.socket.as_str(),
                            "TT IPC client disconnected"
                        );
                        client_read
                            .close_connection("TT daemon connection closed")
                            .await;
                        break;
                    }
                    Err(error) => {
                        warn!(
                            socket = client_read.socket.as_str(),
                            error = %error,
                            "TT IPC client read failed"
                        );
                        client_read
                            .close_connection(format!("TT daemon read failed: {error}").as_str())
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(client)
    }

    async fn handle_line(&self, raw: &str) -> TTResult<()> {
        let message: JsonRpcMessage = serde_json::from_str(raw).map_err(|error| {
            warn!(
                socket = self.socket.as_str(),
                error = %error,
                "failed to decode TT daemon JSON-RPC message"
            );
            error
        })?;
        match message {
            JsonRpcMessage::Response(response) => self.resolve_response(response).await,
            JsonRpcMessage::Error(error) => self.resolve_error(error).await,
            JsonRpcMessage::Notification(notification) => {
                self.handle_notification(notification).await
            }
            JsonRpcMessage::Request(request) => {
                let error = JsonRpcMessage::Error(JsonRpcError {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    error: JsonRpcErrorObject {
                        code: -32601,
                        message: "TT IPC client does not serve requests".to_string(),
                        data: None,
                    },
                });
                let raw = serde_json::to_string(&error)?;
                self.outbound.send(raw).await.map_err(|send_error| {
                    TTError::Transport(format!("failed to reject daemon request: {send_error}"))
                })?;
            }
        }
        Ok(())
    }

    async fn handle_notification(&self, notification: JsonRpcNotification) {
        if notification.method == ipc::methods::EVENTS_NOTIFICATION
            && let Some(params) = notification.params
            && let Ok(event) = serde_json::from_value::<ipc::EventsNotification>(params)
        {
            let event_tx = self
                .event_tx
                .read()
                .expect("event sender lock poisoned")
                .as_ref()
                .cloned();
            if let Some(event_tx) = event_tx {
                let _ = event_tx.send(event.event);
            }
        }
    }

    async fn resolve_response(&self, response: JsonRpcResponse) {
        if let Some(pending) = self.pending.lock().await.remove(&response.id) {
            let _ = pending.send(Ok(response.result));
        }
    }

    async fn resolve_error(&self, error: JsonRpcError) {
        if let Some(pending) = self.pending.lock().await.remove(&error.id) {
            let _ = pending.send(Err(TTError::Protocol(format!(
                "json-rpc error {}: {}",
                error.error.code, error.error.message
            ))));
        }
    }

    async fn fail_pending(&self, message: &str) {
        let mut pending = self.pending.lock().await;
        for (_, waiter) in pending.drain() {
            let _ = waiter.send(Err(TTError::Transport(message.to_string())));
        }
    }

    async fn close_connection(&self, message: &str) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        // Event subscriptions are scoped to one daemon socket lifetime. Closing the sender makes
        // existing receivers terminate cleanly so callers resubscribe after reconnect instead of
        // hanging as if missed events could be replayed from the old connection.
        self.close_event_stream();
        self.fail_pending(message).await;
    }

    fn close_event_stream(&self) {
        self.event_tx
            .write()
            .expect("event sender lock poisoned")
            .take();
    }

    async fn request<T>(&self, method: &str, params: &impl Serialize) -> TTResult<T>
    where
        T: DeserializeOwned,
    {
        // Once the client is closed, the request path is intentionally hard
        // fail: a dead socket must be rebuilt rather than recovered in place.
        if self.closed.load(Ordering::Acquire) {
            return Err(TTError::Transport(format!(
                "TT daemon connection is closed; cannot send `{method}` request"
            )));
        }
        let start = Instant::now();
        let payload = serde_json::to_value(params)?;
        let request_id = RequestId::Integer(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        if self.closed.load(Ordering::Acquire) {
            self.pending.lock().await.remove(&request_id);
            return Err(TTError::Transport(format!(
                "TT daemon connection is closed; cannot send `{method}` request"
            )));
        }

        debug!(
            socket = self.socket.as_str(),
            request_id = %request_id_label(&request_id),
            method,
            "sending TT IPC request"
        );
        let request = JsonRpcRequest::new(request_id.clone(), method, Some(payload));
        let raw = serde_json::to_string(&request)?;
        if let Err(error) = self.outbound.send(raw).await {
            self.pending.lock().await.remove(&request_id);
            warn!(
                socket = self.socket.as_str(),
                request_id = %request_id_label(&request_id),
                method,
                duration_ms = start.elapsed().as_millis() as u64,
                error = %error,
                "failed to send TT IPC request"
            );
            return Err(TTError::Transport(format!(
                "failed to send `{method}` request: {error}"
            )));
        }

        let response = timeout(Self::REQUEST_TIMEOUT, rx).await;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                warn!(
                    socket = self.socket.as_str(),
                    request_id = %request_id_label(&request_id),
                    method,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "TT IPC request timed out"
                );
                return Err(TTError::Transport(format!(
                    "timed out waiting for `{method}` response"
                )));
            }
        };
        let response = response.map_err(|error| {
            warn!(
                socket = self.socket.as_str(),
                request_id = %request_id_label(&request_id),
                method,
                duration_ms = start.elapsed().as_millis() as u64,
                error = %error,
                "TT IPC response channel dropped"
            );
            TTError::Transport(format!("response channel dropped for `{method}`: {error}"))
        })?;
        let response = response?;
        let decoded = serde_json::from_value(response).map_err(|error| {
            warn!(
                socket = self.socket.as_str(),
                request_id = %request_id_label(&request_id),
                method,
                duration_ms = start.elapsed().as_millis() as u64,
                error = %error,
                "failed to decode TT IPC response"
            );
            error
        })?;
        debug!(
            socket = self.socket.as_str(),
            request_id = %request_id_label(&request_id),
            method,
            duration_ms = start.elapsed().as_millis() as u64,
            "TT IPC request completed"
        );
        Ok(decoded)
    }
}

fn closed_event_subscription() -> EventSubscription {
    let (tx, rx) = broadcast::channel(1);
    drop(tx);
    rx
}

fn request_id_label(request_id: &RequestId) -> String {
    match request_id {
        RequestId::Integer(value) => value.to_string(),
        RequestId::String(value) => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::time::{Duration, timeout};

    use tt_core::ipc::{
        self, CollaborationSnapshot, DaemonEvent, DaemonEventEnvelope, DaemonRuntimeMetadata,
        SessionState, StateSnapshot, ThreadSummary, ThreadView,
    };

    use super::*;

    struct IpcTestServer {
        lines: Lines<BufReader<OwnedReadHalf>>,
        write: Option<OwnedWriteHalf>,
    }

    impl IpcTestServer {
        async fn recv_message(&mut self) -> JsonRpcMessage {
            let line = self
                .lines
                .next_line()
                .await
                .expect("read server line")
                .expect("expected client request line");
            serde_json::from_str(&line).expect("client request should be valid JSON-RPC")
        }

        async fn send_message(&mut self, message: JsonRpcMessage) {
            let raw = serde_json::to_string(&message).expect("serialize server message");
            self.send_raw(&raw).await;
        }

        async fn send_raw(&mut self, raw: &str) {
            let write = self
                .write
                .as_mut()
                .expect("server write half already closed");
            write
                .write_all(raw.as_bytes())
                .await
                .expect("write raw server payload");
            write.write_all(b"\n").await.expect("write server newline");
            write.flush().await.expect("flush server payload");
        }

        fn close(&mut self) {
            self.write = None;
        }
    }

    fn test_client_and_server() -> (Arc<TTIpcClient>, IpcTestServer) {
        let (client_stream, server_stream) = UnixStream::pair().expect("create UnixStream pair");
        let client = TTIpcClient::from_stream(client_stream, "/tmp/test-ttd.sock".to_string())
            .expect("create ipc client");
        let (read_half, write_half) = server_stream.into_split();
        (
            client,
            IpcTestServer {
                lines: BufReader::new(read_half).lines(),
                write: Some(write_half),
            },
        )
    }

    fn sample_daemon_status() -> ipc::DaemonStatusResponse {
        ipc::DaemonStatusResponse {
            socket_path: "/tmp/tt.sock".to_string(),
            metadata_path: "/tmp/tt.json".to_string(),
            tt_endpoint: "ws://tt.test".to_string(),
            tt_binary_path: "/usr/bin/tt".to_string(),
            upstream: tt_core::ConnectionState {
                endpoint: "ws://tt.test".to_string(),
                status: "connected".to_string(),
                detail: None,
            },
            client_count: 2,
            known_threads: 1,
            runtime: DaemonRuntimeMetadata {
                pid: 1000,
                started_at: Utc::now(),
                version: "0.1.0".to_string(),
                build_fingerprint: "test-build".to_string(),
                binary_path: "/usr/bin/ttd".to_string(),
                socket_path: "/tmp/tt.sock".to_string(),
                metadata_path: "/tmp/tt.json".to_string(),
                git_commit: Some("deadbeef".to_string()),
            },
            workstream_runtimes: Vec::new(),
        }
    }

    fn sample_thread_summary(id: &str) -> ThreadSummary {
        ThreadSummary {
            id: id.to_string(),
            preview: "thread preview".to_string(),
            name: Some("Thread".to_string()),
            model_provider: "openai".to_string(),
            cwd: "/tmp".to_string(),
            endpoint: None,
            runtime_workstream_id: None,
            owner_workstream_id: None,
            status: "idle".to_string(),
            created_at: 1,
            updated_at: 2,
            scope: "workspace".to_string(),
            archived: false,
            loaded_status: ipc::ThreadLoadedStatus::Idle,
            active_flags: Vec::new(),
            active_turn_id: None,
            last_seen_turn_id: None,
            recent_output: Some("output".to_string()),
            recent_event: Some("event".to_string()),
            turn_in_flight: false,
            monitor_state: ipc::ThreadMonitorState::Detached,
            last_sync_at: Utc::now(),
            management_state: ipc::ThreadManagementState::Managed,
            source_kind: Some("workspace".to_string()),
            raw_summary: None,
        }
    }

    fn sample_snapshot() -> StateSnapshot {
        StateSnapshot {
            daemon: sample_daemon_status(),
            session: SessionState::default(),
            threads: vec![sample_thread_summary("thread-1")],
            active_thread: Some(ThreadView {
                summary: sample_thread_summary("thread-1"),
                history_loaded: true,
                turns: Vec::new(),
            }),
            collaboration: CollaborationSnapshot::default(),
            operator_inbox: ipc::OperatorInboxState::default(),
            recent_events: Vec::new(),
        }
    }

    async fn recv_request(server: &mut IpcTestServer) -> JsonRpcRequest {
        match server.recv_message().await {
            JsonRpcMessage::Request(request) => request,
            other => panic!("expected request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn daemon_status_round_trip_uses_jsonrpc_newline_framing() {
        let (client, mut server) = test_client_and_server();

        let response_task = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.daemon_status().await })
        };
        let request = recv_request(&mut server).await;
        assert_eq!(request.method, ipc::methods::DAEMON_STATUS);
        server
            .send_message(JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: serde_json::to_value(sample_daemon_status())
                    .expect("serialize daemon status response"),
            }))
            .await;

        let response = response_task
            .await
            .expect("join daemon status task")
            .expect("daemon status succeeds");
        assert_eq!(response.client_count, 2);
        assert_eq!(response.runtime.pid, 1000);
    }

    #[tokio::test]
    async fn jsonrpc_errors_are_propagated() {
        let (client, mut server) = test_client_and_server();

        let response_task = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.daemon_status().await })
        };
        let request = recv_request(&mut server).await;
        server
            .send_message(JsonRpcMessage::Error(JsonRpcError {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                error: JsonRpcErrorObject {
                    code: -32001,
                    message: "daemon unavailable".to_string(),
                    data: None,
                },
            }))
            .await;

        let error = response_task
            .await
            .expect("join daemon status task")
            .expect_err("request should fail");
        assert!(
            matches!(error, TTError::Protocol(message) if message.contains("json-rpc error -32001: daemon unavailable"))
        );
    }

    #[tokio::test]
    async fn subscribe_events_returns_snapshot_and_fanouts_notifications() {
        let (client, mut server) = test_client_and_server();
        let mut secondary = client.subscribe();

        let subscribe_task = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.subscribe_events(true).await })
        };
        let request = recv_request(&mut server).await;
        assert_eq!(request.method, ipc::methods::EVENTS_SUBSCRIBE);
        assert_eq!(
            request.params.expect("subscribe params")["include_snapshot"],
            json!(true)
        );
        let snapshot = sample_snapshot();
        server
            .send_message(JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: serde_json::to_value(ipc::EventsSubscribeResponse {
                    subscribed: true,
                    snapshot: Some(snapshot.clone()),
                })
                .expect("serialize events subscribe response"),
            }))
            .await;

        let (mut primary, returned_snapshot) = subscribe_task
            .await
            .expect("join subscribe task")
            .expect("subscribe succeeds");
        let returned_snapshot = returned_snapshot.expect("snapshot should be included");
        assert_eq!(returned_snapshot.threads[0].id, snapshot.threads[0].id);

        for message in ["first", "second"] {
            server
                .send_message(JsonRpcMessage::Notification(JsonRpcNotification::new(
                    ipc::methods::EVENTS_NOTIFICATION,
                    Some(
                        serde_json::to_value(ipc::EventsNotification {
                            event: DaemonEventEnvelope::new(DaemonEvent::Warning {
                                message: message.to_string(),
                            }),
                        })
                        .expect("serialize events notification"),
                    ),
                )))
                .await;
        }

        let primary_first = primary.recv().await.expect("primary first notification");
        let secondary_first = secondary
            .recv()
            .await
            .expect("secondary first notification");
        let primary_second = primary.recv().await.expect("primary second notification");

        match primary_first.event {
            DaemonEvent::Warning { message } => assert_eq!(message, "first"),
            other => panic!("expected warning event, got {other:?}"),
        }
        match secondary_first.event {
            DaemonEvent::Warning { message } => assert_eq!(message, "first"),
            other => panic!("expected warning event, got {other:?}"),
        }
        match primary_second.event {
            DaemonEvent::Warning { message } => assert_eq!(message, "second"),
            other => panic!("expected warning event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn server_requests_are_rejected_with_method_not_found() {
        let (client, mut server) = test_client_and_server();
        drop(client);

        server
            .send_message(JsonRpcMessage::Request(JsonRpcRequest::new(
                RequestId::Integer(41),
                "daemon/push",
                None,
            )))
            .await;

        match server.recv_message().await {
            JsonRpcMessage::Error(JsonRpcError { id, error, .. }) => {
                assert_eq!(id, RequestId::Integer(41));
                assert_eq!(error.code, -32601);
                assert_eq!(error.message, "TT IPC client does not serve requests");
            }
            other => panic!("expected method-not-found error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invalid_payload_fails_pending_request() {
        let (client, mut server) = test_client_and_server();

        let pending = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.state_get().await })
        };
        let request = recv_request(&mut server).await;
        assert_eq!(request.method, ipc::methods::STATE_GET);
        server.send_raw("not-json").await;

        let error = pending
            .await
            .expect("join pending request")
            .expect_err("invalid payload should fail request");
        match error {
            TTError::Transport(message) => {
                assert!(!message.is_empty());
                assert_ne!(message, "TT daemon connection closed");
            }
            other => panic!("expected transport error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn server_close_fails_pending_request() {
        let (client, mut server) = test_client_and_server();

        let pending = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.state_get().await })
        };
        let request = recv_request(&mut server).await;
        assert_eq!(request.method, ipc::methods::STATE_GET);
        server.close();

        let error = pending
            .await
            .expect("join pending request")
            .expect_err("closed server should fail request");
        assert!(
            matches!(error, TTError::Transport(message) if message.contains("TT daemon connection closed"))
        );
    }

    #[tokio::test]
    async fn server_close_closes_existing_event_subscriptions() {
        let (client, mut server) = test_client_and_server();

        let subscribe_task = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.subscribe_events(false).await })
        };
        let request = recv_request(&mut server).await;
        assert_eq!(request.method, ipc::methods::EVENTS_SUBSCRIBE);
        server
            .send_message(JsonRpcMessage::Response(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: serde_json::to_value(ipc::EventsSubscribeResponse {
                    subscribed: true,
                    snapshot: None,
                })
                .expect("serialize subscribe response"),
            }))
            .await;

        let (mut events, _) = subscribe_task
            .await
            .expect("join subscribe task")
            .expect("subscribe succeeds");
        server.close();

        let recv_result = timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("event receiver should resolve after server close");
        assert!(matches!(
            recv_result,
            Err(tokio::sync::broadcast::error::RecvError::Closed)
        ));
    }

    #[tokio::test]
    async fn requests_fail_immediately_after_server_close() {
        let (client, mut server) = test_client_and_server();

        let pending = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.state_get().await })
        };
        let request = recv_request(&mut server).await;
        assert_eq!(request.method, ipc::methods::STATE_GET);
        server.close();
        pending
            .await
            .expect("join pending request")
            .expect_err("closed server should fail request");

        let error = client
            .state_get()
            .await
            .expect_err("subsequent request should fail immediately");
        assert!(matches!(
            error,
            TTError::Transport(message)
                if message.contains("TT daemon connection is closed")
        ));
    }
}
