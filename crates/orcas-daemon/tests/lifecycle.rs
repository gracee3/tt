#![allow(unused_crate_dependencies)]

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use chrono::Utc;
use tokio::process::Command;
use tokio::time::{Instant, sleep, timeout};

use orcas_core::AppPaths;
use orcas_daemon::OrcasIpcClient;

fn temp_root(test_name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "orcas-daemon-it-{test_name}-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ))
}

fn app_paths(root: &PathBuf) -> AppPaths {
    AppPaths {
        config_dir: root.join("config/orcas"),
        config_file: root.join("config/orcas/config.toml"),
        data_dir: root.join("data/orcas"),
        state_file: root.join("data/orcas/state.json"),
        state_db_file: root.join("data/orcas/state.db"),
        logs_dir: root.join("data/orcas/logs"),
        runtime_dir: root.join("runtime/orcas"),
        socket_file: root.join("runtime/orcas/orcasd.sock"),
        daemon_metadata_file: root.join("runtime/orcas/orcasd.json"),
        daemon_log_file: root.join("data/orcas/logs/orcasd.log"),
    }
}

async fn wait_for_socket(paths: &AppPaths) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if paths.socket_file.exists() && OrcasIpcClient::connect(paths).await.is_ok() {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "timed out waiting for daemon socket {}",
        paths.socket_file.display()
    );
}

#[tokio::test]
async fn daemon_stop_removes_runtime_artifacts() {
    let root = temp_root("stop");
    let paths = app_paths(&root);
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir_all(root.join("runtime")).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_orcasd"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("ORCAS_CONNECTION_MODE", "connect_only")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    wait_for_socket(&paths).await;
    assert!(paths.daemon_metadata_file.exists());

    let client = OrcasIpcClient::connect(&paths).await.unwrap();
    let response = client.daemon_stop().await.unwrap();
    assert!(response.stopping);

    let exit_status = timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("daemon did not stop in time")
        .unwrap();
    assert!(exit_status.success());
    assert!(!paths.socket_file.exists());
    assert!(!paths.daemon_metadata_file.exists());
}
