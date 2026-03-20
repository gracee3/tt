#![allow(unused_crate_dependencies)]

#[path = "../../orcasd/tests/fake_codex.rs"]
mod fake_codex;
#[path = "../../orcasd/tests/fake_supervisor.rs"]
mod fake_supervisor;
#[path = "../../orcasd/tests/harness.rs"]
mod harness;

use std::process::Command;

use fake_codex::FakeCodexAppServer;
use fake_supervisor::FakeSupervisorResponsesServer;
use harness::TestDaemon;
use orcas_core::{AppConfig, ipc};

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

fn field_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}: ")))
}

async fn spawn_assignment_ready_daemon(
    test_name: &str,
) -> (FakeCodexAppServer, TestDaemon, ipc::AssignmentStartResponse) {
    let fake_codex = FakeCodexAppServer::spawn().await;
    let daemon = TestDaemon::spawn_with_env(
        test_name,
        vec![(
            "ORCAS_CODEX_LISTEN_URL".to_string(),
            fake_codex.endpoint.clone(),
        )],
    )
    .await;
    let client = daemon.connect().await;

    let workstream = client
        .workstream_create(&ipc::WorkstreamCreateRequest {
            title: format!("{test_name} root"),
            objective: "Exercise a bounded daemon-local CLI read.".to_string(),
            priority: Some("high".to_string()),
        })
        .await
        .expect("create workstream")
        .workstream;
    let work_unit = client
        .workunit_create(&ipc::WorkunitCreateRequest {
            workstream_id: workstream.id,
            title: format!("{test_name} unit"),
            task_statement: "Produce one bounded report for CLI inspection.".to_string(),
            dependencies: Vec::new(),
        })
        .await
        .expect("create workunit")
        .work_unit;
    let started = client
        .assignment_start(&ipc::AssignmentStartRequest {
            work_unit_id: work_unit.id,
            worker_id: format!("worker-{test_name}"),
            worker_kind: Some("codex".to_string()),
            instructions: Some("Run the bounded report path.".to_string()),
            model: None,
            cwd: None,
        })
        .await
        .expect("start bounded assignment");

    (fake_codex, daemon, started)
}

async fn configure_fake_supervisor(daemon: &mut TestDaemon, base_url: &str, api_key_env: &str) {
    daemon.stop().await;
    let mut config = AppConfig::write_default_if_missing(&daemon.paths)
        .await
        .expect("load daemon config");
    config.supervisor.base_url = base_url.to_string();
    config.supervisor.api_key_env = api_key_env.to_string();
    config.supervisor.model = "fake-supervisor-model".to_string();
    tokio::fs::write(
        &daemon.paths.config_file,
        toml::to_string_pretty(&config).expect("serialize daemon config"),
    )
    .await
    .expect("write daemon config");
    daemon.start().await;
}

