use std::collections::HashMap;
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

use tracing::{debug, error, info, warn};
use tt_core::{
    AppPaths, TT_APP_SERVER_LISTEN_URL_ENV, TT_APP_SERVER_OWNER_KIND_ENV,
    TT_APP_SERVER_OWNER_PID_ENV, TT_APP_SERVER_STARTED_AT_ENV, TT_APP_SERVER_TAG_ENV,
    TT_APP_SERVER_TAG_VALUE, TTDaemonConfig, TTError, TTResult,
};

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
pub trait TTDaemonManager: Send + Sync {
    async fn status(&self) -> TTResult<DaemonStatus>;
    async fn ensure_running(&self, launch: DaemonLaunch) -> TTResult<DaemonStatus>;
    async fn spawn_background(&self) -> TTResult<DaemonStatus>;
    async fn stop_background(&self) -> TTResult<DaemonStatus>;
}

#[derive(Debug, Clone)]
pub struct LocalTTDaemonManager {
    config: TTDaemonConfig,
    cwd: Option<PathBuf>,
    log_path: PathBuf,
    owner_kind: String,
    owner_pid: u32,
    extra_env: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct LocalTTDaemonLaunchSpec {
    pub config: TTDaemonConfig,
    pub cwd: Option<PathBuf>,
    pub log_path: PathBuf,
    pub owner_kind: String,
    pub owner_pid: u32,
    pub extra_env: Vec<(String, String)>,
}

impl LocalTTDaemonManager {
    pub fn new(config: TTDaemonConfig, paths: &AppPaths, cwd: Option<PathBuf>) -> Self {
        Self::from_launch_spec(LocalTTDaemonLaunchSpec {
            config,
            cwd,
            log_path: paths.logs_dir.join("tt-app-server.log"),
            owner_kind: "ttd".to_string(),
            owner_pid: std::process::id(),
            extra_env: Vec::new(),
        })
    }

    pub fn from_launch_spec(spec: LocalTTDaemonLaunchSpec) -> Self {
        Self {
            config: spec.config,
            cwd: spec.cwd,
            log_path: spec.log_path,
            owner_kind: spec.owner_kind,
            owner_pid: spec.owner_pid,
            extra_env: spec.extra_env,
        }
    }

    async fn wait_for_endpoint(&self, timeout: Duration) -> TTResult<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if Self::endpoint_reachable(&self.config.listen_url).await? {
                return Ok(());
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err(TTError::Transport(format!(
            "timed out waiting for TT endpoint {}",
            self.config.listen_url
        )))
    }

    async fn endpoint_reachable(endpoint: &str) -> TTResult<bool> {
        let url = Url::parse(endpoint).map_err(|error| {
            TTError::Config(format!("invalid listen URL `{endpoint}`: {error}"))
        })?;
        let host = url
            .host_str()
            .ok_or_else(|| TTError::Config(format!("listen URL `{endpoint}` is missing a host")))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| TTError::Config(format!("listen URL `{endpoint}` is missing a port")))?;
        match tokio::time::timeout(Duration::from_millis(300), TcpStream::connect((host, port)))
            .await
        {
            Ok(Ok(_)) => Ok(true),
            Ok(Err(_)) | Err(_) => Ok(false),
        }
    }
}

#[async_trait]
impl TTDaemonManager for LocalTTDaemonManager {
    async fn status(&self) -> TTResult<DaemonStatus> {
        let reachable = Self::endpoint_reachable(&self.config.listen_url).await?;
        Ok(DaemonStatus {
            endpoint: self.config.listen_url.clone(),
            reachable,
            binary_path: self.config.binary_path.clone(),
            log_path: self.log_path.clone(),
        })
    }

    async fn ensure_running(&self, launch: DaemonLaunch) -> TTResult<DaemonStatus> {
        let status = self.status().await?;
        if status.reachable && !matches!(launch, DaemonLaunch::Always) {
            return Ok(status);
        }
        if matches!(launch, DaemonLaunch::Never) {
            return Err(TTError::Transport(format!(
                "TT endpoint {} is not reachable and launching is disabled",
                status.endpoint
            )));
        }
        self.spawn_background().await
    }

