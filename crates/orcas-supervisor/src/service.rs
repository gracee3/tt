use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};

use orcas_core::{
    AppConfig, AppPaths, ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest,
    TurnStartRequest, ipc,
};
use orcas_daemon::{
    EventSubscription, OrcasDaemonLaunch, OrcasDaemonProcessManager, OrcasIpcClient,
    OrcasRuntimeOverrides, apply_runtime_overrides,
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
        let client = self.ready_client().await?;
        client
            .thread_resume(&ThreadResumeRequest {
                thread_id: thread_id.to_string(),
                cwd: self
                    .config
                    .defaults
                    .cwd
                    .clone()
                    .map(|path| path.display().to_string()),
                model: self.config.defaults.model.clone(),
            })
            .await?;
        self.send_turn(client, thread_id, text).await
    }

    pub async fn quickstart(
        &self,
        cwd: Option<PathBuf>,
        model: Option<String>,
        text: &str,
    ) -> Result<()> {
        let client = self.ready_client().await?;
        let thread = client
            .thread_start(&ThreadStartRequest {
                cwd: cwd
                    .or_else(|| self.config.defaults.cwd.clone())
                    .map(|path| path.display().to_string()),
                model: model.or_else(|| self.config.defaults.model.clone()),
                ephemeral: false,
            })
            .await?;
        let final_text = self
            .send_turn(Arc::clone(&client), &thread.thread.id, text)
            .await?;
        println!("\nthread_id: {}", thread.thread.id);
        println!("final_text_len: {}", final_text.len());
        Ok(())
    }

    async fn send_turn(
        &self,
        client: Arc<OrcasIpcClient>,
        thread_id: &str,
        text: &str,
    ) -> Result<String> {
        let (mut events, _) = client.subscribe_events(false).await?;
        let response = client
            .turn_start(&TurnStartRequest {
                thread_id: thread_id.to_string(),
                text: text.to_string(),
                cwd: None,
                model: None,
            })
            .await?;
        self.stream_turn(thread_id, &response.turn_id, &mut events)
            .await
    }

    async fn stream_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
        events: &mut EventSubscription,
    ) -> Result<String> {
        let mut buffer = String::new();
        loop {
            match events.recv().await {
                Ok(envelope) => match envelope.event {
                    ipc::DaemonEvent::OutputDelta {
                        thread_id: event_thread_id,
                        turn_id: event_turn_id,
                        delta,
                        ..
                    } if event_thread_id == thread_id && event_turn_id == turn_id => {
                        print!("{delta}");
                        io::stdout().flush().ok();
                        buffer.push_str(&delta);
                    }
                    ipc::DaemonEvent::TurnUpdated {
                        thread_id: event_thread_id,
                        turn,
                    } if event_thread_id == thread_id
                        && turn.id == turn_id
                        && matches!(
                            turn.status.as_str(),
                            "completed" | "failed" | "cancelled" | "interrupted"
                        ) =>
                    {
                        println!("\n[turn completed: {}]", turn.status);
                        return Ok(buffer);
                    }
                    ipc::DaemonEvent::Warning { message } => {
                        eprintln!("warning: {message}");
                    }
                    _ => {}
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    eprintln!("warning: event stream lagged, skipped {skipped} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    anyhow::bail!("event stream closed before turn completed");
                }
            }
        }
    }

    async fn ready_client(&self) -> Result<Arc<OrcasIpcClient>> {
        let client = self
            .connect_client(if self.overrides.force_spawn {
                OrcasDaemonLaunch::Always
            } else {
                OrcasDaemonLaunch::IfNeeded
            })
            .await?;
        client
            .daemon_connect()
            .await
            .context("connect Orcas daemon to Codex")?;
        Ok(client)
    }

    async fn connect_client(&self, launch: OrcasDaemonLaunch) -> Result<Arc<OrcasIpcClient>> {
        self.daemon.ensure_running(launch).await?;
        OrcasIpcClient::connect(&self.paths)
            .await
            .context("connect to Orcas daemon")
    }
}
