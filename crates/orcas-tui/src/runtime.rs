use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};

use crate::app::{Action, AppState, Effect, UiEvent, reduce};
use crate::backend::{BackendCommand, BackendCommandResult, TuiBackend};
use tracing::debug;

const RECONNECT_BASE_DELAY: Duration = Duration::from_millis(250);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
struct ReconnectSchedule {
    due_at: Instant,
}

#[derive(Debug)]
struct EffectCompletion {
    effect: Effect,
    actions: Vec<Action>,
    follow_up_effects: Vec<Effect>,
    set_event_rx: Option<mpsc::Receiver<orcas_core::ipc::DaemonEventEnvelope>>,
    clear_event_rx: bool,
    clear_reconnect: bool,
    schedule_reconnect: bool,
    request_event_subscription: bool,
}

impl EffectCompletion {
    fn success(effect: Effect, actions: Vec<Action>) -> Self {
        Self {
            effect,
            actions,
            follow_up_effects: Vec::new(),
            set_event_rx: None,
            clear_event_rx: false,
            clear_reconnect: false,
            schedule_reconnect: false,
            request_event_subscription: false,
        }
    }

    fn failure(effect: Effect, action: Action) -> Self {
        Self {
            effect,
            actions: vec![action],
            follow_up_effects: Vec::new(),
            set_event_rx: None,
            clear_event_rx: false,
            clear_reconnect: false,
            schedule_reconnect: false,
            request_event_subscription: false,
        }
    }
}

pub struct AppRuntime<B: TuiBackend> {
    backend: Arc<B>,
    state: AppState,
    pending_effects: VecDeque<Effect>,
    event_rx: Option<mpsc::Receiver<orcas_core::ipc::DaemonEventEnvelope>>,
    reconnect: Option<ReconnectSchedule>,
    running_effects: HashSet<Effect>,
    effect_tx: mpsc::UnboundedSender<EffectCompletion>,
    effect_rx: mpsc::UnboundedReceiver<EffectCompletion>,
}