    async fn spawn_background(&self) -> TTResult<DaemonStatus> {
        let start = Instant::now();
        std::fs::create_dir_all(
            self.log_path
                .parent()
                .ok_or_else(|| TTError::Config("log path has no parent directory".to_string()))?,
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
            "starting TT app-server"
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
            .env(TT_APP_SERVER_TAG_ENV, TT_APP_SERVER_TAG_VALUE)
            .env(TT_APP_SERVER_OWNER_KIND_ENV, &self.owner_kind)
            .env(TT_APP_SERVER_OWNER_PID_ENV, self.owner_pid.to_string())
            .env(TT_APP_SERVER_LISTEN_URL_ENV, &self.config.listen_url)
            .env(TT_APP_SERVER_STARTED_AT_ENV, started_at)
            .arg("app-server")
            .arg("--listen")
            .arg(&self.config.listen_url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &self.extra_env {
            command.env(key, value);
        }
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }

        let mut child = command.spawn().map_err(|error| {
            TTError::Transport(format!(
                "failed to spawn TT app-server from {}: {error}",
                self.config.binary_path.display()
            ))
        })?;
        let pid = child.id();
        info!(
            pid,
            listen_url = self.config.listen_url.as_str(),
            "TT app-server spawned"
        );
        let stdout = child.stdout.take().ok_or_else(|| {
            TTError::Transport("failed to capture TT app-server stdout".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            TTError::Transport("failed to capture TT app-server stderr".to_string())
        })?;

        Self::spawn_output_mirror("stdout", stdout, stdout_log.try_clone().await?);
        Self::spawn_output_mirror("stderr", stderr, stdout_log);

        debug!(
            pid,
            listen_url = self.config.listen_url.as_str(),
            "waiting for TT app-server readiness"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if Self::endpoint_reachable(&self.config.listen_url).await? {
                info!(
                    pid,
                    listen_url = self.config.listen_url.as_str(),
                    duration_ms = start.elapsed().as_millis() as u64,
                    "TT app-server is ready"
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
                    "TT app-server exited before becoming ready"
                );
                return Err(TTError::Transport(format!(
                    "TT app-server exited early with status {status}"
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
                    "TT app-server failed during readiness wait"
                );
            } else {
                error!(
                    pid,
                    listen_url = self.config.listen_url.as_str(),
                    duration_ms = start.elapsed().as_millis() as u64,
                    error = %error,
                    "TT app-server did not become ready in time"
                );
            }
            return Err(error);
        }
        info!(
            pid,
            listen_url = self.config.listen_url.as_str(),
            duration_ms = start.elapsed().as_millis() as u64,
            "TT app-server is ready"
        );
        self.status().await
    }

    async fn stop_background(&self) -> TTResult<DaemonStatus> {
        let processes = self.discover_managed_processes().await?;
        if processes.is_empty() {
            return self.status().await;
        }

        for pid in processes.iter().map(|process| process.pid) {
            let status = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await
                .map_err(|error| {
                    TTError::Transport(format!(
                        "failed to send SIGTERM to TT app-server pid {pid}: {error}"
                    ))
                })?;
            if !status.success() {
                warn!(pid, %status, "SIGTERM returned non-zero status for TT app-server");
            }
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !Self::endpoint_reachable(&self.config.listen_url).await? {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }

        if Self::endpoint_reachable(&self.config.listen_url).await? {
            for pid in processes.iter().map(|process| process.pid) {
                let status = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status()
                    .await
                    .map_err(|error| {
                        TTError::Transport(format!(
                            "failed to send SIGKILL to TT app-server pid {pid}: {error}"
                        ))
                    })?;
                if !status.success() {
                    warn!(pid, %status, "SIGKILL returned non-zero status for TT app-server");
                }
            }
        }

        self.status().await
    }
}

impl LocalTTDaemonManager {
    fn spawn_output_mirror<R>(stream: &'static str, reader: R, log_file: File)
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        tokio::spawn(async move {
            if let Err(error) = Self::mirror_output(reader, log_file).await {
                warn!(stream, error = %error, "failed to mirror TT app-server output");
            }
        });
    }

    async fn mirror_output<R>(reader: R, mut log_file: File) -> TTResult<()>
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

    async fn discover_managed_processes(&self) -> TTResult<Vec<ManagedTTProcess>> {
        let output = Command::new("ss")
            .args(["-ltnpH"])
            .output()
            .await
            .map_err(|error| {
                TTError::Transport(format!(
                    "failed to run `ss -ltnpH` while stopping runtime: {error}"
                ))
            })?;
        if !output.status.success() {
            return Err(TTError::Transport(format!(
                "`ss -ltnpH` failed with status {} while stopping runtime",
                output.status
            )));
        }

        let stdout = String::from_utf8(output.stdout).map_err(|error| {
            TTError::Transport(format!("failed to decode `ss` output as utf-8: {error}"))
        })?;
        let mut processes = Vec::new();
        for line in stdout.lines() {
            if let Some(process) = self.parse_listener_process(line).await? {
                processes.push(process);
            }
        }
        Ok(processes)
    }

    async fn parse_listener_process(&self, line: &str) -> TTResult<Option<ManagedTTProcess>> {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.contains("users:(") {
            return Ok(None);
        }

        let columns = trimmed.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 6 {
            return Ok(None);
        }
        let listen_address = columns[3];
        let users_index = match trimmed.find("users:(") {
            Some(index) => index,
            None => return Ok(None),
        };
        let users = &trimmed[users_index..];
        let Some(process_name) = Self::extract_quoted_value(users) else {
            return Ok(None);
        };
        if process_name != "tt" {
            return Ok(None);
        }
        let Some(pid) = Self::extract_pid(users) else {
            return Ok(None);
        };

        let environment = Self::read_process_environment(pid)
            .await
            .unwrap_or_default();
        let managed = environment
            .get(TT_APP_SERVER_TAG_ENV)
            .is_some_and(|value| value == TT_APP_SERVER_TAG_VALUE);
        if !managed {
            return Ok(None);
        }

        let owner_kind = environment.get(TT_APP_SERVER_OWNER_KIND_ENV).cloned();
        let owner_pid = environment
            .get(TT_APP_SERVER_OWNER_PID_ENV)
            .and_then(|value| value.parse::<u32>().ok());
        let listen_url = environment.get(TT_APP_SERVER_LISTEN_URL_ENV).cloned();
        let endpoint = format!("ws://{listen_address}");
        if endpoint != self.config.listen_url {
            return Ok(None);
        }
        if owner_kind.as_deref() != Some(self.owner_kind.as_str()) {
            return Ok(None);
        }
        if owner_pid != Some(self.owner_pid) {
            return Ok(None);
        }
        if listen_url.as_deref() != Some(self.config.listen_url.as_str()) {
            return Ok(None);
        }

        Ok(Some(ManagedTTProcess { pid }))
    }

    fn extract_quoted_value(text: &str) -> Option<&str> {
        let start = text.find('"')?;
        let rest = &text[start + 1..];
        let end = rest.find('"')?;
        Some(&rest[..end])
    }

    fn extract_pid(text: &str) -> Option<u32> {
        let pid_marker = "pid=";
        let start = text.find(pid_marker)? + pid_marker.len();
        let digits = text[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        digits.parse().ok()
    }

    async fn read_process_environment(pid: u32) -> Option<HashMap<String, String>> {
        let path = format!("/proc/{pid}/environ");
        let bytes = tokio::fs::read(path).await.ok()?;
        let mut env = HashMap::new();
        for entry in bytes
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            let text = String::from_utf8_lossy(entry);
            let mut parts = text.splitn(2, '=');
            let key = parts.next()?.trim();
            let value = parts.next().unwrap_or_default();
            env.insert(key.to_string(), value.to_string());
        }
        Some(env)
    }
}

#[derive(Debug, Clone, Copy)]
struct ManagedTTProcess {
    pid: u32,
}
