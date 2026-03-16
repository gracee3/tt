use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc};

use orcas_core::{AppPaths, ipc};
use orcas_daemon::{
    OrcasDaemonLaunch, OrcasDaemonProcessManager, OrcasIpcClient, OrcasRuntimeOverrides,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendCommand {
    GetThread { thread_id: String },
    GetTurn { thread_id: String, turn_id: String },
    GetActiveTurns,
    SubmitPrompt { thread_id: String, text: String },
}

#[derive(Debug, Clone)]
pub enum BackendCommandResult {
    Snapshot(ipc::StateSnapshot),
    Thread(ipc::ThreadView),
    Turn(ipc::TurnAttachResponse),
    ActiveTurns(Vec<ipc::TurnStateView>),
    PromptStarted { thread_id: String, turn_id: String },
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

    async fn ensure_client(&self) -> Result<Arc<OrcasIpcClient>> {
        if let Some(client) = self.client.lock().await.clone() {
            return Ok(client);
        }

        self.connect_client().await
    }

    async fn connect_client(&self) -> Result<Arc<OrcasIpcClient>> {
        self.daemon.ensure_running(OrcasDaemonLaunch::Never).await?;
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
        let client = self.ensure_client().await?;
        match client.state_get().await {
            Ok(response) => Ok(response.snapshot),
            Err(_) => {
                self.invalidate_client().await;
                let client = self.connect_client().await?;
                Ok(client.state_get().await?.snapshot)
            }
        }
    }

    async fn subscribe_events(&self) -> Result<mpsc::Receiver<ipc::DaemonEventEnvelope>> {
        let client = self.ensure_client().await?;
        let (events, _) = match client.subscribe_events(false).await {
            Ok(response) => response,
            Err(_) => {
                self.invalidate_client().await;
                let client = self.connect_client().await?;
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
        let client = self.ensure_client().await?;
        match Self::execute_with_client(&client, command.clone()).await {
            Ok(result) => Ok(result),
            Err(_) => {
                self.invalidate_client().await;
                let client = self.connect_client().await?;
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
                    .thread_get(&ipc::ThreadGetRequest { thread_id })
                    .await?
                    .thread,
            )),
            BackendCommand::GetTurn { thread_id, turn_id } => Ok(BackendCommandResult::Turn(
                client
                    .turn_attach(&ipc::TurnAttachRequest { thread_id, turn_id })
                    .await?,
            )),
            BackendCommand::GetActiveTurns => Ok(BackendCommandResult::ActiveTurns(
                client.turns_list_active().await?.turns,
            )),
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
    active_turns: Vec<ipc::TurnStateView>,
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
                snapshot,
                threads,
                turns: HashMap::new(),
                active_turns,
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
        guard.snapshot = snapshot;
        guard.active_turns = active_turns;
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
            BackendCommand::GetActiveTurns => Ok(BackendCommandResult::ActiveTurns(
                guard.active_turns.clone(),
            )),
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
