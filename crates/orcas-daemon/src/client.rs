use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio::time::{Duration, timeout};

use orcas_core::ipc;
use orcas_core::jsonrpc::{
    JsonRpcError, JsonRpcErrorObject, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, RequestId,
};
use orcas_core::{AppPaths, OrcasError, OrcasResult};

type PendingResponse = oneshot::Sender<OrcasResult<Value>>;
pub type EventSubscription = broadcast::Receiver<ipc::DaemonEventEnvelope>;

pub struct OrcasIpcClient {
    pending: Mutex<HashMap<RequestId, PendingResponse>>,
    outbound: mpsc::Sender<String>,
    event_tx: broadcast::Sender<ipc::DaemonEventEnvelope>,
    next_request_id: AtomicI64,
}

impl OrcasIpcClient {
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

    pub async fn connect(paths: &AppPaths) -> OrcasResult<Arc<Self>> {
        let stream = UnixStream::connect(&paths.socket_file)
            .await
            .map_err(|error| {
                OrcasError::Transport(format!(
                    "failed to connect to Orcas daemon at {}: {error}",
                    paths.socket_file.display()
                ))
            })?;
        Self::from_stream(stream)
    }

    pub fn subscribe(&self) -> EventSubscription {
        self.event_tx.subscribe()
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

    pub async fn workstream_create(
        &self,
        params: &ipc::WorkstreamCreateRequest,
    ) -> OrcasResult<ipc::WorkstreamCreateResponse> {
        self.request(ipc::methods::WORKSTREAM_CREATE, params).await
    }

    pub async fn workstream_list(&self) -> OrcasResult<ipc::WorkstreamListResponse> {
        self.request(
            ipc::methods::WORKSTREAM_LIST,
            &ipc::WorkstreamListRequest::default(),
        )
        .await
    }

    pub async fn workstream_get(
        &self,
        params: &ipc::WorkstreamGetRequest,
    ) -> OrcasResult<ipc::WorkstreamGetResponse> {
        self.request(ipc::methods::WORKSTREAM_GET, params).await
    }

    pub async fn workunit_create(
        &self,
        params: &ipc::WorkunitCreateRequest,
    ) -> OrcasResult<ipc::WorkunitCreateResponse> {
        self.request(ipc::methods::WORKUNIT_CREATE, params).await
    }

    pub async fn workunit_list(
        &self,
        params: &ipc::WorkunitListRequest,
    ) -> OrcasResult<ipc::WorkunitListResponse> {
        self.request(ipc::methods::WORKUNIT_LIST, params).await
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
        let events = self.subscribe();
        let response: ipc::EventsSubscribeResponse = self
            .request(
                ipc::methods::EVENTS_SUBSCRIBE,
                &ipc::EventsSubscribeRequest { include_snapshot },
            )
            .await?;
        Ok((events, response.snapshot))
    }

    fn from_stream(stream: UnixStream) -> OrcasResult<Arc<Self>> {
        let (read_half, mut write_half) = stream.into_split();
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<String>(256);
        let (event_tx, _) = broadcast::channel(512);
        let client = Arc::new(Self {
            pending: Mutex::new(HashMap::new()),
            outbound: outbound_tx,
            event_tx,
            next_request_id: AtomicI64::new(1),
        });

        tokio::spawn(async move {
            while let Some(raw) = outbound_rx.recv().await {
                if write_half.write_all(raw.as_bytes()).await.is_err() {
                    break;
                }
                if write_half.write_all(b"\n").await.is_err() {
                    break;
                }
            }
        });

        let client_read = Arc::clone(&client);
        tokio::spawn(async move {
            let mut lines = BufReader::new(read_half).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if let Err(error) = client_read.handle_line(&line).await {
                            client_read.fail_pending(error.to_string().as_str()).await;
                            break;
                        }
                    }
                    Ok(None) => {
                        client_read
                            .fail_pending("Orcas daemon connection closed")
                            .await;
                        break;
                    }
                    Err(error) => {
                        client_read
                            .fail_pending(format!("Orcas daemon read failed: {error}").as_str())
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(client)
    }

    async fn handle_line(&self, raw: &str) -> OrcasResult<()> {
        let message: JsonRpcMessage = serde_json::from_str(raw)?;
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
            let _ = self.event_tx.send(event.event);
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

    async fn request<T>(&self, method: &str, params: &impl Serialize) -> OrcasResult<T>
    where
        T: DeserializeOwned,
    {
        let payload = serde_json::to_value(params)?;
        let request_id = RequestId::Integer(self.next_request_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        let request = JsonRpcRequest::new(request_id.clone(), method, Some(payload));
        let raw = serde_json::to_string(&request)?;
        if let Err(error) = self.outbound.send(raw).await {
            self.pending.lock().await.remove(&request_id);
            return Err(OrcasError::Transport(format!(
                "failed to send `{method}` request: {error}"
            )));
        }

        let response = timeout(Self::REQUEST_TIMEOUT, rx).await;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                return Err(OrcasError::Transport(format!(
                    "timed out waiting for `{method}` response"
                )));
            }
        };
        let response = response.map_err(|error| {
            OrcasError::Transport(format!("response channel dropped for `{method}`: {error}"))
        })?;
        let response = response?;

        serde_json::from_value(response).map_err(Into::into)
    }
}
