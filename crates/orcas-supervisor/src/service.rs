use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Error, Result};
use tokio::time::sleep;

use orcas_core::{AppConfig, AppPaths, ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest};
use orcas_daemon::{
    OrcasDaemonLaunch, OrcasDaemonProcessManager, OrcasIpcClient, OrcasRuntimeOverrides,
    apply_runtime_overrides,
};

use crate::streaming::{
    ConsoleReporter, OrcasSupervisorStreamingBackend, RetryPolicy, StreamReporter,
    StreamingCommandRunner,
};

pub use orcas_daemon::OrcasRuntimeOverrides as RuntimeOverrides;

pub struct SupervisorService {
    pub paths: AppPaths,
    pub config: AppConfig,
    daemon: OrcasDaemonProcessManager,
    overrides: OrcasRuntimeOverrides,
}

impl SupervisorService {
    pub async fn load(overrides: &RuntimeOverrides) -> Result<Self> {
        let paths = AppPaths::discover()?;
        paths.ensure().await?;
        let mut config = AppConfig::write_default_if_missing(&paths).await?;
        apply_runtime_overrides(&mut config, overrides);
        let daemon = OrcasDaemonProcessManager::new(paths.clone(), overrides.clone());

        Ok(Self {
            paths,
            config,
            daemon,
            overrides: overrides.clone(),
        })
    }

    pub async fn doctor(&self) -> Result<()> {
        let daemon_status = self.daemon.status().await?;
        println!("config: {}", self.paths.config_file.display());
        println!("state: {}", self.paths.state_file.display());
        println!("socket: {}", daemon_status.socket_path.display());
        println!("metadata: {}", daemon_status.metadata_path.display());
        println!("daemon_running: {}", daemon_status.running);
        println!("daemon_log: {}", daemon_status.log_path.display());
        println!("codex_bin: {}", self.config.codex.binary_path.display());
        println!("codex_endpoint: {}", self.config.codex.listen_url);
        println!("connection_mode: {:?}", self.config.codex.connection_mode);
        Ok(())
    }

    pub async fn daemon_status(&self) -> Result<()> {
        let socket_status = self.daemon.status().await?;
        println!("socket: {}", socket_status.socket_path.display());
        println!("metadata: {}", socket_status.metadata_path.display());
        println!("running: {}", socket_status.running);
        println!("socket_exists: {}", socket_status.socket_exists);
        println!("socket_responsive: {}", socket_status.socket_responsive);
        println!("pid_running: {}", socket_status.pid_running);
        if let Some(pid) = socket_status.socket_owner_pid {
            println!("socket_owner_pid: {pid}");
        }
        println!("stale_socket: {}", socket_status.stale_socket);
        println!("stale_metadata: {}", socket_status.stale_metadata);
        println!("log_file: {}", socket_status.log_path.display());
        if let Some(expected) = socket_status.expected_binary.as_ref() {
            println!("expected_binary: {}", expected.binary_path);
            println!("expected_version: {}", expected.version);
            println!("expected_fingerprint: {}", expected.build_fingerprint);
        }
        if let Some(matches) = socket_status.binary_matches_expected {
            println!("binary_matches_expected: {matches}");
        }
        if let Some(runtime) = socket_status.runtime_metadata.as_ref() {
            println!("daemon_pid: {}", runtime.pid);
            println!("daemon_started_at: {}", runtime.started_at);
            println!("daemon_version: {}", runtime.version);
            println!("daemon_fingerprint: {}", runtime.build_fingerprint);
            println!("daemon_binary: {}", runtime.binary_path);
            if let Some(git_commit) = runtime.git_commit.as_ref() {
                println!("daemon_git_commit: {git_commit}");
            }
        } else if socket_status.running {
            println!("daemon_runtime: legacy daemon without runtime metadata");
        }
        if let Some(status) = socket_status.daemon_status.as_ref() {
            println!("codex_endpoint: {}", status.codex_endpoint);
            println!("codex_binary: {}", status.codex_binary_path);
            println!("upstream_status: {}", status.upstream.status);
            if let Some(detail) = status.upstream.detail.as_ref() {
                println!("upstream_detail: {detail}");
            }
            println!("client_count: {}", status.client_count);
            println!("known_threads: {}", status.known_threads);
        }
        Ok(())
    }

