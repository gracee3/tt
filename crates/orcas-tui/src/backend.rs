use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc};

use orcas_core::{AppPaths, Assignment, Decision, Report, SupervisorTurnDecision, WorkUnit, ipc};
use orcas_daemon::{
    OrcasDaemonLaunch, OrcasDaemonProcessManager, OrcasIpcClient, OrcasRuntimeOverrides,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendCommand {
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
    ApproveSupervisorDecision {
        decision_id: String,
    },
    RejectSupervisorDecision {
        decision_id: String,
    },
}

#[derive(Debug, Clone)]
pub enum BackendCommandResult {
    Snapshot(ipc::StateSnapshot),
    Thread(ipc::ThreadView),
    ThreadAttached(ipc::ThreadAttachResponse),
    Turn(ipc::TurnAttachResponse),
    WorkUnit(ipc::WorkunitGetResponse),
    ActiveTurns(Vec<ipc::TurnStateView>),
    Models(Vec<ipc::ModelSummary>),
    DaemonStarted { connected: bool },
    DaemonStopped { stopping: bool },
    PromptStarted { thread_id: String, turn_id: String },
    SupervisorDecision(SupervisorTurnDecision),
}

#[async_trait]
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
        let paths = AppPaths::discover()?;
        paths.ensure().await?;
        let daemon =
            OrcasDaemonProcessManager::new(paths.clone(), OrcasRuntimeOverrides::default());
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

        self.connect_client(launch).await
    }

    async fn connect_client(&self, launch: OrcasDaemonLaunch) -> Result<Arc<OrcasIpcClient>> {
        self.daemon.ensure_running(launch).await?;
        let client = OrcasIpcClient::connect(&self.paths).await?;
        client.daemon_connect().await?;
        let mut guard = self.client.lock().await;
        *guard = Some(Arc::clone(&client));
        Ok(client)
    }

    async fn invalidate_client(&self) {
        let mut guard = self.client.lock().await;
        *guard = None;
    }
}

#[async_trait]
impl TuiBackend for OrcasDaemonBackend {
    async fn get_snapshot(&self) -> Result<ipc::StateSnapshot> {
        let client = self.ensure_client(OrcasDaemonLaunch::Never).await?;
        match client.state_get().await {
            Ok(response) => Ok(response.snapshot),
            Err(_) => {
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
        let launch = match command {
            BackendCommand::StartDaemon => OrcasDaemonLaunch::IfNeeded,
            _ => OrcasDaemonLaunch::Never,
        };
        let client = self.ensure_client(launch).await?;
        match Self::execute_with_client(&client, command.clone()).await {
            Ok(result) => Ok(result),
            Err(_) => {
                self.invalidate_client().await;
                let client = self.connect_client(launch).await?;
                Self::execute_with_client(&client, command).await
            }
        }
    }
}

impl OrcasDaemonBackend {
    async fn execute_with_client(
        client: &Arc<OrcasIpcClient>,
        command: BackendCommand,
    ) -> Result<BackendCommandResult> {
        match command {
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
            BackendCommand::GetWorkUnit { work_unit_id } => Ok(BackendCommandResult::WorkUnit(
                client
                    .workunit_get(&ipc::WorkunitGetRequest { work_unit_id })
                    .await?,
            )),
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

#[derive(Clone)]
pub struct FakeBackend {
    inner: Arc<Mutex<FakeBackendState>>,
}

struct FakeBackendState {
    snapshot: ipc::StateSnapshot,
    threads: HashMap<String, ipc::ThreadView>,
    turns: HashMap<(String, String), ipc::TurnAttachResponse>,
    work_unit_details: HashMap<String, ipc::WorkunitGetResponse>,
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
        let active_turns = snapshot
            .session
            .active_turns
            .iter()
            .map(turn_state_from_active_turn)
            .collect();
        let mut guard = self.inner.lock().await;
        guard.work_unit_details = workunit_details_from_snapshot(&snapshot);
        guard.snapshot = snapshot;
        guard.active_turns = active_turns;
    }

    pub async fn set_workunit_detail(&self, detail: ipc::WorkunitGetResponse) {
        self.inner
            .lock()
            .await
            .work_unit_details
            .insert(detail.work_unit.id.clone(), detail);
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
