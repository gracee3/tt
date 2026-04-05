#![allow(warnings)]

mod harness;

use tokio::process::Command;

use harness::TestDaemon;

#[tokio::test]
async fn direct_daemon_flags_shape_runtime_status() {
    let daemon = TestDaemon::spawn_with_args(
        "direct-daemon-flags",
        vec![
            "--tt-bin".to_string(),
            "/bin/true".to_string(),
            "--listen-url".to_string(),
            "ws://127.0.0.1:4510".to_string(),
            "--connect-only".to_string(),
        ],
        Vec::new(),
    )
    .await;

    let client = daemon.connect().await;
    let status = client.daemon_status().await.expect("daemon status");

    assert_eq!(status.tt_binary_path, "/bin/true");
    assert_eq!(status.tt_endpoint, "ws://127.0.0.1:4510");
}

#[tokio::test]
async fn direct_daemon_flags_override_environment_values() {
    let daemon = TestDaemon::spawn_with_args(
        "direct-daemon-env-precedence",
        vec![
            "--tt-bin".to_string(),
            "/bin/true".to_string(),
            "--listen-url".to_string(),
            "ws://127.0.0.1:4511".to_string(),
            "--connect-only".to_string(),
        ],
        vec![
            ("TT_RUNTIME_BIN".to_string(), "/bin/false".to_string()),
            (
                "TT_RUNTIME_LISTEN_URL".to_string(),
                "ws://127.0.0.1:4512".to_string(),
            ),
        ],
    )
    .await;

    let client = daemon.connect().await;
    let status = client.daemon_status().await.expect("daemon status");

    assert_eq!(status.tt_binary_path, "/bin/true");
    assert_eq!(status.tt_endpoint, "ws://127.0.0.1:4511");
}

#[cfg(unix)]
#[tokio::test]
async fn direct_daemon_exits_cleanly_on_sigterm() {
    let mut daemon = TestDaemon::spawn_with_args(
        "direct-daemon-sigterm",
        vec!["--connect-only".to_string()],
        Vec::new(),
    )
    .await;

    let pid = daemon.pid().expect("daemon pid");
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .await
        .expect("send SIGTERM");
    assert!(status.success());

    daemon.wait_for_exit().await;
    assert!(!daemon.paths.socket_file.exists());
}
