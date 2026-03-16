use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::app::{Action, AppState, Effect, UiEvent, reduce};
use crate::backend::{BackendCommand, BackendCommandResult, TuiBackend};

const RECONNECT_BASE_DELAY: Duration = Duration::from_millis(250);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
struct ReconnectSchedule {
    due_at: Instant,
}

pub struct AppRuntime<B: TuiBackend> {
    backend: Arc<B>,
    state: AppState,
    pending_effects: VecDeque<Effect>,
    event_rx: Option<mpsc::Receiver<orcas_core::ipc::DaemonEventEnvelope>>,
    reconnect: Option<ReconnectSchedule>,
}

impl<B: TuiBackend> AppRuntime<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self {
            backend,
            state: AppState::default(),
            pending_effects: VecDeque::new(),
            event_rx: None,
            reconnect: None,
        }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn dispatch(&mut self, action: Action) {
        let effects = reduce(&mut self.state, action);
        self.pending_effects.extend(effects);
    }

    pub async fn bootstrap(&mut self) {
        self.dispatch(Action::Start);
        self.process_all().await;
    }

    pub async fn process_all(&mut self) {
        self.enqueue_due_reconnect();
        self.drain_backend_events();
        while let Some(effect) = self.pending_effects.pop_front() {
            self.run_effect(effect).await;
            self.enqueue_due_reconnect();
            self.drain_backend_events();
        }
    }

    pub fn drain_backend_events(&mut self) {
        loop {
            let next = match self.event_rx.as_mut() {
                Some(rx) => rx.try_recv(),
                None => break,
            };
            match next {
                Ok(event) => self.dispatch(Action::Event(UiEvent::from_daemon(event))),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.event_rx = None;
                    self.dispatch(Action::Event(UiEvent::ConnectionLost(
                        "Daemon event stream disconnected.".to_string(),
                    )));
                    break;
                }
            }
        }
    }

    pub fn force_reconnect_now(&mut self) {
        if let Some(reconnect) = self.reconnect.as_mut() {
            reconnect.due_at = Instant::now();
        }
    }

    fn enqueue_due_reconnect(&mut self) {
        let Some(reconnect) = self.reconnect else {
            return;
        };
        if reconnect.due_at > Instant::now() {
            return;
        }
        self.reconnect = None;
        if !self
            .pending_effects
            .iter()
            .any(|effect| matches!(effect, Effect::RefreshSnapshot))
        {
            self.pending_effects.push_back(Effect::RefreshSnapshot);
        }
    }

    fn schedule_reconnect(&mut self) {
        if self.reconnect.is_some() {
            return;
        }

        let attempt = self.state.reconnect_attempt.saturating_add(1);
        let backoff_multiplier = 2_u32.saturating_pow(attempt.saturating_sub(1).min(6));
        let delay = (RECONNECT_BASE_DELAY * backoff_multiplier).min(RECONNECT_MAX_DELAY);
        self.reconnect = Some(ReconnectSchedule {
            due_at: Instant::now() + delay,
        });
        self.dispatch(Action::Event(UiEvent::ReconnectScheduled {
            attempt,
            delay_ms: delay.as_millis() as u64,
        }));
    }

    async fn run_effect(&mut self, effect: Effect) {
        match effect {
            Effect::RefreshSnapshot => match self.backend.get_snapshot().await {
                Ok(snapshot) => {
                    self.reconnect = None;
                    self.dispatch(Action::Event(UiEvent::SnapshotLoaded(snapshot)));
                    if self.event_rx.is_none() {
                        self.pending_effects.push_back(Effect::SubscribeEvents);
                    }
                }
                Err(error) => {
                    self.dispatch(Action::Event(UiEvent::ConnectionLost(format!(
                        "snapshot failed: {error}"
                    ))));
                }
            },
            Effect::SubscribeEvents => match self.backend.subscribe_events().await {
                Ok(events) => {
                    self.event_rx = Some(events);
                }
                Err(error) => {
                    self.event_rx = None;
                    self.dispatch(Action::Event(UiEvent::ConnectionLost(format!(
                        "subscribe failed: {error}"
                    ))));
                }
            },
            Effect::ScheduleReconnect => {
                self.event_rx = None;
                self.schedule_reconnect();
            }
            Effect::LoadActiveTurns => {
                match self.backend.execute(BackendCommand::GetActiveTurns).await {
                    Ok(BackendCommandResult::ActiveTurns(turns)) => {
                        self.dispatch(Action::Event(UiEvent::ActiveTurnsLoaded(turns)));
                    }
                    Ok(other) => {
                        self.dispatch(Action::Event(UiEvent::Error(format!(
                            "unexpected active-turn response: {other:?}"
                        ))));
                    }
                    Err(error) => {
                        if Self::is_disconnect_error(&error) {
                            self.dispatch(Action::Event(UiEvent::ConnectionLost(format!(
                                "active turn load failed: {error}"
                            ))));
                        } else {
                            self.dispatch(Action::Event(UiEvent::Error(format!(
                                "active turn load failed: {error}"
                            ))));
                        }
                    }
                }
            }
            Effect::LoadThread { thread_id } => {
                match self
                    .backend
                    .execute(BackendCommand::GetThread {
                        thread_id: thread_id.clone(),
                    })
                    .await
                {
                    Ok(BackendCommandResult::Thread(thread)) => {
                        self.dispatch(Action::Event(UiEvent::ThreadLoaded(thread)));
                    }
                    Ok(other) => {
                        self.dispatch(Action::Event(UiEvent::Error(format!(
                            "unexpected thread response: {other:?}"
                        ))));
                    }
                    Err(error) => {
                        if Self::is_disconnect_error(&error) {
                            self.dispatch(Action::Event(UiEvent::ConnectionLost(format!(
                                "thread load failed for {thread_id}: {error}"
                            ))));
                        } else {
                            self.dispatch(Action::Event(UiEvent::Error(format!(
                                "thread load failed for {thread_id}: {error}"
                            ))));
                        }
                    }
                }
            }
            Effect::LoadTurnState { thread_id, turn_id } => {
                match self
                    .backend
                    .execute(BackendCommand::GetTurn {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                    })
                    .await
                {
                    Ok(BackendCommandResult::Turn(turn)) => {
                        self.dispatch(Action::Event(UiEvent::TurnStateLoaded(turn)));
                    }
                    Ok(other) => {
                        self.dispatch(Action::Event(UiEvent::Error(format!(
                            "unexpected turn response: {other:?}"
                        ))));
                    }
                    Err(error) => {
                        if Self::is_disconnect_error(&error) {
                            self.dispatch(Action::Event(UiEvent::ConnectionLost(format!(
                                "turn load failed for {thread_id}/{turn_id}: {error}"
                            ))));
                        } else {
                            self.dispatch(Action::Event(UiEvent::Error(format!(
                                "turn load failed for {thread_id}/{turn_id}: {error}"
                            ))));
                        }
                    }
                }
            }
            Effect::SubmitPrompt { thread_id, text } => {
                match self
                    .backend
                    .execute(BackendCommand::SubmitPrompt {
                        thread_id: thread_id.clone(),
                        text,
                    })
                    .await
                {
                    Ok(BackendCommandResult::PromptStarted { thread_id, turn_id }) => {
                        self.dispatch(Action::Event(UiEvent::PromptStarted { thread_id, turn_id }));
                    }
                    Ok(other) => {
                        self.dispatch(Action::Event(UiEvent::Error(format!(
                            "unexpected prompt response: {other:?}"
                        ))));
                    }
                    Err(error) => {
                        if Self::is_disconnect_error(&error) {
                            self.dispatch(Action::Event(UiEvent::ConnectionLost(format!(
                                "prompt submit failed: {error}"
                            ))));
                        } else {
                            self.dispatch(Action::Event(UiEvent::Error(format!(
                                "prompt submit failed: {error}"
                            ))));
                        }
                    }
                }
            }
        }
    }

    fn is_disconnect_error(error: &anyhow::Error) -> bool {
        let message = error.to_string().to_ascii_lowercase();
        [
            "failed to connect to orcas daemon",
            "orcas daemon connection closed",
            "orcas daemon read failed",
            "response channel dropped",
            "connection refused",
            "broken pipe",
            "no such file or directory",
            "daemon is not reachable",
        ]
        .iter()
        .any(|needle| message.contains(needle))
    }
}

pub async fn bootstrap_runtime<B: TuiBackend>(backend: Arc<B>) -> Result<AppRuntime<B>> {
    let mut runtime = AppRuntime::new(backend);
    runtime.bootstrap().await;
    Ok(runtime)
}
