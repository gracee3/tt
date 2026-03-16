use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::time::sleep;

use orcas_core::{AppConfig, AppPaths, CodexConnectionMode, OrcasError, OrcasResult, ipc};

use crate::client::OrcasIpcClient;

pub const ENV_CODEX_BIN: &str = "ORCAS_CODEX_BIN";
pub const ENV_CODEX_LISTEN_URL: &str = "ORCAS_CODEX_LISTEN_URL";
pub const ENV_DEFAULT_CWD: &str = "ORCAS_DEFAULT_CWD";
pub const ENV_DEFAULT_MODEL: &str = "ORCAS_DEFAULT_MODEL";
pub const ENV_CONNECTION_MODE: &str = "ORCAS_CONNECTION_MODE";
pub const ENV_DAEMON_BINARY_PATH: &str = "ORCAS_DAEMON_BINARY_PATH";
pub const ENV_DAEMON_BUILD_FINGERPRINT: &str = "ORCAS_DAEMON_BUILD_FINGERPRINT";

#[derive(Debug, Clone, Default)]
pub struct OrcasRuntimeOverrides {
    pub codex_bin: Option<PathBuf>,
    pub listen_url: Option<String>,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub connect_only: bool,
    pub force_spawn: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum OrcasDaemonLaunch {
    Never,
    IfNeeded,
    Always,
}

#[derive(Debug, Clone)]
pub struct OrcasDaemonSocketStatus {
    pub socket_path: PathBuf,
    pub metadata_path: PathBuf,
    pub log_path: PathBuf,
    pub running: bool,
    pub socket_exists: bool,
    pub socket_responsive: bool,
    pub pid_running: bool,
    pub socket_owner_pid: Option<u32>,
    pub stale_socket: bool,
    pub stale_metadata: bool,
    pub daemon_status: Option<ipc::DaemonStatusResponse>,
    pub runtime_metadata: Option<ipc::DaemonRuntimeMetadata>,
    pub expected_binary: Option<ipc::DaemonBinarySummary>,
    pub binary_matches_expected: Option<bool>,
}

pub fn apply_runtime_overrides(config: &mut AppConfig, overrides: &OrcasRuntimeOverrides) {
    if let Some(codex_bin) = &overrides.codex_bin {
        config.codex.binary_path = codex_bin.clone();
    }
    if let Some(listen_url) = &overrides.listen_url {
        config.codex.listen_url = listen_url.clone();
    }
    if let Some(cwd) = &overrides.cwd {
        config.defaults.cwd = Some(cwd.clone());
    }
    if let Some(model) = &overrides.model {
        config.defaults.model = Some(model.clone());
    }
    if overrides.connect_only {
        config.codex.connection_mode = CodexConnectionMode::ConnectOnly;
    }
    if overrides.force_spawn {
        config.codex.connection_mode = CodexConnectionMode::SpawnAlways;
    }
}

#[derive(Debug, Clone)]
pub struct OrcasDaemonProcessManager {
    paths: AppPaths,
    overrides: OrcasRuntimeOverrides,
}

impl OrcasDaemonProcessManager {
    pub fn new(paths: AppPaths, overrides: OrcasRuntimeOverrides) -> Self {
        Self { paths, overrides }
    }

