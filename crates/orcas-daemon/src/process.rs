use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::time::sleep;

use orcas_core::{AppConfig, AppPaths, CodexConnectionMode, OrcasError, OrcasResult};

pub const ENV_CODEX_BIN: &str = "ORCAS_CODEX_BIN";
pub const ENV_CODEX_LISTEN_URL: &str = "ORCAS_CODEX_LISTEN_URL";
pub const ENV_DEFAULT_CWD: &str = "ORCAS_DEFAULT_CWD";
pub const ENV_DEFAULT_MODEL: &str = "ORCAS_DEFAULT_MODEL";
pub const ENV_CONNECTION_MODE: &str = "ORCAS_CONNECTION_MODE";

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
    pub log_path: PathBuf,
    pub running: bool,
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
        Ok(OrcasDaemonSocketStatus {
            socket_path: self.paths.socket_file.clone(),
            log_path: self.paths.daemon_log_file.clone(),
            running: Self::socket_responsive(&self.paths.socket_file).await,
        })
    }

    pub async fn ensure_running(
        &self,
        launch: OrcasDaemonLaunch,
    ) -> OrcasResult<OrcasDaemonSocketStatus> {
        let status = self.status().await?;
        if status.running && !matches!(launch, OrcasDaemonLaunch::Always) {
            return Ok(status);
        }
        if matches!(launch, OrcasDaemonLaunch::Never) {
            return Err(OrcasError::Transport(format!(
                "Orcas daemon is not reachable at {}",
                status.socket_path.display()
            )));
        }
        self.spawn_background().await
    }

    pub async fn spawn_background(&self) -> OrcasResult<OrcasDaemonSocketStatus> {
        self.paths.ensure().await?;
        std::fs::create_dir_all(self.paths.logs_dir.clone())?;
        let stdout = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.paths.daemon_log_file)?;
        let stderr = stdout.try_clone()?;

        let daemon_binary = self.resolve_daemon_binary().await?;
        let mut command = Command::new("setsid");
        command.arg(daemon_binary);
        command
            .kill_on_drop(false)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        self.apply_spawn_env(&mut command);

        let mut child = command.spawn().map_err(|error| {
            OrcasError::Transport(format!("failed to spawn Orcas daemon: {error}"))
        })?;

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if Self::socket_responsive(&self.paths.socket_file).await {
                std::mem::forget(child);
                return self.status().await;
            }
            if let Some(status) = child.try_wait()? {
                return Err(OrcasError::Transport(format!(
                    "Orcas daemon exited early with status {status}"
                )));
            }
            sleep(Duration::from_millis(100)).await;
        }

        Err(OrcasError::Transport(format!(
            "timed out waiting for Orcas daemon socket {}",
            self.paths.socket_file.display()
        )))
    }

    async fn resolve_daemon_binary(&self) -> OrcasResult<PathBuf> {
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
        if built_binary.exists() {
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
        if built_binary.exists() {
            return Ok(built_binary);
        }

        Err(OrcasError::Transport(format!(
            "orcasd binary not found at {} after build",
            built_binary.display()
        )))
    }

    fn apply_spawn_env(&self, command: &mut Command) {
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

    pub async fn socket_responsive(path: &Path) -> bool {
        tokio::time::timeout(Duration::from_millis(300), UnixStream::connect(path))
            .await
            .map(|result| result.is_ok())
            .unwrap_or(false)
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .expect("workspace root")
    }
}
