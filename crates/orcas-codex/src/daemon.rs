use std::path::PathBuf;
use std::process::Stdio;
use std::time::SystemTime;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::sleep;
use url::Url;

use orcas_core::{
    AppPaths, CodexDaemonConfig, ORCAS_APP_SERVER_LISTEN_URL_ENV, ORCAS_APP_SERVER_OWNER_KIND_ENV,
    ORCAS_APP_SERVER_OWNER_PID_ENV, ORCAS_APP_SERVER_STARTED_AT_ENV, ORCAS_APP_SERVER_TAG_ENV,
    ORCAS_APP_SERVER_TAG_VALUE, OrcasError, OrcasResult,
};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Copy)]
pub enum DaemonLaunch {
    Never,
    IfNeeded,
    Always,
}

#[derive(Debug, Clone)]
pub struct DaemonStatus {
    pub endpoint: String,
    pub reachable: bool,
    pub binary_path: PathBuf,
    pub log_path: PathBuf,
}

#[async_trait]
pub trait CodexDaemonManager: Send + Sync {
    async fn status(&self) -> OrcasResult<DaemonStatus>;
    async fn ensure_running(&self, launch: DaemonLaunch) -> OrcasResult<DaemonStatus>;
    async fn spawn_background(&self) -> OrcasResult<DaemonStatus>;
}

#[derive(Debug, Clone)]
pub struct LocalCodexDaemonManager {
    config: CodexDaemonConfig,
    cwd: Option<PathBuf>,
    log_path: PathBuf,
}

impl LocalCodexDaemonManager {
    pub fn new(config: CodexDaemonConfig, paths: &AppPaths, cwd: Option<PathBuf>) -> Self {
        Self {
            config,
            cwd,
            log_path: paths.logs_dir.join("codex-app-server.log"),
        }
    }

    async fn wait_for_endpoint(&self, timeout: Duration) -> OrcasResult<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if Self::endpoint_reachable(&self.config.listen_url).await? {
                return Ok(());
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err(OrcasError::Transport(format!(
            "timed out waiting for Codex endpoint {}",
            self.config.listen_url
        )))
    }

    async fn endpoint_reachable(endpoint: &str) -> OrcasResult<bool> {
        let url = Url::parse(endpoint).map_err(|error| {
            OrcasError::Config(format!("invalid listen URL `{endpoint}`: {error}"))
        })?;
        let host = url.host_str().ok_or_else(|| {
            OrcasError::Config(format!("listen URL `{endpoint}` is missing a host"))
        })?;
        let port = url.port_or_known_default().ok_or_else(|| {
            OrcasError::Config(format!("listen URL `{endpoint}` is missing a port"))
        })?;
        match tokio::time::timeout(Duration::from_millis(300), TcpStream::connect((host, port)))
            .await
        {
            Ok(Ok(_)) => Ok(true),
            Ok(Err(_)) | Err(_) => Ok(false),
        }
    }
}

#[async_trait]
impl CodexDaemonManager for LocalCodexDaemonManager {
    async fn status(&self) -> OrcasResult<DaemonStatus> {
        let reachable = Self::endpoint_reachable(&self.config.listen_url).await?;
        Ok(DaemonStatus {
            endpoint: self.config.listen_url.clone(),
            reachable,
            binary_path: self.config.binary_path.clone(),
            log_path: self.log_path.clone(),
        })
    }

    async fn ensure_running(&self, launch: DaemonLaunch) -> OrcasResult<DaemonStatus> {
        let status = self.status().await?;
        if status.reachable && !matches!(launch, DaemonLaunch::Always) {
            return Ok(status);
        }
        if matches!(launch, DaemonLaunch::Never) {
            return Err(OrcasError::Transport(format!(
                "Codex endpoint {} is not reachable and launching is disabled",
                status.endpoint
            )));
        }
        self.spawn_background().await
    }

