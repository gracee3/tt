//! Backend command surface used by the TUI.
//!
//! The commands here are not a flat bag of interchangeable RPCs. Some target
//! canonical authority planning reads/writes, some target collaboration/runtime
//! surfaces, and one retained path exists only for runtime-detail reads. The
//! distinction matters for both the production backend and the fake backend
//! used in tests.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use orcas_core::{
    AppPaths, Assignment, Decision, Report, SupervisorProposalRecord, SupervisorTurnDecision,
    WorkUnit, authority, ipc,
};
use orcasd::{OrcasDaemonLaunch, OrcasDaemonProcessManager, OrcasIpcClient, OrcasRuntimeOverrides};

/// Commands issued by the TUI backend.
///
/// Most commands fall into canonical authority planning, collaboration/runtime
/// reads, or operator actions. `GetWorkUnit` is the retained runtime-detail
/// exception and should not accumulate new planning behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendCommand {
    GetAuthorityHierarchy {
        include_deleted: bool,
    },
    GetAuthorityDeletePlan {
        target: authority::DeleteTarget,
    },
    GetAuthorityWorkstream {
        workstream_id: authority::WorkstreamId,
    },
    GetAuthorityWorkUnit {
        work_unit_id: authority::WorkUnitId,
    },
    GetAuthorityTrackedThread {
        tracked_thread_id: authority::TrackedThreadId,
    },
    CreateAuthorityWorkstream {
        command: authority::CreateWorkstream,
    },
    EditAuthorityWorkstream {
        command: authority::EditWorkstream,
    },
    DeleteAuthorityWorkstream {
        command: authority::DeleteWorkstream,
    },
    CreateAuthorityWorkUnit {
        command: authority::CreateWorkUnit,
    },
    EditAuthorityWorkUnit {
        command: authority::EditWorkUnit,
    },
    DeleteAuthorityWorkUnit {
        command: authority::DeleteWorkUnit,
    },
    CreateAuthorityTrackedThread {
        command: authority::CreateTrackedThread,
    },
    EditAuthorityTrackedThread {
        command: authority::EditTrackedThread,
    },
    DeleteAuthorityTrackedThread {
        command: authority::DeleteTrackedThread,
    },
    GetThread {
        thread_id: String,
    },
    AttachThread {
        thread_id: String,
    },
    GetTurn {
        thread_id: String,
        turn_id: String,
    },
    GetWorkUnit {
        work_unit_id: String,
    },
    GetProposalArtifactSummaryListForWorkUnit {
        work_unit_id: String,
    },
    GetProposalArtifactSummary {
        proposal_id: String,
    },
    GetProposalArtifactDetail {
        proposal_id: String,
    },
    GetProposalArtifactExport {
        proposal_id: String,
    },
    GetActiveTurns,
    LoadModels,
    StartDaemon,
    StopDaemon,
    SubmitPrompt {
        thread_id: String,
        text: String,
    },
    ProposeSteerSupervisorDecision {
        assignment_id: String,
        proposed_text: String,
    },
    ReplacePendingSteerSupervisorDecision {
        decision_id: String,
        proposed_text: String,
    },
    ProposeInterruptSupervisorDecision {
        assignment_id: String,
    },
    RecordNoActionSupervisorDecision {
        decision_id: String,
    },
    ManualRefreshSupervisorDecision {
        assignment_id: String,
    },
    ApproveSupervisorDecision {
        decision_id: String,
    },
    RejectSupervisorDecision {
        decision_id: String,
    },
}

/// Results returned by the TUI backend.
///
/// The result family mirrors the command classification: authority planning,
/// collaboration/runtime, retained runtime-detail, and operator lifecycle
/// actions.
#[derive(Debug, Clone)]
pub enum BackendCommandResult {
    AuthorityHierarchy(authority::HierarchySnapshot),
    AuthorityDeletePlan(authority::DeletePlan),
    AuthorityWorkstreamDetail(ipc::AuthorityWorkstreamGetResponse),
    AuthorityWorkUnitDetail(ipc::AuthorityWorkunitGetResponse),
    AuthorityTrackedThreadDetail(ipc::AuthorityTrackedThreadGetResponse),
    AuthorityWorkstream(authority::WorkstreamRecord),
    AuthorityWorkUnit(authority::WorkUnitRecord),
    AuthorityTrackedThread(authority::TrackedThreadRecord),
    Snapshot(ipc::StateSnapshot),
    Thread(ipc::ThreadView),
    ThreadAttached(ipc::ThreadAttachResponse),
    Turn(ipc::TurnAttachResponse),
    WorkUnit(ipc::WorkunitGetResponse),
    ProposalArtifactSummaryListForWorkUnit(ipc::ProposalArtifactSummaryListForWorkunitResponse),
    ProposalArtifactSummary(ipc::SupervisorProposalArtifactSummary),
    ProposalArtifactDetail(ipc::SupervisorProposalArtifactDetail),
    ProposalArtifactExport(ipc::SupervisorProposalArtifactExport),
    ActiveTurns(Vec<ipc::TurnStateView>),
    Models(Vec<ipc::ModelSummary>),
    DaemonStarted { connected: bool },
    DaemonStopped { stopping: bool },
    PromptStarted { thread_id: String, turn_id: String },
    SupervisorDecision(SupervisorTurnDecision),
}

#[async_trait]
/// TUI backend abstraction.
///
/// Production uses the daemon-backed implementation, while tests can provide a
/// fake backend. Both must preserve the same command classification even when
/// the transport is mocked.
pub trait TuiBackend: Send + Sync {
    async fn get_snapshot(&self) -> Result<ipc::StateSnapshot>;
    async fn subscribe_events(&self) -> Result<mpsc::Receiver<ipc::DaemonEventEnvelope>>;
    async fn execute(&self, command: BackendCommand) -> Result<BackendCommandResult>;
}

pub struct OrcasDaemonBackend {
    paths: AppPaths,
    daemon: OrcasDaemonProcessManager,
    client: Mutex<Option<Arc<OrcasIpcClient>>>,
}

impl OrcasDaemonBackend {
    pub async fn discover() -> Result<Self> {
        Self::discover_with_overrides(OrcasRuntimeOverrides::default()).await
    }

    pub async fn discover_with_overrides(overrides: OrcasRuntimeOverrides) -> Result<Self> {
        let paths = AppPaths::discover()?;
        paths.ensure().await?;
        let daemon = OrcasDaemonProcessManager::new(paths.clone(), overrides);
        Ok(Self {
            paths,
            daemon,
            client: Mutex::new(None),
        })
    }

    async fn ensure_client(&self, launch: OrcasDaemonLaunch) -> Result<Arc<OrcasIpcClient>> {
        if let Some(client) = self.client.lock().await.clone() {
            return Ok(client);
        }

        debug!(
            launch = launch_label(launch),
            "TUI backend establishing daemon client"
        );
        self.connect_client(launch).await
    }

    async fn connect_client(&self, launch: OrcasDaemonLaunch) -> Result<Arc<OrcasIpcClient>> {
        let start = Instant::now();
        let socket = self.paths.socket_file.display().to_string();
        info!(
            socket,
            launch = launch_label(launch),
            "TUI backend connecting to daemon"
        );
        self.daemon.ensure_running(launch).await?;
        let client = OrcasIpcClient::connect(&self.paths).await?;
        client.daemon_connect().await?;
        let mut guard = self.client.lock().await;
        *guard = Some(Arc::clone(&client));
        info!(
            socket,
            duration_ms = start.elapsed().as_millis() as u64,
            "TUI backend connected to daemon"
        );
        Ok(client)
    }

    async fn invalidate_client(&self) {
        let mut guard = self.client.lock().await;
        *guard = None;
        debug!(
            socket = %self.paths.socket_file.display(),
            "TUI backend invalidated cached daemon client"
        );
    }
}

#[async_trait]
impl TuiBackend for OrcasDaemonBackend {
    async fn get_snapshot(&self) -> Result<ipc::StateSnapshot> {
        let client = self.ensure_client(OrcasDaemonLaunch::Never).await?;
        match client.state_get().await {
            Ok(response) => Ok(response.snapshot),
            Err(_) => {
                warn!(
                    socket = %self.paths.socket_file.display(),
                    "TUI backend lost daemon snapshot connection; reconnecting"
                );
                self.invalidate_client().await;
                let client = self.connect_client(OrcasDaemonLaunch::Never).await?;
                Ok(client.state_get().await?.snapshot)
            }
        }
    }

    async fn subscribe_events(&self) -> Result<mpsc::Receiver<ipc::DaemonEventEnvelope>> {
        let client = self.ensure_client(OrcasDaemonLaunch::Never).await?;
        let (events, _) = match client.subscribe_events(false).await {
            Ok(response) => response,
            Err(_) => {
                warn!(
                    socket = %self.paths.socket_file.display(),
                    "TUI backend event subscription dropped; reconnecting"
                );
                self.invalidate_client().await;
                let client = self.connect_client(OrcasDaemonLaunch::Never).await?;
                client.subscribe_events(false).await?
            }
        };
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            let mut events = events;
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(rx)
    }

    async fn execute(&self, command: BackendCommand) -> Result<BackendCommandResult> {
        let started_at = Instant::now();
        let review_meta = review_command_target(&command)
            .map(|(action, target_id, target_field)| (action, target_id.to_string(), target_field));
        let authoring_meta = authoring_command_target(&command)
            .map(|(action, target_id, target_field)| (action, target_id.to_string(), target_field));
        if let Some((action, target_id, target_field)) = review_meta.as_ref() {
            log_review_action_start(action, target_field, target_id);
        }
        if let Some((action, target_id, target_field)) = authoring_meta.as_ref() {
            log_authoring_action_start(action, target_field, target_id);
        }
        let launch = match command {
            BackendCommand::StartDaemon => OrcasDaemonLaunch::IfNeeded,
            _ => OrcasDaemonLaunch::Never,
        };
        let client = match self.ensure_client(launch).await {
            Ok(client) => client,
            Err(error) => {
                if let Some((action, target_id, target_field)) = review_meta.as_ref() {
                    log_review_action_failure(
                        action,
                        target_field,
                        target_id,
                        started_at.elapsed().as_millis() as u64,
                        &error,
                    );
                }
                if let Some((action, target_id, target_field)) = authoring_meta.as_ref() {
                    log_authoring_action_failure(
                        action,
                        target_field,
                        target_id,
                        started_at.elapsed().as_millis() as u64,
                        &error,
                    );
                }
                return Err(error);
            }
        };
        match Self::execute_with_client(&client, command.clone()).await {
            Ok(result) => {
                if let (
                    Some((action, target_id, target_field)),
                    BackendCommandResult::SupervisorDecision(decision),
                ) = (review_meta.as_ref(), &result)
                {
                    log_review_action_success(
                        action,
                        target_field,
                        target_id,
                        decision,
                        started_at.elapsed().as_millis() as u64,
                    );
                }
                if let (
                    Some((action, target_id, target_field)),
                    BackendCommandResult::SupervisorDecision(decision),
                ) = (authoring_meta.as_ref(), &result)
                {
                    log_authoring_action_success(
                        action,
                        target_field,
                        target_id,
                        decision,
                        started_at.elapsed().as_millis() as u64,
                    );
                }
                Ok(result)
            }
            Err(error) => {
                let retry_launch = if Self::should_restart_for_error(&command, &error) {
                    warn!(
                        command = backend_command_label(&command),
                        error = %error,
                        "TUI backend restarting daemon after authority method mismatch"
                    );
                    self.daemon.restart().await?;
                    info!(
                        command = backend_command_label(&command),
                        "TUI backend daemon restart completed"
                    );
                    OrcasDaemonLaunch::Never
                } else {
                    warn!(
                        command = backend_command_label(&command),
                        error = %error,
                        "TUI backend retrying command after daemon client failure"
                    );
                    launch
                };
                self.invalidate_client().await;
                let client = match self.connect_client(retry_launch).await {
                    Ok(client) => client,
                    Err(error) => {
                        if let Some((action, target_id, target_field)) = review_meta.as_ref() {
                            log_review_action_failure(
                                action,
                                target_field,
                                target_id,
                                started_at.elapsed().as_millis() as u64,
                                &error,
                            );
                        }
                        if let Some((action, target_id, target_field)) = authoring_meta.as_ref() {
                            log_authoring_action_failure(
                                action,
                                target_field,
                                target_id,
                                started_at.elapsed().as_millis() as u64,
                                &error,
                            );
                        }
                        return Err(error);
                    }
                };
                let retried = Self::execute_with_client(&client, command).await;
                match &retried {
                    Ok(result) => {
                        info!("TUI backend reconnect succeeded");
                        if let Some((action, target_id, target_field)) = review_meta.as_ref() {
                            if let BackendCommandResult::SupervisorDecision(decision) = result {
                                log_review_action_success(
                                    action,
                                    target_field,
                                    target_id,
                                    decision,
                                    started_at.elapsed().as_millis() as u64,
                                );
                            }
                        }
                        if let Some((action, target_id, target_field)) = authoring_meta.as_ref() {
                            if let BackendCommandResult::SupervisorDecision(decision) = result {
                                log_authoring_action_success(
                                    action,
                                    target_field,
                                    target_id,
                                    decision,
                                    started_at.elapsed().as_millis() as u64,
                                );
                            }
                        }
                    }
                    Err(error) => {
                        warn!(error = %error, "TUI backend reconnect retry failed");
                        if let Some((action, target_id, target_field)) = review_meta.as_ref() {
                            log_review_action_failure(
                                action,
                                target_field,
                                target_id,
                                started_at.elapsed().as_millis() as u64,
                                error,
                            );
                        }
                        if let Some((action, target_id, target_field)) = authoring_meta.as_ref() {
                            log_authoring_action_failure(
                                action,
                                target_field,
                                target_id,
                                started_at.elapsed().as_millis() as u64,
                                error,
                            );
                        }
                    }
                }
                retried
            }
        }
    }
}