async fn spawn_proposal_ready_daemon(
    test_name: &str,
) -> (
    FakeCodexAppServer,
    FakeSupervisorResponsesServer,
    TestDaemon,
    ipc::AssignmentStartResponse,
) {
    let fake_codex = FakeCodexAppServer::spawn().await;
    let fake_supervisor = FakeSupervisorResponsesServer::spawn().await;
    let mut daemon = TestDaemon::spawn_with_env(
        test_name,
        vec![
            (
                "ORCAS_CODEX_LISTEN_URL".to_string(),
                fake_codex.endpoint.clone(),
            ),
            (
                "ORCAS_TEST_SUPERVISOR_API_KEY".to_string(),
                "test-supervisor-key".to_string(),
            ),
        ],
    )
    .await;
    configure_fake_supervisor(
        &mut daemon,
        &fake_supervisor.base_url,
        "ORCAS_TEST_SUPERVISOR_API_KEY",
    )
    .await;
    let client = daemon.connect().await;

    let workstream = client
        .workstream_create(&ipc::WorkstreamCreateRequest {
            title: format!("{test_name} root"),
            objective: "Exercise a bounded CLI proposal workflow.".to_string(),
            priority: Some("high".to_string()),
        })
        .await
        .expect("create workstream");
    let work_unit = client
        .workunit_create(&ipc::WorkunitCreateRequest {
            workstream_id: workstream.workstream.id,
            title: format!("{test_name} unit"),
            task_statement: "Produce one bounded proposal candidate.".to_string(),
            dependencies: Vec::new(),
        })
        .await
        .expect("create workunit");
    let started = client
        .assignment_start(&ipc::AssignmentStartRequest {
            work_unit_id: work_unit.work_unit.id,
            worker_id: format!("worker-{test_name}"),
            worker_kind: Some("codex".to_string()),
            instructions: Some("Run the bounded proposal path.".to_string()),
            model: None,
            cwd: None,
        })
        .await
        .expect("start bounded proposal assignment");

    (fake_codex, fake_supervisor, daemon, started)
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
async fn real_cli_can_create_workstream_and_read_it_back() {
    let mut daemon = TestDaemon::spawn("cli-workstream-create").await;

    let create_output = run_orcas(
        &daemon,
        &[
            "workstreams",
            "create",
            "--title",
            "CLI Created Root",
            "--objective",
            "Create a workstream entirely through the CLI.",
            "--priority",
            "high",
        ],
    );
    assert!(
        create_output.status.success(),
        "stderr: {}",
        stderr(&create_output)
    );
    let create_stdout = stdout(&create_output);
    let workstream_id = field_value(&create_stdout, "workstream_id")
        .expect("workstream create should print workstream_id")
        .to_string();
    assert!(create_stdout.contains("status: Active"));

    let get_output = run_orcas(
        &daemon,
        &["workstreams", "get", "--workstream", &workstream_id],
    );
    assert!(
        get_output.status.success(),
        "stderr: {}",
        stderr(&get_output)
    );
    let get_stdout = stdout(&get_output);
    assert!(get_stdout.contains(&format!("workstream_id: {workstream_id}")));
    assert!(get_stdout.contains("title: CLI Created Root"));
    assert!(get_stdout.contains("objective: Create a workstream entirely through the CLI."));
    assert!(get_stdout.contains("status: Active"));
    assert!(get_stdout.contains("priority: high"));
    assert!(get_stdout.contains("work_units: 0"));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_create_workunit_and_read_it_back() {
    let mut daemon = TestDaemon::spawn("cli-workunit-create").await;

    let create_workstream = run_orcas(
        &daemon,
        &[
            "workstreams",
            "create",
            "--title",
            "CLI Parent Root",
            "--objective",
            "Create a parent workstream for a CLI-created work unit.",
            "--priority",
            "medium",
        ],
    );
    assert!(
        create_workstream.status.success(),
        "stderr: {}",
        stderr(&create_workstream)
    );
    let workstream_id = field_value(&stdout(&create_workstream), "workstream_id")
        .expect("workstream create should print workstream_id")
        .to_string();

    let create_workunit = run_orcas(
        &daemon,
        &[
            "workunits",
            "create",
            "--workstream",
            &workstream_id,
            "--title",
            "CLI Created Unit",
            "--task",
            "Create and inspect this work unit entirely through the CLI.",
        ],
    );
    assert!(
        create_workunit.status.success(),
        "stderr: {}",
        stderr(&create_workunit)
    );
    let create_workunit_stdout = stdout(&create_workunit);
    let work_unit_id = field_value(&create_workunit_stdout, "work_unit_id")
        .expect("workunit create should print work_unit_id")
        .to_string();
    assert!(create_workunit_stdout.contains("status: Ready"));

    let get_workunit = run_orcas(&daemon, &["workunits", "get", "--workunit", &work_unit_id]);
    assert!(
        get_workunit.status.success(),
        "stderr: {}",
        stderr(&get_workunit)
    );
    let get_workunit_stdout = stdout(&get_workunit);
    assert!(get_workunit_stdout.contains(&format!("work_unit_id: {work_unit_id}")));
    assert!(get_workunit_stdout.contains(&format!("workstream_id: {workstream_id}")));
    assert!(get_workunit_stdout.contains("title: CLI Created Unit"));
    assert!(
        get_workunit_stdout.contains(
            "task_statement: Create and inspect this work unit entirely through the CLI."
        )
    );
    assert!(get_workunit_stdout.contains("status: Ready"));
    assert!(get_workunit_stdout.contains("assignments: 0"));
    assert!(get_workunit_stdout.contains("reports: 0"));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_read_report_state_after_real_assignment_setup() {
    let (_fake_codex, mut daemon, started) = spawn_assignment_ready_daemon("cli-report-get").await;

    let output = run_orcas(&daemon, &["reports", "get", "--report", &started.report.id]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let stdout = stdout(&output);
    assert!(stdout.contains(&format!("report_id: {}", started.report.id)));
    assert!(stdout.contains(&format!("work_unit_id: {}", started.report.work_unit_id)));
    assert!(stdout.contains(&format!("assignment_id: {}", started.assignment.id)));
    assert!(stdout.contains("parse_result:"));
    assert!(stdout.contains("needs_supervisor_review:"));
    assert!(stdout.contains("summary:"));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_read_assignment_state_after_real_assignment_setup() {
    let (_fake_codex, mut daemon, started) =
        spawn_assignment_ready_daemon("cli-assignment-get").await;

    let output = run_orcas(
        &daemon,
        &["assignments", "get", "--assignment", &started.assignment.id],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let stdout = stdout(&output);
    assert!(stdout.contains(&format!("assignment_id: {}", started.assignment.id)));
    assert!(stdout.contains(&format!(
        "work_unit_id: {}",
        started.assignment.work_unit_id
    )));
    assert!(stdout.contains(&format!("worker_id: {}", started.worker.id)));
    assert!(stdout.contains("status: AwaitingDecision"));
    assert!(stdout.contains("attempt: 1"));
    assert!(stdout.contains(&format!("worker_session_id: {}", started.worker_session.id)));
    assert!(stdout.contains(&format!("report_id: {}", started.report.id)));
    assert!(stdout.contains("report_parse_result:"));
    assert!(stdout.contains("report_needs_supervisor_review:"));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_list_reports_for_workunit_after_real_assignment_setup() {
    let (_fake_codex, mut daemon, started) = spawn_assignment_ready_daemon("cli-report-list").await;

    let output = run_orcas(
        &daemon,
        &[
            "reports",
            "list-for-workunit",
            "--workunit",
            &started.report.work_unit_id,
        ],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let stdout = stdout(&output);
    assert!(stdout.contains(&started.report.id));
    assert!(stdout.contains("review="));
    assert!(stdout.contains(&started.report.summary));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_apply_decision_after_real_assignment_setup() {
    let (_fake_codex, mut daemon, started) =
        spawn_assignment_ready_daemon("cli-decision-apply").await;

    let apply_output = run_orcas(
        &daemon,
        &[
            "decisions",
            "apply",
            "--workunit",
            &started.report.work_unit_id,
            "--report",
            &started.report.id,
            "--type",
            "accept",
            "--rationale",
            "Operator accepted the bounded report via CLI integration test",
        ],
    );
    assert!(
        apply_output.status.success(),
        "stderr: {}",
        stderr(&apply_output)
    );
    let apply_stdout = stdout(&apply_output);
    let decision_id = field_value(&apply_stdout, "decision_id")
        .expect("decision apply should print decision_id")
        .to_string();
    assert!(apply_stdout.contains("decision_type: Accept"));
    assert!(apply_stdout.contains("work_unit_status: Accepted"));

    let workunit_output = run_orcas(
        &daemon,
        &[
            "workunits",
            "get",
            "--workunit",
            &started.report.work_unit_id,
        ],
    );
    assert!(
        workunit_output.status.success(),
        "stderr: {}",
        stderr(&workunit_output)
    );
    let workunit_stdout = stdout(&workunit_output);
    assert!(workunit_stdout.contains(&format!("work_unit_id: {}", started.report.work_unit_id)));
    assert!(workunit_stdout.contains("status: Accepted"));
    assert!(workunit_stdout.contains("reports: 1"));
    assert!(workunit_stdout.contains("decisions: 1"));
    assert!(!workunit_stdout.contains("current_assignment_id:"));

    let report_output = run_orcas(&daemon, &["reports", "get", "--report", &started.report.id]);
    assert!(
        report_output.status.success(),
        "stderr: {}",
        stderr(&report_output)
    );
    let report_stdout = stdout(&report_output);
    assert!(report_stdout.contains(&format!("report_id: {}", started.report.id)));
    assert!(report_stdout.contains(&format!("assignment_id: {}", started.assignment.id)));
    assert!(!decision_id.is_empty());

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn real_cli_can_create_list_and_approve_proposal_after_real_assignment_setup() {
    let (_fake_codex, _fake_supervisor, mut daemon, started) =
        spawn_proposal_ready_daemon("cli-proposal-workflow").await;

    let create_output = run_orcas(
        &daemon,
        &[
            "proposals",
            "create",
            "--workunit",
            &started.report.work_unit_id,
            "--report",
            &started.report.id,
            "--requested-by",
            "cli_operator",
            "--note",
            "Bounded CLI proposal workflow",
        ],
    );
    assert!(
        create_output.status.success(),
        "stderr: {}",
        stderr(&create_output)
    );
    let create_stdout = stdout(&create_output);
    let proposal_id = field_value(&create_stdout, "proposal_id")
        .expect("proposal create should print proposal_id")
        .to_string();
    assert!(create_stdout.contains(&format!("work_unit_id: {}", started.report.work_unit_id)));
    assert!(create_stdout.contains(&format!("source_report_id: {}", started.report.id)));
    assert!(create_stdout.contains("status: Open"));
    assert!(create_stdout.contains("reasoner_model: fake-supervisor-model"));
    assert!(create_stdout.contains("model_proposed_decision_type:"));

    let list_output = run_orcas(
        &daemon,
        &[
            "proposals",
            "list-for-workunit",
            "--workunit",
            &started.report.work_unit_id,
        ],
    );
    assert!(
        list_output.status.success(),
        "stderr: {}",
        stderr(&list_output)
    );
    let list_stdout = stdout(&list_output);
    assert!(list_stdout.contains(&proposal_id));
    assert!(list_stdout.contains("Open"));
    assert!(list_stdout.contains("fake-supervisor-model"));
    assert!(list_stdout.contains(&started.report.id));

    let approve_output = run_orcas(
        &daemon,
        &[
            "proposals",
            "approve",
            "--proposal",
            &proposal_id,
            "--reviewed-by",
            "cli_operator",
            "--review-note",
            "Approve the bounded fake supervisor proposal",
        ],
    );
    assert!(
        approve_output.status.success(),
        "stderr: {}",
        stderr(&approve_output)
    );
    let approve_stdout = stdout(&approve_output);
    assert!(approve_stdout.contains(&format!("proposal_id: {proposal_id}")));
    assert!(approve_stdout.contains("status: Approved"));
    assert!(approve_stdout.contains("reviewed_by: cli_operator"));
    assert!(approve_stdout.contains("decision_id:"));
    assert!(approve_stdout.contains("decision_type:"));

    let get_output = run_orcas(&daemon, &["proposals", "get", "--proposal", &proposal_id]);
    assert!(
        get_output.status.success(),
        "stderr: {}",
        stderr(&get_output)
    );
    let get_stdout = stdout(&get_output);
    assert!(get_stdout.contains(&format!("proposal_id: {proposal_id}")));
    assert!(get_stdout.contains("status: Approved"));
    assert!(get_stdout.contains("reviewed_by: cli_operator"));
    assert!(get_stdout.contains("approved_decision_id:"));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_reports_missing_report_with_nonzero_exit() {
    let mut daemon = TestDaemon::spawn("cli-missing-report").await;

    let output = run_orcas(&daemon, &["reports", "get", "--report", "missing-report"]);
    assert!(!output.status.success());
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("unknown report `missing-report`"));

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