    async fn spawn_background(&self) -> OrcasResult<DaemonStatus> {
        let start = Instant::now();
        std::fs::create_dir_all(
            self.log_path.parent().ok_or_else(|| {
                OrcasError::Config("log path has no parent directory".to_string())
            })?,
        )?;
        let stdout_log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .await?;

        info!(
            binary_path = %self.config.binary_path.display(),
            listen_url = self.config.listen_url.as_str(),
            cwd = self.cwd.as_ref().map(|cwd| cwd.display().to_string()),
            "starting Codex app-server"
        );
        let mut command = Command::new(&self.config.binary_path);
        command.kill_on_drop(false);
        for override_kv in &self.config.config_overrides {
            command.arg("--config").arg(override_kv);
        }
        let started_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string());
        command
            .env(ORCAS_APP_SERVER_TAG_ENV, ORCAS_APP_SERVER_TAG_VALUE)
            .env(ORCAS_APP_SERVER_OWNER_KIND_ENV, "orcasd")
            .env(
                ORCAS_APP_SERVER_OWNER_PID_ENV,
                std::process::id().to_string(),
            )
            .env(ORCAS_APP_SERVER_LISTEN_URL_ENV, &self.config.listen_url)
            .env(ORCAS_APP_SERVER_STARTED_AT_ENV, started_at)
            .arg("app-server")
            .arg("--listen")
            .arg(&self.config.listen_url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }

        let mut child = command.spawn().map_err(|error| {
            OrcasError::Transport(format!(
                "failed to spawn Codex app-server from {}: {error}",
                self.config.binary_path.display()
            ))
        })?;
        let pid = child.id();
        info!(
            pid,
            listen_url = self.config.listen_url.as_str(),
            "Codex app-server spawned"
        );
        let stdout = child.stdout.take().ok_or_else(|| {
            OrcasError::Transport("failed to capture Codex app-server stdout".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            OrcasError::Transport("failed to capture Codex app-server stderr".to_string())
        })?;

        Self::spawn_output_mirror("stdout", stdout, stdout_log.try_clone().await?);
        Self::spawn_output_mirror("stderr", stderr, stdout_log);

        debug!(
            pid,
            listen_url = self.config.listen_url.as_str(),
            "waiting for Codex app-server readiness"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if Self::endpoint_reachable(&self.config.listen_url).await? {
                info!(
                    pid,
                    listen_url = self.config.listen_url.as_str(),
                    duration_ms = start.elapsed().as_millis() as u64,
                    "Codex app-server is ready"
                );
                std::mem::forget(child);
                return self.status().await;
            }
            if let Some(status) = child.try_wait()? {
                error!(
                    pid,
                    listen_url = self.config.listen_url.as_str(),
                    exit_status = %status,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "Codex app-server exited before becoming ready"
                );
                return Err(OrcasError::Transport(format!(
                    "Codex app-server exited early with status {status}"
                )));
            }
            sleep(Duration::from_millis(100)).await;
        }

        if let Err(error) = self.wait_for_endpoint(Duration::from_secs(1)).await {
            if let Some(status) = child.try_wait()? {
                error!(
                    pid,
                    listen_url = self.config.listen_url.as_str(),
                    exit_status = %status,
                    duration_ms = start.elapsed().as_millis() as u64,
                    error = %error,
                    "Codex app-server failed during readiness wait"
                );
            } else {
                error!(
                    pid,
                    listen_url = self.config.listen_url.as_str(),
                    duration_ms = start.elapsed().as_millis() as u64,
                    error = %error,
                    "Codex app-server did not become ready in time"
                );
            }
            return Err(error);
        }
        info!(
            pid,
            listen_url = self.config.listen_url.as_str(),
            duration_ms = start.elapsed().as_millis() as u64,
            "Codex app-server is ready"
        );
        self.status().await
    }
}

impl LocalCodexDaemonManager {
    fn spawn_output_mirror<R>(stream: &'static str, reader: R, log_file: File)
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        tokio::spawn(async move {
            if let Err(error) = Self::mirror_output(reader, log_file).await {
                warn!(stream, error = %error, "failed to mirror Codex app-server output");
            }
        });
    }

    async fn mirror_output<R>(reader: R, mut log_file: File) -> OrcasResult<()>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();

        loop {
            buffer.clear();
            let read = reader.read_until(b'\n', &mut buffer).await?;
            if read == 0 {
                break;
            }

            log_file.write_all(&buffer).await?;
            log_file.flush().await?;
        }

        Ok(())
    }
}