fn launch_label(launch: OrcasDaemonLaunch) -> &'static str {
    match launch {
        OrcasDaemonLaunch::Never => "never",
        OrcasDaemonLaunch::IfNeeded => "if_needed",
        OrcasDaemonLaunch::Always => "always",
    }
}

fn backend_command_label(command: &BackendCommand) -> &'static str {
    match command {
        BackendCommand::GetAuthorityHierarchy { .. } => "get_authority_hierarchy",
        BackendCommand::GetAuthorityDeletePlan { .. } => "get_authority_delete_plan",
        BackendCommand::GetAuthorityWorkstream { .. } => "get_authority_workstream",
        BackendCommand::GetAuthorityWorkUnit { .. } => "get_authority_work_unit",
        BackendCommand::GetAuthorityTrackedThread { .. } => "get_authority_tracked_thread",
        BackendCommand::CreateAuthorityWorkstream { .. } => "create_authority_workstream",
        BackendCommand::EditAuthorityWorkstream { .. } => "edit_authority_workstream",
        BackendCommand::DeleteAuthorityWorkstream { .. } => "delete_authority_workstream",
        BackendCommand::CreateAuthorityWorkUnit { .. } => "create_authority_work_unit",
        BackendCommand::EditAuthorityWorkUnit { .. } => "edit_authority_work_unit",
        BackendCommand::DeleteAuthorityWorkUnit { .. } => "delete_authority_work_unit",
        BackendCommand::CreateAuthorityTrackedThread { .. } => "create_authority_tracked_thread",
        BackendCommand::EditAuthorityTrackedThread { .. } => "edit_authority_tracked_thread",
        BackendCommand::DeleteAuthorityTrackedThread { .. } => "delete_authority_tracked_thread",
        BackendCommand::GetThread { .. } => "get_thread",
        BackendCommand::AttachThread { .. } => "attach_thread",
        BackendCommand::GetTurn { .. } => "get_turn",
        BackendCommand::GetWorkUnit { .. } => "get_work_unit",
        BackendCommand::GetProposalArtifactSummaryListForWorkUnit { .. } => {
            "get_proposal_artifact_summary_list_for_workunit"
        }
        BackendCommand::GetProposalArtifactSummary { .. } => "get_proposal_artifact_summary",
        BackendCommand::GetProposalArtifactDetail { .. } => "get_proposal_artifact_detail",
        BackendCommand::GetProposalArtifactExport { .. } => "get_proposal_artifact_export",
        BackendCommand::GetActiveTurns => "get_active_turns",
        BackendCommand::LoadModels => "load_models",
        BackendCommand::StartDaemon => "start_daemon",
        BackendCommand::StopDaemon => "stop_daemon",
        BackendCommand::SubmitPrompt { .. } => "submit_prompt",
        BackendCommand::ProposeSteerSupervisorDecision { .. } => {
            "propose_steer_supervisor_decision"
        }
        BackendCommand::ReplacePendingSteerSupervisorDecision { .. } => {
            "replace_pending_steer_supervisor_decision"
        }
        BackendCommand::ProposeInterruptSupervisorDecision { .. } => {
            "propose_interrupt_supervisor_decision"
        }
        BackendCommand::RecordNoActionSupervisorDecision { .. } => {
            "record_no_action_supervisor_decision"
        }
        BackendCommand::ManualRefreshSupervisorDecision { .. } => {
            "manual_refresh_supervisor_decision"
        }
        BackendCommand::ApproveSupervisorDecision { .. } => "approve_supervisor_decision",
        BackendCommand::RejectSupervisorDecision { .. } => "reject_supervisor_decision",
    }
}

impl OrcasDaemonBackend {
    fn is_authority_command(command: &BackendCommand) -> bool {
        matches!(
            command,
            BackendCommand::GetAuthorityHierarchy { .. }
                | BackendCommand::GetAuthorityDeletePlan { .. }
                | BackendCommand::GetAuthorityWorkstream { .. }
                | BackendCommand::GetAuthorityWorkUnit { .. }
                | BackendCommand::GetAuthorityTrackedThread { .. }
                | BackendCommand::CreateAuthorityWorkstream { .. }
                | BackendCommand::EditAuthorityWorkstream { .. }
                | BackendCommand::DeleteAuthorityWorkstream { .. }
                | BackendCommand::CreateAuthorityWorkUnit { .. }
                | BackendCommand::EditAuthorityWorkUnit { .. }
                | BackendCommand::DeleteAuthorityWorkUnit { .. }
                | BackendCommand::CreateAuthorityTrackedThread { .. }
                | BackendCommand::EditAuthorityTrackedThread { .. }
                | BackendCommand::DeleteAuthorityTrackedThread { .. }
        )
    }

    fn should_restart_for_error(command: &BackendCommand, error: &anyhow::Error) -> bool {
        Self::is_authority_command(command) && {
            let text = error.to_string();
            text.contains("unknown method") || text.contains("-32601")
        }
    }

    async fn execute_with_client(
        client: &Arc<OrcasIpcClient>,
        command: BackendCommand,
    ) -> Result<BackendCommandResult> {
        match command {
            BackendCommand::GetAuthorityHierarchy { include_deleted } => {
                Ok(BackendCommandResult::AuthorityHierarchy(
                    client
                        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest {
                            include_deleted,
                        })
                        .await?
                        .hierarchy,
                ))
            }
            BackendCommand::GetAuthorityDeletePlan { target } => {
                Ok(BackendCommandResult::AuthorityDeletePlan(
                    client
                        .authority_delete_plan(&ipc::AuthorityDeletePlanRequest { target })
                        .await?
                        .delete_plan,
                ))
            }
            BackendCommand::GetAuthorityWorkstream { workstream_id } => {
                Ok(BackendCommandResult::AuthorityWorkstreamDetail(
                    client
                        .authority_workstream_get(&ipc::AuthorityWorkstreamGetRequest {
                            workstream_id,
                        })
                        .await?,
                ))
            }
            BackendCommand::GetAuthorityWorkUnit { work_unit_id } => {
                Ok(BackendCommandResult::AuthorityWorkUnitDetail(
                    client
                        .authority_workunit_get(&ipc::AuthorityWorkunitGetRequest { work_unit_id })
                        .await?,
                ))
            }
            BackendCommand::GetAuthorityTrackedThread { tracked_thread_id } => {
                Ok(BackendCommandResult::AuthorityTrackedThreadDetail(
                    client
                        .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
                            tracked_thread_id,
                        })
                        .await?,
                ))
            }
            BackendCommand::CreateAuthorityWorkstream { command } => {
                Ok(BackendCommandResult::AuthorityWorkstream(
                    client
                        .authority_workstream_create(&ipc::AuthorityWorkstreamCreateRequest {
                            command,
                        })
                        .await?
                        .workstream,
                ))
            }
            BackendCommand::EditAuthorityWorkstream { command } => {
                Ok(BackendCommandResult::AuthorityWorkstream(
                    client
                        .authority_workstream_edit(&ipc::AuthorityWorkstreamEditRequest { command })
                        .await?
                        .workstream,
                ))
            }
            BackendCommand::DeleteAuthorityWorkstream { command } => {
                Ok(BackendCommandResult::AuthorityWorkstream(
                    client
                        .authority_workstream_delete(&ipc::AuthorityWorkstreamDeleteRequest {
                            command,
                        })
                        .await?
                        .workstream,
                ))
            }
            BackendCommand::CreateAuthorityWorkUnit { command } => {
                Ok(BackendCommandResult::AuthorityWorkUnit(
                    client
                        .authority_workunit_create(&ipc::AuthorityWorkunitCreateRequest { command })
                        .await?
                        .work_unit,
                ))
            }
            BackendCommand::EditAuthorityWorkUnit { command } => {
                Ok(BackendCommandResult::AuthorityWorkUnit(
                    client
                        .authority_workunit_edit(&ipc::AuthorityWorkunitEditRequest { command })
                        .await?
                        .work_unit,
                ))
            }
            BackendCommand::DeleteAuthorityWorkUnit { command } => {
                Ok(BackendCommandResult::AuthorityWorkUnit(
                    client
                        .authority_workunit_delete(&ipc::AuthorityWorkunitDeleteRequest { command })
                        .await?
                        .work_unit,
                ))
            }
            BackendCommand::CreateAuthorityTrackedThread { command } => {
                Ok(BackendCommandResult::AuthorityTrackedThread(
                    client
                        .authority_tracked_thread_create(
                            &ipc::AuthorityTrackedThreadCreateRequest { command },
                        )
                        .await?
                        .tracked_thread,
                ))
            }
            BackendCommand::EditAuthorityTrackedThread { command } => {
                Ok(BackendCommandResult::AuthorityTrackedThread(
                    client
                        .authority_tracked_thread_edit(&ipc::AuthorityTrackedThreadEditRequest {
                            command,
                        })
                        .await?
                        .tracked_thread,
                ))
            }
            BackendCommand::DeleteAuthorityTrackedThread { command } => {
                Ok(BackendCommandResult::AuthorityTrackedThread(
                    client
                        .authority_tracked_thread_delete(
                            &ipc::AuthorityTrackedThreadDeleteRequest { command },
                        )
                        .await?
                        .tracked_thread,
                ))
            }
            BackendCommand::GetThread { thread_id } => Ok(BackendCommandResult::Thread(
                client
                    .thread_read_history(&ipc::ThreadReadHistoryRequest { thread_id })
                    .await?
                    .thread,
            )),
            BackendCommand::AttachThread { thread_id } => Ok(BackendCommandResult::ThreadAttached(
                client
                    .thread_attach(&ipc::ThreadAttachRequest {
                        thread_id,
                        cwd: None,
                        model: None,
                    })
                    .await?,
            )),
            BackendCommand::GetTurn { thread_id, turn_id } => Ok(BackendCommandResult::Turn(
                client
                    .turn_attach(&ipc::TurnAttachRequest { thread_id, turn_id })
                    .await?,
            )),
            // Retained runtime-detail exception: this collaboration read carries execution
            // detail that is outside the canonical authority planning hierarchy.
            // New planning features should not target this path.
            BackendCommand::GetWorkUnit { work_unit_id } => Ok(BackendCommandResult::WorkUnit(
                client
                    .workunit_get(&ipc::WorkunitGetRequest { work_unit_id })
                    .await?,
            )),
            BackendCommand::GetProposalArtifactSummaryListForWorkUnit { work_unit_id } => Ok(
                BackendCommandResult::ProposalArtifactSummaryListForWorkUnit(
                    client
                        .proposal_artifact_summary_list_for_workunit(
                            &ipc::ProposalArtifactSummaryListForWorkunitRequest { work_unit_id },
                        )
                        .await?,
                ),
            ),
            BackendCommand::GetProposalArtifactSummary { proposal_id } => {
                Ok(BackendCommandResult::ProposalArtifactSummary(
                    client
                        .proposal_artifact_summary_get(&ipc::ProposalArtifactSummaryGetRequest {
                            proposal_id,
                        })
                        .await?
                        .summary,
                ))
            }
            BackendCommand::GetProposalArtifactDetail { proposal_id } => {
                Ok(BackendCommandResult::ProposalArtifactDetail(
                    client
                        .proposal_artifact_detail_get(&ipc::ProposalArtifactDetailGetRequest {
                            proposal_id,
                        })
                        .await?
                        .detail,
                ))
            }
            BackendCommand::GetProposalArtifactExport { proposal_id } => {
                Ok(BackendCommandResult::ProposalArtifactExport(
                    client
                        .proposal_artifact_export_get(&ipc::ProposalArtifactExportGetRequest {
                            proposal_id,
                        })
                        .await?
                        .export,
                ))
            }
            BackendCommand::GetActiveTurns => Ok(BackendCommandResult::ActiveTurns(
                client.turns_list_active().await?.turns,
            )),
            BackendCommand::LoadModels => Ok(BackendCommandResult::Models(
                client.models_list().await?.data,
            )),
            BackendCommand::StartDaemon => Ok(BackendCommandResult::DaemonStarted {
                connected: client.daemon_connect().await?.status.upstream.status == "connected",
            }),
            BackendCommand::StopDaemon => Ok(BackendCommandResult::DaemonStopped {
                stopping: client.daemon_stop().await?.stopping,
            }),
            BackendCommand::SubmitPrompt { thread_id, text } => {
                let response = client
                    .turn_start(&ipc::TurnStartRequest {
                        thread_id: thread_id.clone(),
                        text,
                        cwd: None,
                        model: None,
                    })
                    .await?;
                Ok(BackendCommandResult::PromptStarted {
                    thread_id,
                    turn_id: response.turn_id,
                })
            }
            BackendCommand::ProposeSteerSupervisorDecision {
                assignment_id,
                proposed_text,
            } => Ok(BackendCommandResult::SupervisorDecision(
                client
                    .supervisor_decision_propose_steer(
                        &ipc::SupervisorDecisionProposeSteerRequest {
                            assignment_id,
                            requested_by: Some("tui_operator".to_string()),
                            proposed_text: Some(proposed_text),
                            rationale_note: None,
                        },
                    )
                    .await?
                    .decision,
            )),
            BackendCommand::ReplacePendingSteerSupervisorDecision {
                decision_id,
                proposed_text,
            } => Ok(BackendCommandResult::SupervisorDecision(
                client
                    .supervisor_decision_replace_pending_steer(
                        &ipc::SupervisorDecisionReplacePendingSteerRequest {
                            decision_id,
                            requested_by: Some("tui_operator".to_string()),
                            proposed_text,
                            rationale_note: None,
                        },
                    )
                    .await?
                    .decision,
            )),
            BackendCommand::ProposeInterruptSupervisorDecision { assignment_id } => {
                Ok(BackendCommandResult::SupervisorDecision(
                    client
                        .supervisor_decision_propose_interrupt(
                            &ipc::SupervisorDecisionProposeInterruptRequest {
                                assignment_id,
                                requested_by: Some("tui_operator".to_string()),
                                rationale_note: None,
                            },
                        )
                        .await?
                        .decision,
                ))
            }
            BackendCommand::RecordNoActionSupervisorDecision { decision_id } => {
                Ok(BackendCommandResult::SupervisorDecision(
                    client
                        .supervisor_decision_record_no_action(
                            &ipc::SupervisorDecisionRecordNoActionRequest {
                                decision_id,
                                reviewed_by: Some("tui_operator".to_string()),
                                review_note: None,
                            },
                        )
                        .await?
                        .decision,
                ))
            }
            BackendCommand::ManualRefreshSupervisorDecision { assignment_id } => {
                Ok(BackendCommandResult::SupervisorDecision(
                    client
                        .supervisor_decision_manual_refresh(
                            &ipc::SupervisorDecisionManualRefreshRequest {
                                assignment_id,
                                requested_by: Some("tui_operator".to_string()),
                                rationale_note: None,
                            },
                        )
                        .await?
                        .decision,
                ))
            }
            BackendCommand::ApproveSupervisorDecision { decision_id } => {
                Ok(BackendCommandResult::SupervisorDecision(
                    client
                        .supervisor_decision_approve_and_send(
                            &ipc::SupervisorDecisionApproveAndSendRequest {
                                decision_id,
                                reviewed_by: Some("tui_operator".to_string()),
                                review_note: None,
                            },
                        )
                        .await?
                        .decision,
                ))
            }
            BackendCommand::RejectSupervisorDecision { decision_id } => {
                Ok(BackendCommandResult::SupervisorDecision(
                    client
                        .supervisor_decision_reject(&ipc::SupervisorDecisionRejectRequest {
                            decision_id,
                            reviewed_by: Some("tui_operator".to_string()),
                            review_note: None,
                        })
                        .await?
                        .decision,
                ))
            }
        }
    }
}

