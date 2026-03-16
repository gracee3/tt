use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;

use crate::app::{Action, AppState, Effect, UiEvent, reduce};
use crate::backend::{BackendCommand, BackendCommandResult, TuiBackend};

pub struct AppRuntime<B: TuiBackend> {
    backend: Arc<B>,
    state: AppState,
    pending_effects: VecDeque<Effect>,
    event_rx: Option<mpsc::Receiver<orcas_core::ipc::DaemonEventEnvelope>>,
}

impl<B: TuiBackend> AppRuntime<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self {
            backend,
            state: AppState::default(),
            pending_effects: VecDeque::new(),
            event_rx: None,
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
        self.drain_backend_events();
        while let Some(effect) = self.pending_effects.pop_front() {
            self.run_effect(effect).await;
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
                    self.dispatch(Action::Event(UiEvent::Error(
                        "Daemon event stream disconnected.".to_string(),
                    )));
                    self.event_rx = None;
                    break;
                }
            }
        }
    }

    async fn run_effect(&mut self, effect: Effect) {
        match effect {
            Effect::SubscribeEvents => match self.backend.subscribe_events().await {
                Ok(events) => {
                    self.event_rx = Some(events);
                }
                Err(error) => {
                    self.dispatch(Action::Event(UiEvent::Error(format!(
                        "subscribe failed: {error}"
                    ))));
                }
            },
            Effect::RefreshSnapshot => match self.backend.get_snapshot().await {
                Ok(snapshot) => {
                    self.dispatch(Action::Event(UiEvent::SnapshotLoaded(snapshot)));
                }
                Err(error) => {
                    self.dispatch(Action::Event(UiEvent::Error(format!(
                        "snapshot failed: {error}"
                    ))));
                }
            },
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
                        self.dispatch(Action::Event(UiEvent::Error(format!(
                            "thread load failed for {thread_id}: {error}"
                        ))));
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
                        self.dispatch(Action::Event(UiEvent::Error(format!(
                            "prompt submit failed: {error}"
                        ))));
                    }
                }
            }
        }
    }
}

pub async fn bootstrap_runtime<B: TuiBackend>(backend: Arc<B>) -> Result<AppRuntime<B>> {
    let mut runtime = AppRuntime::new(backend);
    runtime.bootstrap().await;
    Ok(runtime)
}
