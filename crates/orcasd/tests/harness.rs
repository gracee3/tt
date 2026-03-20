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
}

impl TestDaemon {
    pub async fn spawn(test_name: &str) -> Self {
        Self::spawn_with_env(test_name, Vec::new()).await
    }

    pub async fn spawn_with_env(test_name: &str, extra_env: Vec<(String, String)>) -> Self {
        let root = std::env::temp_dir().join(format!("orcasd-it-{test_name}-{}", Uuid::new_v4()));
        let paths = AppPaths::from_roots(
            root.join("config/orcas"),
            root.join("data/orcas"),
            root.join("runtime/orcas"),
        );
        let mut daemon = Self {
            root,
            paths,
            child: None,
            extra_env,
        };
        daemon.start().await;
        daemon
    }

    pub async fn start(&mut self) {
        self.paths.ensure().await.expect("create app paths");
        let mut command = Command::new(env!("CARGO_BIN_EXE_orcasd"));
        command
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("ORCAS_CONNECTION_MODE", "connect_only")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in &self.extra_env {
            command.env(key, value);
        }
        let child = command.spawn().expect("spawn orcasd");
        self.child = Some(child);
        self.wait_until_ready().await;
    }

    pub async fn restart(&mut self) {
        self.stop().await;
        self.start().await;
    }

    pub async fn connect(&self) -> std::sync::Arc<OrcasIpcClient> {
        OrcasIpcClient::connect(&self.paths)
            .await
            .expect("connect real OrcasIpcClient")
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
