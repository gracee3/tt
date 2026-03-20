#![allow(unused_crate_dependencies)]

#[path = "../../orcasd/tests/harness.rs"]
mod harness;

use std::process::Command;

use harness::TestDaemon;
use orcas_core::ipc;

fn run_orcas(daemon: &TestDaemon, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_orcas"));
    command.arg("--connect-only");
    command.args(args);
    for (key, value) in daemon.xdg_env() {
        command.env(key, value);
    }
    command.output().expect("run orcas CLI")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout should be utf-8")
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr should be utf-8")
}

#[tokio::test]
async fn real_cli_can_connect_to_daemon_and_read_basic_state() {
    let mut daemon = TestDaemon::spawn("cli-daemon-status").await;

    let output = run_orcas(&daemon, &["daemon", "status"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let stdout = stdout(&output);
    assert!(stdout.contains("running: true"));
    assert!(stdout.contains("socket_responsive: true"));
    assert!(stdout.contains("client_count:"));
    assert!(stdout.contains("known_threads:"));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_observe_hierarchy_via_workstream_get() {
    let mut daemon = TestDaemon::spawn("cli-workstream-get").await;
    let client = daemon.connect().await;

    let workstream = client
        .workstream_create(&ipc::WorkstreamCreateRequest {
            title: "CLI Root".to_string(),
            objective: "Surface one workstream through the CLI.".to_string(),
            priority: Some("high".to_string()),
        })
        .await
        .expect("create workstream")
        .workstream;
    let work_unit = client
        .workunit_create(&ipc::WorkunitCreateRequest {
            workstream_id: workstream.id.clone(),
            title: "CLI Unit".to_string(),
            task_statement: "Inspect the user-visible hierarchy.".to_string(),
            dependencies: Vec::new(),
        })
        .await
        .expect("create workunit")
        .work_unit;

    let output = run_orcas(
        &daemon,
        &["workstreams", "get", "--workstream", &workstream.id],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let stdout = stdout(&output);
    assert!(stdout.contains(&format!("workstream_id: {}", workstream.id)));
    assert!(stdout.contains("title: CLI Root"));
    assert!(stdout.contains("work_units: 1"));
    assert!(stdout.contains(&format!("work_unit\t{}", work_unit.id)));
    assert!(stdout.contains("CLI Unit"));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_read_workunit_detail_after_real_setup() {
    let mut daemon = TestDaemon::spawn("cli-workunit-get").await;
    let client = daemon.connect().await;

    let workstream = client
        .workstream_create(&ipc::WorkstreamCreateRequest {
            title: "CLI Detail Root".to_string(),
            objective: "Surface one work unit detail through the CLI.".to_string(),
            priority: Some("medium".to_string()),
        })
        .await
        .expect("create workstream")
        .workstream;
    let work_unit = client
        .workunit_create(&ipc::WorkunitCreateRequest {
            workstream_id: workstream.id.clone(),
            title: "CLI Detail Unit".to_string(),
            task_statement: "Inspect this work unit via the operator CLI.".to_string(),
            dependencies: Vec::new(),
        })
        .await
        .expect("create workunit")
        .work_unit;

    let output = run_orcas(&daemon, &["workunits", "get", "--workunit", &work_unit.id]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let stdout = stdout(&output);
    assert!(stdout.contains(&format!("work_unit_id: {}", work_unit.id)));
    assert!(stdout.contains(&format!("workstream_id: {}", workstream.id)));
    assert!(stdout.contains("title: CLI Detail Unit"));
    assert!(stdout.contains("task_statement: Inspect this work unit via the operator CLI."));
    assert!(stdout.contains("status: Ready"));
    assert!(stdout.contains("assignments: 0"));
    assert!(stdout.contains("reports: 0"));
    assert!(stdout.contains("decisions: 0"));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_reports_missing_workunit_with_nonzero_exit() {
    let mut daemon = TestDaemon::spawn("cli-missing-workunit").await;

    let output = run_orcas(
        &daemon,
        &["workunits", "get", "--workunit", "missing-workunit"],
    );
    assert!(!output.status.success());
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("unknown work unit `missing-workunit`"));

    daemon.stop().await;
}
