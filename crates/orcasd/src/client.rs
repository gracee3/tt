//! Socket-lifetime IPC client for the Orcas daemon.
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

use orcas_core::ipc;
use orcas_core::jsonrpc::{
    JsonRpcError, JsonRpcErrorObject, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, RequestId,
};
use orcas_core::{AppPaths, OrcasError, OrcasResult};

type PendingResponse = oneshot::Sender<OrcasResult<Value>>;
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
pub struct OrcasIpcClient {
    pending: Mutex<HashMap<RequestId, PendingResponse>>,
    outbound: mpsc::Sender<String>,
    event_tx: RwLock<Option<broadcast::Sender<ipc::DaemonEventEnvelope>>>,
    closed: std::sync::atomic::AtomicBool,
    next_request_id: AtomicI64,
    socket: String,
}

impl OrcasIpcClient {
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

    pub async fn connect(paths: &AppPaths) -> OrcasResult<Arc<Self>> {
        let start = Instant::now();
        let socket = paths.socket_file.display().to_string();
        info!(socket, "connecting Orcas IPC client");
        let stream = UnixStream::connect(&paths.socket_file)
            .await
            .map_err(|error| {
                OrcasError::Transport(format!(
                    "failed to connect to Orcas daemon at {}: {error}",
                    paths.socket_file.display()
                ))
            })?;
        let client = Self::from_stream(stream, socket.clone());
        if client.is_ok() {
            info!(
                socket,
                connected = true,
                duration_ms = start.elapsed().as_millis() as u64,
                "Orcas IPC client connected"
            );
        }
        client
    }

    pub fn subscribe(&self) -> EventSubscription {
        // Subscriptions are per-socket, not per logical daemon identity.
        debug!(
            socket = self.socket.as_str(),
            "subscribing to Orcas daemon events"
        );
        self.event_tx
            .read()
            .expect("event sender lock poisoned")
            .as_ref()
            .map(broadcast::Sender::subscribe)
            .unwrap_or_else(closed_event_subscription)
    }

    pub async fn daemon_status(&self) -> OrcasResult<ipc::DaemonStatusResponse> {
        self.request(ipc::methods::DAEMON_STATUS, &ipc::Empty::default())
            .await
    }

    pub async fn daemon_connect(&self) -> OrcasResult<ipc::DaemonConnectResponse> {
        self.request(
            ipc::methods::DAEMON_CONNECT,
            &ipc::DaemonConnectRequest::default(),
        )
        .await
    }

    pub async fn daemon_stop(&self) -> OrcasResult<ipc::DaemonStopResponse> {
        self.request(
            ipc::methods::DAEMON_STOP,
            &ipc::DaemonStopRequest::default(),
        )
        .await
    }

    pub async fn state_get(&self) -> OrcasResult<ipc::StateGetResponse> {
        self.request(ipc::methods::STATE_GET, &ipc::StateGetRequest::default())
            .await
    }

    pub async fn session_get_active(&self) -> OrcasResult<ipc::SessionGetActiveResponse> {
        self.request(
            ipc::methods::SESSION_GET_ACTIVE,
            &ipc::SessionGetActiveRequest::default(),
        )
        .await
    }

    pub async fn models_list(&self) -> OrcasResult<ipc::ModelsListResponse> {
        self.request(ipc::methods::MODELS_LIST, &ipc::Empty::default())
            .await
    }

    pub async fn threads_list(&self) -> OrcasResult<ipc::ThreadsListResponse> {
        self.request(
            ipc::methods::THREADS_LIST,
            &ipc::ThreadsListRequest::default(),
        )
        .await
    }

    pub async fn threads_list_scoped(&self) -> OrcasResult<ipc::ThreadsListResponse> {
        self.request(
            ipc::methods::THREADS_LIST_SCOPED,
            &ipc::ThreadsListScopedRequest::default(),
        )
        .await
    }

    pub async fn threads_list_loaded(&self) -> OrcasResult<ipc::ThreadsListResponse> {
        self.request(
            ipc::methods::THREADS_LIST_LOADED,
            &ipc::ThreadsListLoadedRequest::default(),
        )
        .await
    }