    pub async fn daemon_start(&self, force: bool) -> Result<()> {
        let launch = if force || self.overrides.force_spawn {
            OrcasDaemonLaunch::Always
        } else {
            OrcasDaemonLaunch::IfNeeded
        };
        let socket_status = self.daemon.ensure_running(launch).await?;
        let client = self.connect_client(OrcasDaemonLaunch::Never).await?;
        let status = client.daemon_connect().await?.status;
        println!("socket: {}", socket_status.socket_path.display());
        println!("metadata: {}", socket_status.metadata_path.display());
        println!("running: {}", socket_status.running);
        println!("log_file: {}", socket_status.log_path.display());
        println!("upstream_status: {}", status.upstream.status);
        println!("codex_endpoint: {}", status.codex_endpoint);
        println!("daemon_pid: {}", status.runtime.pid);
        println!("daemon_version: {}", status.runtime.version);
        println!("daemon_fingerprint: {}", status.runtime.build_fingerprint);
        println!("daemon_binary: {}", status.runtime.binary_path);
        Ok(())
    }

    pub async fn daemon_restart(&self) -> Result<()> {
        let socket_status = self.daemon.restart().await?;
        let client = self.connect_client(OrcasDaemonLaunch::Never).await?;
        let status = client.daemon_connect().await?.status;
        println!("socket: {}", socket_status.socket_path.display());
        println!("metadata: {}", socket_status.metadata_path.display());
        println!("running: {}", socket_status.running);
        println!("log_file: {}", socket_status.log_path.display());
        println!("upstream_status: {}", status.upstream.status);
        println!("codex_endpoint: {}", status.codex_endpoint);
        println!("daemon_pid: {}", status.runtime.pid);
        println!("daemon_version: {}", status.runtime.version);
        println!("daemon_fingerprint: {}", status.runtime.build_fingerprint);
        println!("daemon_binary: {}", status.runtime.binary_path);
        Ok(())
    }

    pub async fn daemon_stop(&self) -> Result<()> {
        let before = self.daemon.status().await?;
        let after = self.daemon.stop().await?;
        println!("socket: {}", before.socket_path.display());
        println!("metadata: {}", before.metadata_path.display());
        println!("running: {}", after.running);
        println!("socket_exists: {}", after.socket_exists);
        println!("stale_socket: {}", after.stale_socket);
        println!("stale_metadata: {}", after.stale_metadata);
        if before.running {
            println!("stopped: true");
        } else if before.stale_socket || before.stale_metadata {
            println!("cleaned_stale_runtime: true");
        } else {
            println!("daemon_already_stopped: true");
        }
        Ok(())
    }

