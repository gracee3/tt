#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

use orcas_core::AppPaths;
use orcasd::OrcasIpcClient;

pub struct TestDaemon {
    root: PathBuf,
    pub paths: AppPaths,
    child: Option<Child>,
    extra_env: Vec<(String, String)>,
    extra_args: Vec<String>,
}

impl TestDaemon {
    pub async fn spawn(test_name: &str) -> Self {
        Self::spawn_with_env(test_name, Vec::new()).await
    }

    pub async fn spawn_with_env(test_name: &str, extra_env: Vec<(String, String)>) -> Self {
        Self::spawn_with_args(test_name, Vec::new(), extra_env).await
    }

    pub async fn spawn_with_args(
        test_name: &str,
        extra_args: Vec<String>,
        extra_env: Vec<(String, String)>,
    ) -> Self {
        let root = std::env::temp_dir().join(format!("orcasd-it-{test_name}-{}", Uuid::new_v4()));
        let paths = AppPaths::from_home(root.join(".orcas"));
        let mut daemon = Self {
            root,
            paths,
            child: None,
            extra_env,
            extra_args,
        };
        daemon.start().await;
        daemon
    }

    pub async fn start(&mut self) {
        self.paths.ensure().await.expect("create app paths");
        let mut command = Command::new(Self::orcasd_binary_path());
        command
            .args(&self.extra_args)
            .env("ORCAS_HOME", self.paths.config_dir.clone())
            .env("ORCAS_CONNECTION_MODE", "connect_only")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        for (key, value) in &self.extra_env {
            command.env(key, value);
        }
        let child = command.spawn().expect("spawn orcasd");
        self.child = Some(child);
        self.wait_until_ready().await;
    }

    fn orcasd_binary_path() -> PathBuf {
        if let Some(path) = option_env!("CARGO_BIN_EXE_orcasd") {
            return PathBuf::from(path);
        }

        let current_exe = std::env::current_exe().expect("resolve current test executable path");
        let exe_dir = current_exe
            .parent()
            .expect("current test executable should have a parent");
        let bin_dir = exe_dir
            .parent()
            .filter(|_| exe_dir.file_name().is_some_and(|name| name == "deps"))
            .unwrap_or(exe_dir);
        bin_dir.join("orcasd")
    }

    pub async fn restart(&mut self) {
        self.stop().await;
        self.start().await;
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }

    pub async fn wait_for_exit(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        let _ = timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("daemon did not exit in time");
        self.child = None;
    }

    pub async fn connect(&self) -> std::sync::Arc<OrcasIpcClient> {
        OrcasIpcClient::connect(&self.paths)
            .await
            .expect("connect real OrcasIpcClient")
    }

    pub fn orcas_home_env(&self) -> Vec<(String, String)> {
        vec![(
            "ORCAS_HOME".to_string(),
            self.paths.config_dir.display().to_string(),
        )]
    }

    pub async fn wait_until_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.paths.socket_file.exists()
                && let Ok(client) = OrcasIpcClient::connect(&self.paths).await
                && client.daemon_status().await.is_ok()
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "timed out waiting for daemon socket {}",
            self.paths.socket_file.display()
        );
    }

    pub async fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };

        if self.paths.socket_file.exists()
            && let Ok(client) = OrcasIpcClient::connect(&self.paths).await
            && let Ok(response) = client.daemon_stop().await
        {
            assert!(response.stopping);
            let status = timeout(Duration::from_secs(10), child.wait())
                .await
                .expect("daemon did not stop in time")
                .expect("wait for daemon exit");
            assert!(status.success());
        } else {
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(10), child.wait()).await;
        }
    }

    pub async fn next_event_matching<F>(
        events: &mut orcasd::EventSubscription,
        predicate: F,
    ) -> orcas_core::ipc::DaemonEventEnvelope
    where
        F: Fn(&orcas_core::ipc::DaemonEventEnvelope) -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = timeout(remaining, events.recv())
                .await
                .expect("timed out waiting for daemon event")
                .expect("event subscription closed");
            if predicate(&event) {
                return event;
            }
        }
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