    pub async fn status(&self) -> OrcasResult<OrcasDaemonSocketStatus> {
        self.paths.ensure().await?;
        let socket_exists = tokio::fs::try_exists(&self.paths.socket_file).await?;
        let socket_responsive = Self::socket_responsive(&self.paths.socket_file).await;
        let metadata = Self::read_runtime_metadata(&self.paths).await.ok();
        let socket_owner_pid = if socket_exists {
            Self::socket_owner_pid(&self.paths.socket_file)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let pid_running = metadata
            .as_ref()
            .map(|runtime| Self::process_exists(runtime.pid))
            .or_else(|| socket_owner_pid.map(Self::process_exists))
            .unwrap_or(false);
        let daemon_status = if socket_responsive {
            Self::fetch_daemon_status(&self.paths).await
        } else {
            None
        };
        let runtime_metadata = daemon_status
            .as_ref()
            .map(|status| status.runtime.clone())
            .or(metadata);
        let expected_binary = self
            .resolve_daemon_binary(false)
            .await
            .ok()
            .and_then(|path| {
                std::fs::metadata(&path)
                    .ok()
                    .and_then(|_| Self::binary_summary_from_path(&path).ok())
            });
        let binary_matches_expected = daemon_status.as_ref().and_then(|status| {
            expected_binary
                .as_ref()
                .map(|expected| status.runtime.build_fingerprint == expected.build_fingerprint)
        });

        Ok(OrcasDaemonSocketStatus {
            socket_path: self.paths.socket_file.clone(),
            metadata_path: self.paths.daemon_metadata_file.clone(),
            log_path: self.paths.daemon_log_file.clone(),
            running: socket_responsive,
            socket_exists,
            socket_responsive,
            pid_running,
            socket_owner_pid,
            stale_socket: socket_exists && !socket_responsive,
            stale_metadata: runtime_metadata.is_some() && !pid_running && !socket_responsive,
            daemon_status,
            runtime_metadata,
            expected_binary,
            binary_matches_expected,
        })
    }

    pub async fn ensure_running(
        &self,
        launch: OrcasDaemonLaunch,
    ) -> OrcasResult<OrcasDaemonSocketStatus> {
        let status = self.status().await?;
        match launch {
            OrcasDaemonLaunch::Never => {
                if status.running {
                    Ok(status)
                } else {
                    Err(OrcasError::Transport(format!(
                        "Orcas daemon is not reachable at {}",
                        status.socket_path.display()
                    )))
                }
            }
            OrcasDaemonLaunch::IfNeeded => {
                if status.running {
                    Ok(status)
                } else {
                    self.spawn_background().await
                }
            }
            OrcasDaemonLaunch::Always => self.restart().await,
        }
    }

    pub async fn restart(&self) -> OrcasResult<OrcasDaemonSocketStatus> {
        let status = self.status().await?;
        if status.running {
            self.stop_process(&status).await?;
        }
        self.cleanup_stale_runtime().await?;
        self.spawn_background().await
    }

    pub async fn spawn_background(&self) -> OrcasResult<OrcasDaemonSocketStatus> {
        self.paths.ensure().await?;
        self.cleanup_stale_runtime().await?;
        std::fs::create_dir_all(self.paths.logs_dir.clone())?;
        let stdout = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.paths.daemon_log_file)?;
        let stderr = stdout.try_clone()?;

        let daemon_binary = self.resolve_daemon_binary(true).await?;
        let binary_summary = Self::binary_summary_from_path(&daemon_binary)?;
        let mut command = Command::new("setsid");
        command.arg(&daemon_binary);
        command
            .kill_on_drop(false)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        self.apply_spawn_env(&mut command, &binary_summary);

        let mut child = command.spawn().map_err(|error| {
            OrcasError::Transport(format!("failed to spawn Orcas daemon: {error}"))
        })?;

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let status = self.status().await?;
            if status.running {
                std::mem::forget(child);
                return Ok(status);
            }
            if let Some(exit_status) = child.try_wait()? {
                return Err(OrcasError::Transport(format!(
                    "Orcas daemon exited early with status {exit_status}"
                )));
            }
            sleep(Duration::from_millis(100)).await;
        }

        Err(OrcasError::Transport(format!(
            "timed out waiting for Orcas daemon socket {}",
            self.paths.socket_file.display()
        )))
    }

    async fn stop_process(&self, status: &OrcasDaemonSocketStatus) -> OrcasResult<()> {
        let Some(runtime) = status
            .daemon_status
            .as_ref()
            .map(|daemon| daemon.runtime.clone())
            .or_else(|| status.runtime_metadata.clone())
            .or_else(|| {
                status
                    .socket_owner_pid
                    .map(|pid| ipc::DaemonRuntimeMetadata {
                        pid,
                        started_at: Utc::now(),
                        version: "unknown".to_string(),
                        build_fingerprint: "unknown".to_string(),
                        binary_path: "unknown".to_string(),
                        socket_path: self.paths.socket_file.display().to_string(),
                        metadata_path: self.paths.daemon_metadata_file.display().to_string(),
                        git_commit: None,
                    })
            })
        else {
            return Ok(());
        };

        Self::signal_pid(runtime.pid, "TERM").await?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !Self::process_exists(runtime.pid)
                && !Self::socket_responsive(&self.paths.socket_file).await
            {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }

        if Self::process_exists(runtime.pid) {
            Self::signal_pid(runtime.pid, "KILL").await?;
        }
        self.cleanup_stale_runtime().await
    }