    pub async fn thread_start(
        &self,
        params: &ipc::ThreadStartRequest,
    ) -> OrcasResult<ipc::ThreadStartResponse> {
        self.request(ipc::methods::THREAD_START, params).await
    }

    pub async fn thread_read(
        &self,
        params: &ipc::ThreadReadRequest,
    ) -> OrcasResult<ipc::ThreadReadResponse> {
        self.request(ipc::methods::THREAD_READ, params).await
    }

    pub async fn thread_read_history(
        &self,
        params: &ipc::ThreadReadHistoryRequest,
    ) -> OrcasResult<ipc::ThreadReadHistoryResponse> {
        self.request(ipc::methods::THREAD_READ_HISTORY, params)
            .await
    }

    pub async fn thread_get(
        &self,
        params: &ipc::ThreadGetRequest,
    ) -> OrcasResult<ipc::ThreadGetResponse> {
        self.request(ipc::methods::THREAD_GET, params).await
    }

    pub async fn thread_attach(
        &self,
        params: &ipc::ThreadAttachRequest,
    ) -> OrcasResult<ipc::ThreadAttachResponse> {
        self.request(ipc::methods::THREAD_ATTACH, params).await
    }

    pub async fn thread_detach(
        &self,
        params: &ipc::ThreadDetachRequest,
    ) -> OrcasResult<ipc::ThreadDetachResponse> {
        self.request(ipc::methods::THREAD_DETACH, params).await
    }

    pub async fn thread_resume(
        &self,
        params: &ipc::ThreadResumeRequest,
    ) -> OrcasResult<ipc::ThreadResumeResponse> {
        self.request(ipc::methods::THREAD_RESUME, params).await
    }

    pub async fn turns_recent(
        &self,
        params: &ipc::TurnsRecentRequest,
    ) -> OrcasResult<ipc::TurnsRecentResponse> {
        self.request(ipc::methods::TURNS_RECENT, params).await
    }

    pub async fn turns_list_active(&self) -> OrcasResult<ipc::TurnsListActiveResponse> {
        self.request(
            ipc::methods::TURNS_LIST_ACTIVE,
            &ipc::TurnsListActiveRequest::default(),
        )
        .await
    }

    pub async fn turn_get(
        &self,
        params: &ipc::TurnGetRequest,
    ) -> OrcasResult<ipc::TurnGetResponse> {
        self.request(ipc::methods::TURN_GET, params).await
    }

    pub async fn turn_attach(
        &self,
        params: &ipc::TurnAttachRequest,
    ) -> OrcasResult<ipc::TurnAttachResponse> {
        self.request(ipc::methods::TURN_ATTACH, params).await
    }

    pub async fn turn_start(
        &self,
        params: &ipc::TurnStartRequest,
    ) -> OrcasResult<ipc::TurnStartResponse> {
        self.request(ipc::methods::TURN_START, params).await
    }

    pub async fn turn_steer(
        &self,
        params: &ipc::TurnSteerRequest,
    ) -> OrcasResult<ipc::TurnSteerResponse> {
        self.request(ipc::methods::TURN_STEER, params).await
    }

    pub async fn turn_interrupt(&self, params: &ipc::TurnInterruptRequest) -> OrcasResult<()> {
        let _: ipc::Empty = self.request(ipc::methods::TURN_INTERRUPT, params).await?;
        Ok(())
    }