fn review_command_target(command: &BackendCommand) -> Option<(&'static str, &str, &'static str)> {
    match command {
        BackendCommand::RecordNoActionSupervisorDecision { decision_id } => {
            Some(("record_no_action", decision_id.as_str(), "decision_id"))
        }
        BackendCommand::ManualRefreshSupervisorDecision { assignment_id } => {
            Some(("manual_refresh", assignment_id.as_str(), "assignment_id"))
        }
        BackendCommand::ApproveSupervisorDecision { decision_id } => {
            Some(("approve_and_send", decision_id.as_str(), "decision_id"))
        }
        BackendCommand::RejectSupervisorDecision { decision_id } => {
            Some(("reject_decision", decision_id.as_str(), "decision_id"))
        }
        _ => None,
    }
}

fn authoring_command_target(
    command: &BackendCommand,
) -> Option<(&'static str, &str, &'static str)> {
    match command {
        BackendCommand::ProposeSteerSupervisorDecision { assignment_id, .. } => {
            Some(("propose_steer", assignment_id.as_str(), "assignment_id"))
        }
        BackendCommand::ReplacePendingSteerSupervisorDecision { decision_id, .. } => {
            Some(("replace_pending_steer", decision_id.as_str(), "decision_id"))
        }
        BackendCommand::ProposeInterruptSupervisorDecision { assignment_id } => {
            Some(("propose_interrupt", assignment_id.as_str(), "assignment_id"))
        }
        _ => None,
    }
}

fn log_review_action_start(action: &str, target_field: &str, target_id: &str) {
    match target_field {
        "decision_id" => info!(
            surface = "tui",
            action,
            decision_id = target_id,
            "starting review action"
        ),
        "assignment_id" => info!(
            surface = "tui",
            action,
            assignment_id = target_id,
            "starting review action"
        ),
        _ => {}
    }
}

fn log_review_action_failure(
    action: &str,
    target_field: &str,
    target_id: &str,
    duration_ms: u64,
    error: &anyhow::Error,
) {
    match target_field {
        "decision_id" => warn!(
            surface = "tui",
            action,
            decision_id = target_id,
            result = "failed",
            duration_ms,
            error = %error,
            "review action failed"
        ),
        "assignment_id" => warn!(
            surface = "tui",
            action,
            assignment_id = target_id,
            result = "failed",
            duration_ms,
            error = %error,
            "review action failed"
        ),
        _ => {}
    }
}

fn log_review_action_success(
    action: &str,
    target_field: &str,
    target_id: &str,
    decision: &SupervisorTurnDecision,
    duration_ms: u64,
) {
    match target_field {
        "decision_id" => info!(
            surface = "tui",
            action,
            decision_id = target_id,
            assignment_id = %decision.assignment_id,
            result = "completed",
            duration_ms,
            "review action completed"
        ),
        "assignment_id" => info!(
            surface = "tui",
            action,
            assignment_id = target_id,
            decision_id = %decision.decision_id,
            result = "completed",
            duration_ms,
            "review action completed"
        ),
        _ => {}
    }
}

fn log_authoring_action_start(action: &str, target_field: &str, target_id: &str) {
    match target_field {
        "decision_id" => info!(
            surface = "tui",
            action,
            decision_id = target_id,
            "starting proposal authoring action"
        ),
        "assignment_id" => info!(
            surface = "tui",
            action,
            assignment_id = target_id,
            "starting proposal authoring action"
        ),
        _ => {}
    }
}

fn log_authoring_action_failure(
    action: &str,
    target_field: &str,
    target_id: &str,
    duration_ms: u64,
    error: &anyhow::Error,
) {
    match target_field {
        "decision_id" => warn!(
            surface = "tui",
            action,
            decision_id = target_id,
            result = "failed",
            duration_ms,
            error = %error,
            "proposal authoring action failed"
        ),
        "assignment_id" => warn!(
            surface = "tui",
            action,
            assignment_id = target_id,
            result = "failed",
            duration_ms,
            error = %error,
            "proposal authoring action failed"
        ),
        _ => {}
    }
}