    async fn cleanup_stale_runtime(&self) -> OrcasResult<()> {
        let status = self.status().await?;
        if status.stale_socket && tokio::fs::try_exists(&self.paths.socket_file).await? {
            tokio::fs::remove_file(&self.paths.socket_file).await?;
        }
        if status.stale_metadata && tokio::fs::try_exists(&self.paths.daemon_metadata_file).await? {
            tokio::fs::remove_file(&self.paths.daemon_metadata_file).await?;
        }
        Ok(())
    }

    async fn resolve_daemon_binary(&self, build_if_needed: bool) -> OrcasResult<PathBuf> {
        if let Some(orcasd) = std::env::current_exe()
            .ok()
            .as_ref()
            .and_then(|exe| exe.parent().map(|parent| parent.join("orcasd")))
            .filter(|path| path.exists())
        {
            return Ok(orcasd);
        }

        let repo_root = Self::repo_root();
        let built_binary = repo_root.join("target/debug/orcasd");
        if built_binary.exists() || !build_if_needed {
            return Ok(built_binary);
        }

        let status = Command::new("cargo")
            .arg("build")
            .arg("-q")
            .arg("-p")
            .arg("orcas-daemon")
            .arg("--bin")
            .arg("orcasd")
            .current_dir(&repo_root)
            .status()
            .await
            .map_err(|error| {
                OrcasError::Transport(format!("failed to build or locate orcasd: {error}"))
            })?;
        if !status.success() {
            return Err(OrcasError::Transport(format!(
                "failed to prepare orcasd binary, cargo exited with {status}"
            )));
        }

        Ok(built_binary)
    }

    fn apply_spawn_env(&self, command: &mut Command, binary: &ipc::DaemonBinarySummary) {
        command
            .env(ENV_DAEMON_BINARY_PATH, &binary.binary_path)
            .env(ENV_DAEMON_BUILD_FINGERPRINT, &binary.build_fingerprint);

        if let Some(codex_bin) = &self.overrides.codex_bin {
            command.env(ENV_CODEX_BIN, codex_bin);
        }
        if let Some(listen_url) = &self.overrides.listen_url {
            command.env(ENV_CODEX_LISTEN_URL, listen_url);
        }
        if let Some(cwd) = &self.overrides.cwd {
            command.env(ENV_DEFAULT_CWD, cwd);
        }
        if let Some(model) = &self.overrides.model {
            command.env(ENV_DEFAULT_MODEL, model);
        }
        if self.overrides.connect_only {
            command.env(ENV_CONNECTION_MODE, "connect_only");
        } else if self.overrides.force_spawn {
            command.env(ENV_CONNECTION_MODE, "spawn_always");
        }
    }

    pub async fn runtime_metadata_for_current_process(
        paths: &AppPaths,
    ) -> OrcasResult<ipc::DaemonRuntimeMetadata> {
        let binary_path = std::env::var(ENV_DAEMON_BINARY_PATH)
            .map(PathBuf::from)
            .or_else(|_| std::env::current_exe())
            .map_err(|error| {
                OrcasError::Transport(format!("failed to resolve current daemon binary: {error}"))
            })?;
        let build_fingerprint = std::env::var(ENV_DAEMON_BUILD_FINGERPRINT)
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| Self::binary_fingerprint_sync(&binary_path));