    pub async fn supervisor_decision_list(
        &self,
        params: &ipc::SupervisorDecisionListRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionListResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_LIST, params)
            .await
    }

    pub async fn supervisor_decision_get(
        &self,
        params: &ipc::SupervisorDecisionGetRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionGetResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_GET, params)
            .await
    }

    pub async fn supervisor_decision_propose_interrupt(
        &self,
        params: &ipc::SupervisorDecisionProposeInterruptRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionProposeInterruptResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_PROPOSE_INTERRUPT, params)
            .await
    }

    pub async fn supervisor_decision_record_no_action(
        &self,
        params: &ipc::SupervisorDecisionRecordNoActionRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionRecordNoActionResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_RECORD_NO_ACTION, params)
            .await
    }

    pub async fn supervisor_decision_manual_refresh(
        &self,
        params: &ipc::SupervisorDecisionManualRefreshRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionManualRefreshResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_MANUAL_REFRESH, params)
            .await
    }

    pub async fn supervisor_decision_propose_steer(
        &self,
        params: &ipc::SupervisorDecisionProposeSteerRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionProposeSteerResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_PROPOSE_STEER, params)
            .await
    }

    pub async fn supervisor_decision_replace_pending_steer(
        &self,
        params: &ipc::SupervisorDecisionReplacePendingSteerRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionReplacePendingSteerResponse> {
        self.request(
            ipc::methods::SUPERVISOR_DECISION_REPLACE_PENDING_STEER,
            params,
        )
        .await
    }

    pub async fn supervisor_decision_approve_and_send(
        &self,
        params: &ipc::SupervisorDecisionApproveAndSendRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionApproveAndSendResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_APPROVE_AND_SEND, params)
            .await
    }

    pub async fn supervisor_decision_reject(
        &self,
        params: &ipc::SupervisorDecisionRejectRequest,
    ) -> OrcasResult<ipc::SupervisorDecisionRejectResponse> {
        self.request(ipc::methods::SUPERVISOR_DECISION_REJECT, params)
            .await
    }

    pub async fn workunit_get(
        &self,
        params: &ipc::WorkunitGetRequest,
    ) -> OrcasResult<ipc::WorkunitGetResponse> {
        self.request(ipc::methods::WORKUNIT_GET, params).await
    }

    pub async fn authority_hierarchy_get(
        &self,
        params: &ipc::AuthorityHierarchyGetRequest,
    ) -> OrcasResult<ipc::AuthorityHierarchyGetResponse> {
        self.request(ipc::methods::AUTHORITY_HIERARCHY_GET, params)
            .await
    }

    pub async fn authority_delete_plan(
        &self,
        params: &ipc::AuthorityDeletePlanRequest,
    ) -> OrcasResult<ipc::AuthorityDeletePlanResponse> {
        self.request(ipc::methods::AUTHORITY_DELETE_PLAN, params)
            .await
    }

    pub async fn authority_workstream_create(
        &self,
        params: &ipc::AuthorityWorkstreamCreateRequest,
    ) -> OrcasResult<ipc::AuthorityWorkstreamCreateResponse> {
        self.request(ipc::methods::AUTHORITY_WORKSTREAM_CREATE, params)
            .await
    }

    pub async fn authority_workstream_edit(
        &self,
        params: &ipc::AuthorityWorkstreamEditRequest,
    ) -> OrcasResult<ipc::AuthorityWorkstreamEditResponse> {
        self.request(ipc::methods::AUTHORITY_WORKSTREAM_EDIT, params)
            .await
    }

    pub async fn authority_workstream_delete(
        &self,
        params: &ipc::AuthorityWorkstreamDeleteRequest,
    ) -> OrcasResult<ipc::AuthorityWorkstreamDeleteResponse> {
        self.request(ipc::methods::AUTHORITY_WORKSTREAM_DELETE, params)
            .await
    }

    pub async fn authority_workstream_list(
        &self,
        params: &ipc::AuthorityWorkstreamListRequest,
    ) -> OrcasResult<ipc::AuthorityWorkstreamListResponse> {
        self.request(ipc::methods::AUTHORITY_WORKSTREAM_LIST, params)
            .await
    }

    pub async fn authority_workstream_get(
        &self,
        params: &ipc::AuthorityWorkstreamGetRequest,
    ) -> OrcasResult<ipc::AuthorityWorkstreamGetResponse> {
        self.request(ipc::methods::AUTHORITY_WORKSTREAM_GET, params)
            .await
    }

    pub async fn authority_workunit_create(
        &self,
        params: &ipc::AuthorityWorkunitCreateRequest,
    ) -> OrcasResult<ipc::AuthorityWorkunitCreateResponse> {
        self.request(ipc::methods::AUTHORITY_WORKUNIT_CREATE, params)
            .await
    }

    pub async fn authority_workunit_edit(
        &self,
        params: &ipc::AuthorityWorkunitEditRequest,
    ) -> OrcasResult<ipc::AuthorityWorkunitEditResponse> {
        self.request(ipc::methods::AUTHORITY_WORKUNIT_EDIT, params)
            .await
    }

    pub async fn authority_workunit_delete(
        &self,
        params: &ipc::AuthorityWorkunitDeleteRequest,
    ) -> OrcasResult<ipc::AuthorityWorkunitDeleteResponse> {
        self.request(ipc::methods::AUTHORITY_WORKUNIT_DELETE, params)
            .await
    }

    pub async fn authority_workunit_list(
        &self,
        params: &ipc::AuthorityWorkunitListRequest,
    ) -> OrcasResult<ipc::AuthorityWorkunitListResponse> {
        self.request(ipc::methods::AUTHORITY_WORKUNIT_LIST, params)
            .await
    }

    pub async fn authority_workunit_get(
        &self,
        params: &ipc::AuthorityWorkunitGetRequest,
    ) -> OrcasResult<ipc::AuthorityWorkunitGetResponse> {
        self.request(ipc::methods::AUTHORITY_WORKUNIT_GET, params)
            .await
    }

    pub async fn authority_tracked_thread_create(
        &self,
        params: &ipc::AuthorityTrackedThreadCreateRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadCreateResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_CREATE, params)
            .await
    }

    pub async fn authority_tracked_thread_edit(
        &self,
        params: &ipc::AuthorityTrackedThreadEditRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadEditResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_EDIT, params)
            .await
    }

    pub async fn authority_tracked_thread_delete(
        &self,
        params: &ipc::AuthorityTrackedThreadDeleteRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadDeleteResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_DELETE, params)
            .await
    }

    pub async fn authority_tracked_thread_list(
        &self,
        params: &ipc::AuthorityTrackedThreadListRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadListResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_LIST, params)
            .await
    }

    pub async fn authority_tracked_thread_get(
        &self,
        params: &ipc::AuthorityTrackedThreadGetRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadGetResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_GET, params)
            .await
    }

    pub async fn authority_tracked_thread_prepare_workspace(
        &self,
        params: &ipc::AuthorityTrackedThreadPrepareWorkspaceRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadPrepareWorkspaceResponse> {
        self.request(
            ipc::methods::AUTHORITY_TRACKED_THREAD_PREPARE_WORKSPACE,
            params,
        )
        .await
    }

    pub async fn authority_tracked_thread_refresh_workspace(
        &self,
        params: &ipc::AuthorityTrackedThreadRefreshWorkspaceRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadRefreshWorkspaceResponse> {
        self.request(
            ipc::methods::AUTHORITY_TRACKED_THREAD_REFRESH_WORKSPACE,
            params,
        )
        .await
    }

    pub async fn authority_tracked_thread_merge_prep(
        &self,
        params: &ipc::AuthorityTrackedThreadMergePrepRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadMergePrepResponse> {
        self.request(ipc::methods::AUTHORITY_TRACKED_THREAD_MERGE_PREP, params)
            .await
    }

    pub async fn authority_tracked_thread_authorize_merge(
        &self,
        params: &ipc::AuthorityTrackedThreadAuthorizeMergeRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadAuthorizeMergeResponse> {
        self.request(
            ipc::methods::AUTHORITY_TRACKED_THREAD_AUTHORIZE_MERGE,
            params,
        )
        .await
    }

    pub async fn authority_tracked_thread_execute_landing(
        &self,
        params: &ipc::AuthorityTrackedThreadExecuteLandingRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadExecuteLandingResponse> {
        self.request(
            ipc::methods::AUTHORITY_TRACKED_THREAD_EXECUTE_LANDING,
            params,
        )
        .await
    }

    pub async fn authority_tracked_thread_prune_workspace(
        &self,
        params: &ipc::AuthorityTrackedThreadPruneWorkspaceRequest,
    ) -> OrcasResult<ipc::AuthorityTrackedThreadPruneWorkspaceResponse> {
        self.request(
            ipc::methods::AUTHORITY_TRACKED_THREAD_PRUNE_WORKSPACE,
            params,
        )
        .await
    }

    pub async fn assignment_start(
        &self,
        params: &ipc::AssignmentStartRequest,
    ) -> OrcasResult<ipc::AssignmentStartResponse> {
        self.request(ipc::methods::ASSIGNMENT_START, params).await
    }

    pub async fn assignment_get(
        &self,
        params: &ipc::AssignmentGetRequest,
    ) -> OrcasResult<ipc::AssignmentGetResponse> {
        self.request(ipc::methods::ASSIGNMENT_GET, params).await
    }

    pub async fn codex_assignment_create(
        &self,
        params: &ipc::CodexAssignmentCreateRequest,
    ) -> OrcasResult<ipc::CodexAssignmentCreateResponse> {
        self.request(ipc::methods::CODEX_ASSIGNMENT_CREATE, params)
            .await
    }

    pub async fn codex_assignment_get(
        &self,
        params: &ipc::CodexAssignmentGetRequest,
    ) -> OrcasResult<ipc::CodexAssignmentGetResponse> {
        self.request(ipc::methods::CODEX_ASSIGNMENT_GET, params)
            .await
    }

    pub async fn codex_assignment_list(
        &self,
        params: &ipc::CodexAssignmentListRequest,
    ) -> OrcasResult<ipc::CodexAssignmentListResponse> {
        self.request(ipc::methods::CODEX_ASSIGNMENT_LIST, params)
            .await
    }

    pub async fn codex_assignment_pause(
        &self,
        params: &ipc::CodexAssignmentPauseRequest,
    ) -> OrcasResult<ipc::CodexAssignmentPauseResponse> {
        self.request(ipc::methods::CODEX_ASSIGNMENT_PAUSE, params)
            .await
    }

    pub async fn codex_assignment_resume(
        &self,
        params: &ipc::CodexAssignmentResumeRequest,
    ) -> OrcasResult<ipc::CodexAssignmentResumeResponse> {
        self.request(ipc::methods::CODEX_ASSIGNMENT_RESUME, params)
            .await
    }

    pub async fn codex_assignment_release(
        &self,
        params: &ipc::CodexAssignmentReleaseRequest,
    ) -> OrcasResult<ipc::CodexAssignmentReleaseResponse> {
        self.request(ipc::methods::CODEX_ASSIGNMENT_RELEASE, params)
            .await
    }

    pub async fn assignment_communication_get(
        &self,
        params: &ipc::AssignmentCommunicationGetRequest,
    ) -> OrcasResult<ipc::AssignmentCommunicationGetResponse> {
        self.request(ipc::methods::ASSIGNMENT_COMMUNICATION_GET, params)
            .await
    }

    pub async fn report_get(
        &self,
        params: &ipc::ReportGetRequest,
    ) -> OrcasResult<ipc::ReportGetResponse> {
        self.request(ipc::methods::REPORT_GET, params).await
    }

    pub async fn report_list_for_workunit(
        &self,
        params: &ipc::ReportListForWorkunitRequest,
    ) -> OrcasResult<ipc::ReportListForWorkunitResponse> {
        self.request(ipc::methods::REPORT_LIST_FOR_WORKUNIT, params)
            .await
    }

    pub async fn decision_apply(
        &self,
        params: &ipc::DecisionApplyRequest,
    ) -> OrcasResult<ipc::DecisionApplyResponse> {
        self.request(ipc::methods::DECISION_APPLY, params).await
    }

    pub async fn proposal_create(
        &self,
        params: &ipc::ProposalCreateRequest,
    ) -> OrcasResult<ipc::ProposalCreateResponse> {
        self.request(ipc::methods::PROPOSAL_CREATE, params).await
    }

    pub async fn proposal_get(
        &self,
        params: &ipc::ProposalGetRequest,
    ) -> OrcasResult<ipc::ProposalGetResponse> {
        self.request(ipc::methods::PROPOSAL_GET, params).await
    }

    pub async fn proposal_artifact_summary_get(
        &self,
        params: &ipc::ProposalArtifactSummaryGetRequest,
    ) -> OrcasResult<ipc::ProposalArtifactSummaryGetResponse> {
        self.request(ipc::methods::PROPOSAL_ARTIFACT_SUMMARY_GET, params)
            .await
    }

    pub async fn proposal_artifact_detail_get(
        &self,
        params: &ipc::ProposalArtifactDetailGetRequest,
    ) -> OrcasResult<ipc::ProposalArtifactDetailGetResponse> {
        self.request(ipc::methods::PROPOSAL_ARTIFACT_DETAIL_GET, params)
            .await
    }

    pub async fn proposal_artifact_export_get(
        &self,
        params: &ipc::ProposalArtifactExportGetRequest,
    ) -> OrcasResult<ipc::ProposalArtifactExportGetResponse> {
        self.request(ipc::methods::PROPOSAL_ARTIFACT_EXPORT_GET, params)
            .await
    }

    pub async fn proposal_artifact_summary_list_for_workunit(
        &self,
        params: &ipc::ProposalArtifactSummaryListForWorkunitRequest,
    ) -> OrcasResult<ipc::ProposalArtifactSummaryListForWorkunitResponse> {
        self.request(
            ipc::methods::PROPOSAL_ARTIFACT_SUMMARY_LIST_FOR_WORKUNIT,
            params,
        )
        .await
    }

    pub async fn proposal_list_for_workunit(
        &self,
        params: &ipc::ProposalListForWorkunitRequest,
    ) -> OrcasResult<ipc::ProposalListForWorkunitResponse> {
        self.request(ipc::methods::PROPOSAL_LIST_FOR_WORKUNIT, params)
            .await
    }

    pub async fn proposal_approve(
        &self,
        params: &ipc::ProposalApproveRequest,
    ) -> OrcasResult<ipc::ProposalApproveResponse> {
        self.request(ipc::methods::PROPOSAL_APPROVE, params).await
    }

    pub async fn proposal_reject(
        &self,
        params: &ipc::ProposalRejectRequest,
    ) -> OrcasResult<ipc::ProposalRejectResponse> {
        self.request(ipc::methods::PROPOSAL_REJECT, params).await
    }

    pub async fn subscribe_events(
        &self,
        include_snapshot: bool,
    ) -> OrcasResult<(EventSubscription, Option<ipc::StateSnapshot>)> {
        // Snapshot-first recovery happens above this layer: request the current
        // snapshot first, then consume incremental events from this connection.
        debug!(
            socket = self.socket.as_str(),
            include_snapshot, "starting Orcas daemon event subscription"
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
            "Orcas daemon event subscription ready"
        );
        Ok((events, response.snapshot))
    }

    fn from_stream(stream: UnixStream, socket: String) -> OrcasResult<Arc<Self>> {
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
                        "Orcas IPC client write failed"
                    );
                    client_write
                        .close_connection(format!("Orcas daemon write failed: {error}").as_str())
                        .await;
                    break;
                }
                if let Err(error) = write_half.write_all(b"\n").await {
                    warn!(
                        socket = client_write.socket.as_str(),
                        error = %error,
                        "Orcas IPC client write framing failed"
                    );
                    client_write
                        .close_connection(format!("Orcas daemon write failed: {error}").as_str())
                        .await;
                    break;
                }
            }
            debug!(
                socket = client_write.socket.as_str(),
                "Orcas IPC client write loop stopped"
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
                                "Orcas IPC client protocol handling failed"
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
                            "Orcas IPC client disconnected"
                        );
                        client_read
                            .close_connection("Orcas daemon connection closed")
                            .await;
                        break;
                    }
                    Err(error) => {
                        warn!(
                            socket = client_read.socket.as_str(),
                            error = %error,
                            "Orcas IPC client read failed"
                        );
                        client_read
                            .close_connection(format!("Orcas daemon read failed: {error}").as_str())
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(client)
    }

    async fn handle_line(&self, raw: &str) -> OrcasResult<()> {
        let message: JsonRpcMessage = serde_json::from_str(raw).map_err(|error| {
            warn!(
                socket = self.socket.as_str(),
                error = %error,
                "failed to decode Orcas daemon JSON-RPC message"
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
                        message: "Orcas IPC client does not serve requests".to_string(),
                        data: None,
                    },
                });
                let raw = serde_json::to_string(&error)?;
                self.outbound.send(raw).await.map_err(|send_error| {
                    OrcasError::Transport(format!("failed to reject daemon request: {send_error}"))
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
            let _ = pending.send(Err(OrcasError::Protocol(format!(
                "json-rpc error {}: {}",
                error.error.code, error.error.message
            ))));
        }
    }

    async fn fail_pending(&self, message: &str) {
        let mut pending = self.pending.lock().await;
        for (_, waiter) in pending.drain() {
            let _ = waiter.send(Err(OrcasError::Transport(message.to_string())));
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

    async fn request<T>(&self, method: &str, params: &impl Serialize) -> OrcasResult<T>
    where
        T: DeserializeOwned,
    {
        // Once the client is closed, the request path is intentionally hard
        // fail: a dead socket must be rebuilt rather than recovered in place.
        if self.closed.load(Ordering::Acquire) {
            return Err(OrcasError::Transport(format!(
                "Orcas daemon connection is closed; cannot send `{method}` request"
            )));
        }
        let start = Instant::now();
        let payload = serde_json::to_value(params)?;
        let request_id = RequestId::Integer(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        if self.closed.load(Ordering::Acquire) {
            self.pending.lock().await.remove(&request_id);
            return Err(OrcasError::Transport(format!(
                "Orcas daemon connection is closed; cannot send `{method}` request"
            )));
        }

        debug!(
            socket = self.socket.as_str(),
            request_id = %request_id_label(&request_id),
            method,
            "sending Orcas IPC request"
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
                "failed to send Orcas IPC request"
            );
            return Err(OrcasError::Transport(format!(
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
                    "Orcas IPC request timed out"
                );
                return Err(OrcasError::Transport(format!(
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
                "Orcas IPC response channel dropped"
            );
            OrcasError::Transport(format!("response channel dropped for `{method}`: {error}"))
        })?;
        let response = response?;
        let decoded = serde_json::from_value(response).map_err(|error| {
            warn!(
                socket = self.socket.as_str(),
                request_id = %request_id_label(&request_id),
                method,
                duration_ms = start.elapsed().as_millis() as u64,
                error = %error,
                "failed to decode Orcas IPC response"
            );
            error
        })?;
        debug!(
            socket = self.socket.as_str(),
            request_id = %request_id_label(&request_id),
            method,
            duration_ms = start.elapsed().as_millis() as u64,
            "Orcas IPC request completed"
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

    use orcas_core::ipc::{
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

    fn test_client_and_server() -> (Arc<OrcasIpcClient>, IpcTestServer) {
        let (client_stream, server_stream) = UnixStream::pair().expect("create UnixStream pair");
        let client =
            OrcasIpcClient::from_stream(client_stream, "/tmp/test-orcasd.sock".to_string())
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
            socket_path: "/tmp/orcas.sock".to_string(),
            metadata_path: "/tmp/orcas.json".to_string(),
            codex_endpoint: "ws://codex.test".to_string(),
            codex_binary_path: "/usr/bin/codex".to_string(),
            upstream: orcas_core::ConnectionState {
                endpoint: "ws://codex.test".to_string(),
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
                binary_path: "/usr/bin/orcasd".to_string(),
                socket_path: "/tmp/orcas.sock".to_string(),
                metadata_path: "/tmp/orcas.json".to_string(),
                git_commit: Some("deadbeef".to_string()),
            },
        }
    }

    fn sample_thread_summary(id: &str) -> ThreadSummary {
        ThreadSummary {
            id: id.to_string(),
            preview: "thread preview".to_string(),
            name: Some("Thread".to_string()),
            model_provider: "openai".to_string(),
            cwd: "/tmp".to_string(),
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
            matches!(error, OrcasError::Protocol(message) if message.contains("json-rpc error -32001: daemon unavailable"))
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
                assert_eq!(error.message, "Orcas IPC client does not serve requests");
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
            OrcasError::Transport(message) => {
                assert!(!message.is_empty());
                assert_ne!(message, "Orcas daemon connection closed");
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
            matches!(error, OrcasError::Transport(message) if message.contains("Orcas daemon connection closed"))
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
            OrcasError::Transport(message)
                if message.contains("Orcas daemon connection is closed")
        ));
    }
}