fn log_authoring_action_success(
    action: &str,
    target_field: &str,
    target_id: &str,
    decision: &SupervisorTurnDecision,
    duration_ms: u64,
) {
    match target_field {
        "decision_id" => info!(
            surface = "tui",
            action,
            decision_id = target_id,
            assignment_id = %decision.assignment_id,
            result = "completed",
            duration_ms,
            "proposal authoring action completed"
        ),
        "assignment_id" => info!(
            surface = "tui",
            action,
            assignment_id = target_id,
            decision_id = %decision.decision_id,
            result = "completed",
            duration_ms,
            "proposal authoring action completed"
        ),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_unknown_method_errors_trigger_restart_retry() {
        let command = BackendCommand::CreateAuthorityWorkstream {
            command: authority::CreateWorkstream {
                metadata: authority::CommandMetadata {
                    command_id: authority::CommandId::new(),
                    issued_at: chrono::Utc::now(),
                    origin_node_id: authority::OriginNodeId::parse("orcas-tui")
                        .expect("origin node id"),
                    actor: authority::CommandActor::parse("tui_operator").expect("actor"),
                    correlation_id: None,
                },
                workstream_id: authority::WorkstreamId::new(),
                title: "alpha".to_string(),
                objective: "beta".to_string(),
                status: orcas_core::WorkstreamStatus::Active,
                priority: "normal".to_string(),
            },
        };
        let error = anyhow!(
            "protocol error: json-rpc error -32601: unknown method `authority/workstream/create`"
        );
        assert!(OrcasDaemonBackend::should_restart_for_error(
            &command, &error
        ));
    }

    #[test]
    fn non_authority_errors_do_not_trigger_restart_retry() {
        let command = BackendCommand::LoadModels;
        let error = anyhow!("protocol error: json-rpc error -32601: unknown method `models/list`");
        assert!(!OrcasDaemonBackend::should_restart_for_error(
            &command, &error
        ));
    }
}

#[derive(Clone)]
pub struct FakeBackend {
    inner: Arc<Mutex<FakeBackendState>>,
}

struct FakeBackendState {
    snapshot: ipc::StateSnapshot,
    threads: HashMap<String, ipc::ThreadView>,
    turns: HashMap<(String, String), ipc::TurnAttachResponse>,
    work_unit_details: HashMap<String, ipc::WorkunitGetResponse>,
    proposal_artifact_summary_lists:
        HashMap<String, ipc::ProposalArtifactSummaryListForWorkunitResponse>,
    proposal_artifact_summaries: HashMap<String, ipc::SupervisorProposalArtifactSummary>,
    proposal_artifact_details: HashMap<String, ipc::SupervisorProposalArtifactDetail>,
    proposal_artifact_exports: HashMap<String, ipc::SupervisorProposalArtifactExport>,
    authority_workstreams: HashMap<String, authority::WorkstreamRecord>,
    authority_work_units: HashMap<String, authority::WorkUnitRecord>,
    authority_tracked_threads: HashMap<String, authority::TrackedThreadRecord>,
    active_turns: Vec<ipc::TurnStateView>,
    models: Vec<ipc::ModelSummary>,
    next_submit_id: usize,
    fail_snapshot: Option<String>,
    fail_subscribe: Option<String>,
    fail_next_command: Option<String>,
    recorded_commands: Vec<BackendCommand>,
    snapshot_requests: usize,
    subscribe_requests: usize,
    event_tx: Option<mpsc::Sender<ipc::DaemonEventEnvelope>>,
}

impl FakeBackend {
    pub fn new(snapshot: ipc::StateSnapshot) -> Self {
        let authority_state = authority_state_from_snapshot(&snapshot);
        let threads = snapshot
            .active_thread
            .clone()
            .into_iter()
            .map(|thread| (thread.summary.id.clone(), thread))
            .collect();
        let active_turns = snapshot
            .session
            .active_turns
            .iter()
            .map(turn_state_from_active_turn)
            .collect();
        Self {
            inner: Arc::new(Mutex::new(FakeBackendState {
                work_unit_details: workunit_details_from_snapshot(&snapshot),
                proposal_artifact_summary_lists: proposal_artifact_summary_lists_from_snapshot(
                    &snapshot,
                ),
                proposal_artifact_summaries: proposal_artifact_summaries_from_snapshot(&snapshot),
                proposal_artifact_details: proposal_artifact_details_from_snapshot(&snapshot),
                proposal_artifact_exports: proposal_artifact_exports_from_snapshot(&snapshot),
                authority_workstreams: authority_state.workstreams,
                authority_work_units: authority_state.work_units,
                authority_tracked_threads: authority_state.tracked_threads,
                snapshot,
                threads,
                turns: HashMap::new(),
                active_turns,
                models: vec![
                    ipc::ModelSummary {
                        id: "codex-small".to_string(),
                        display_name: "Codex Small".to_string(),
                        hidden: false,
                        is_default: true,
                    },
                    ipc::ModelSummary {
                        id: "codex-large".to_string(),
                        display_name: "Codex Large".to_string(),
                        hidden: true,
                        is_default: false,
                    },
                ],
                next_submit_id: 1,
                fail_snapshot: None,
                fail_subscribe: None,
                fail_next_command: None,
                recorded_commands: Vec::new(),
                snapshot_requests: 0,
                subscribe_requests: 0,
                event_tx: None,
            })),
        }
    }

    pub async fn set_thread(&self, thread: ipc::ThreadView) {
        self.inner
            .lock()
            .await
            .threads
            .insert(thread.summary.id.clone(), thread);
    }

    pub async fn replace_snapshot(&self, snapshot: ipc::StateSnapshot) {
        let authority_state = authority_state_from_snapshot(&snapshot);
        let active_turns = snapshot
            .session
            .active_turns
            .iter()
            .map(turn_state_from_active_turn)
            .collect();
        let mut guard = self.inner.lock().await;
        guard.work_unit_details = workunit_details_from_snapshot(&snapshot);
        guard.proposal_artifact_summary_lists =
            proposal_artifact_summary_lists_from_snapshot(&snapshot);
        guard.proposal_artifact_summaries = proposal_artifact_summaries_from_snapshot(&snapshot);
        guard.proposal_artifact_details = proposal_artifact_details_from_snapshot(&snapshot);
        guard.proposal_artifact_exports = proposal_artifact_exports_from_snapshot(&snapshot);
        guard.authority_workstreams = authority_state.workstreams;
        guard.authority_work_units = authority_state.work_units;
        guard.authority_tracked_threads = authority_state.tracked_threads;
        guard.snapshot = snapshot;
        guard.active_turns = active_turns;
    }

    pub async fn set_workunit_detail(&self, detail: ipc::WorkunitGetResponse) {
        let summaries = detail
            .proposals
            .iter()
            .map(proposal_artifact_summary_from_record)
            .collect::<Vec<_>>();
        let details = detail
            .proposals
            .iter()
            .map(proposal_artifact_detail_from_record)
            .collect::<Vec<_>>();
        let exports = detail
            .proposals
            .iter()
            .map(|proposal| proposal_artifact_export_from_record(&detail.work_unit.id, proposal))
            .collect::<Vec<_>>();
        let summary_list = ipc::ProposalArtifactSummaryListForWorkunitResponse {
            work_unit_id: detail.work_unit.id.clone(),
            summaries: summaries.clone(),
        };
        let mut guard = self.inner.lock().await;
        for summary in summaries {
            guard
                .proposal_artifact_summaries
                .insert(summary.proposal_id.clone(), summary);
        }
        for artifact_detail in details {
            guard
                .proposal_artifact_details
                .insert(artifact_detail.proposal_id.clone(), artifact_detail);
        }
        for export in exports {
            guard
                .proposal_artifact_exports
                .insert(export.proposal_id.clone(), export);
        }
        guard
            .proposal_artifact_summary_lists
            .insert(summary_list.work_unit_id.clone(), summary_list);
        guard
            .work_unit_details
            .insert(detail.work_unit.id.clone(), detail);
    }

    pub async fn set_proposal_artifact_summary(
        &self,
        summary: ipc::SupervisorProposalArtifactSummary,
    ) {
        self.inner
            .lock()
            .await
            .proposal_artifact_summaries
            .insert(summary.proposal_id.clone(), summary);
    }

    pub async fn set_proposal_artifact_detail(
        &self,
        detail: ipc::SupervisorProposalArtifactDetail,
    ) {
        self.inner
            .lock()
            .await
            .proposal_artifact_details
            .insert(detail.proposal_id.clone(), detail);
    }

    pub async fn set_turn(&self, response: ipc::TurnAttachResponse) {
        let mut guard = self.inner.lock().await;
        if let Some(turn) = response.turn.as_ref() {
            guard.turns.insert(
                (turn.thread_id.clone(), turn.turn_id.clone()),
                response.clone(),
            );
        }
    }

    pub async fn set_active_turns(&self, turns: Vec<ipc::TurnStateView>) {
        self.inner.lock().await.active_turns = turns;
    }

    pub async fn fail_snapshot_once(&self, message: impl Into<String>) {
        self.inner.lock().await.fail_snapshot = Some(message.into());
    }

    pub async fn fail_next_command(&self, message: impl Into<String>) {
        self.inner.lock().await.fail_next_command = Some(message.into());
    }

    pub async fn fail_subscribe_once(&self, message: impl Into<String>) {
        self.inner.lock().await.fail_subscribe = Some(message.into());
    }

    pub async fn inject_event(&self, event: ipc::DaemonEventEnvelope) -> Result<()> {
        let tx = {
            let mut guard = self.inner.lock().await;
            if let ipc::DaemonEvent::TurnUpdated { thread_id, turn } = &event.event {
                let lifecycle = match turn.status.as_str() {
                    "completed" => ipc::TurnLifecycleState::Completed,
                    "failed" => ipc::TurnLifecycleState::Failed,
                    "cancelled" | "interrupted" => ipc::TurnLifecycleState::Interrupted,
                    "lost" => ipc::TurnLifecycleState::Lost,
                    _ => ipc::TurnLifecycleState::Active,
                };
                let attachable = matches!(lifecycle, ipc::TurnLifecycleState::Active);
                let state = ipc::TurnStateView {
                    thread_id: thread_id.clone(),
                    turn_id: turn.id.clone(),
                    lifecycle,
                    status: turn.status.clone(),
                    attachable,
                    live_stream: attachable,
                    terminal: !attachable,
                    recent_output: turn
                        .items
                        .iter()
                        .filter_map(|item| item.text.as_deref())
                        .next_back()
                        .map(ToOwned::to_owned),
                    recent_event: Some(format!("turn {}", turn.status)),
                    updated_at: chrono::Utc::now(),
                    error_message: turn.error_message.clone(),
                };
                guard.turns.insert(
                    (thread_id.clone(), turn.id.clone()),
                    ipc::TurnAttachResponse {
                        turn: Some(state.clone()),
                        attached: attachable,
                        reason: if attachable {
                            None
                        } else {
                            Some(format!(
                                "turn already {}; only terminal state is queryable",
                                turn.status
                            ))
                        },
                    },
                );
                guard.active_turns.retain(|active| {
                    !(active.thread_id == *thread_id && active.turn_id == turn.id)
                });
                if attachable {
                    guard.active_turns.push(state);
                }
            }
            guard
                .event_tx
                .clone()
                .ok_or_else(|| anyhow!("event subscription has not been established"))?
        };
        tx.send(event).await?;
        Ok(())
    }

    pub async fn recorded_commands(&self) -> Vec<BackendCommand> {
        self.inner.lock().await.recorded_commands.clone()
    }

    pub async fn snapshot_requests(&self) -> usize {
        self.inner.lock().await.snapshot_requests
    }

    pub async fn subscribe_requests(&self) -> usize {
        self.inner.lock().await.subscribe_requests
    }

    pub async fn disconnect_events(&self) {
        self.inner.lock().await.event_tx = None;
    }
}

#[async_trait]
impl TuiBackend for FakeBackend {
    async fn get_snapshot(&self) -> Result<ipc::StateSnapshot> {
        let mut guard = self.inner.lock().await;
        guard.snapshot_requests += 1;
        if let Some(message) = guard.fail_snapshot.take() {
            return Err(anyhow!(message));
        }
        Ok(guard.snapshot.clone())
    }

    async fn subscribe_events(&self) -> Result<mpsc::Receiver<ipc::DaemonEventEnvelope>> {
        let mut guard = self.inner.lock().await;
        guard.subscribe_requests += 1;
        if let Some(message) = guard.fail_subscribe.take() {
            return Err(anyhow!(message));
        }
        let (tx, rx) = mpsc::channel(256);
        guard.event_tx = Some(tx);
        Ok(rx)
    }

    async fn execute(&self, command: BackendCommand) -> Result<BackendCommandResult> {
        let mut guard = self.inner.lock().await;
        guard.recorded_commands.push(command.clone());
        if let Some(message) = guard.fail_next_command.take() {
            return Err(anyhow!(message));
        }
        match command {
            BackendCommand::GetAuthorityHierarchy { include_deleted } => {
                Ok(BackendCommandResult::AuthorityHierarchy(
                    build_authority_hierarchy(&guard, include_deleted),
                ))
            }
            BackendCommand::GetAuthorityDeletePlan { target } => {
                let plan = build_delete_plan(&guard, &target)
                    .ok_or_else(|| anyhow!("unknown authority delete target"))?;
                Ok(BackendCommandResult::AuthorityDeletePlan(plan))
            }
            BackendCommand::GetAuthorityWorkstream { workstream_id } => {
                let workstream = guard
                    .authority_workstreams
                    .get(workstream_id.as_str())
                    .cloned()
                    .ok_or_else(|| anyhow!("unknown authority workstream `{workstream_id}`"))?;
                let mut work_units = guard
                    .authority_work_units
                    .values()
                    .filter(|work_unit| {
                        work_unit.workstream_id == workstream_id && work_unit.deleted_at.is_none()
                    })
                    .map(authority::WorkUnitSummary::from)
                    .collect::<Vec<_>>();
                work_units.sort_by(|left, right| {
                    right
                        .updated_at
                        .cmp(&left.updated_at)
                        .then_with(|| left.id.as_str().cmp(right.id.as_str()))
                });
                Ok(BackendCommandResult::AuthorityWorkstreamDetail(
                    ipc::AuthorityWorkstreamGetResponse {
                        workstream,
                        work_units,
                    },
                ))
            }
            BackendCommand::GetAuthorityWorkUnit { work_unit_id } => {
                let work_unit = guard
                    .authority_work_units
                    .get(work_unit_id.as_str())
                    .cloned()
                    .ok_or_else(|| anyhow!("unknown authority work unit `{work_unit_id}`"))?;
                let mut tracked_threads = guard
                    .authority_tracked_threads
                    .values()
                    .filter(|tracked_thread| {
                        tracked_thread.work_unit_id == work_unit_id
                            && tracked_thread.deleted_at.is_none()
                    })
                    .map(authority::TrackedThreadSummary::from)
                    .collect::<Vec<_>>();
                tracked_threads.sort_by(|left, right| {
                    right
                        .updated_at
                        .cmp(&left.updated_at)
                        .then_with(|| left.id.as_str().cmp(right.id.as_str()))
                });
                Ok(BackendCommandResult::AuthorityWorkUnitDetail(
                    ipc::AuthorityWorkunitGetResponse {
                        work_unit,
                        tracked_threads,
                    },
                ))
            }
            BackendCommand::GetAuthorityTrackedThread { tracked_thread_id } => {
                Ok(BackendCommandResult::AuthorityTrackedThreadDetail(
                    ipc::AuthorityTrackedThreadGetResponse {
                        tracked_thread: guard
                            .authority_tracked_threads
                            .get(tracked_thread_id.as_str())
                            .cloned()
                            .ok_or_else(|| {
                                anyhow!("unknown authority tracked thread `{tracked_thread_id}`")
                            })?,
                        workspace_inspection: None,
                        workspace_operation: None,
                    },
                ))
            }
            BackendCommand::CreateAuthorityWorkstream { command } => {
                let now = command.metadata.issued_at;
                let record = authority::WorkstreamRecord {
                    id: command.workstream_id.clone(),
                    title: command.title.clone(),
                    objective: command.objective.clone(),
                    status: command.status,
                    priority: command.priority.clone(),
                    revision: authority::Revision::initial(),
                    origin_node_id: command.metadata.origin_node_id.clone(),
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                };
                guard
                    .authority_workstreams
                    .insert(record.id.to_string(), record.clone());
                Ok(BackendCommandResult::AuthorityWorkstream(record))
            }
            BackendCommand::EditAuthorityWorkstream { command } => {
                let record = guard
                    .authority_workstreams
                    .get_mut(command.workstream_id.as_str())
                    .ok_or_else(|| anyhow!("unknown authority workstream"))?;
                if record.revision != command.expected_revision {
                    return Err(anyhow!("unexpected workstream revision"));
                }
                if let Some(title) = command.changes.title.as_ref() {
                    record.title = title.clone();
                }
                if let Some(objective) = command.changes.objective.as_ref() {
                    record.objective = objective.clone();
                }
                if let Some(status) = command.changes.status {
                    record.status = status;
                }
                if let Some(priority) = command.changes.priority.as_ref() {
                    record.priority = priority.clone();
                }
                record.revision = record.revision.next();
                record.updated_at = command.metadata.issued_at;
                Ok(BackendCommandResult::AuthorityWorkstream(record.clone()))
            }
            BackendCommand::DeleteAuthorityWorkstream { command } => {
                let record = guard
                    .authority_workstreams
                    .get_mut(command.workstream_id.as_str())
                    .ok_or_else(|| anyhow!("unknown authority workstream"))?;
                if record.revision != command.expected_revision {
                    return Err(anyhow!("unexpected workstream revision"));
                }
                let deleted_at = command.metadata.issued_at;
                record.revision = record.revision.next();
                record.updated_at = deleted_at;
                record.deleted_at = Some(deleted_at);
                let deleted_record = record.clone();
                let descendant_work_unit_ids = guard
                    .authority_work_units
                    .values()
                    .filter(|work_unit| {
                        work_unit.workstream_id == command.workstream_id
                            && work_unit.deleted_at.is_none()
                    })
                    .map(|work_unit| work_unit.id.to_string())
                    .collect::<Vec<_>>();
                for work_unit_id in &descendant_work_unit_ids {
                    if let Some(work_unit) = guard.authority_work_units.get_mut(work_unit_id) {
                        work_unit.revision = work_unit.revision.next();
                        work_unit.updated_at = deleted_at;
                        work_unit.deleted_at = Some(deleted_at);
                    }
                }
                for tracked_thread in guard.authority_tracked_threads.values_mut() {
                    if descendant_work_unit_ids
                        .iter()
                        .any(|work_unit_id| work_unit_id == tracked_thread.work_unit_id.as_str())
                        && tracked_thread.deleted_at.is_none()
                    {
                        tracked_thread.revision = tracked_thread.revision.next();
                        tracked_thread.updated_at = deleted_at;
                        tracked_thread.deleted_at = Some(deleted_at);
                    }
                }
                Ok(BackendCommandResult::AuthorityWorkstream(deleted_record))
            }
            BackendCommand::CreateAuthorityWorkUnit { command } => {
                if !guard
                    .authority_workstreams
                    .contains_key(command.workstream_id.as_str())
                {
                    return Err(anyhow!("unknown authority parent workstream"));
                }
                let now = command.metadata.issued_at;
                let record = authority::WorkUnitRecord {
                    id: command.work_unit_id.clone(),
                    workstream_id: command.workstream_id.clone(),
                    title: command.title.clone(),
                    task_statement: command.task_statement.clone(),
                    status: command.status,
                    revision: authority::Revision::initial(),
                    origin_node_id: command.metadata.origin_node_id.clone(),
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                };
                guard
                    .authority_work_units
                    .insert(record.id.to_string(), record.clone());
                Ok(BackendCommandResult::AuthorityWorkUnit(record))
            }
            BackendCommand::EditAuthorityWorkUnit { command } => {
                let record = guard
                    .authority_work_units
                    .get_mut(command.work_unit_id.as_str())
                    .ok_or_else(|| anyhow!("unknown authority work unit"))?;
                if record.revision != command.expected_revision {
                    return Err(anyhow!("unexpected work unit revision"));
                }
                if let Some(title) = command.changes.title.as_ref() {
                    record.title = title.clone();
                }
                if let Some(task_statement) = command.changes.task_statement.as_ref() {
                    record.task_statement = task_statement.clone();
                }
                if let Some(status) = command.changes.status {
                    record.status = status;
                }
                record.revision = record.revision.next();
                record.updated_at = command.metadata.issued_at;
                Ok(BackendCommandResult::AuthorityWorkUnit(record.clone()))
            }
            BackendCommand::DeleteAuthorityWorkUnit { command } => {
                let record = guard
                    .authority_work_units
                    .get_mut(command.work_unit_id.as_str())
                    .ok_or_else(|| anyhow!("unknown authority work unit"))?;
                if record.revision != command.expected_revision {
                    return Err(anyhow!("unexpected work unit revision"));
                }
                let deleted_at = command.metadata.issued_at;
                record.revision = record.revision.next();
                record.updated_at = deleted_at;
                record.deleted_at = Some(deleted_at);
                let deleted_record = record.clone();
                for tracked_thread in guard.authority_tracked_threads.values_mut() {
                    if tracked_thread.work_unit_id == command.work_unit_id
                        && tracked_thread.deleted_at.is_none()
                    {
                        tracked_thread.revision = tracked_thread.revision.next();
                        tracked_thread.updated_at = deleted_at;
                        tracked_thread.deleted_at = Some(deleted_at);
                    }
                }
                Ok(BackendCommandResult::AuthorityWorkUnit(deleted_record))
            }
            BackendCommand::CreateAuthorityTrackedThread { command } => {
                if !guard
                    .authority_work_units
                    .contains_key(command.work_unit_id.as_str())
                {
                    return Err(anyhow!("unknown authority parent work unit"));
                }
                let now = command.metadata.issued_at;
                let binding_state = if command.upstream_thread_id.is_some() {
                    authority::TrackedThreadBindingState::Bound
                } else {
                    authority::TrackedThreadBindingState::Unbound
                };
                let record = authority::TrackedThreadRecord {
                    id: command.tracked_thread_id.clone(),
                    work_unit_id: command.work_unit_id.clone(),
                    title: command.title.clone(),
                    notes: command.notes.clone(),
                    backend_kind: command.backend_kind,
                    upstream_thread_id: command.upstream_thread_id.clone(),
                    binding_state,
                    preferred_cwd: command.preferred_cwd.clone(),
                    preferred_model: command.preferred_model.clone(),
                    last_seen_turn_id: None,
                    workspace: command.workspace.clone(),
                    revision: authority::Revision::initial(),
                    origin_node_id: command.metadata.origin_node_id.clone(),
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                };
                guard
                    .authority_tracked_threads
                    .insert(record.id.to_string(), record.clone());
                Ok(BackendCommandResult::AuthorityTrackedThread(record))
            }
            BackendCommand::EditAuthorityTrackedThread { command } => {
                let record = guard
                    .authority_tracked_threads
                    .get_mut(command.tracked_thread_id.as_str())
                    .ok_or_else(|| anyhow!("unknown authority tracked thread"))?;
                if record.revision != command.expected_revision {
                    return Err(anyhow!("unexpected tracked thread revision"));
                }
                if let Some(title) = command.changes.title.as_ref() {
                    record.title = title.clone();
                }
                if let Some(notes) = command.changes.notes.as_ref() {
                    record.notes = notes.clone();
                }
                if let Some(backend_kind) = command.changes.backend_kind {
                    record.backend_kind = backend_kind;
                }
                if let Some(upstream_thread_id) = command.changes.upstream_thread_id.as_ref() {
                    record.upstream_thread_id = upstream_thread_id.clone();
                }
                if let Some(binding_state) = command.changes.binding_state {
                    record.binding_state = binding_state;
                } else {
                    record.binding_state = if record.upstream_thread_id.is_some() {
                        authority::TrackedThreadBindingState::Bound
                    } else {
                        authority::TrackedThreadBindingState::Unbound
                    };
                }
                if let Some(preferred_cwd) = command.changes.preferred_cwd.as_ref() {
                    record.preferred_cwd = preferred_cwd.clone();
                }
                if let Some(preferred_model) = command.changes.preferred_model.as_ref() {
                    record.preferred_model = preferred_model.clone();
                }
                if let Some(last_seen_turn_id) = command.changes.last_seen_turn_id.as_ref() {
                    record.last_seen_turn_id = last_seen_turn_id.clone();
                }
                record.revision = record.revision.next();
                record.updated_at = command.metadata.issued_at;
                Ok(BackendCommandResult::AuthorityTrackedThread(record.clone()))
            }
            BackendCommand::DeleteAuthorityTrackedThread { command } => {
                let record = guard
                    .authority_tracked_threads
                    .get_mut(command.tracked_thread_id.as_str())
                    .ok_or_else(|| anyhow!("unknown authority tracked thread"))?;
                if record.revision != command.expected_revision {
                    return Err(anyhow!("unexpected tracked thread revision"));
                }
                let deleted_at = command.metadata.issued_at;
                record.revision = record.revision.next();
                record.updated_at = deleted_at;
                record.deleted_at = Some(deleted_at);
                Ok(BackendCommandResult::AuthorityTrackedThread(record.clone()))
            }
            BackendCommand::GetThread { thread_id } => guard
                .threads
                .get(&thread_id)
                .cloned()
                .map(BackendCommandResult::Thread)
                .ok_or_else(|| anyhow!("unknown thread `{thread_id}`")),
            BackendCommand::AttachThread { thread_id } => {
                let thread = guard
                    .threads
                    .get_mut(&thread_id)
                    .ok_or_else(|| anyhow!("unknown thread `{thread_id}`"))?;
                thread.summary.monitor_state = ipc::ThreadMonitorState::Attached;
                Ok(BackendCommandResult::ThreadAttached(
                    ipc::ThreadAttachResponse {
                        thread: Some(thread.clone()),
                        attached: true,
                        reason: None,
                    },
                ))
            }
            BackendCommand::GetTurn { thread_id, turn_id } => Ok(BackendCommandResult::Turn(
                guard
                    .turns
                    .get(&(thread_id.clone(), turn_id.clone()))
                    .cloned()
                    .unwrap_or(ipc::TurnAttachResponse {
                        turn: None,
                        attached: false,
                        reason: Some(
                            "turn was not found in the current Orcas daemon state".to_string(),
                        ),
                    }),
            )),
            BackendCommand::GetWorkUnit { work_unit_id } => guard
                .work_unit_details
                .get(&work_unit_id)
                .cloned()
                .map(BackendCommandResult::WorkUnit)
                .ok_or_else(|| anyhow!("unknown work unit `{work_unit_id}`")),
            BackendCommand::GetProposalArtifactSummaryListForWorkUnit { work_unit_id } => guard
                .proposal_artifact_summary_lists
                .get(&work_unit_id)
                .cloned()
                .map(BackendCommandResult::ProposalArtifactSummaryListForWorkUnit)
                .ok_or_else(|| {
                    anyhow!("unknown proposal artifact summary list for work unit `{work_unit_id}`")
                }),
            BackendCommand::GetProposalArtifactSummary { proposal_id } => guard
                .proposal_artifact_summaries
                .get(&proposal_id)
                .cloned()
                .map(BackendCommandResult::ProposalArtifactSummary)
                .ok_or_else(|| anyhow!("unknown proposal artifact summary `{proposal_id}`")),
            BackendCommand::GetProposalArtifactDetail { proposal_id } => guard
                .proposal_artifact_details
                .get(&proposal_id)
                .cloned()
                .map(BackendCommandResult::ProposalArtifactDetail)
                .ok_or_else(|| anyhow!("unknown proposal artifact detail `{proposal_id}`")),
            BackendCommand::GetProposalArtifactExport { proposal_id } => guard
                .proposal_artifact_exports
                .get(&proposal_id)
                .cloned()
                .map(BackendCommandResult::ProposalArtifactExport)
                .ok_or_else(|| anyhow!("unknown proposal artifact export `{proposal_id}`")),
            BackendCommand::GetActiveTurns => Ok(BackendCommandResult::ActiveTurns(
                guard.active_turns.clone(),
            )),
            BackendCommand::LoadModels => Ok(BackendCommandResult::Models(guard.models.clone())),
            BackendCommand::StartDaemon => {
                let daemon = &mut guard.snapshot.daemon;
                daemon.upstream.status = "connected".to_string();
                daemon.upstream.detail = None;
                Ok(BackendCommandResult::DaemonStarted { connected: true })
            }
            BackendCommand::StopDaemon => {
                let daemon = &mut guard.snapshot.daemon;
                daemon.upstream.status = "disconnected".to_string();
                daemon.upstream.detail = Some("stop requested".to_string());
                Ok(BackendCommandResult::DaemonStopped { stopping: true })
            }
            BackendCommand::SubmitPrompt { thread_id, .. } => {
                let turn_id = format!("turn-{}", guard.next_submit_id);
                guard.next_submit_id += 1;
                let turn = ipc::TurnStateView {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    lifecycle: ipc::TurnLifecycleState::Active,
                    status: "submitted".to_string(),
                    attachable: true,
                    live_stream: true,
                    terminal: false,
                    recent_output: None,
                    recent_event: Some("turn submitted".to_string()),
                    updated_at: chrono::Utc::now(),
                    error_message: None,
                };
                guard.turns.insert(
                    (thread_id.clone(), turn_id.clone()),
                    ipc::TurnAttachResponse {
                        turn: Some(turn.clone()),
                        attached: true,
                        reason: None,
                    },
                );
                guard.active_turns.push(turn);
                Ok(BackendCommandResult::PromptStarted { thread_id, turn_id })
            }
            BackendCommand::ProposeSteerSupervisorDecision {
                assignment_id,
                proposed_text,
            } => {
                let trimmed_text = proposed_text.trim();
                if trimmed_text.is_empty() {
                    return Err(anyhow!("steer proposal requires non-empty proposed_text"));
                }
                let assignment_index = guard
                    .snapshot
                    .collaboration
                    .codex_thread_assignments
                    .iter()
                    .position(|assignment| assignment.assignment_id == assignment_id)
                    .ok_or_else(|| anyhow!("unknown Codex assignment `{assignment_id}`"))?;
                let assignment_snapshot =
                    guard.snapshot.collaboration.codex_thread_assignments[assignment_index].clone();
                if !assignment_snapshot.active {
                    return Err(anyhow!("Codex assignment `{assignment_id}` is not active"));
                }
                if guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .iter()
                    .any(|decision| decision.assignment_id == assignment_id && decision.open)
                {
                    return Err(anyhow!(
                        "Codex assignment `{assignment_id}` already has an open supervisor decision"
                    ));
                }
                let active_turn_id = guard
                    .snapshot
                    .threads
                    .iter()
                    .find(|thread| thread.id == assignment_snapshot.codex_thread_id)
                    .and_then(|thread| thread.active_turn_id.clone())
                    .ok_or_else(|| {
                        anyhow!(
                            "thread `{}` has no active turn to steer",
                            assignment_snapshot.codex_thread_id
                        )
                    })?;
                let now = chrono::Utc::now();
                let decision = ipc::SupervisorTurnDecisionSummary {
                    decision_id: format!("std-{}", guard.next_submit_id),
                    assignment_id: assignment_snapshot.assignment_id.clone(),
                    codex_thread_id: assignment_snapshot.codex_thread_id.clone(),
                    workstream_id: Some(assignment_snapshot.workstream_id.clone()),
                    work_unit_id: Some(assignment_snapshot.work_unit_id.clone()),
                    supervisor_id: Some(assignment_snapshot.supervisor_id.clone()),
                    basis_turn_id: Some(active_turn_id.clone()),
                    kind: orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn,
                    proposal_kind: orcas_core::SupervisorTurnProposalKind::OperatorSteer,
                    proposed_text: Some(trimmed_text.to_string()),
                    rationale_summary: format!(
                        "Operator requested review of steering active turn `{active_turn_id}`."
                    ),
                    status: orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
                    created_at: now,
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: Some("steer proposal requested by tui_operator".to_string()),
                    open: true,
                };
                guard.next_submit_id += 1;
                if let Some(assignment) = guard
                    .snapshot
                    .collaboration
                    .codex_thread_assignments
                    .get_mut(assignment_index)
                {
                    assignment.latest_decision_id = Some(decision.decision_id.clone());
                    assignment.latest_basis_turn_id = decision.basis_turn_id.clone();
                    assignment.updated_at = now;
                }
                guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .push(decision.clone());
                Ok(BackendCommandResult::SupervisorDecision(
                    SupervisorTurnDecision {
                        decision_id: decision.decision_id,
                        assignment_id: decision.assignment_id,
                        codex_thread_id: decision.codex_thread_id,
                        basis_turn_id: decision.basis_turn_id,
                        kind: decision.kind,
                        proposal_kind: decision.proposal_kind,
                        proposed_text: decision.proposed_text,
                        rationale_summary: decision.rationale_summary,
                        status: decision.status,
                        created_at: decision.created_at,
                        approved_at: decision.approved_at,
                        rejected_at: decision.rejected_at,
                        sent_at: decision.sent_at,
                        superseded_by: decision.superseded_by,
                        sent_turn_id: decision.sent_turn_id,
                        notes: decision.notes,
                    },
                ))
            }
            BackendCommand::ReplacePendingSteerSupervisorDecision {
                decision_id,
                proposed_text,
            } => {
                let decision_index = guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .iter()
                    .position(|decision| decision.decision_id == decision_id)
                    .ok_or_else(|| anyhow!("unknown supervisor decision `{decision_id}`"))?;
                let existing =
                    guard.snapshot.collaboration.supervisor_turn_decisions[decision_index].clone();
                if existing.kind != orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn {
                    return Err(anyhow!(
                        "supervisor decision `{decision_id}` is not a steer decision"
                    ));
                }
                if existing.status != orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman {
                    return Err(anyhow!(
                        "supervisor decision `{decision_id}` is no longer editable"
                    ));
                }
                let trimmed_text = proposed_text.trim();
                if trimmed_text.is_empty() {
                    return Err(anyhow!(
                        "pending steer replacement requires non-empty proposed_text"
                    ));
                }
                if guard
                    .snapshot
                    .threads
                    .iter()
                    .find(|thread| thread.id == existing.codex_thread_id)
                    .and_then(|thread| thread.active_turn_id.as_deref())
                    != existing.basis_turn_id.as_deref()
                {
                    let existing_decision = guard
                        .snapshot
                        .collaboration
                        .supervisor_turn_decisions
                        .get_mut(decision_index)
                        .expect("existing decision");
                    existing_decision.status = orcas_core::SupervisorTurnDecisionStatus::Stale;
                    existing_decision.open = false;
                    return Err(anyhow!(
                        "steer decision `{decision_id}` became stale: active turn changed"
                    ));
                }
                let replacement_id = format!("std-{}", guard.next_submit_id);
                guard.next_submit_id += 1;
                let now = chrono::Utc::now();
                let existing_decision = guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .get_mut(decision_index)
                    .expect("existing decision");
                existing_decision.status = orcas_core::SupervisorTurnDecisionStatus::Superseded;
                existing_decision.superseded_by = Some(replacement_id.clone());
                existing_decision.open = false;
                let replacement = ipc::SupervisorTurnDecisionSummary {
                    decision_id: replacement_id.clone(),
                    assignment_id: existing.assignment_id.clone(),
                    codex_thread_id: existing.codex_thread_id.clone(),
                    workstream_id: existing.workstream_id.clone(),
                    work_unit_id: existing.work_unit_id.clone(),
                    supervisor_id: existing.supervisor_id.clone(),
                    basis_turn_id: existing.basis_turn_id.clone(),
                    kind: orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn,
                    proposal_kind: orcas_core::SupervisorTurnProposalKind::OperatorSteer,
                    proposed_text: Some(trimmed_text.to_string()),
                    rationale_summary: "Operator revised the pending steer guidance.".to_string(),
                    status: orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
                    created_at: now,
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: Some(format!(
                        "steer text replaced from prior decision {} by tui_operator",
                        decision_id
                    )),
                    open: true,
                };
                if let Some(assignment) = guard
                    .snapshot
                    .collaboration
                    .codex_thread_assignments
                    .iter_mut()
                    .find(|assignment| assignment.assignment_id == existing.assignment_id)
                {
                    assignment.latest_decision_id = Some(replacement_id);
                    assignment.latest_basis_turn_id = existing.basis_turn_id.clone();
                    assignment.updated_at = now;
                }
                guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .push(replacement.clone());
                Ok(BackendCommandResult::SupervisorDecision(
                    SupervisorTurnDecision {
                        decision_id: replacement.decision_id,
                        assignment_id: replacement.assignment_id,
                        codex_thread_id: replacement.codex_thread_id,
                        basis_turn_id: replacement.basis_turn_id,
                        kind: replacement.kind,
                        proposal_kind: replacement.proposal_kind,
                        proposed_text: replacement.proposed_text,
                        rationale_summary: replacement.rationale_summary,
                        status: replacement.status,
                        created_at: replacement.created_at,
                        approved_at: replacement.approved_at,
                        rejected_at: replacement.rejected_at,
                        sent_at: replacement.sent_at,
                        superseded_by: replacement.superseded_by,
                        sent_turn_id: replacement.sent_turn_id,
                        notes: replacement.notes,
                    },
                ))
            }
            BackendCommand::ProposeInterruptSupervisorDecision { assignment_id } => {
                let assignment_index = guard
                    .snapshot
                    .collaboration
                    .codex_thread_assignments
                    .iter()
                    .position(|assignment| assignment.assignment_id == assignment_id)
                    .ok_or_else(|| anyhow!("unknown Codex assignment `{assignment_id}`"))?;
                let assignment_snapshot =
                    guard.snapshot.collaboration.codex_thread_assignments[assignment_index].clone();
                if !assignment_snapshot.active {
                    return Err(anyhow!("Codex assignment `{assignment_id}` is not active"));
                }
                if guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .iter()
                    .any(|decision| decision.assignment_id == assignment_id && decision.open)
                {
                    return Err(anyhow!(
                        "Codex assignment `{assignment_id}` already has an open supervisor decision"
                    ));
                }
                let active_turn_id = guard
                    .snapshot
                    .threads
                    .iter()
                    .find(|thread| thread.id == assignment_snapshot.codex_thread_id)
                    .and_then(|thread| thread.active_turn_id.clone())
                    .ok_or_else(|| {
                        anyhow!(
                            "thread `{}` has no active turn to interrupt",
                            assignment_snapshot.codex_thread_id
                        )
                    })?;
                let now = chrono::Utc::now();
                let decision_id = format!("std-{}", guard.next_submit_id);
                let decision = ipc::SupervisorTurnDecisionSummary {
                    decision_id,
                    assignment_id: assignment_snapshot.assignment_id.clone(),
                    codex_thread_id: assignment_snapshot.codex_thread_id.clone(),
                    workstream_id: Some(assignment_snapshot.workstream_id.clone()),
                    work_unit_id: Some(assignment_snapshot.work_unit_id.clone()),
                    supervisor_id: Some(assignment_snapshot.supervisor_id.clone()),
                    basis_turn_id: Some(active_turn_id.clone()),
                    kind: orcas_core::SupervisorTurnDecisionKind::InterruptActiveTurn,
                    proposal_kind: orcas_core::SupervisorTurnProposalKind::OperatorInterrupt,
                    proposed_text: None,
                    rationale_summary: format!(
                        "Operator requested review of interrupting active turn `{active_turn_id}`."
                    ),
                    status: orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
                    created_at: now,
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: Some("interrupt proposal requested by tui_operator".to_string()),
                    open: true,
                };
                guard.next_submit_id += 1;
                if let Some(assignment) = guard
                    .snapshot
                    .collaboration
                    .codex_thread_assignments
                    .get_mut(assignment_index)
                {
                    assignment.latest_decision_id = Some(decision.decision_id.clone());
                    assignment.latest_basis_turn_id = decision.basis_turn_id.clone();
                    assignment.updated_at = now;
                }
                guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .push(decision.clone());
                Ok(BackendCommandResult::SupervisorDecision(
                    SupervisorTurnDecision {
                        decision_id: decision.decision_id,
                        assignment_id: decision.assignment_id,
                        codex_thread_id: decision.codex_thread_id,
                        basis_turn_id: decision.basis_turn_id,
                        kind: decision.kind,
                        proposal_kind: decision.proposal_kind,
                        proposed_text: decision.proposed_text,
                        rationale_summary: decision.rationale_summary,
                        status: decision.status,
                        created_at: decision.created_at,
                        approved_at: decision.approved_at,
                        rejected_at: decision.rejected_at,
                        sent_at: decision.sent_at,
                        superseded_by: decision.superseded_by,
                        sent_turn_id: decision.sent_turn_id,
                        notes: decision.notes,
                    },
                ))
            }
            BackendCommand::RecordNoActionSupervisorDecision { decision_id } => {
                let decision_index = guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .iter()
                    .position(|decision| decision.decision_id == decision_id)
                    .ok_or_else(|| anyhow!("unknown supervisor decision `{decision_id}`"))?;
                let existing =
                    guard.snapshot.collaboration.supervisor_turn_decisions[decision_index].clone();
                if existing.kind != orcas_core::SupervisorTurnDecisionKind::NextTurn {
                    return Err(anyhow!(
                        "supervisor decision `{decision_id}` is not a next-turn decision"
                    ));
                }
                if existing.status != orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman {
                    return Err(anyhow!(
                        "supervisor decision `{decision_id}` is not pending human review"
                    ));
                }
                let now = chrono::Utc::now();
                let replacement_id = format!("std-{}", guard.next_submit_id);
                guard.next_submit_id += 1;
                let previous = guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .get_mut(decision_index)
                    .expect("existing decision");
                previous.status = orcas_core::SupervisorTurnDecisionStatus::Superseded;
                previous.open = false;
                previous.superseded_by = Some(replacement_id.clone());
                let recorded = ipc::SupervisorTurnDecisionSummary {
                    decision_id: replacement_id.clone(),
                    assignment_id: existing.assignment_id.clone(),
                    codex_thread_id: existing.codex_thread_id.clone(),
                    workstream_id: existing.workstream_id.clone(),
                    work_unit_id: existing.work_unit_id.clone(),
                    supervisor_id: existing.supervisor_id.clone(),
                    basis_turn_id: existing.basis_turn_id.clone(),
                    kind: orcas_core::SupervisorTurnDecisionKind::NoAction,
                    proposal_kind: existing.proposal_kind,
                    proposed_text: None,
                    rationale_summary: "Operator chose to wait on the current idle-thread basis."
                        .to_string(),
                    status: orcas_core::SupervisorTurnDecisionStatus::Recorded,
                    created_at: now,
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: Some("no_action recorded by tui_operator".to_string()),
                    open: false,
                };
                if let Some(assignment) = guard
                    .snapshot
                    .collaboration
                    .codex_thread_assignments
                    .iter_mut()
                    .find(|assignment| assignment.assignment_id == existing.assignment_id)
                {
                    assignment.latest_decision_id = Some(replacement_id);
                    assignment.latest_basis_turn_id = existing.basis_turn_id.clone();
                    assignment.updated_at = now;
                    if existing.proposal_kind == orcas_core::SupervisorTurnProposalKind::Bootstrap {
                        assignment.bootstrap_state =
                            orcas_core::CodexThreadBootstrapState::NotNeeded;
                    }
                }
                guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .push(recorded.clone());
                Ok(BackendCommandResult::SupervisorDecision(
                    SupervisorTurnDecision {
                        decision_id: recorded.decision_id,
                        assignment_id: recorded.assignment_id,
                        codex_thread_id: recorded.codex_thread_id,
                        basis_turn_id: recorded.basis_turn_id,
                        kind: recorded.kind,
                        proposal_kind: recorded.proposal_kind,
                        proposed_text: recorded.proposed_text,
                        rationale_summary: recorded.rationale_summary,
                        status: recorded.status,
                        created_at: recorded.created_at,
                        approved_at: recorded.approved_at,
                        rejected_at: recorded.rejected_at,
                        sent_at: recorded.sent_at,
                        superseded_by: recorded.superseded_by,
                        sent_turn_id: recorded.sent_turn_id,
                        notes: recorded.notes,
                    },
                ))
            }
            BackendCommand::ManualRefreshSupervisorDecision { assignment_id } => {
                let assignment_index = guard
                    .snapshot
                    .collaboration
                    .codex_thread_assignments
                    .iter()
                    .position(|assignment| assignment.assignment_id == assignment_id)
                    .ok_or_else(|| anyhow!("unknown Codex assignment `{assignment_id}`"))?;
                let assignment_snapshot =
                    guard.snapshot.collaboration.codex_thread_assignments[assignment_index].clone();
                if assignment_snapshot.status != orcas_core::CodexThreadAssignmentStatus::Active {
                    return Err(anyhow!(
                        "Codex thread assignment `{assignment_id}` is not active"
                    ));
                }
                if guard
                    .snapshot
                    .threads
                    .iter()
                    .find(|thread| thread.id == assignment_snapshot.codex_thread_id)
                    .and_then(|thread| thread.active_turn_id.as_ref())
                    .is_some()
                {
                    return Err(anyhow!(
                        "thread `{}` has an active turn and cannot manual-refresh a next-turn proposal",
                        assignment_snapshot.codex_thread_id
                    ));
                }
                if guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .iter()
                    .any(|decision| decision.assignment_id == assignment_id && decision.open)
                {
                    return Err(anyhow!(
                        "assignment `{assignment_id}` already has an open supervisor decision"
                    ));
                }
                let basis_turn_id = guard
                    .snapshot
                    .threads
                    .iter()
                    .find(|thread| thread.id == assignment_snapshot.codex_thread_id)
                    .and_then(|thread| thread.last_seen_turn_id.clone());
                let latest_basis_decision = guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .iter()
                    .filter(|decision| {
                        decision.assignment_id == assignment_id
                            && decision.basis_turn_id == basis_turn_id
                    })
                    .max_by(|left, right| {
                        left.created_at
                            .cmp(&right.created_at)
                            .then_with(|| left.decision_id.cmp(&right.decision_id))
                    })
                    .cloned()
                    .ok_or_else(|| {
                        anyhow!(
                            "assignment `{assignment_id}` has no recorded no_action for the current basis"
                        )
                    })?;
                if latest_basis_decision.kind != orcas_core::SupervisorTurnDecisionKind::NoAction
                    || latest_basis_decision.status
                        != orcas_core::SupervisorTurnDecisionStatus::Recorded
                {
                    return Err(anyhow!(
                        "assignment `{assignment_id}` has no recorded no_action for the current basis"
                    ));
                }
                let now = chrono::Utc::now();
                let decision_id = format!("std-{}", guard.next_submit_id);
                guard.next_submit_id += 1;
                let decision = ipc::SupervisorTurnDecisionSummary {
                    decision_id,
                    assignment_id: assignment_snapshot.assignment_id.clone(),
                    codex_thread_id: assignment_snapshot.codex_thread_id.clone(),
                    workstream_id: Some(assignment_snapshot.workstream_id.clone()),
                    work_unit_id: Some(assignment_snapshot.work_unit_id.clone()),
                    supervisor_id: Some(assignment_snapshot.supervisor_id.clone()),
                    basis_turn_id: basis_turn_id.clone(),
                    kind: orcas_core::SupervisorTurnDecisionKind::NextTurn,
                    proposal_kind: orcas_core::SupervisorTurnProposalKind::ManualRefresh,
                    proposed_text: Some(
                        "Continue under Orcas supervision for the assigned work unit.".to_string(),
                    ),
                    rationale_summary:
                        "Operator requested manual refresh of the next-turn proposal.".to_string(),
                    status: orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
                    created_at: now,
                    approved_at: None,
                    rejected_at: None,
                    sent_at: None,
                    superseded_by: None,
                    sent_turn_id: None,
                    notes: Some("manual refresh requested by tui_operator".to_string()),
                    open: true,
                };
                if let Some(assignment) = guard
                    .snapshot
                    .collaboration
                    .codex_thread_assignments
                    .get_mut(assignment_index)
                {
                    assignment.latest_decision_id = Some(decision.decision_id.clone());
                    assignment.latest_basis_turn_id = basis_turn_id;
                    assignment.updated_at = now;
                }
                guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .push(decision.clone());
                Ok(BackendCommandResult::SupervisorDecision(
                    SupervisorTurnDecision {
                        decision_id: decision.decision_id,
                        assignment_id: decision.assignment_id,
                        codex_thread_id: decision.codex_thread_id,
                        basis_turn_id: decision.basis_turn_id,
                        kind: decision.kind,
                        proposal_kind: decision.proposal_kind,
                        proposed_text: decision.proposed_text,
                        rationale_summary: decision.rationale_summary,
                        status: decision.status,
                        created_at: decision.created_at,
                        approved_at: decision.approved_at,
                        rejected_at: decision.rejected_at,
                        sent_at: decision.sent_at,
                        superseded_by: decision.superseded_by,
                        sent_turn_id: decision.sent_turn_id,
                        notes: decision.notes,
                    },
                ))
            }
            BackendCommand::ApproveSupervisorDecision { decision_id } => {
                let decision_index = guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .iter()
                    .position(|decision| decision.decision_id == decision_id)
                    .ok_or_else(|| anyhow!("unknown supervisor decision `{decision_id}`"))?;
                let kind =
                    guard.snapshot.collaboration.supervisor_turn_decisions[decision_index].kind;
                let sent_turn_id = if kind == orcas_core::SupervisorTurnDecisionKind::NextTurn {
                    let next_turn_id = format!("turn-{}", guard.next_submit_id);
                    guard.next_submit_id += 1;
                    Some(next_turn_id)
                } else {
                    None
                };
                let decision = guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .get_mut(decision_index)
                    .expect("decision index remains valid");
                decision.status = orcas_core::SupervisorTurnDecisionStatus::Sent;
                decision.open = false;
                decision.approved_at = Some(chrono::Utc::now());
                decision.sent_at = Some(chrono::Utc::now());
                decision.sent_turn_id = sent_turn_id;
                Ok(BackendCommandResult::SupervisorDecision(
                    SupervisorTurnDecision {
                        decision_id: decision.decision_id.clone(),
                        assignment_id: decision.assignment_id.clone(),
                        codex_thread_id: decision.codex_thread_id.clone(),
                        basis_turn_id: decision.basis_turn_id.clone(),
                        kind: decision.kind,
                        proposal_kind: decision.proposal_kind,
                        proposed_text: decision.proposed_text.clone(),
                        rationale_summary: decision.rationale_summary.clone(),
                        status: decision.status,
                        created_at: decision.created_at,
                        approved_at: decision.approved_at,
                        rejected_at: decision.rejected_at,
                        sent_at: decision.sent_at,
                        superseded_by: decision.superseded_by.clone(),
                        sent_turn_id: decision.sent_turn_id.clone(),
                        notes: decision.notes.clone(),
                    },
                ))
            }
            BackendCommand::RejectSupervisorDecision { decision_id } => {
                let decision = guard
                    .snapshot
                    .collaboration
                    .supervisor_turn_decisions
                    .iter_mut()
                    .find(|decision| decision.decision_id == decision_id)
                    .ok_or_else(|| anyhow!("unknown supervisor decision `{decision_id}`"))?;
                decision.status = orcas_core::SupervisorTurnDecisionStatus::Rejected;
                decision.open = false;
                decision.rejected_at = Some(chrono::Utc::now());
                Ok(BackendCommandResult::SupervisorDecision(
                    SupervisorTurnDecision {
                        decision_id: decision.decision_id.clone(),
                        assignment_id: decision.assignment_id.clone(),
                        codex_thread_id: decision.codex_thread_id.clone(),
                        basis_turn_id: decision.basis_turn_id.clone(),
                        kind: decision.kind,
                        proposal_kind: decision.proposal_kind,
                        proposed_text: decision.proposed_text.clone(),
                        rationale_summary: decision.rationale_summary.clone(),
                        status: decision.status,
                        created_at: decision.created_at,
                        approved_at: decision.approved_at,
                        rejected_at: decision.rejected_at,
                        sent_at: decision.sent_at,
                        superseded_by: decision.superseded_by.clone(),
                        sent_turn_id: decision.sent_turn_id.clone(),
                        notes: decision.notes.clone(),
                    },
                ))
            }
        }
    }
}

struct FakeAuthorityState {
    workstreams: HashMap<String, authority::WorkstreamRecord>,
    work_units: HashMap<String, authority::WorkUnitRecord>,
    tracked_threads: HashMap<String, authority::TrackedThreadRecord>,
}

fn authority_state_from_snapshot(snapshot: &ipc::StateSnapshot) -> FakeAuthorityState {
    let mut workstreams = HashMap::new();
    let mut work_units = HashMap::new();
    let mut tracked_threads = HashMap::new();

    for workstream in &snapshot.collaboration.workstreams {
        let id = authority::WorkstreamId::parse(workstream.id.clone()).unwrap_or_else(|_| {
            authority::WorkstreamId::parse(format!("ws-{}", authority::WorkstreamId::new()))
                .expect("generated workstream id")
        });
        workstreams.insert(
            workstream.id.clone(),
            authority::WorkstreamRecord {
                id,
                title: workstream.title.clone(),
                objective: workstream.objective.clone(),
                status: workstream.status,
                priority: workstream.priority.clone(),
                revision: authority::Revision::initial(),
                origin_node_id: fake_origin_node_id(),
                created_at: workstream.updated_at,
                updated_at: workstream.updated_at,
                deleted_at: None,
            },
        );
    }

    for work_unit in &snapshot.collaboration.work_units {
        let workstream_id = workstreams
            .get(work_unit.workstream_id.as_str())
            .map(|record| record.id.clone())
            .unwrap_or_else(|| {
                authority::WorkstreamId::parse(work_unit.workstream_id.clone())
                    .expect("snapshot workstream id")
            });
        work_units.insert(
            work_unit.id.clone(),
            authority::WorkUnitRecord {
                id: authority::WorkUnitId::parse(work_unit.id.clone()).expect("snapshot workunit"),
                workstream_id,
                title: work_unit.title.clone(),
                task_statement: work_unit.title.clone(),
                status: work_unit.status,
                revision: authority::Revision::initial(),
                origin_node_id: fake_origin_node_id(),
                created_at: work_unit.updated_at,
                updated_at: work_unit.updated_at,
                deleted_at: None,
            },
        );
    }

    for assignment in &snapshot.collaboration.codex_thread_assignments {
        let Some(work_unit) = work_units.get(assignment.work_unit_id.as_str()) else {
            continue;
        };
        let thread = snapshot
            .threads
            .iter()
            .find(|thread| thread.id == assignment.codex_thread_id);
        let title = thread
            .and_then(|thread| thread.name.clone())
            .unwrap_or_else(|| assignment.codex_thread_id.clone());
        tracked_threads.insert(
            assignment.codex_thread_id.clone(),
            authority::TrackedThreadRecord {
                id: authority::TrackedThreadId::parse(assignment.codex_thread_id.clone())
                    .unwrap_or_else(|_| authority::TrackedThreadId::new()),
                work_unit_id: work_unit.id.clone(),
                title,
                notes: assignment.notes.clone(),
                backend_kind: authority::TrackedThreadBackendKind::Codex,
                upstream_thread_id: Some(assignment.codex_thread_id.clone()),
                binding_state: authority::TrackedThreadBindingState::Bound,
                preferred_cwd: thread.map(|thread| thread.cwd.clone()),
                preferred_model: None,
                last_seen_turn_id: thread.and_then(|thread| thread.last_seen_turn_id.clone()),
                workspace: None,
                revision: authority::Revision::initial(),
                origin_node_id: fake_origin_node_id(),
                created_at: assignment.assigned_at,
                updated_at: assignment.updated_at,
                deleted_at: None,
            },
        );
    }

    FakeAuthorityState {
        workstreams,
        work_units,
        tracked_threads,
    }
}

fn build_authority_hierarchy(
    state: &FakeBackendState,
    include_deleted: bool,
) -> authority::HierarchySnapshot {
    let mut workstreams = state
        .authority_workstreams
        .values()
        .filter(|workstream| include_deleted || workstream.deleted_at.is_none())
        .cloned()
        .collect::<Vec<_>>();
    workstreams.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });

    authority::HierarchySnapshot {
        workstreams: workstreams
            .into_iter()
            .map(|workstream| {
                let mut work_units = state
                    .authority_work_units
                    .values()
                    .filter(|work_unit| {
                        work_unit.workstream_id == workstream.id
                            && (include_deleted || work_unit.deleted_at.is_none())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                work_units.sort_by(|left, right| {
                    right
                        .updated_at
                        .cmp(&left.updated_at)
                        .then_with(|| left.id.as_str().cmp(right.id.as_str()))
                });

                authority::WorkstreamNode {
                    workstream: authority::WorkstreamSummary::from(&workstream),
                    work_units: work_units
                        .into_iter()
                        .map(|work_unit| {
                            let mut tracked_threads = state
                                .authority_tracked_threads
                                .values()
                                .filter(|tracked_thread| {
                                    tracked_thread.work_unit_id == work_unit.id
                                        && (include_deleted || tracked_thread.deleted_at.is_none())
                                })
                                .cloned()
                                .collect::<Vec<_>>();
                            tracked_threads.sort_by(|left, right| {
                                right
                                    .updated_at
                                    .cmp(&left.updated_at)
                                    .then_with(|| left.id.as_str().cmp(right.id.as_str()))
                            });
                            authority::WorkUnitNode {
                                work_unit: authority::WorkUnitSummary::from(&work_unit),
                                tracked_threads: tracked_threads
                                    .iter()
                                    .map(authority::TrackedThreadSummary::from)
                                    .collect(),
                            }
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

fn build_delete_plan(
    state: &FakeBackendState,
    target: &authority::DeleteTarget,
) -> Option<authority::DeletePlan> {
    match target {
        authority::DeleteTarget::Workstream { workstream_id } => {
            let workstream = state.authority_workstreams.get(workstream_id.as_str())?;
            let work_units = state
                .authority_work_units
                .values()
                .filter(|work_unit| {
                    work_unit.workstream_id == *workstream_id && work_unit.deleted_at.is_none()
                })
                .collect::<Vec<_>>();
            let tracked_threads = state
                .authority_tracked_threads
                .values()
                .filter(|tracked_thread| {
                    tracked_thread.deleted_at.is_none()
                        && work_units
                            .iter()
                            .any(|work_unit| work_unit.id == tracked_thread.work_unit_id)
                })
                .collect::<Vec<_>>();
            Some(authority::DeletePlan {
                target: authority::DeletePlanTarget {
                    aggregate_key: authority::AggregateKey::workstream(workstream_id),
                    label: workstream.title.clone(),
                },
                expected_revision: workstream.revision,
                affected_work_units: work_units.len() as u64,
                affected_tracked_threads: tracked_threads.len() as u64,
                has_upstream_bindings: tracked_threads
                    .iter()
                    .any(|tracked_thread| tracked_thread.upstream_thread_id.is_some()),
                confirmation_token: authority::DeleteToken::new(),
                requires_typed_confirmation: !work_units.is_empty() || !tracked_threads.is_empty(),
                expires_at: Utc::now() + chrono::TimeDelta::minutes(5),
            })
        }
        authority::DeleteTarget::WorkUnit { work_unit_id } => {
            let work_unit = state.authority_work_units.get(work_unit_id.as_str())?;
            let tracked_threads = state
                .authority_tracked_threads
                .values()
                .filter(|tracked_thread| {
                    tracked_thread.work_unit_id == *work_unit_id
                        && tracked_thread.deleted_at.is_none()
                })
                .collect::<Vec<_>>();
            Some(authority::DeletePlan {
                target: authority::DeletePlanTarget {
                    aggregate_key: authority::AggregateKey::work_unit(work_unit_id),
                    label: work_unit.title.clone(),
                },
                expected_revision: work_unit.revision,
                affected_work_units: 0,
                affected_tracked_threads: tracked_threads.len() as u64,
                has_upstream_bindings: tracked_threads
                    .iter()
                    .any(|tracked_thread| tracked_thread.upstream_thread_id.is_some()),
                confirmation_token: authority::DeleteToken::new(),
                requires_typed_confirmation: !tracked_threads.is_empty(),
                expires_at: Utc::now() + chrono::TimeDelta::minutes(5),
            })
        }
        authority::DeleteTarget::TrackedThread { tracked_thread_id } => {
            let tracked_thread = state
                .authority_tracked_threads
                .get(tracked_thread_id.as_str())?;
            Some(authority::DeletePlan {
                target: authority::DeletePlanTarget {
                    aggregate_key: authority::AggregateKey::tracked_thread(tracked_thread_id),
                    label: tracked_thread.title.clone(),
                },
                expected_revision: tracked_thread.revision,
                affected_work_units: 0,
                affected_tracked_threads: 0,
                has_upstream_bindings: tracked_thread.upstream_thread_id.is_some(),
                confirmation_token: authority::DeleteToken::new(),
                requires_typed_confirmation: false,
                expires_at: Utc::now() + chrono::TimeDelta::minutes(5),
            })
        }
    }
}

fn fake_origin_node_id() -> authority::OriginNodeId {
    authority::OriginNodeId::parse("fake-authority-node").expect("static fake origin node id")
}

fn turn_state_from_active_turn(turn: &ipc::ActiveTurn) -> ipc::TurnStateView {
    ipc::TurnStateView {
        thread_id: turn.thread_id.clone(),
        turn_id: turn.turn_id.clone(),
        lifecycle: ipc::TurnLifecycleState::Active,
        status: turn.status.clone(),
        attachable: true,
        live_stream: true,
        terminal: false,
        recent_output: None,
        recent_event: Some(format!("turn {}", turn.status)),
        updated_at: turn.updated_at,
        error_message: None,
    }
}

fn workunit_details_from_snapshot(
    snapshot: &ipc::StateSnapshot,
) -> HashMap<String, ipc::WorkunitGetResponse> {
    let mut details = HashMap::new();
    for work_unit in &snapshot.collaboration.work_units {
        let assignments = snapshot
            .collaboration
            .assignments
            .iter()
            .filter(|assignment| assignment.work_unit_id == work_unit.id)
            .map(|assignment| Assignment {
                id: assignment.id.clone(),
                work_unit_id: assignment.work_unit_id.clone(),
                worker_id: assignment.worker_id.clone(),
                worker_session_id: assignment.worker_session_id.clone(),
                instructions: String::new(),
                communication_seed: None,
                status: assignment.status,
                attempt_number: assignment.attempt_number,
                created_at: assignment.updated_at,
                updated_at: assignment.updated_at,
            })
            .collect::<Vec<_>>();
        let reports = snapshot
            .collaboration
            .reports
            .iter()
            .filter(|report| report.work_unit_id == work_unit.id)
            .map(|report| Report {
                id: report.id.clone(),
                work_unit_id: report.work_unit_id.clone(),
                assignment_id: report.assignment_id.clone(),
                worker_id: report.worker_id.clone(),
                disposition: report.disposition,
                summary: report.summary.clone(),
                findings: Vec::new(),
                blockers: Vec::new(),
                questions: Vec::new(),
                recommended_next_actions: Vec::new(),
                confidence: report.confidence,
                raw_output: String::new(),
                parse_result: report.parse_result,
                needs_supervisor_review: report.needs_supervisor_review,
                created_at: report.created_at,
            })
            .collect::<Vec<_>>();
        let decisions = snapshot
            .collaboration
            .decisions
            .iter()
            .filter(|decision| decision.work_unit_id == work_unit.id)
            .map(|decision| Decision {
                id: decision.id.clone(),
                work_unit_id: decision.work_unit_id.clone(),
                report_id: decision.report_id.clone(),
                decision_type: decision.decision_type,
                rationale: decision.rationale.clone(),
                created_at: decision.created_at,
            })
            .collect::<Vec<_>>();
        details.insert(
            work_unit.id.clone(),
            ipc::WorkunitGetResponse {
                work_unit: WorkUnit {
                    id: work_unit.id.clone(),
                    workstream_id: work_unit.workstream_id.clone(),
                    title: work_unit.title.clone(),
                    task_statement: String::new(),
                    status: work_unit.status,
                    dependencies: Vec::new(),
                    latest_report_id: work_unit.latest_report_id.clone(),
                    current_assignment_id: work_unit.current_assignment_id.clone(),
                    created_at: work_unit.updated_at,
                    updated_at: work_unit.updated_at,
                },
                assignments,
                reports,
                decisions,
                proposals: Vec::new(),
            },
        );
    }
    details
}

fn proposal_artifact_summaries_from_snapshot(
    snapshot: &ipc::StateSnapshot,
) -> HashMap<String, ipc::SupervisorProposalArtifactSummary> {
    snapshot
        .collaboration
        .work_units
        .iter()
        .filter_map(|work_unit| {
            let proposal = work_unit.proposal.as_ref()?;
            Some(ipc::SupervisorProposalArtifactSummary {
                proposal_id: proposal.latest_proposal_id.clone(),
                proposal_status: proposal.latest_status,
                prompt_artifact_present: false,
                prompt_template_version: None,
                prompt_hash: None,
                request_body_hash: None,
                response_artifact_present: false,
                response_hash: None,
                raw_response_body_present: false,
                raw_response_body_hash: None,
                reasoner_backend: String::new(),
                reasoner_model: String::new(),
                reasoner_response_id: None,
                parsed_proposal_present: false,
                approved_proposal_present: false,
                generation_failure_stage: proposal.latest_failure_stage,
            })
        })
        .map(|summary| (summary.proposal_id.clone(), summary))
        .collect()
}

fn proposal_artifact_summary_lists_from_snapshot(
    snapshot: &ipc::StateSnapshot,
) -> HashMap<String, ipc::ProposalArtifactSummaryListForWorkunitResponse> {
    snapshot
        .collaboration
        .work_units
        .iter()
        .map(|work_unit| {
            let work_unit_id = work_unit.id.clone();
            let summaries = work_unit
                .proposal
                .as_ref()
                .map(|proposal| {
                    vec![ipc::SupervisorProposalArtifactSummary {
                        proposal_id: proposal.latest_proposal_id.clone(),
                        proposal_status: proposal.latest_status,
                        prompt_artifact_present: false,
                        prompt_template_version: None,
                        prompt_hash: None,
                        request_body_hash: None,
                        response_artifact_present: false,
                        response_hash: None,
                        raw_response_body_present: false,
                        raw_response_body_hash: None,
                        reasoner_backend: String::new(),
                        reasoner_model: String::new(),
                        reasoner_response_id: None,
                        parsed_proposal_present: false,
                        approved_proposal_present: false,
                        generation_failure_stage: proposal.latest_failure_stage,
                    }]
                })
                .unwrap_or_default();
            (
                work_unit_id.clone(),
                ipc::ProposalArtifactSummaryListForWorkunitResponse {
                    work_unit_id,
                    summaries,
                },
            )
        })
        .collect()
}

fn proposal_artifact_details_from_snapshot(
    snapshot: &ipc::StateSnapshot,
) -> HashMap<String, ipc::SupervisorProposalArtifactDetail> {
    workunit_details_from_snapshot(snapshot)
        .into_values()
        .flat_map(|detail| {
            detail
                .proposals
                .into_iter()
                .map(|proposal| proposal_artifact_detail_from_record(&proposal))
        })
        .map(|detail| (detail.proposal_id.clone(), detail))
        .collect()
}

fn proposal_artifact_exports_from_snapshot(
    snapshot: &ipc::StateSnapshot,
) -> HashMap<String, ipc::SupervisorProposalArtifactExport> {
    workunit_details_from_snapshot(snapshot)
        .into_values()
        .flat_map(|detail| {
            let work_unit_id = detail.work_unit.id.clone();
            detail
                .proposals
                .into_iter()
                .map(move |proposal| proposal_artifact_export_from_record(&work_unit_id, &proposal))
        })
        .map(|export| (export.proposal_id.clone(), export))
        .collect()
}

fn proposal_artifact_summary_from_record(
    proposal: &SupervisorProposalRecord,
) -> ipc::SupervisorProposalArtifactSummary {
    ipc::SupervisorProposalArtifactSummary {
        proposal_id: proposal.id.clone(),
        proposal_status: proposal.status,
        prompt_artifact_present: proposal.prompt_render.is_some(),
        prompt_template_version: proposal
            .prompt_render
            .as_ref()
            .map(|artifact| artifact.render_spec.template_version.clone()),
        prompt_hash: proposal
            .prompt_render
            .as_ref()
            .map(|artifact| artifact.prompt_hash.clone()),
        request_body_hash: proposal
            .prompt_render
            .as_ref()
            .and_then(|artifact| artifact.request_body_hash.clone()),
        response_artifact_present: proposal.response_artifact.is_some(),
        response_hash: proposal
            .response_artifact
            .as_ref()
            .map(|artifact| artifact.response_hash.clone()),
        raw_response_body_present: proposal
            .response_artifact
            .as_ref()
            .is_some_and(|artifact| artifact.raw_response_body.is_some()),
        raw_response_body_hash: proposal
            .response_artifact
            .as_ref()
            .and_then(|artifact| artifact.raw_response_body_hash.clone()),
        reasoner_backend: proposal.reasoner_backend.clone(),
        reasoner_model: proposal.reasoner_model.clone(),
        reasoner_response_id: proposal.reasoner_response_id.clone(),
        parsed_proposal_present: proposal.proposal.is_some(),
        approved_proposal_present: proposal.approved_proposal.is_some(),
        generation_failure_stage: proposal
            .generation_failure
            .as_ref()
            .map(|failure| failure.stage),
    }
}

fn proposal_artifact_detail_from_record(
    proposal: &SupervisorProposalRecord,
) -> ipc::SupervisorProposalArtifactDetail {
    ipc::SupervisorProposalArtifactDetail {
        proposal_id: proposal.id.clone(),
        proposal_status: proposal.status,
        created_at: proposal.created_at,
        validated_at: proposal.validated_at,
        reviewed_at: proposal.reviewed_at,
        reasoner_backend: proposal.reasoner_backend.clone(),
        reasoner_model: proposal.reasoner_model.clone(),
        reasoner_response_id: proposal.reasoner_response_id.clone(),
        prompt_render: proposal.prompt_render.clone(),
        response_artifact: proposal.response_artifact.clone(),
        reasoner_output_text: proposal.reasoner_output_text.clone(),
        parsed_proposal: proposal.proposal.clone(),
        approved_proposal: proposal.approved_proposal.clone(),
        generation_failure: proposal.generation_failure.clone(),
    }
}

fn proposal_artifact_export_from_record(
    work_unit_id: &str,
    proposal: &SupervisorProposalRecord,
) -> ipc::SupervisorProposalArtifactExport {
    ipc::SupervisorProposalArtifactExport {
        proposal_id: proposal.id.clone(),
        primary_work_unit_id: work_unit_id.to_string(),
        source_report_id: proposal.source_report_id.clone(),
        proposal_status: proposal.status,
        created_at: proposal.created_at,
        validated_at: proposal.validated_at,
        reviewed_at: proposal.reviewed_at,
        reviewed_by: proposal.reviewed_by.clone(),
        review_note: proposal.review_note.clone(),
        approved_decision_id: proposal.approved_decision_id.clone(),
        approved_assignment_id: proposal.approved_assignment_id.clone(),
        artifact_summary: proposal_artifact_summary_from_record(proposal),
        artifact_detail: proposal_artifact_detail_from_record(proposal),
    }
}
