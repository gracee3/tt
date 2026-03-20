#![allow(unused_crate_dependencies)]

mod harness;

use harness::TestDaemon;

#[tokio::test]
async fn daemon_stop_removes_runtime_artifacts() {
    let mut daemon = TestDaemon::spawn("stop").await;

    assert!(daemon.paths.daemon_metadata_file.exists());
    daemon.stop().await;

    assert!(!daemon.paths.socket_file.exists());
    assert!(!daemon.paths.daemon_metadata_file.exists());
}
