//! TUI runtime loop and recovery scheduler.
//!
//! This module drives effect execution, IPC interaction, and reconnect
//! scheduling for the TUI. Recovery is snapshot-first and event subscriptions
//! are socket-bound, so this layer rebuilds runtime state around reconnect
//! boundaries instead of trying to replay missed daemon history.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};
use tracing::{info, trace, warn};

use crate::app::{Action, AppState, Effect, UiEvent, UserAction, reduce};
use crate::backend::{BackendCommand, BackendCommandResult, TuiBackend};
use orcas_core::logging::runtime_cycle_enabled;

const RECONNECT_BASE_DELAY: Duration = Duration::from_millis(250);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
/// Internal timer for a deferred reconnect attempt.
struct ReconnectSchedule {
    due_at: Instant,
}

#[derive(Debug)]
/// Bookkeeping for a finished effect.
///
/// Some completions intentionally schedule follow-up effects or connection
/// invalidation because recovery is staged: snapshot first, then event
/// subscription, then incremental updates.
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

/// Runtime controller for the TUI reducer.
///
/// The runtime owns effect execution, event subscription, and reconnect timing.
/// It does not own durable domain state; it rebuilds state by feeding snapshots
/// and incremental events back through the reducer.
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
        trace!(action = action_label(&action), "dispatching app action");
        let effects = reduce(&mut self.state, action);
        trace!(effect_count = effects.len(), "action reduced to effects");
        for effect in effects {
            self.enqueue_effect(effect);
        }
    }

    pub async fn bootstrap(&mut self) {
        self.dispatch(Action::Start);
        self.process_all().await;
    }

    pub async fn process_all(&mut self) {
        if runtime_cycle_enabled() {
            trace!(
                pending = self.pending_effects.len(),
                running = self.running_effects.len(),
                "processing runtime cycle"
            );
        }
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
        trace!(effect_kind = effect_label(&effect), "enqueueing effect");
        if self.running_effects.contains(&effect) {
            return;
        }
        if self.pending_effects.contains(&effect) {
            return;
        }
        self.pending_effects.push_back(effect);
    }

    fn start_effect(&mut self, effect: Effect) {
        trace!(effect_kind = effect_label(&effect), "starting effect");
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
        trace!(
            effect_kind = effect_label(&completion.effect),
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
            // Subscription is recreated only after a fresh snapshot has been
            // applied; the old socket stream is not treated as replayable state.
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
            trace!(completion_count, "drained effect completions");
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
        // Reconnect always re-enters through a snapshot reload rather than
        // assuming missed daemon events can be replayed.
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
                    // The snapshot re-establishes the current state first. Once
                    // it is applied, the runtime requests a new event
                    // subscription on the same socket lifecycle.
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
                    // This subscription is tied to the current daemon socket.
                    // If it fails, the runtime invalidates the stream and lets
                    // reconnect rebuild it from a fresh snapshot.
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
                // Scheduling reconnect is a state-machine transition, not a
                // replay request; the old event stream is intentionally dropped.
                let mut completion = EffectCompletion::success(effect, Vec::new());
                completion.clear_event_rx = true;
                completion.schedule_reconnect = true;
                completion
            }
            effect @ Effect::LoadAuthorityHierarchy => {
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetAuthorityHierarchy {
                        include_deleted: false,
                    },
                    |response| match response {
                        BackendCommandResult::AuthorityHierarchy(hierarchy) => {
                            vec![Action::Event(UiEvent::AuthorityHierarchyLoaded(hierarchy))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority hierarchy response: {other:?}"
                        )))],
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "authority hierarchy load failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "authority hierarchy load failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::LoadAuthorityWorkstreamDetail { workstream_id } => {
                let effect = Effect::LoadAuthorityWorkstreamDetail {
                    workstream_id: workstream_id.clone(),
                };
                let parsed_workstream_id =
                    match orcas_core::authority::WorkstreamId::parse(workstream_id.clone()) {
                        Ok(id) => id,
                        Err(error) => {
                            return EffectCompletion::failure(
                                effect,
                                Action::Event(UiEvent::Error(format!(
                                    "authority workstream id parse failed: {error}"
                                ))),
                            );
                        }
                    };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetAuthorityWorkstream {
                        workstream_id: parsed_workstream_id,
                    },
                    |response| match response {
                        BackendCommandResult::AuthorityWorkstreamDetail(detail) => {
                            vec![Action::Event(UiEvent::AuthorityWorkstreamDetailLoaded(
                                detail,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority workstream response: {other:?}"
                        )))],
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "authority workstream load failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "authority workstream load failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::LoadAuthorityWorkUnitDetail { work_unit_id } => {
                let effect = Effect::LoadAuthorityWorkUnitDetail {
                    work_unit_id: work_unit_id.clone(),
                };
                let parsed_work_unit_id =
                    match orcas_core::authority::WorkUnitId::parse(work_unit_id.clone()) {
                        Ok(id) => id,
                        Err(error) => {
                            return EffectCompletion::failure(
                                effect,
                                Action::Event(UiEvent::Error(format!(
                                    "authority work unit id parse failed: {error}"
                                ))),
                            );
                        }
                    };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetAuthorityWorkUnit {
                        work_unit_id: parsed_work_unit_id,
                    },
                    |response| match response {
                        BackendCommandResult::AuthorityWorkUnitDetail(detail) => {
                            vec![Action::Event(UiEvent::AuthorityWorkUnitDetailLoaded(
                                detail,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority work unit response: {other:?}"
                        )))],
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "authority work unit load failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "authority work unit load failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::LoadAuthorityTrackedThreadDetail { tracked_thread_id } => {
                let effect = Effect::LoadAuthorityTrackedThreadDetail {
                    tracked_thread_id: tracked_thread_id.clone(),
                };
                let parsed_tracked_thread_id = match orcas_core::authority::TrackedThreadId::parse(
                    tracked_thread_id.clone(),
                ) {
                    Ok(id) => id,
                    Err(error) => {
                        return EffectCompletion::failure(
                            effect,
                            Action::Event(UiEvent::Error(format!(
                                "authority tracked thread id parse failed: {error}"
                            ))),
                        );
                    }
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetAuthorityTrackedThread {
                        tracked_thread_id: parsed_tracked_thread_id,
                    },
                    |response| match response {
                        BackendCommandResult::AuthorityTrackedThreadDetail(detail) => {
                            vec![Action::Event(UiEvent::AuthorityTrackedThreadDetailLoaded(
                                detail,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority tracked thread response: {other:?}"
                        )))],
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "authority tracked thread load failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "authority tracked thread load failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::LoadAuthorityDeletePlan { target } => {
                let effect = Effect::LoadAuthorityDeletePlan {
                    target: target.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetAuthorityDeletePlan { target },
                    |response| match response {
                        BackendCommandResult::AuthorityDeletePlan(plan) => {
                            vec![Action::Event(UiEvent::AuthorityDeletePlanLoaded(plan))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority delete plan response: {other:?}"
                        )))],
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "authority delete plan failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "authority delete plan failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::CreateAuthorityWorkstream { command } => {
                let effect = Effect::CreateAuthorityWorkstream {
                    command: command.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::CreateAuthorityWorkstream { command },
                    |response| match response {
                        BackendCommandResult::AuthorityWorkstream(workstream) => {
                            vec![Action::Event(UiEvent::AuthorityWorkstreamCreated(
                                workstream,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority workstream create response: {other:?}"
                        )))],
                    },
                    |error| {
                        Action::Event(UiEvent::Error(format!(
                            "authority workstream create failed: {error}"
                        )))
                    },
                )
                .await
            }
            Effect::EditAuthorityWorkstream { command } => {
                let effect = Effect::EditAuthorityWorkstream {
                    command: command.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::EditAuthorityWorkstream { command },
                    |response| match response {
                        BackendCommandResult::AuthorityWorkstream(workstream) => {
                            vec![Action::Event(UiEvent::AuthorityWorkstreamEdited(
                                workstream,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority workstream edit response: {other:?}"
                        )))],
                    },
                    |error| {
                        Action::Event(UiEvent::Error(format!(
                            "authority workstream edit failed: {error}"
                        )))
                    },
                )
                .await
            }
            Effect::DeleteAuthorityWorkstream { command } => {
                let effect = Effect::DeleteAuthorityWorkstream {
                    command: command.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::DeleteAuthorityWorkstream { command },
                    |response| match response {
                        BackendCommandResult::AuthorityWorkstream(workstream) => {
                            vec![Action::Event(UiEvent::AuthorityWorkstreamDeleted(
                                workstream,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority workstream delete response: {other:?}"
                        )))],
                    },
                    |error| {
                        Action::Event(UiEvent::Error(format!(
                            "authority workstream delete failed: {error}"
                        )))
                    },
                )
                .await
            }
            Effect::CreateAuthorityWorkUnit { command } => {
                let effect = Effect::CreateAuthorityWorkUnit {
                    command: command.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::CreateAuthorityWorkUnit { command },
                    |response| match response {
                        BackendCommandResult::AuthorityWorkUnit(work_unit) => {
                            vec![Action::Event(UiEvent::AuthorityWorkUnitCreated(work_unit))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority work unit create response: {other:?}"
                        )))],
                    },
                    |error| {
                        Action::Event(UiEvent::Error(format!(
                            "authority work unit create failed: {error}"
                        )))
                    },
                )
                .await
            }
            Effect::EditAuthorityWorkUnit { command } => {
                let effect = Effect::EditAuthorityWorkUnit {
                    command: command.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::EditAuthorityWorkUnit { command },
                    |response| match response {
                        BackendCommandResult::AuthorityWorkUnit(work_unit) => {
                            vec![Action::Event(UiEvent::AuthorityWorkUnitEdited(work_unit))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority work unit edit response: {other:?}"
                        )))],
                    },
                    |error| {
                        Action::Event(UiEvent::Error(format!(
                            "authority work unit edit failed: {error}"
                        )))
                    },
                )
                .await
            }
            Effect::DeleteAuthorityWorkUnit { command } => {
                let effect = Effect::DeleteAuthorityWorkUnit {
                    command: command.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::DeleteAuthorityWorkUnit { command },
                    |response| match response {
                        BackendCommandResult::AuthorityWorkUnit(work_unit) => {
                            vec![Action::Event(UiEvent::AuthorityWorkUnitDeleted(work_unit))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority work unit delete response: {other:?}"
                        )))],
                    },
                    |error| {
                        Action::Event(UiEvent::Error(format!(
                            "authority work unit delete failed: {error}"
                        )))
                    },
                )
                .await
            }
            Effect::CreateAuthorityTrackedThread { command } => {
                let effect = Effect::CreateAuthorityTrackedThread {
                    command: command.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::CreateAuthorityTrackedThread { command },
                    |response| match response {
                        BackendCommandResult::AuthorityTrackedThread(tracked_thread) => {
                            vec![Action::Event(UiEvent::AuthorityTrackedThreadCreated(
                                tracked_thread,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority tracked thread create response: {other:?}"
                        )))],
                    },
                    |error| {
                        Action::Event(UiEvent::Error(format!(
                            "authority tracked thread create failed: {error}"
                        )))
                    },
                )
                .await
            }
            Effect::EditAuthorityTrackedThread { command } => {
                let effect = Effect::EditAuthorityTrackedThread {
                    command: command.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::EditAuthorityTrackedThread { command },
                    |response| match response {
                        BackendCommandResult::AuthorityTrackedThread(tracked_thread) => {
                            vec![Action::Event(UiEvent::AuthorityTrackedThreadEdited(
                                tracked_thread,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority tracked thread edit response: {other:?}"
                        )))],
                    },
                    |error| {
                        Action::Event(UiEvent::Error(format!(
                            "authority tracked thread edit failed: {error}"
                        )))
                    },
                )
                .await
            }
            Effect::DeleteAuthorityTrackedThread { command } => {
                let effect = Effect::DeleteAuthorityTrackedThread {
                    command: command.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::DeleteAuthorityTrackedThread { command },
                    |response| match response {
                        BackendCommandResult::AuthorityTrackedThread(tracked_thread) => {
                            vec![Action::Event(UiEvent::AuthorityTrackedThreadDeleted(
                                tracked_thread,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected authority tracked thread delete response: {other:?}"
                        )))],
                    },
                    |error| {
                        Action::Event(UiEvent::Error(format!(
                            "authority tracked thread delete failed: {error}"
                        )))
                    },
                )
                .await
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
            Effect::AttachThread { thread_id } => {
                let effect = Effect::AttachThread {
                    thread_id: thread_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::AttachThread {
                        thread_id: thread_id.clone(),
                    },
                    |response| match response {
                        BackendCommandResult::ThreadAttached(response) => {
                            vec![Action::Event(UiEvent::ThreadAttached(response))]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected thread-attach response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "thread attach failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!("thread attach failed: {error}")))
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
            Effect::LoadProposalArtifactSummary { proposal_id } => {
                let effect = Effect::LoadProposalArtifactSummary {
                    proposal_id: proposal_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetProposalArtifactSummary {
                        proposal_id: proposal_id.clone(),
                    },
                    |response| match response {
                        BackendCommandResult::ProposalArtifactSummary(summary) => {
                            vec![Action::Event(UiEvent::ProposalArtifactSummaryLoaded(
                                summary,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected proposal artifact summary response: {other:?}"
                        )))],
                    },
                    move |error| {
                        Action::Event(UiEvent::ProposalArtifactSummaryLoadFailed {
                            proposal_id: proposal_id.clone(),
                            message: error.to_string(),
                        })
                    },
                )
                .await
            }
            Effect::LoadProposalArtifactSummaryListForWorkUnit { work_unit_id } => {
                let effect = Effect::LoadProposalArtifactSummaryListForWorkUnit {
                    work_unit_id: work_unit_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetProposalArtifactSummaryListForWorkUnit {
                        work_unit_id: work_unit_id.clone(),
                    },
                    |response| match response {
                        BackendCommandResult::ProposalArtifactSummaryListForWorkUnit(response) => {
                            vec![Action::Event(UiEvent::ProposalArtifactSummaryListLoaded(
                                response,
                            ))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected proposal artifact summary list response: {other:?}"
                        )))],
                    },
                    move |error| {
                        Action::Event(UiEvent::ProposalArtifactSummaryListLoadFailed {
                            work_unit_id: work_unit_id.clone(),
                            message: error.to_string(),
                        })
                    },
                )
                .await
            }
            Effect::LoadProposalArtifactDetail { proposal_id } => {
                let effect = Effect::LoadProposalArtifactDetail {
                    proposal_id: proposal_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetProposalArtifactDetail {
                        proposal_id: proposal_id.clone(),
                    },
                    |response| match response {
                        BackendCommandResult::ProposalArtifactDetail(detail) => {
                            vec![Action::Event(UiEvent::ProposalArtifactDetailLoaded(detail))]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected proposal artifact detail response: {other:?}"
                        )))],
                    },
                    move |error| {
                        Action::Event(UiEvent::ProposalArtifactDetailLoadFailed {
                            proposal_id: proposal_id.clone(),
                            message: error.to_string(),
                        })
                    },
                )
                .await
            }
            Effect::ExportProposalArtifact {
                proposal_id,
                destination,
                format,
            } => {
                let effect = Effect::ExportProposalArtifact {
                    proposal_id: proposal_id.clone(),
                    destination: destination.clone(),
                    format,
                };
                let success_proposal_id = proposal_id.clone();
                let success_destination = destination.clone();
                let failure_proposal_id = proposal_id.clone();
                let success_format = format;
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::GetProposalArtifactExport {
                        proposal_id: proposal_id.clone(),
                    },
                    move |response| match response {
                        BackendCommandResult::ProposalArtifactExport(export) => {
                            vec![Action::Event(
                                match write_proposal_artifact_export(
                                    &success_destination,
                                    success_format,
                                    &export,
                                ) {
                                    Ok(()) => UiEvent::ProposalArtifactExported {
                                        proposal_id: success_proposal_id.clone(),
                                        destination: success_destination.clone(),
                                        format: success_format,
                                    },
                                    Err(error) => UiEvent::ProposalArtifactExportFailed {
                                        proposal_id: success_proposal_id.clone(),
                                        message: error.to_string(),
                                        format: success_format,
                                    },
                                },
                            )]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected proposal artifact export response: {other:?}"
                        )))],
                    },
                    move |error| {
                        Action::Event(UiEvent::ProposalArtifactExportFailed {
                            proposal_id: failure_proposal_id.clone(),
                            message: error.to_string(),
                            format,
                        })
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
                info!("starting daemon from TUI");
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
                            Action::Event(UiEvent::DaemonStartFailed(error.to_string()))
                        }
                    },
                )
                .await
            }
            effect @ Effect::StopDaemon => {
                info!("stopping daemon from TUI");
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
                            Action::Event(UiEvent::DaemonStopFailed(error.to_string()))
                        }
                    },
                )
                .await
            }
            effect @ Effect::RestartDaemon => {
                info!("restarting daemon from TUI");
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
                            Action::Event(UiEvent::DaemonStopFailed(error.to_string()))
                        }
                    },
                )
                .await;
                let stop_succeeded = stop_completion.actions.iter().any(|action| {
                    matches!(
                        action,
                        Action::Event(UiEvent::DaemonStopped { stopping: true })
                    )
                });
                actions.extend(stop_completion.actions);
                if !stop_succeeded {
                    return EffectCompletion::success(effect, actions);
                }
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
                            Action::Event(UiEvent::DaemonStartFailed(format!(
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
            Effect::ProposeSteerDecision {
                assignment_id,
                proposed_text,
            } => {
                let effect = Effect::ProposeSteerDecision {
                    assignment_id: assignment_id.clone(),
                    proposed_text: proposed_text.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::ProposeSteerSupervisorDecision {
                        assignment_id,
                        proposed_text,
                    },
                    |response| match response {
                        BackendCommandResult::SupervisorDecision(decision) => {
                            vec![
                                Action::Event(UiEvent::SteerComposeCommitted {
                                    decision_id: decision.decision_id,
                                }),
                                Action::User(UserAction::Refresh),
                            ]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected supervisor steer response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "supervisor steer proposal failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "supervisor steer proposal failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::ReplacePendingSteerDecision {
                decision_id,
                proposed_text,
            } => {
                let effect = Effect::ReplacePendingSteerDecision {
                    decision_id: decision_id.clone(),
                    proposed_text: proposed_text.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::ReplacePendingSteerSupervisorDecision {
                        decision_id,
                        proposed_text,
                    },
                    |response| match response {
                        BackendCommandResult::SupervisorDecision(decision) => {
                            vec![
                                Action::Event(UiEvent::SteerComposeCommitted {
                                    decision_id: decision.decision_id,
                                }),
                                Action::User(UserAction::Refresh),
                            ]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected supervisor steer replacement response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "supervisor steer replacement failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "supervisor steer replacement failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::ProposeInterruptDecision { assignment_id } => {
                let effect = Effect::ProposeInterruptDecision {
                    assignment_id: assignment_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::ProposeInterruptSupervisorDecision { assignment_id },
                    |response| match response {
                        BackendCommandResult::SupervisorDecision(_) => {
                            vec![Action::User(UserAction::Refresh)]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected supervisor interrupt response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "supervisor interrupt proposal failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "supervisor interrupt proposal failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::RecordNoActionDecision { decision_id } => {
                let effect = Effect::RecordNoActionDecision {
                    decision_id: decision_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::RecordNoActionSupervisorDecision { decision_id },
                    |response| match response {
                        BackendCommandResult::SupervisorDecision(_) => {
                            vec![Action::User(UserAction::Refresh)]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected supervisor no_action response: {other:?}"
                        )))],
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "record no_action failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "record no_action failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::ManualRefreshDecision { assignment_id } => {
                let effect = Effect::ManualRefreshDecision {
                    assignment_id: assignment_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::ManualRefreshSupervisorDecision { assignment_id },
                    |response| match response {
                        BackendCommandResult::SupervisorDecision(_) => {
                            vec![Action::User(UserAction::Refresh)]
                        }
                        other => vec![Action::Event(UiEvent::Error(format!(
                            "unexpected manual refresh response: {other:?}"
                        )))],
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "manual refresh failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!("manual refresh failed: {error}")))
                        }
                    },
                )
                .await
            }
            Effect::ApproveSupervisorDecision { decision_id } => {
                let effect = Effect::ApproveSupervisorDecision {
                    decision_id: decision_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::ApproveSupervisorDecision { decision_id },
                    |response| match response {
                        BackendCommandResult::SupervisorDecision(_) => {
                            vec![Action::User(UserAction::Refresh)]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected supervisor approve response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "supervisor approve failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "supervisor approve failed: {error}"
                            )))
                        }
                    },
                )
                .await
            }
            Effect::RejectSupervisorDecision { decision_id } => {
                let effect = Effect::RejectSupervisorDecision {
                    decision_id: decision_id.clone(),
                };
                Self::run_backend_effect(
                    backend,
                    effect,
                    BackendCommand::RejectSupervisorDecision { decision_id },
                    |response| match response {
                        BackendCommandResult::SupervisorDecision(_) => {
                            vec![Action::User(UserAction::Refresh)]
                        }
                        other => {
                            vec![Action::Event(UiEvent::Error(format!(
                                "unexpected supervisor reject response: {other:?}"
                            )))]
                        }
                    },
                    |error| {
                        if Self::is_disconnect_error(&error) {
                            Action::Event(UiEvent::ConnectionLost(format!(
                                "supervisor reject failed: {error}"
                            )))
                        } else {
                            Action::Event(UiEvent::Error(format!(
                                "supervisor reject failed: {error}"
                            )))
                        }
                    },
                )
                .await
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
        trace!(
            effect_kind = effect_label(&effect),
            command = backend_command_label(&command),
            "running backend effect"
        );
        match backend.execute(command).await {
            Ok(result) => {
                let elapsed_ms = started.elapsed().as_millis() as u64;
                match effect {
                    Effect::RefreshSnapshot => {
                        info!(duration_ms = elapsed_ms, "TUI backend snapshot loaded")
                    }
                    Effect::SubscribeEvents => info!(
                        duration_ms = elapsed_ms,
                        "TUI backend event subscription established"
                    ),
                    Effect::StartDaemon => {
                        info!(duration_ms = elapsed_ms, "TUI daemon start completed")
                    }
                    Effect::StopDaemon => {
                        info!(duration_ms = elapsed_ms, "TUI daemon stop completed")
                    }
                    Effect::RestartDaemon => {
                        info!(duration_ms = elapsed_ms, "TUI daemon restart completed")
                    }
                    _ => trace!(
                        effect_kind = effect_label(&effect),
                        duration_ms = elapsed_ms,
                        "backend effect succeeded"
                    ),
                }
                EffectCompletion::success(effect, on_success(result))
            }
            Err(error) => {
                let elapsed_ms = started.elapsed().as_millis() as u64;
                match effect {
                    Effect::RefreshSnapshot => warn!(
                        duration_ms = elapsed_ms,
                        error = %error,
                        "TUI backend snapshot load failed"
                    ),
                    Effect::SubscribeEvents => warn!(
                        duration_ms = elapsed_ms,
                        error = %error,
                        "TUI backend event subscription failed"
                    ),
                    Effect::StartDaemon => {
                        warn!(duration_ms = elapsed_ms, error = %error, "TUI daemon start failed")
                    }
                    Effect::StopDaemon => {
                        warn!(duration_ms = elapsed_ms, error = %error, "TUI daemon stop failed")
                    }
                    Effect::RestartDaemon => warn!(
                        duration_ms = elapsed_ms,
                        error = %error,
                        "TUI daemon restart failed"
                    ),
                    _ => trace!(
                        effect_kind = effect_label(&effect),
                        duration_ms = elapsed_ms,
                        error = %error,
                        "backend effect failed"
                    ),
                }
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

fn write_proposal_artifact_export(
    destination: &str,
    format: crate::app::ReviewArtifactExportFormat,
    export: &orcas_core::ipc::SupervisorProposalArtifactExport,
) -> Result<()> {
    let path = std::path::Path::new(destination);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let serialized = match format {
        crate::app::ReviewArtifactExportFormat::Json => {
            format!("{}\n", serde_json::to_string_pretty(export)?)
        }
        crate::app::ReviewArtifactExportFormat::Markdown => {
            render_proposal_artifact_export_markdown(export)?
        }
    };
    std::fs::write(path, serialized)?;
    Ok(())
}

fn render_proposal_artifact_export_markdown(
    export: &orcas_core::ipc::SupervisorProposalArtifactExport,
) -> Result<String> {
    let mut out = String::new();
    out.push_str("# Supervisor Proposal Artifact Export\n\n");
    out.push_str("## Proposal Metadata\n");
    out.push_str(&format!("- Proposal ID: `{}`\n", export.proposal_id));
    out.push_str(&format!(
        "- Work Unit ID: `{}`\n",
        export.primary_work_unit_id
    ));
    out.push_str(&format!(
        "- Source Report ID: `{}`\n",
        export.source_report_id
    ));
    out.push_str(&format!("- Status: `{:?}`\n", export.proposal_status));
    out.push_str(&format!("- Created At: `{}`\n", export.created_at));
    out.push_str(&format!(
        "- Validated At: `{}`\n",
        export
            .validated_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    out.push_str(&format!(
        "- Reviewed At: `{}`\n",
        export
            .reviewed_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    out.push_str(&format!(
        "- Reviewed By: `{}`\n",
        export.reviewed_by.as_deref().unwrap_or("-")
    ));
    out.push_str(&format!(
        "- Review Note: `{}`\n",
        export.review_note.as_deref().unwrap_or("-")
    ));
    out.push_str(&format!(
        "- Approved Decision ID: `{}`\n",
        export.approved_decision_id.as_deref().unwrap_or("-")
    ));
    out.push_str(&format!(
        "- Approved Assignment ID: `{}`\n\n",
        export.approved_assignment_id.as_deref().unwrap_or("-")
    ));

    push_markdown_json_section(
        &mut out,
        "Artifact Summary",
        serde_json::to_value(&export.artifact_summary)?,
    )?;
    push_markdown_json_section(
        &mut out,
        "Prompt Artifact",
        serde_json::to_value(&export.artifact_detail.prompt_render)?,
    )?;
    push_markdown_json_section(
        &mut out,
        "Response Artifact",
        serde_json::to_value(&export.artifact_detail.response_artifact)?,
    )?;
    push_markdown_text_section(
        &mut out,
        "Extracted Output Text",
        export.artifact_detail.reasoner_output_text.as_deref(),
    );
    push_markdown_json_section(
        &mut out,
        "Parsed Proposal",
        serde_json::to_value(&export.artifact_detail.parsed_proposal)?,
    )?;
    push_markdown_json_section(
        &mut out,
        "Approved Proposal",
        serde_json::to_value(&export.artifact_detail.approved_proposal)?,
    )?;
    push_markdown_json_section(
        &mut out,
        "Failure Metadata",
        serde_json::to_value(&export.artifact_detail.generation_failure)?,
    )?;
    Ok(out)
}

fn push_markdown_json_section(
    out: &mut String,
    title: &str,
    value: serde_json::Value,
) -> Result<()> {
    out.push_str(&format!("## {title}\n"));
    out.push_str("```json\n");
    out.push_str(&serde_json::to_string_pretty(&value)?);
    out.push_str("\n```\n\n");
    Ok(())
}

fn push_markdown_text_section(out: &mut String, title: &str, value: Option<&str>) {
    out.push_str(&format!("## {title}\n"));
    match value {
        Some(value) => {
            out.push_str("```text\n");
            out.push_str(value);
            if !value.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n\n");
        }
        None => out.push_str("_none_\n\n"),
    }
}

fn effect_label(effect: &Effect) -> &'static str {
    match effect {
        Effect::RefreshSnapshot => "refresh_snapshot",
        Effect::LoadAuthorityHierarchy => "load_authority_hierarchy",
        Effect::LoadAuthorityWorkstreamDetail { .. } => "load_authority_workstream_detail",
        Effect::LoadAuthorityWorkUnitDetail { .. } => "load_authority_work_unit_detail",
        Effect::LoadAuthorityTrackedThreadDetail { .. } => "load_authority_tracked_thread_detail",
        Effect::LoadAuthorityDeletePlan { .. } => "load_authority_delete_plan",
        Effect::CreateAuthorityWorkstream { .. } => "create_authority_workstream",
        Effect::EditAuthorityWorkstream { .. } => "edit_authority_workstream",
        Effect::DeleteAuthorityWorkstream { .. } => "delete_authority_workstream",
        Effect::CreateAuthorityWorkUnit { .. } => "create_authority_work_unit",
        Effect::EditAuthorityWorkUnit { .. } => "edit_authority_work_unit",
        Effect::DeleteAuthorityWorkUnit { .. } => "delete_authority_work_unit",
        Effect::CreateAuthorityTrackedThread { .. } => "create_authority_tracked_thread",
        Effect::EditAuthorityTrackedThread { .. } => "edit_authority_tracked_thread",
        Effect::DeleteAuthorityTrackedThread { .. } => "delete_authority_tracked_thread",
        Effect::SubscribeEvents => "subscribe_events",
        Effect::ScheduleReconnect => "schedule_reconnect",
        Effect::LoadActiveTurns => "load_active_turns",
        Effect::LoadThread { .. } => "load_thread",
        Effect::AttachThread { .. } => "attach_thread",
        Effect::LoadTurnState { .. } => "load_turn_state",
        Effect::LoadWorkUnitDetail { .. } => "load_work_unit_detail",
        Effect::LoadProposalArtifactSummaryListForWorkUnit { .. } => {
            "load_proposal_artifact_summary_list_for_workunit"
        }
        Effect::LoadProposalArtifactSummary { .. } => "load_proposal_artifact_summary",
        Effect::LoadProposalArtifactDetail { .. } => "load_proposal_artifact_detail",
        Effect::ExportProposalArtifact { .. } => "export_proposal_artifact",
        Effect::SubmitPrompt { .. } => "submit_prompt",
        Effect::ProposeSteerDecision { .. } => "propose_steer_decision",
        Effect::ReplacePendingSteerDecision { .. } => "replace_pending_steer_decision",
        Effect::ProposeInterruptDecision { .. } => "propose_interrupt_decision",
        Effect::RecordNoActionDecision { .. } => "record_no_action_decision",
        Effect::ManualRefreshDecision { .. } => "manual_refresh_decision",
        Effect::ApproveSupervisorDecision { .. } => "approve_supervisor_decision",
        Effect::RejectSupervisorDecision { .. } => "reject_supervisor_decision",
        Effect::LoadModels => "load_models",
        Effect::StartDaemon => "start_daemon",
        Effect::RestartDaemon => "restart_daemon",
        Effect::StopDaemon => "stop_daemon",
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

fn action_label(action: &Action) -> &'static str {
    match action {
        Action::Start => "start",
        Action::User(user_action) => user_action_label(user_action),
        Action::Event(_) => "event",
    }
}

fn user_action_label(action: &UserAction) -> &'static str {
    match action {
        UserAction::Refresh => "refresh",
        UserAction::LoadModels => "load_models",
        UserAction::StartDaemon => "start_daemon",
        UserAction::RestartDaemon => "restart_daemon",
        UserAction::StopDaemon => "stop_daemon",
        UserAction::ToggleHelp => "toggle_help",
        UserAction::CycleView => "cycle_view",
        UserAction::ShowView(_) => "show_view",
        UserAction::CycleProgramView => "cycle_program_view",
        UserAction::ShowProgramView(_) => "show_program_view",
        UserAction::CycleCollaborationFocus => "cycle_collaboration_focus",
        UserAction::SelectNextInView => "select_next_in_view",
        UserAction::SelectPreviousInView => "select_previous_in_view",
        UserAction::ExpandSelectedInView => "expand_selected_in_view",
        UserAction::CollapseSelectedInView => "collapse_selected_in_view",
        UserAction::SelectNextThread => "select_next_thread",
        UserAction::SelectPreviousThread => "select_previous_thread",
        UserAction::SelectThread(_) => "select_thread",
        UserAction::CreateWorkstream => "create_workstream",
        UserAction::CreateWorkUnitForSelection => "create_work_unit_for_selection",
        UserAction::CreateTrackedThreadForSelection => "create_tracked_thread_for_selection",
        UserAction::EditSelectedMainEntity => "edit_selected_main_entity",
        UserAction::DeleteSelectedMainEntity => "delete_selected_main_entity",
        UserAction::MainFooterAppend(_) => "main_footer_append",
        UserAction::MainFooterBackspace => "main_footer_backspace",
        UserAction::MainFooterDelete => "main_footer_delete",
        UserAction::MainFooterMoveLeft => "main_footer_move_left",
        UserAction::MainFooterMoveRight => "main_footer_move_right",
        UserAction::MainFooterNextField => "main_footer_next_field",
        UserAction::MainFooterPreviousField => "main_footer_previous_field",
        UserAction::SubmitMainFooter => "submit_main_footer",
        UserAction::CancelMainFooter => "cancel_main_footer",
        UserAction::EnterPromptMode => "enter_prompt_mode",
        UserAction::ExitPromptMode => "exit_prompt_mode",
        UserAction::PromptAppend(_) => "prompt_append",
        UserAction::PromptBackspace => "prompt_backspace",
        UserAction::SubmitPrompt => "submit_prompt",
        UserAction::ResumeSelectedThreadInCodex => "resume_selected_thread_in_codex",
        UserAction::ProposeSteerForSelectedThread => "propose_steer_for_selected_thread",
        UserAction::EditPendingSteerForSelectedThread => "edit_pending_steer_for_selected_thread",
        UserAction::SteerComposeAppend(_) => "steer_compose_append",
        UserAction::SteerComposeInsertNewline => "steer_compose_insert_newline",
        UserAction::SteerComposeBackspace => "steer_compose_backspace",
        UserAction::SteerComposeDelete => "steer_compose_delete",
        UserAction::SteerComposeMoveLeft => "steer_compose_move_left",
        UserAction::SteerComposeMoveRight => "steer_compose_move_right",
        UserAction::SteerComposeMoveUp => "steer_compose_move_up",
        UserAction::SteerComposeMoveDown => "steer_compose_move_down",
        UserAction::SubmitSteerCompose => "submit_steer_compose",
        UserAction::CancelSteerCompose => "cancel_steer_compose",
        UserAction::ProposeInterruptForSelectedThread => "propose_interrupt_for_selected_thread",
        UserAction::RecordNoActionForSelectedThread => "record_no_action_for_selected_thread",
        UserAction::ManualRefreshForSelectedThread => "manual_refresh_for_selected_thread",
        UserAction::ApproveSelectedSupervisorDecision => "approve_selected_supervisor_decision",
        UserAction::RejectSelectedSupervisorDecision => "reject_selected_supervisor_decision",
        UserAction::OpenSelectedProposalArtifactDetail => "open_selected_proposal_artifact_detail",
        UserAction::CloseReviewArtifactDetail => "close_review_artifact_detail",
        UserAction::ScrollReviewArtifactDetail(_) => "scroll_review_artifact_detail",
        UserAction::OpenSelectedProposalArtifactExport => "open_selected_proposal_artifact_export",
        UserAction::CloseReviewArtifactExport => "close_review_artifact_export",
        UserAction::SubmitReviewArtifactExport => "submit_review_artifact_export",
        UserAction::ReviewArtifactExportToggleFormat => "review_artifact_export_toggle_format",
        UserAction::ReviewArtifactExportAppend(_) => "review_artifact_export_append",
        UserAction::ReviewArtifactExportBackspace => "review_artifact_export_backspace",
        UserAction::ReviewArtifactExportDelete => "review_artifact_export_delete",
        UserAction::ReviewArtifactExportMoveLeft => "review_artifact_export_move_left",
        UserAction::ReviewArtifactExportMoveRight => "review_artifact_export_move_right",
    }
}

pub async fn bootstrap_runtime<B: TuiBackend + Send + Sync + 'static>(
    backend: Arc<B>,
) -> Result<AppRuntime<B>> {
    let mut runtime = AppRuntime::new(backend);
    runtime.bootstrap().await;
    Ok(runtime)
}
