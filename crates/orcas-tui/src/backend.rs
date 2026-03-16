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
    SubmitPrompt { thread_id: String, text: String },
}

#[derive(Debug, Clone)]
pub enum BackendCommandResult {
    Snapshot(ipc::StateSnapshot),
    Thread(ipc::ThreadView),
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

        self.daemon
            .ensure_running(OrcasDaemonLaunch::IfNeeded)
            .await?;
        let client = OrcasIpcClient::connect(&self.paths).await?;
        client.daemon_connect().await?;
        let mut guard = self.client.lock().await;
        *guard = Some(Arc::clone(&client));
        Ok(client)
    }
}

#[async_trait]
impl TuiBackend for OrcasDaemonBackend {
    async fn get_snapshot(&self) -> Result<ipc::StateSnapshot> {
        let client = self.ensure_client().await?;
        Ok(client.state_get().await?.snapshot)
    }

    async fn subscribe_events(&self) -> Result<mpsc::Receiver<ipc::DaemonEventEnvelope>> {
        let client = self.ensure_client().await?;
        let (events, _) = client.subscribe_events(false).await?;
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
        match command {
            BackendCommand::GetThread { thread_id } => Ok(BackendCommandResult::Thread(
                client
                    .thread_get(&ipc::ThreadGetRequest { thread_id })
                    .await?
                    .thread,
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
    next_submit_id: usize,
    fail_snapshot: Option<String>,
    fail_next_command: Option<String>,
    recorded_commands: Vec<BackendCommand>,
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
        Self {
            inner: Arc::new(Mutex::new(FakeBackendState {
                snapshot,
                threads,
                next_submit_id: 1,
                fail_snapshot: None,
                fail_next_command: None,
                recorded_commands: Vec::new(),
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
        self.inner.lock().await.snapshot = snapshot;
    }

    pub async fn fail_snapshot_once(&self, message: impl Into<String>) {
        self.inner.lock().await.fail_snapshot = Some(message.into());
    }

    pub async fn fail_next_command(&self, message: impl Into<String>) {
        self.inner.lock().await.fail_next_command = Some(message.into());
    }

    pub async fn inject_event(&self, event: ipc::DaemonEventEnvelope) -> Result<()> {
        let tx = self
            .inner
            .lock()
            .await
            .event_tx
            .clone()
            .ok_or_else(|| anyhow!("event subscription has not been established"))?;
        tx.send(event).await?;
        Ok(())
    }

    pub async fn recorded_commands(&self) -> Vec<BackendCommand> {
        self.inner.lock().await.recorded_commands.clone()
    }
}

#[async_trait]
impl TuiBackend for FakeBackend {
    async fn get_snapshot(&self) -> Result<ipc::StateSnapshot> {
        let mut guard = self.inner.lock().await;
        if let Some(message) = guard.fail_snapshot.take() {
            return Err(anyhow!(message));
        }
        Ok(guard.snapshot.clone())
    }

    async fn subscribe_events(&self) -> Result<mpsc::Receiver<ipc::DaemonEventEnvelope>> {
        let (tx, rx) = mpsc::channel(256);
        self.inner.lock().await.event_tx = Some(tx);
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
            BackendCommand::SubmitPrompt { thread_id, .. } => {
                let turn_id = format!("turn-{}", guard.next_submit_id);
                guard.next_submit_id += 1;
                Ok(BackendCommandResult::PromptStarted { thread_id, turn_id })
            }
        }
    }
}