        Ok(ipc::DaemonRuntimeMetadata {
            pid: std::process::id(),
            started_at: Utc::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            build_fingerprint,
            binary_path: binary_path.display().to_string(),
            socket_path: paths.socket_file.display().to_string(),
            metadata_path: paths.daemon_metadata_file.display().to_string(),
            git_commit: option_env!("ORCAS_GIT_COMMIT").map(ToOwned::to_owned),
        })
    }

    pub async fn write_runtime_metadata(
        paths: &AppPaths,
        runtime: &ipc::DaemonRuntimeMetadata,
    ) -> OrcasResult<()> {
        paths.ensure().await?;
        let raw = serde_json::to_string_pretty(runtime)?;
        tokio::fs::write(&paths.daemon_metadata_file, raw).await?;
        Ok(())
    }

    pub async fn read_runtime_metadata(
        paths: &AppPaths,
    ) -> OrcasResult<ipc::DaemonRuntimeMetadata> {
        let raw = tokio::fs::read_to_string(&paths.daemon_metadata_file).await?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub async fn socket_responsive(path: &Path) -> bool {
        tokio::time::timeout(Duration::from_millis(300), UnixStream::connect(path))
            .await
            .map(|result| result.is_ok())
            .unwrap_or(false)
    }

    fn process_exists(pid: u32) -> bool {
        PathBuf::from(format!("/proc/{pid}")).exists()
    }

    async fn signal_pid(pid: u32, signal: &str) -> OrcasResult<()> {
        let status = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(pid.to_string())
            .status()
            .await
            .map_err(|error| {
                OrcasError::Transport(format!("failed to signal daemon pid {pid}: {error}"))
            })?;
        if status.success() {
            Ok(())
        } else {
            Err(OrcasError::Transport(format!(
                "failed to signal daemon pid {pid} with {signal}: {status}"
            )))
        }
    }

    async fn fetch_daemon_status(paths: &AppPaths) -> Option<ipc::DaemonStatusResponse> {
        let client = OrcasIpcClient::connect(paths).await.ok()?;
        client.daemon_status().await.ok()
    }

    async fn socket_owner_pid(path: &Path) -> OrcasResult<Option<u32>> {
        let output = Command::new("lsof")
            .arg("-t")
            .arg(path)
            .output()
            .await
            .map_err(|error| {
                OrcasError::Transport(format!(
                    "failed to inspect Orcas daemon socket owner for {}: {error}",
                    path.display()
                ))
            })?;
        if !output.status.success() {
            return Ok(None);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .find_map(|line| line.trim().parse::<u32>().ok()))
    }

    fn binary_summary_from_path(path: &Path) -> OrcasResult<ipc::DaemonBinarySummary> {
        Ok(ipc::DaemonBinarySummary {
            version: env!("CARGO_PKG_VERSION").to_string(),
            build_fingerprint: Self::binary_fingerprint_sync(path),
            binary_path: path.display().to_string(),
        })
    }

    fn binary_fingerprint_sync(path: &Path) -> String {
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        if let Ok(metadata) = std::fs::metadata(path) {
            metadata.len().hash(&mut hasher);
            if let Ok(modified) = metadata.modified()
                && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                duration.as_secs().hash(&mut hasher);
                duration.subsec_nanos().hash(&mut hasher);
            }
        }
        if let Ok(bytes) = std::fs::read(path) {
            bytes.hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .expect("workspace root")
    }
}

#[cfg(test)]
mod tests {
    use super::{OrcasDaemonProcessManager, OrcasRuntimeOverrides};
    use chrono::Utc;
    use orcas_core::AppPaths;

    fn temp_paths(test_name: &str) -> AppPaths {
        let root = std::env::temp_dir().join(format!(
            "orcas-process-test-{test_name}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        AppPaths {
            config_dir: root.join("config"),
            config_file: root.join("config/config.toml"),
            data_dir: root.join("data"),
            state_file: root.join("data/state.json"),
            logs_dir: root.join("data/logs"),
            runtime_dir: root.join("runtime"),
            socket_file: root.join("runtime/orcasd.sock"),
            daemon_metadata_file: root.join("runtime/orcasd.json"),
            daemon_log_file: root.join("data/logs/orcasd.log"),
        }
    }

    #[tokio::test]
    async fn runtime_metadata_round_trip_works() {
        let paths = temp_paths("metadata");
        paths.ensure().await.unwrap();
        let metadata = OrcasDaemonProcessManager::runtime_metadata_for_current_process(&paths)
            .await
            .unwrap();
        OrcasDaemonProcessManager::write_runtime_metadata(&paths, &metadata)
            .await
            .unwrap();

        let loaded = OrcasDaemonProcessManager::read_runtime_metadata(&paths)
            .await
            .unwrap();
        assert_eq!(loaded.pid, metadata.pid);
        assert_eq!(loaded.build_fingerprint, metadata.build_fingerprint);
        assert_eq!(loaded.socket_path, metadata.socket_path);
    }

    #[tokio::test]
    async fn status_marks_stale_metadata_without_socket() {
        let paths = temp_paths("stale");
        let manager =
            OrcasDaemonProcessManager::new(paths.clone(), OrcasRuntimeOverrides::default());
        let mut metadata = OrcasDaemonProcessManager::runtime_metadata_for_current_process(&paths)
            .await
            .unwrap();
        metadata.pid = 999_999;
        OrcasDaemonProcessManager::write_runtime_metadata(&paths, &metadata)
            .await
            .unwrap();

        let status = manager.status().await.unwrap();
        assert!(!status.running);
        assert!(status.stale_metadata);
    }
}
