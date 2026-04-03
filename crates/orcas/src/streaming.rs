use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio::time::sleep;

use orcas_core::{AppConfig, AppPaths, TurnStartRequest, ipc};
use orcasd::{
    EventSubscription, OrcasDaemonLaunch, OrcasDaemonProcessManager, OrcasIpcClient,
    OrcasRuntimeOverrides,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamOutcomeState {
    Completed,
    Interrupted,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct StreamOutcome {
    pub state: StreamOutcomeState,
    pub final_text: String,
    pub turn_id: String,
    pub turn_status: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            base_delay: Duration::from_millis(150),
            max_delay: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Envelope(ipc::DaemonEventEnvelope),
    Lagged(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPhase {
    Recovered,
    Interrupted,
}

#[derive(Debug, Clone)]
pub struct StreamRecovery {
    pub phase: StreamPhase,
    pub status: Option<String>,
    pub recovered_output: Option<String>,
    pub message: String,
}

pub trait StreamReporter {
    fn status(&mut self, message: &str);
    fn delta(&mut self, delta: &str);
}

pub struct ConsoleReporter;

impl StreamReporter for ConsoleReporter {
    fn status(&mut self, message: &str) {
        println!("{message}");
    }

    fn delta(&mut self, delta: &str) {
        print!("{delta}");
        io::stdout().flush().ok();
    }
}

#[async_trait]
pub trait SupervisorStreamingBackend: Send + Sync {
    type Client: Clone + Send + Sync + 'static;
    type Stream: Send;

    async fn connect(&self, reconnect: bool) -> Result<Self::Client>;
    async fn subscribe_events(&self, client: Self::Client) -> Result<Self::Stream>;
    async fn recv_event(&self, stream: &mut Self::Stream) -> Result<StreamEvent>;
    async fn start_turn(
        &self,
        client: Self::Client,
        request: &TurnStartRequest,
    ) -> Result<ipc::TurnStartResponse>;
    async fn state_get(&self, client: Self::Client) -> Result<ipc::StateSnapshot>;
    async fn turn_attach(
        &self,
        client: Self::Client,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<ipc::TurnAttachResponse>;
}

#[derive(Debug, Clone)]
pub struct OrcasSupervisorStreamingBackend {
    paths: AppPaths,
    daemon: OrcasDaemonProcessManager,
    initial_launch: OrcasDaemonLaunch,
}

impl OrcasSupervisorStreamingBackend {
    pub fn new(paths: AppPaths, config: &AppConfig, overrides: &OrcasRuntimeOverrides) -> Self {
        let initial_launch = if overrides.force_spawn {
            OrcasDaemonLaunch::Always
        } else {
            match config.codex.connection_mode {
                orcas_core::CodexConnectionMode::ConnectOnly => OrcasDaemonLaunch::Never,
                orcas_core::CodexConnectionMode::SpawnIfNeeded => OrcasDaemonLaunch::IfNeeded,
                orcas_core::CodexConnectionMode::SpawnAlways => OrcasDaemonLaunch::Always,
            }
        };
        Self {
            daemon: OrcasDaemonProcessManager::new(paths.clone(), overrides.clone()),
            paths,
            initial_launch,
        }
    }
}

#[async_trait]
impl SupervisorStreamingBackend for OrcasSupervisorStreamingBackend {
    type Client = Arc<OrcasIpcClient>;
    type Stream = EventSubscription;

    async fn connect(&self, reconnect: bool) -> Result<Self::Client> {
        let launch = if reconnect {
            OrcasDaemonLaunch::Never
        } else {
            self.initial_launch
        };
        self.daemon.ensure_running(launch).await?;
        let client = OrcasIpcClient::connect(&self.paths).await?;
        client.daemon_connect().await?;
        Ok(client)
    }

    async fn subscribe_events(&self, client: Self::Client) -> Result<Self::Stream> {
        Ok(client.subscribe_events(false).await?.0)
    }

    async fn recv_event(&self, stream: &mut Self::Stream) -> Result<StreamEvent> {
        match stream.recv().await {
            Ok(event) => Ok(StreamEvent::Envelope(event)),
            Err(broadcast::error::RecvError::Lagged(skipped)) => Ok(StreamEvent::Lagged(
                usize::try_from(skipped).unwrap_or(usize::MAX),
            )),
            Err(broadcast::error::RecvError::Closed) => {
                Err(anyhow!("event stream closed before turn completed"))
            }
        }
    }

    async fn start_turn(
        &self,
        client: Self::Client,
        request: &TurnStartRequest,
    ) -> Result<ipc::TurnStartResponse> {
        Ok(client.turn_start(request).await?)
    }

    async fn state_get(&self, client: Self::Client) -> Result<ipc::StateSnapshot> {
        Ok(client.state_get().await?.snapshot)
    }

    async fn turn_attach(
        &self,
        client: Self::Client,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<ipc::TurnAttachResponse> {
        Ok(client
            .turn_attach(&ipc::TurnAttachRequest {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
            })
            .await?)
    }
}

pub struct StreamingCommandRunner<B> {
    backend: B,
    retry_policy: RetryPolicy,
}

impl<B> StreamingCommandRunner<B> {
    pub fn new(backend: B, retry_policy: RetryPolicy) -> Self {
        Self {
            backend,
            retry_policy,
        }
    }
}

impl<B> StreamingCommandRunner<B>
where
    B: SupervisorStreamingBackend,
{
    pub async fn run_turn(
        &self,
        thread_id: &str,
        text: &str,
        reporter: &mut dyn StreamReporter,
    ) -> Result<StreamOutcome> {
        let (client, mut stream) = self
            .establish_session(false, reporter, "before starting the turn")
            .await?;
        let turn = match self
            .backend
            .start_turn(
                client.clone(),
                &TurnStartRequest {
                    thread_id: thread_id.to_string(),
                    text: text.to_string(),
                    cwd: None,
                    model: None,
                },
            )
            .await
        {
            Ok(turn) => turn,
            Err(error) => {
                reporter.status(
                    "[daemon connection was lost while starting the turn; submission could not be confirmed]",
                );
                return Err(error);
            }
        };

        self.watch_turn(thread_id, &turn.turn_id, &mut stream, reporter)
            .await
    }

    async fn establish_session(
        &self,
        reconnect: bool,
        reporter: &mut dyn StreamReporter,
        context: &str,
    ) -> Result<(B::Client, B::Stream)> {
        let mut delay = self.retry_policy.base_delay;

        for attempt in 1..=self.retry_policy.max_attempts {
            match self.backend.connect(reconnect).await {
                Ok(client) => match self.backend.subscribe_events(client.clone()).await {
                    Ok(stream) => return Ok((client, stream)),
                    Err(error) => {
                        if attempt == self.retry_policy.max_attempts {
                            return Err(error);
                        }
                        reporter.status(&format!(
                            "[daemon connection was lost {context}; retrying session setup ({attempt}/{})]",
                            self.retry_policy.max_attempts
                        ));
                    }
                },
                Err(error) => {
                    if attempt == self.retry_policy.max_attempts {
                        return Err(error);
                    }
                    reporter.status(&format!(
                        "[daemon is unavailable {context}; retrying session setup ({attempt}/{})]",
                        self.retry_policy.max_attempts
                    ));
                }
            }

            sleep(delay).await;
            delay = (delay * 2).min(self.retry_policy.max_delay);
        }

        Err(anyhow!("daemon session setup failed"))
    }

    async fn watch_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
        stream: &mut B::Stream,
        reporter: &mut dyn StreamReporter,
    ) -> Result<StreamOutcome> {
        let mut buffer = String::new();

        loop {
            match self.backend.recv_event(stream).await {
                Ok(StreamEvent::Envelope(envelope)) => match envelope.event {
                    ipc::DaemonEvent::OutputDelta {
                        thread_id: event_thread_id,
                        turn_id: event_turn_id,
                        delta,
                        ..
                    } if event_thread_id == thread_id && event_turn_id == turn_id => {
                        reporter.delta(&delta);
                        buffer.push_str(&delta);
                    }
                    ipc::DaemonEvent::TurnUpdated {
                        thread_id: event_thread_id,
                        turn,
                    } if event_thread_id == thread_id && turn.id == turn_id => {
                        if is_terminal_status(&turn.status) {
                            reporter.status(&format!("[turn completed: {}]", turn.status));
                            return Ok(StreamOutcome {
                                state: StreamOutcomeState::Completed,
                                final_text: buffer,
                                turn_id: turn_id.to_string(),
                                turn_status: Some(turn.status),
                            });
                        }
                    }
                    ipc::DaemonEvent::Warning { message } => {
                        reporter.status(&format!("[warning] {message}"));
                    }
                    _ => {}
                },
                Ok(StreamEvent::Lagged(skipped)) => {
                    reporter.status(&format!(
                        "[warning] event stream lagged; skipped {skipped} events"
                    ));
                }
                Err(_) => match self
                    .recover_turn(thread_id, turn_id, &buffer, reporter)
                    .await?
                {
                    RecoveryDecision::Resume(recovered_stream) => {
                        *stream = recovered_stream;
                    }
                    RecoveryDecision::Complete {
                        status,
                        recovered_output,
                        message,
                    } => {
                        if let Some(extra) = recovered_output {
                            reporter.delta(&extra);
                            buffer.push_str(&extra);
                        }
                        reporter.status(&message);
                        return Ok(StreamOutcome {
                            state: StreamOutcomeState::Completed,
                            final_text: buffer,
                            turn_id: turn_id.to_string(),
                            turn_status: Some(status),
                        });
                    }
                    RecoveryDecision::Interrupt {
                        recovered_output,
                        message,
                    } => {
                        if let Some(extra) = recovered_output {
                            reporter.status(&format!("[cached recent output] {extra}"));
                        }
                        reporter.status(&message);
                        return Ok(StreamOutcome {
                            state: StreamOutcomeState::Interrupted,
                            final_text: buffer,
                            turn_id: turn_id.to_string(),
                            turn_status: None,
                        });
                    }
                },
            }
        }
    }

    async fn recover_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
        printed_output: &str,
        reporter: &mut dyn StreamReporter,
    ) -> Result<RecoveryDecision<B::Stream>> {
        reporter.status("[daemon connection lost; attempting reconnect]");
        let mut delay = self.retry_policy.base_delay;

        for attempt in 1..=self.retry_policy.max_attempts {
            match self.backend.connect(true).await {
                Ok(client) => {
                    reporter.status("[reconnected to daemon; reloading snapshot]");
                    let snapshot = self.backend.state_get(client.clone()).await?;
                    let attachment = self
                        .backend
                        .turn_attach(client.clone(), thread_id, turn_id)
                        .await
                        .ok();
                    let recovery = analyze_recovery(&snapshot, attachment.as_ref(), turn_id);

                    match recovery.phase {
                        StreamPhase::Recovered => {
                            if let Some(output) = recovery.recovered_output.as_deref() {
                                let suffix = output_suffix(output, printed_output);
                                if let Some(status) = recovery.status.clone() {
                                    reporter.status(&recovery.message);
                                    return Ok(RecoveryDecision::Complete {
                                        status,
                                        recovered_output: suffix,
                                        message: format!(
                                            "[recovered snapshot state after daemon replacement]"
                                        ),
                                    });
                                }
                            }
                            let stream = self.backend.subscribe_events(client).await?;
                            reporter.status(&recovery.message);
                            return Ok(RecoveryDecision::Resume(stream));
                        }
                        StreamPhase::Interrupted => {
                            return Ok(RecoveryDecision::Interrupt {
                                recovered_output: recovery.recovered_output,
                                message: recovery.message,
                            });
                        }
                    }
                }
                Err(error) => {
                    reporter.status(&format!("[reconnect attempt {attempt} failed: {error}]"));
                }
            }

            if attempt < self.retry_policy.max_attempts {
                sleep(delay).await;
                delay = (delay * 2).min(self.retry_policy.max_delay);
            }
        }

        Ok(RecoveryDecision::Interrupt {
            recovered_output: None,
            message: "[stream interrupted; daemon did not become reachable again]".to_string(),
        })
    }
}

enum RecoveryDecision<S> {
    Resume(S),
    Complete {
        status: String,
        recovered_output: Option<String>,
        message: String,
    },
    Interrupt {
        recovered_output: Option<String>,
        message: String,
    },
}

fn analyze_recovery(
    snapshot: &ipc::StateSnapshot,
    attachment: Option<&ipc::TurnAttachResponse>,
    turn_id: &str,
) -> StreamRecovery {
    let recovered_output = attachment
        .and_then(|attachment| {
            attachment
                .turn
                .as_ref()
                .and_then(|turn| turn.recent_output.clone())
        })
        .or_else(|| {
            snapshot
                .threads
                .iter()
                .find(|thread| {
                    attachment
                        .and_then(|attach| attach.turn.as_ref())
                        .map(|turn| thread.id == turn.thread_id)
                        .unwrap_or(false)
                })
                .and_then(|thread| thread.recent_output.clone())
        });

    if let Some(attachment) = attachment
        && let Some(turn) = attachment.turn.as_ref()
    {
        if attachment.attached
            && turn.attachable
            && matches!(turn.lifecycle, ipc::TurnLifecycleState::Active)
        {
            return StreamRecovery {
                phase: StreamPhase::Recovered,
                status: None,
                recovered_output,
                message: format!("[recovered live turn attachment for {turn_id}; resubscribing]"),
            };
        }

        if matches!(
            turn.lifecycle,
            ipc::TurnLifecycleState::Completed
                | ipc::TurnLifecycleState::Failed
                | ipc::TurnLifecycleState::Interrupted
        ) {
            return StreamRecovery {
                phase: StreamPhase::Recovered,
                status: Some(turn.status.clone()),
                recovered_output,
                message: format!(
                    "[recovered turn state after daemon replacement; turn is now {}]",
                    turn.status
                ),
            };
        }

        return StreamRecovery {
            phase: StreamPhase::Interrupted,
            status: None,
            recovered_output,
            message: attachment.reason.as_ref().map_or_else(
                || {
                    "[stream interrupted during daemon replacement; upstream continuation could not be confirmed]"
                        .to_string()
                },
                |reason| format!("[stream interrupted after daemon replacement; {reason}]"),
            ),
        };
    }

    StreamRecovery {
        phase: StreamPhase::Interrupted,
        status: None,
        recovered_output,
        message:
            "[stream interrupted during daemon replacement; upstream continuation could not be confirmed]"
                .to_string(),
    }
}

fn output_suffix(full_output: &str, printed_output: &str) -> Option<String> {
    if printed_output.is_empty() {
        return Some(full_output.to_string());
    }
    full_output
        .strip_prefix(printed_output)
        .map(ToOwned::to_owned)
        .filter(|suffix| !suffix.is_empty())
}

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "interrupted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug)]
    struct FakeClient;

    #[derive(Debug)]
    struct FakeStream {
        events: VecDeque<Result<StreamEvent>>,
    }

    #[derive(Clone, Default)]
    struct FakeReporter {
        lines: Arc<Mutex<Vec<String>>>,
        deltas: Arc<Mutex<Vec<String>>>,
    }

    impl FakeReporter {
        async fn lines(&self) -> Vec<String> {
            self.lines.lock().unwrap().clone()
        }
    }

    impl StreamReporter for FakeReporter {
        fn status(&mut self, message: &str) {
            self.lines.lock().unwrap().push(message.to_string());
        }

        fn delta(&mut self, delta: &str) {
            self.deltas.lock().unwrap().push(delta.to_string());
        }
    }

    #[derive(Default)]
    struct FakeBackendState {
        connect_results: VecDeque<Result<FakeClient>>,
        streams: VecDeque<VecDeque<Result<StreamEvent>>>,
        snapshots: VecDeque<ipc::StateSnapshot>,
        attachments: VecDeque<Result<ipc::TurnAttachResponse>>,
        start_turn_response: Option<ipc::TurnStartResponse>,
        reconnect_flags: Vec<bool>,
    }

    #[derive(Clone, Default)]
    struct FakeBackend {
        state: Arc<tokio::sync::Mutex<FakeBackendState>>,
    }

    #[async_trait]
    impl SupervisorStreamingBackend for FakeBackend {
        type Client = FakeClient;
        type Stream = FakeStream;

        async fn connect(&self, reconnect: bool) -> Result<Self::Client> {
            let mut state = self.state.lock().await;
            state.reconnect_flags.push(reconnect);
            state
                .connect_results
                .pop_front()
                .unwrap_or_else(|| Err(anyhow!("unexpected connect")))
        }

        async fn subscribe_events(&self, _client: Self::Client) -> Result<Self::Stream> {
            let mut state = self.state.lock().await;
            Ok(FakeStream {
                events: state.streams.pop_front().unwrap_or_else(VecDeque::new),
            })
        }

        async fn recv_event(&self, stream: &mut Self::Stream) -> Result<StreamEvent> {
            stream
                .events
                .pop_front()
                .unwrap_or_else(|| Err(anyhow!("unexpected end of stream")))
        }

        async fn start_turn(
            &self,
            _client: Self::Client,
            _request: &TurnStartRequest,
        ) -> Result<ipc::TurnStartResponse> {
            self.state
                .lock()
                .await
                .start_turn_response
                .clone()
                .ok_or_else(|| anyhow!("missing start turn response"))
        }

        async fn state_get(&self, _client: Self::Client) -> Result<ipc::StateSnapshot> {
            self.state
                .lock()
                .await
                .snapshots
                .pop_front()
                .ok_or_else(|| anyhow!("missing snapshot"))
        }

        async fn turn_attach(
            &self,
            _client: Self::Client,
            _thread_id: &str,
            _turn_id: &str,
        ) -> Result<ipc::TurnAttachResponse> {
            self.state
                .lock()
                .await
                .attachments
                .pop_front()
                .unwrap_or_else(|| Err(anyhow!("missing turn attachment")))
        }
    }

    fn sample_snapshot(active: bool, recent_output: Option<&str>) -> ipc::StateSnapshot {
        ipc::StateSnapshot {
            daemon: ipc::DaemonStatusResponse {
                socket_path: "/tmp/orcasd.sock".to_string(),
                metadata_path: "/tmp/orcasd.json".to_string(),
                codex_endpoint: "ws://127.0.0.1:4500".to_string(),
                codex_binary_path: "/tmp/codex".to_string(),
                upstream: orcas_core::ConnectionState {
                    endpoint: "ws://127.0.0.1:4500".to_string(),
                    status: "connected".to_string(),
                    detail: None,
                },
                client_count: 1,
                known_threads: 1,
                runtime: ipc::DaemonRuntimeMetadata {
                    pid: 1,
                    started_at: Utc::now(),
                    version: "0.1.0".to_string(),
                    build_fingerprint: "abc".to_string(),
                    binary_path: "/tmp/orcasd".to_string(),
                    socket_path: "/tmp/orcasd.sock".to_string(),
                    metadata_path: "/tmp/orcasd.json".to_string(),
                    git_commit: None,
                },
            },
            session: ipc::SessionState {
                active_thread_id: Some("thread-1".to_string()),
                active_turns: if active {
                    vec![ipc::ActiveTurn {
                        thread_id: "thread-1".to_string(),
                        turn_id: "turn-1".to_string(),
                        status: "in_progress".to_string(),
                        updated_at: Utc::now(),
                    }]
                } else {
                    Vec::new()
                },
            },
            operator_inbox: ipc::OperatorInboxState::default(),
            threads: vec![ipc::ThreadSummary {
                id: "thread-1".to_string(),
                preview: "preview".to_string(),
                name: None,
                model_provider: "openai".to_string(),
                cwd: "/tmp".to_string(),
                status: "idle".to_string(),
                created_at: 1,
                updated_at: 2,
                scope: "orcas_managed".to_string(),
                archived: false,
                loaded_status: ipc::ThreadLoadedStatus::Idle,
                active_flags: Vec::new(),
                active_turn_id: None,
                last_seen_turn_id: None,
                recent_output: recent_output.map(ToOwned::to_owned),
                recent_event: None,
                turn_in_flight: active,
                monitor_state: if active {
                    ipc::ThreadMonitorState::Attached
                } else {
                    ipc::ThreadMonitorState::Detached
                },
                last_sync_at: Utc::now(),
                source_kind: None,
                raw_summary: None,
            }],
            active_thread: None,
            collaboration: ipc::CollaborationSnapshot::default(),
            recent_events: Vec::new(),
        }
    }

    fn sample_turn_state(
        lifecycle: ipc::TurnLifecycleState,
        status: &str,
        attachable: bool,
        text: &str,
    ) -> ipc::TurnStateView {
        ipc::TurnStateView {
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
            lifecycle,
            status: status.to_string(),
            attachable,
            live_stream: attachable,
            terminal: !matches!(lifecycle, ipc::TurnLifecycleState::Active),
            recent_output: (!text.is_empty()).then(|| text.to_string()),
            recent_event: Some(format!("turn {status}")),
            updated_at: Utc::now(),
            error_message: None,
        }
    }

    #[tokio::test]
    async fn reconnect_resumes_stream_when_turn_is_still_live() {
        let backend = FakeBackend::default();
        {
            let mut state = backend.state.lock().await;
            state.connect_results = VecDeque::from(vec![Ok(FakeClient), Ok(FakeClient)]);
            state.streams = VecDeque::from(vec![
                VecDeque::from(vec![
                    Ok(StreamEvent::Envelope(ipc::DaemonEventEnvelope::new(
                        ipc::DaemonEvent::OutputDelta {
                            thread_id: "thread-1".to_string(),
                            turn_id: "turn-1".to_string(),
                            item_id: "item-1".to_string(),
                            delta: "hello ".to_string(),
                        },
                    ))),
                    Err(anyhow!("socket closed")),
                ]),
                VecDeque::from(vec![
                    Ok(StreamEvent::Envelope(ipc::DaemonEventEnvelope::new(
                        ipc::DaemonEvent::OutputDelta {
                            thread_id: "thread-1".to_string(),
                            turn_id: "turn-1".to_string(),
                            item_id: "item-1".to_string(),
                            delta: "world".to_string(),
                        },
                    ))),
                    Ok(StreamEvent::Envelope(ipc::DaemonEventEnvelope::new(
                        ipc::DaemonEvent::TurnUpdated {
                            thread_id: "thread-1".to_string(),
                            turn: ipc::TurnView {
                                id: "turn-1".to_string(),
                                status: "completed".to_string(),
                                error_message: None,
                                error_summary: None,
                                started_at: None,
                                completed_at: None,
                                latest_diff: None,
                                latest_plan_snapshot: None,
                                token_usage_snapshot: None,
                                latest_plan: None,
                                token_usage: None,
                                items: Vec::new(),
                            },
                        },
                    ))),
                ]),
            ]);
            state.snapshots = VecDeque::from(vec![sample_snapshot(true, Some("hello "))]);
            state.attachments = VecDeque::from(vec![Ok(ipc::TurnAttachResponse {
                turn: Some(sample_turn_state(
                    ipc::TurnLifecycleState::Active,
                    "in_progress",
                    true,
                    "hello ",
                )),
                attached: true,
                reason: None,
            })]);
            state.start_turn_response = Some(ipc::TurnStartResponse {
                turn_id: "turn-1".to_string(),
                thread_id: "thread-1".to_string(),
            });
        }

        let runner = StreamingCommandRunner::new(
            backend.clone(),
            RetryPolicy {
                max_attempts: 2,
                base_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
            },
        );
        let mut reporter = FakeReporter::default();
        let outcome = runner
            .run_turn("thread-1", "say hello", &mut reporter)
            .await
            .unwrap();

        assert_eq!(outcome.state, StreamOutcomeState::Completed);
        assert_eq!(outcome.final_text, "hello world");
        assert_eq!(
            backend.state.lock().await.reconnect_flags,
            vec![false, true]
        );
        assert!(
            reporter
                .lines()
                .await
                .iter()
                .any(|line| line.contains("resubscribing"))
        );
    }

    #[tokio::test]
    async fn initial_session_setup_retries_before_turn_submission() {
        let backend = FakeBackend::default();
        {
            let mut state = backend.state.lock().await;
            state.connect_results =
                VecDeque::from(vec![Err(anyhow!("daemon restarting")), Ok(FakeClient)]);
            state.streams = VecDeque::from(vec![VecDeque::from(vec![Ok(StreamEvent::Envelope(
                ipc::DaemonEventEnvelope::new(ipc::DaemonEvent::TurnUpdated {
                    thread_id: "thread-1".to_string(),
                    turn: ipc::TurnView {
                        id: "turn-1".to_string(),
                        status: "completed".to_string(),
                        error_message: None,
                        error_summary: None,
                        started_at: None,
                        completed_at: None,
                        latest_diff: None,
                        latest_plan_snapshot: None,
                        token_usage_snapshot: None,
                        latest_plan: None,
                        token_usage: None,
                        items: Vec::new(),
                    },
                }),
            ))])]);
            state.start_turn_response = Some(ipc::TurnStartResponse {
                turn_id: "turn-1".to_string(),
                thread_id: "thread-1".to_string(),
            });
        }

        let runner = StreamingCommandRunner::new(
            backend.clone(),
            RetryPolicy {
                max_attempts: 2,
                base_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
            },
        );
        let mut reporter = FakeReporter::default();
        let outcome = runner
            .run_turn("thread-1", "say hello", &mut reporter)
            .await
            .unwrap();

        assert_eq!(outcome.state, StreamOutcomeState::Completed);
        assert_eq!(
            backend.state.lock().await.reconnect_flags,
            vec![false, false]
        );
        assert!(
            reporter
                .lines()
                .await
                .iter()
                .any(|line| line.contains("retrying session setup"))
        );
    }

    #[tokio::test]
    async fn reconnect_reports_interruption_honestly_when_turn_cannot_be_reanchored() {
        let backend = FakeBackend::default();
        {
            let mut state = backend.state.lock().await;
            state.connect_results = VecDeque::from(vec![Ok(FakeClient), Ok(FakeClient)]);
            state.streams =
                VecDeque::from(vec![VecDeque::from(vec![Err(anyhow!("socket closed"))])]);
            state.snapshots = VecDeque::from(vec![sample_snapshot(false, Some("partial output"))]);
            state.attachments = VecDeque::from(vec![Ok(ipc::TurnAttachResponse {
                turn: Some(sample_turn_state(
                    ipc::TurnLifecycleState::Lost,
                    "lost",
                    false,
                    "partial output",
                )),
                attached: false,
                reason: Some(
                    "turn continuity was lost when Orcas lost daemon/upstream ownership"
                        .to_string(),
                ),
            })]);
            state.start_turn_response = Some(ipc::TurnStartResponse {
                turn_id: "turn-1".to_string(),
                thread_id: "thread-1".to_string(),
            });
        }

        let runner = StreamingCommandRunner::new(
            backend,
            RetryPolicy {
                max_attempts: 2,
                base_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
            },
        );
        let mut reporter = FakeReporter::default();
        let outcome = runner
            .run_turn("thread-1", "say hello", &mut reporter)
            .await
            .unwrap();

        assert_eq!(outcome.state, StreamOutcomeState::Interrupted);
        assert!(
            reporter
                .lines()
                .await
                .iter()
                .any(|line| line.contains("interrupted"))
        );
        assert!(
            reporter
                .lines()
                .await
                .iter()
                .any(|line| line.contains("cached recent output"))
        );
    }

    #[tokio::test]
    async fn reconnect_recovers_terminal_turn_state_from_snapshot() {
        let backend = FakeBackend::default();
        {
            let mut state = backend.state.lock().await;
            state.connect_results = VecDeque::from(vec![Ok(FakeClient), Ok(FakeClient)]);
            state.streams = VecDeque::from(vec![VecDeque::from(vec![
                Ok(StreamEvent::Envelope(ipc::DaemonEventEnvelope::new(
                    ipc::DaemonEvent::OutputDelta {
                        thread_id: "thread-1".to_string(),
                        turn_id: "turn-1".to_string(),
                        item_id: "item-1".to_string(),
                        delta: "hello ".to_string(),
                    },
                ))),
                Err(anyhow!("socket closed")),
            ])]);
            state.snapshots = VecDeque::from(vec![sample_snapshot(false, Some("hello world"))]);
            state.attachments = VecDeque::from(vec![Ok(ipc::TurnAttachResponse {
                turn: Some(sample_turn_state(
                    ipc::TurnLifecycleState::Completed,
                    "completed",
                    false,
                    "hello world",
                )),
                attached: false,
                reason: Some(
                    "turn already completed; only terminal state is queryable".to_string(),
                ),
            })]);
            state.start_turn_response = Some(ipc::TurnStartResponse {
                turn_id: "turn-1".to_string(),
                thread_id: "thread-1".to_string(),
            });
        }

        let runner = StreamingCommandRunner::new(
            backend,
            RetryPolicy {
                max_attempts: 2,
                base_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
            },
        );
        let mut reporter = FakeReporter::default();
        let outcome = runner
            .run_turn("thread-1", "say hello", &mut reporter)
            .await
            .unwrap();

        assert_eq!(outcome.state, StreamOutcomeState::Completed);
        assert_eq!(outcome.final_text, "hello world");
        assert_eq!(outcome.turn_status.as_deref(), Some("completed"));
        assert!(
            reporter
                .lines()
                .await
                .iter()
                .any(|line| line.contains("recovered turn state after daemon replacement"))
        );
    }
}