impl<B: TuiBackend + Send + Sync + 'static> AppRuntime<B> {
    pub fn new(backend: Arc<B>) -> Self {
        let (effect_tx, effect_rx) = mpsc::unbounded_channel();
        Self {
            backend,
            state: AppState::default(),
            pending_effects: VecDeque::new(),
            event_rx: None,
            reconnect: None,
            running_effects: HashSet::new(),
            effect_tx,
            effect_rx,
        }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn dispatch(&mut self, action: Action) {
        debug!(?action, "dispatching app action");
        let effects = reduce(&mut self.state, action);
        debug!(?effects, "action reduced to effects");
        for effect in effects {
            self.enqueue_effect(effect);
        }
    }

    pub async fn bootstrap(&mut self) {
        self.dispatch(Action::Start);
        self.process_all().await;
    }

    pub async fn process_all(&mut self) {
        debug!(
            pending = self.pending_effects.len(),
            running = self.running_effects.len(),
            "processing runtime cycle"
        );
        self.enqueue_due_reconnect();
        self.drain_effect_completions();
        self.drain_backend_events();
        self.enqueue_due_reconnect();

        while let Some(effect) = self.pending_effects.pop_front() {
            self.start_effect(effect);
        }
    }

    pub async fn process_until_idle(&mut self, max_iterations: usize) {
        let mut attempts = 0;
        while attempts < max_iterations {
            self.process_all().await;
            if self.is_idle() {
                return;
            }
            sleep(Duration::from_millis(5)).await;
            attempts += 1;
        }
    }

    pub fn is_idle(&self) -> bool {
        self.pending_effects.is_empty() && self.running_effects.is_empty()
    }

    fn enqueue_effect(&mut self, effect: Effect) {
        debug!(?effect, "enqueueing effect");
        if self.running_effects.contains(&effect) {
            return;
        }
        if self.pending_effects.contains(&effect) {
            return;
        }
        self.pending_effects.push_back(effect);
    }

    fn start_effect(&mut self, effect: Effect) {
        debug!(?effect, "starting effect");
        self.running_effects.insert(effect.clone());

        let backend = Arc::clone(&self.backend);
        let tx = self.effect_tx.clone();

        tokio::spawn(async move {
            let completion = AppRuntime::<B>::run_effect(backend, effect).await;
            let _ = tx.send(completion);
        });
    }

    fn apply_completion(&mut self, completion: EffectCompletion) {
        self.running_effects.remove(&completion.effect);
        debug!(
            ?completion.effect,
            actions = completion.actions.len(),
            follow_ups = completion.follow_up_effects.len(),
            "completing effect"
        );

        if completion.clear_reconnect {
            self.reconnect = None;
        }
        if completion.clear_event_rx {
            self.event_rx = None;
        }
        if let Some(event_rx) = completion.set_event_rx {
            self.event_rx = Some(event_rx);
        }
        if completion.schedule_reconnect {
            self.schedule_reconnect();
        }
        if completion.request_event_subscription && self.event_rx.is_none() {
            self.enqueue_effect(Effect::SubscribeEvents);
        }

        for action in completion.actions {
            self.dispatch(action);
        }
        for effect in completion.follow_up_effects {
            self.enqueue_effect(effect);
        }
    }

    fn drain_effect_completions(&mut self) {
        let mut completion_count = 0usize;
        loop {
            match self.effect_rx.try_recv() {
                Ok(completion) => self.apply_completion(completion),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
            completion_count += 1;
        }
        if completion_count > 0 {
            debug!(completion_count, "drained effect completions");
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
            && !self.running_effects.contains(&Effect::RefreshSnapshot)
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

    async fn run_effect(backend: Arc<B>, effect: Effect) -> EffectCompletion {
        match effect {
            effect @ Effect::RefreshSnapshot => match backend.get_snapshot().await {
                Ok(snapshot) => {
                    let mut completion = EffectCompletion::success(
                        effect,
                        vec![Action::Event(UiEvent::SnapshotLoaded(snapshot))],
                    );
                    completion.clear_reconnect = true;
                    completion.request_event_subscription = true;
                    completion
                }
                Err(error) => EffectCompletion::failure(
                    effect,
                    Action::Event(UiEvent::ConnectionLost(format!("snapshot failed: {error}"))),
                ),
            },
            effect @ Effect::SubscribeEvents => match backend.subscribe_events().await {
                Ok(events) => {
                    let mut completion = EffectCompletion::success(effect, Vec::new());
                    completion.set_event_rx = Some(events);
                    completion
                }
                Err(error) => {
                    let mut completion = EffectCompletion::failure(
                        effect,
                        Action::Event(UiEvent::ConnectionLost(format!(
                            "subscribe failed: {error}"
                        ))),
                    );
                    completion.clear_event_rx = true;
                    completion
                }
            },
            effect @ Effect::ScheduleReconnect => {
                let mut completion = EffectCompletion::success(effect, Vec::new());
                completion.clear_event_rx = true;
                completion.schedule_reconnect = true;
                completion
            }
            effect @ Effect::LoadActiveTurns => {
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetActiveTurns,
                    |response| match response {
                        BackendCommandResult::ActiveTurns(turns) => {
                            vec![Action::Event(UiEvent::ActiveTurnsLoaded(turns))]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected active-turn response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "active turn load failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "active turn load failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::LoadThread { thread_id } => {
                let effect = Effect::LoadThread {
                    thread_id: thread_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetThread {
                        thread_id: thread_id.clone(),
                    },
                    |response| match response {
                        BackendCommandResult::Thread(thread) => {
                            vec![Action::Event(UiEvent::ThreadLoaded(thread))]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected thread response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "thread load failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!("thread load failed: {error}")))
                        }
                    },
                )
                .await
            }
            Effect::LoadTurnState { thread_id, turn_id } => {
                let effect = Effect::LoadTurnState {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetTurn {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                    },
                    |response| match response {
                        BackendCommandResult::Turn(turn) => {
                            vec![Action::Event(UiEvent::TurnStateLoaded(turn))]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected turn response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "turn load failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!("turn load failed: {error}")))
                        }
                    },
                )
                .await
            }
            Effect::LoadWorkUnitDetail { work_unit_id } => {
                let effect = Effect::LoadWorkUnitDetail {
                    work_unit_id: work_unit_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetWorkUnit {
                        work_unit_id: work_unit_id.clone(),
                    },
                    |response| match response {
                        BackendCommandResult::WorkUnit(detail) => {
                            vec![Action::Event(UiEvent::WorkUnitDetailLoaded(detail))]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected work-unit response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "work unit load failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!("work unit load failed: {error}")))
                        }
                    },
                )
                .await
            }
            effect @ Effect::LoadModels => {
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::LoadModels,
                    |response| match response {
                        BackendCommandResult::Models(models) => {
                            vec![Action::Event(UiEvent::ModelsLoaded(models))]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected load models response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "model load failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!("model load failed: {error}")))
                        }
                    },
                )
                .await
            }
            effect @ Effect::StartDaemon => {
                debug!(?effect, "starting start-daemon effect");
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::StartDaemon,
                    |response| match response {
                        BackendCommandResult::DaemonStarted { connected } => {
                            vec![Action::Event(UiEvent::DaemonStarted { connected })]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected start daemon response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "daemon start failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!("daemon start failed: {error}")))
                        }
                    },
                )
                .await
            }
            effect @ Effect::StopDaemon => {
                debug!(?effect, "starting stop-daemon effect");
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::StopDaemon,
                    |response| match response {
                        BackendCommandResult::DaemonStopped { stopping } => {
                            vec![Action::Event(UiEvent::DaemonStopped { stopping })]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected stop-daemon response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "daemon stop failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!("daemon stop failed: {error}")))
                        }
                    },
                )
                .await
            }
            effect @ Effect::RestartDaemon => {
                debug!(?effect, "starting restart-daemon effect");
                let mut actions = Vec::new();

                let stop_completion = Self::run_backend_effect(
                    Arc::clone(&backend),
                    Effect::StopDaemon,
                    BackendCommand::StopDaemon,
                    |response| match response {
                        BackendCommandResult::DaemonStopped { stopping } => {
                            vec![Action::Event(UiEvent::DaemonStopped { stopping })]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected stop-daemon response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "daemon stop failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!("daemon stop failed: {error}")))
                        }
                    },
                )
                .await;
                actions.extend(stop_completion.actions);
                let start_completion = Self::run_backend_effect(
                    backend,
                    Effect::StartDaemon,
                    BackendCommand::StartDaemon,
                    |response| match response {
                        BackendCommandResult::DaemonStarted { connected } => {
                            vec![Action::Event(UiEvent::DaemonStarted { connected })]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected start daemon response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "daemon restart start failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "daemon restart start failed: {error}"
                            )))
                        }
                    },
                )
                .await;
                actions.extend(start_completion.actions);
                EffectCompletion::success(effect, actions)
            }
            Effect::SubmitPrompt { thread_id, text } => {
                let completion_effect = Effect::SubmitPrompt {
                    thread_id: thread_id.clone(),
                    text: text.clone(),
                };
                match backend
                    .execute(BackendCommand::SubmitPrompt {
                        thread_id: thread_id.clone(),
                        text,
                    })
                    .await
                {
                    Ok(BackendCommandResult::PromptStarted { thread_id, turn_id }) => {
                        EffectCompletion::success(
                            completion_effect,
                            vec![Action::Event(UiEvent::PromptStarted { thread_id, turn_id })],
                        )
                    }
                    Ok(other) => EffectCompletion::failure(
                        completion_effect,
                        Action::Event(UiEvent::Error(format!(
                            "unexpected prompt response: {other:?}"
                        ))),
                    ),
                    Err(error) => {
                        let message = error.to_string();
                        if Self::is_disconnect_error(&error) {
                            EffectCompletion::failure(
                                completion_effect,
                                Action::Event(UiEvent::ConnectionLost(format!(
                                    "prompt submit failed: {message}"
                                ))),
                            )
                        } else {
                            EffectCompletion::failure(
                                completion_effect,
                                Action::Event(UiEvent::Error(format!(
                                    "prompt submit failed: {message}"
                                ))),
                            )
                        }
                    }
                }
            }
        }
    }

    async fn run_backend_effect<F, G>(
        backend: Arc<B>,
        effect: Effect,
        command: BackendCommand,
        on_success: F,
        on_error: G,
    ) -> EffectCompletion
    where
        F: FnOnce(BackendCommandResult) -> Vec<Action>,
        G: FnOnce(anyhow::Error) -> Action,
    {
        let started = Instant::now();
        debug!(?effect, ?command, "running backend effect");
        match backend.execute(command).await {
            Ok(result) => {
                debug!(
                    ?effect,
                    elapsed_ms = started.elapsed().as_millis(),
                    "backend effect succeeded"
                );
                EffectCompletion::success(effect, on_success(result))
            }
            Err(error) => {
                debug!(
                    ?effect,
                    elapsed_ms = started.elapsed().as_millis(),
                    %error,
                    "backend effect failed"
                );
                EffectCompletion::failure(effect, on_error(error))
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

pub async fn bootstrap_runtime<B: TuiBackend + Send + Sync + 'static>(
    backend: Arc<B>,
) -> Result<AppRuntime<B>> {
    let mut runtime = AppRuntime::new(backend);
    runtime.bootstrap().await;
    Ok(runtime)
}