    pub async fn models_list(&self) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client.models_list().await?;
        for model in response.data {
            println!(
                "{}\t{}\thidden={}\tdefault={}",
                model.id, model.display_name, model.hidden, model.is_default
            );
        }
        Ok(())
    }

    pub async fn threads_list(&self) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client.threads_list_scoped().await?;
        for thread in response.data {
            println!(
                "{}\t{}\t{}\t{}\tin_flight={}\t{}\t{}",
                thread.id,
                thread.status,
                thread.model_provider,
                thread.scope,
                thread.turn_in_flight,
                thread
                    .recent_output
                    .clone()
                    .unwrap_or_else(|| thread.preview.replace('\n', " ")),
                thread.recent_event.unwrap_or_default()
            );
        }
        Ok(())
    }

    pub async fn thread_read(&self, thread_id: &str) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .thread_read(&ThreadReadRequest {
                thread_id: thread_id.to_string(),
                include_turns: true,
            })
            .await?;
        println!("thread: {}", response.thread.summary.id);
        println!("status: {}", response.thread.summary.status);
        println!("scope: {}", response.thread.summary.scope);
        println!("cwd: {}", response.thread.summary.cwd);
        println!("preview: {}", response.thread.summary.preview);
        if let Some(snippet) = response.thread.summary.recent_output.as_ref() {
            println!("recent_output: {snippet}");
        }
        if let Some(event) = response.thread.summary.recent_event.as_ref() {
            println!("recent_event: {event}");
        }
        println!("turn_in_flight: {}", response.thread.summary.turn_in_flight);
        println!("turns: {}", response.thread.turns.len());
        Ok(())
    }

    pub async fn turns_list_active(&self) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client.turns_list_active().await?;
        if response.turns.is_empty() {
            println!("no active attachable turns");
            return Ok(());
        }

        for turn in response.turns {
            println!(
                "{}\t{}\t{}\tattachable={}\tlive_stream={}\t{}\t{}",
                turn.thread_id,
                turn.turn_id,
                format!("{:?}", turn.lifecycle).to_ascii_lowercase(),
                turn.attachable,
                turn.live_stream,
                turn.recent_output.unwrap_or_default(),
                turn.recent_event.unwrap_or_default()
            );
        }
        Ok(())
    }

    pub async fn turn_get(&self, thread_id: &str, turn_id: &str) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .turn_attach(&orcas_core::ipc::TurnAttachRequest {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
            })
            .await?;

        println!("thread_id: {thread_id}");
        println!("turn_id: {turn_id}");
        println!("attached: {}", response.attached);
        if let Some(reason) = response.reason.as_ref() {
            println!("attach_reason: {reason}");
        }

        if let Some(turn) = response.turn {
            println!(
                "lifecycle: {}",
                format!("{:?}", turn.lifecycle).to_ascii_lowercase()
            );
            println!("status: {}", turn.status);
            println!("attachable: {}", turn.attachable);
            println!("live_stream: {}", turn.live_stream);
            println!("terminal: {}", turn.terminal);
            println!("updated_at: {}", turn.updated_at);
            if let Some(output) = turn.recent_output.as_ref() {
                println!("recent_output: {output}");
            }
            if let Some(event) = turn.recent_event.as_ref() {
                println!("recent_event: {event}");
            }
            if let Some(error) = turn.error_message.as_ref() {
                println!("error_message: {error}");
            }
        } else {
            println!("turn: not found");
        }

        Ok(())
    }

    pub async fn thread_start(
        &self,
        cwd: Option<PathBuf>,
        model: Option<String>,
        ephemeral: bool,
    ) -> Result<String> {
        let client = self.ready_client().await?;
        let response = client
            .thread_start(&ThreadStartRequest {
                cwd: cwd
                    .or_else(|| self.config.defaults.cwd.clone())
                    .map(|path| path.display().to_string()),
                model: model.or_else(|| self.config.defaults.model.clone()),
                ephemeral,
            })
            .await?;
        println!("thread_id: {}", response.thread.id);
        Ok(response.thread.id)
    }

    pub async fn thread_resume(
        &self,
        thread_id: &str,
        cwd: Option<PathBuf>,
        model: Option<String>,
    ) -> Result<String> {
        let client = self.ready_client().await?;
        let response = client
            .thread_resume(&ThreadResumeRequest {
                thread_id: thread_id.to_string(),
                cwd: cwd
                    .or_else(|| self.config.defaults.cwd.clone())
                    .map(|path| path.display().to_string()),
                model: model.or_else(|| self.config.defaults.model.clone()),
            })
            .await?;
        println!("thread_id: {}", response.thread.id);
        Ok(response.thread.id)
    }

    pub async fn prompt(&self, thread_id: &str, text: &str) -> Result<String> {
        let mut reporter = ConsoleReporter;
        self.resume_thread_for_streaming(thread_id, &mut reporter)
            .await?;
        self.run_streaming_turn(thread_id, text, &mut reporter)
            .await
    }

    pub async fn quickstart(
        &self,
        cwd: Option<PathBuf>,
        model: Option<String>,
        text: &str,
    ) -> Result<()> {
        let mut reporter = ConsoleReporter;
        let cwd = cwd.or_else(|| self.config.defaults.cwd.clone());
        let model = model.or_else(|| self.config.defaults.model.clone());
        let thread_id = self
            .start_thread_for_streaming(cwd, model, &mut reporter)
            .await?;
        let final_text = self
            .run_streaming_turn(&thread_id, text, &mut reporter)
            .await?;
        println!("\nthread_id: {thread_id}");
        println!("final_text_len: {}", final_text.len());
        Ok(())
    }

    async fn resume_thread_for_streaming(
        &self,
        thread_id: &str,
        reporter: &mut dyn StreamReporter,
    ) -> Result<()> {
        let retry_policy = RetryPolicy::default();
        let request = ThreadResumeRequest {
            thread_id: thread_id.to_string(),
            cwd: self
                .config
                .defaults
                .cwd
                .clone()
                .map(|path| path.display().to_string()),
            model: self.config.defaults.model.clone(),
        };
        let mut delay = retry_policy.base_delay;

        for attempt in 1..=retry_policy.max_attempts {
            let client = self.ready_client().await?;
            match client.thread_resume(&request).await {
                Ok(_) => return Ok(()),
                Err(error) => {
                    if attempt == retry_policy.max_attempts {
                        reporter.status(
                            "[daemon connection was lost while resuming the thread; resume could not be confirmed]",
                        );
                        return Err(error.into());
                    }

                    reporter.status(&format!(
                        "[daemon connection was lost while resuming the thread; retrying ({attempt}/{})]",
                        retry_policy.max_attempts
                    ));
                    sleep(delay).await;
                    delay = (delay * 2).min(retry_policy.max_delay);
                }
            }
        }

        Ok(())
    }

    async fn start_thread_for_streaming(
        &self,
        cwd: Option<PathBuf>,
        model: Option<String>,
        reporter: &mut dyn StreamReporter,
    ) -> Result<String> {
        let client = self.ready_client().await?;
        let thread = match client
            .thread_start(&ThreadStartRequest {
                cwd: cwd.map(|path| path.display().to_string()),
                model,
                ephemeral: false,
            })
            .await
        {
            Ok(thread) => thread,
            Err(error) => {
                reporter.status(
                    "[daemon connection was lost while creating the thread; thread creation could not be confirmed]",
                );
                return Err(error.into());
            }
        };
        Ok(thread.thread.id)
    }

    async fn run_streaming_turn(
        &self,
        thread_id: &str,
        text: &str,
        reporter: &mut dyn StreamReporter,
    ) -> Result<String> {
        let backend =
            OrcasSupervisorStreamingBackend::new(self.paths.clone(), &self.config, &self.overrides);
        let runner = StreamingCommandRunner::new(backend, RetryPolicy::default());
        let outcome = runner.run_turn(thread_id, text, reporter).await?;
        if matches!(
            outcome.state,
            crate::streaming::StreamOutcomeState::Interrupted
        ) {
            println!("[stream state: interrupted]");
        }
        Ok(outcome.final_text)
    }

    async fn ready_client(&self) -> Result<Arc<OrcasIpcClient>> {
        let launch = if self.overrides.force_spawn {
            OrcasDaemonLaunch::Always
        } else {
            OrcasDaemonLaunch::IfNeeded
        };
        let mut last_error: Option<Error> = None;
        let mut delay = Duration::from_millis(100);

        for _ in 0..5 {
            let client = self.connect_client(launch).await?;
            match client.daemon_connect().await {
                Ok(_) => return Ok(client),
                Err(error) => {
                    last_error = Some(Error::new(error).context("connect Orcas daemon to Codex"));
                    sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_millis(800));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| Error::msg("connect Orcas daemon to Codex")))
    }

    async fn connect_client(&self, launch: OrcasDaemonLaunch) -> Result<Arc<OrcasIpcClient>> {
        let mut last_error: Option<Error> = None;
        let mut delay = Duration::from_millis(100);

        for _ in 0..5 {
            match self.daemon.ensure_running(launch).await {
                Ok(_) => match OrcasIpcClient::connect(&self.paths).await {
                    Ok(client) => return Ok(client),
                    Err(error) => {
                        last_error = Some(Error::new(error).context("connect to Orcas daemon"));
                    }
                },
                Err(error) => {
                    last_error = Some(Error::new(error));
                }
            }
            sleep(delay).await;
            delay = (delay * 2).min(Duration::from_millis(800));
        }

        Err(last_error.unwrap_or_else(|| Error::msg("connect to Orcas daemon")))
    }
}
