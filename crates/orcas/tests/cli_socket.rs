#![allow(unused_crate_dependencies)]

#[path = "../../orcasd/tests/fake_codex.rs"]
mod fake_codex;
#[path = "../../orcasd/tests/fake_supervisor.rs"]
mod fake_supervisor;
#[path = "../../orcasd/tests/harness.rs"]
mod harness;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::Utc;
use fake_codex::FakeCodexAppServer;
use fake_supervisor::FakeSupervisorResponsesServer;
use harness::TestDaemon;
use orcas_core::{AppConfig, StoredState, authority, ipc};
use orcas_core::{
    collaboration::{
        Assignment, AssignmentStatus, PlanningSession, PlanningSessionResearchStatus,
        PlanningSessionStatus, PlanningSessionStructuredSummary, Report, ReportConfidence,
        ReportDisposition, ReportParseResult, Worker, WorkerSession, WorkerSessionAttachability,
        WorkerSessionRuntimeStatus, WorkerStatus,
    },
    planning::PlanExecutionKind,
};
use uuid::Uuid;

fn run_orcas(daemon: &TestDaemon, args: &[&str]) -> std::process::Output {
    let envs = daemon.xdg_env();
    run_orcas_with_env(args, &envs)
}

fn run_orcas_with_env(args: &[&str], envs: &[(String, String)]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_orcas"));
    command.arg("--connect-only");
    command.args(args);
    for (key, value) in envs {
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

static PLANNING_SESSION_CLI_TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn planning_session_cli_test_lock() -> &'static tokio::sync::Mutex<()> {
    PLANNING_SESSION_CLI_TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

struct FakePlanningSessionDaemon {
    root: PathBuf,
    paths: orcas_core::AppPaths,
    task: std::thread::JoinHandle<()>,
    stop: Arc<AtomicBool>,
}

impl FakePlanningSessionDaemon {
    async fn spawn(test_name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("orcas-cli-{test_name}-{}", Uuid::new_v4()));
        let paths = orcas_core::AppPaths::from_roots(
            root.join("config/orcas"),
            root.join("data/orcas"),
            root.join("runtime/orcas"),
        );
        paths.ensure().await.expect("create app paths");
        let _ = tokio::fs::remove_file(&paths.socket_file).await;
        let listener = std::os::unix::net::UnixListener::bind(&paths.socket_file)
            .expect("bind fake daemon socket");
        listener
            .set_nonblocking(true)
            .expect("set fake daemon socket nonblocking");
        let request_counts = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let socket_path = paths.socket_file.clone();
        let metadata_path = paths.daemon_metadata_file.clone();
        let root_for_task = root.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_task = Arc::clone(&stop);
        let task = std::thread::spawn(move || {
            loop {
                if stop_for_task.load(Ordering::Acquire) {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let request_counts = Arc::clone(&request_counts);
                        let socket_path = socket_path.clone();
                        let metadata_path = metadata_path.clone();
                        std::thread::spawn(move || {
                            serve_fake_planning_session_connection(
                                stream,
                                request_counts,
                                socket_path,
                                metadata_path,
                            )
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
            let _ = std::fs::remove_file(&socket_path);
            let _ = std::fs::remove_dir_all(&root_for_task);
        });
        Self {
            root,
            paths,
            task,
            stop,
        }
    }

    fn xdg_env(&self) -> Vec<(String, String)> {
        vec![
            (
                "XDG_CONFIG_HOME".to_string(),
                self.root.join("config").display().to_string(),
            ),
            (
                "XDG_DATA_HOME".to_string(),
                self.root.join("data").display().to_string(),
            ),
            (
                "XDG_RUNTIME_DIR".to_string(),
                self.root.join("runtime").display().to_string(),
            ),
        ]
    }

    async fn stop(self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.task.join();
        let _ = tokio::fs::remove_file(&self.paths.socket_file).await;
        let _ = tokio::fs::remove_dir_all(&self.root).await;
    }
}

struct AuthorityFixture {
    origin_node_id: authority::OriginNodeId,
    actor: authority::CommandActor,
}

impl AuthorityFixture {
    fn new() -> Self {
        Self {
            origin_node_id: authority::OriginNodeId::new(),
            actor: authority::CommandActor::parse("cli_socket_test").expect("command actor"),
        }
    }

    fn metadata(&self, label: &str) -> authority::CommandMetadata {
        authority::CommandMetadata {
            command_id: authority::CommandId::new(),
            issued_at: Utc::now(),
            origin_node_id: self.origin_node_id.clone(),
            actor: self.actor.clone(),
            correlation_id: Some(
                authority::CorrelationId::parse(format!("cli-socket-{label}"))
                    .expect("correlation id"),
            ),
        }
    }
}

async fn create_authority_workstream(
    daemon: &TestDaemon,
    fixture: &AuthorityFixture,
    workstream_id: &str,
    title: &str,
) -> authority::WorkstreamRecord {
    daemon
        .connect()
        .await
        .authority_workstream_create(&ipc::AuthorityWorkstreamCreateRequest {
            command: authority::CreateWorkstream {
                metadata: fixture.metadata("ws-create"),
                workstream_id: authority::WorkstreamId::parse(workstream_id)
                    .expect("workstream id"),
                title: title.to_string(),
                objective: format!("Objective for {title}"),
                status: orcas_core::WorkstreamStatus::Active,
                priority: "high".to_string(),
            },
        })
        .await
        .expect("create authority workstream")
        .workstream
}

async fn create_authority_workunit(
    daemon: &TestDaemon,
    fixture: &AuthorityFixture,
    work_unit_id: &str,
    workstream_id: &authority::WorkstreamId,
    title: &str,
) -> authority::WorkUnitRecord {
    daemon
        .connect()
        .await
        .authority_workunit_create(&ipc::AuthorityWorkunitCreateRequest {
            command: authority::CreateWorkUnit {
                metadata: fixture.metadata("wu-create"),
                work_unit_id: authority::WorkUnitId::parse(work_unit_id).expect("work unit id"),
                workstream_id: workstream_id.clone(),
                title: title.to_string(),
                task_statement: format!("Task for {title}"),
                status: orcas_core::WorkUnitStatus::Ready,
            },
        })
        .await
        .expect("create authority workunit")
        .work_unit
}

async fn create_authority_tracked_thread(
    daemon: &TestDaemon,
    fixture: &AuthorityFixture,
    tracked_thread_id: &str,
    work_unit_id: &authority::WorkUnitId,
    title: &str,
) -> authority::TrackedThreadRecord {
    daemon
        .connect()
        .await
        .authority_tracked_thread_create(&ipc::AuthorityTrackedThreadCreateRequest {
            command: authority::CreateTrackedThread {
                metadata: fixture.metadata("tt-create"),
                tracked_thread_id: authority::TrackedThreadId::parse(tracked_thread_id)
                    .expect("tracked thread id"),
                work_unit_id: work_unit_id.clone(),
                title: title.to_string(),
                notes: Some(format!("Notes for {title}")),
                backend_kind: authority::TrackedThreadBackendKind::Codex,
                upstream_thread_id: Some(format!("upstream-{tracked_thread_id}")),
                preferred_cwd: Some("/tmp/orcas".to_string()),
                preferred_model: Some("gpt-5.4".to_string()),
                workspace: None,
            },
        })
        .await
        .expect("create authority tracked thread")
        .tracked_thread
}

async fn spawn_planning_session_cli_daemon(test_name: &str) -> (FakeCodexAppServer, TestDaemon) {
    let fake_codex = FakeCodexAppServer::spawn().await;
    let daemon = TestDaemon::spawn_with_env(
        test_name,
        vec![(
            "ORCAS_CODEX_LISTEN_URL".to_string(),
            fake_codex.endpoint.clone(),
        )],
    )
    .await;
    (fake_codex, daemon)
}

async fn spawn_fake_planning_session_cli_daemon(test_name: &str) -> FakePlanningSessionDaemon {
    FakePlanningSessionDaemon::spawn(test_name).await
}

fn create_cli_workstream(daemon: &TestDaemon, label: &str) -> String {
    let create_output = run_orcas(
        daemon,
        &[
            "workstreams",
            "create",
            "--title",
            &format!("CLI {label} Root"),
            "--objective",
            &format!("Objective for {label}"),
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
    assert!(create_stdout.contains("surface: authority"));
    assert!(create_stdout.contains("status: Active"));
    field_value(&create_stdout, "workstream_id")
        .expect("workstream create should print workstream_id")
        .to_string()
}

fn serve_fake_planning_session_connection(
    stream: std::os::unix::net::UnixStream,
    request_counts: Arc<std::sync::Mutex<HashMap<String, usize>>>,
    socket_path: PathBuf,
    metadata_path: PathBuf,
) {
    let mut reader = std::io::BufReader::new(
        stream
            .try_clone()
            .expect("clone fake planning daemon socket stream"),
    );
    let mut write_half = stream;
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = std::io::BufRead::read_line(&mut reader, &mut line)
            .expect("read fake planning daemon request line");
        if bytes == 0 {
            break;
        }
        let line = line.trim_end_matches(['\n', '\r']);
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            break;
        };
        let Some(method) = message.get("method").and_then(|value| value.as_str()) else {
            continue;
        };
        let id = message
            .get("id")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let params = message.get("params").cloned();
        match method {
            ipc::methods::DAEMON_STATUS => {
                let response = ipc::DaemonStatusResponse {
                    socket_path: socket_path.display().to_string(),
                    metadata_path: metadata_path.display().to_string(),
                    codex_endpoint: "ws://fake-codex".to_string(),
                    codex_binary_path: "/usr/bin/codex".to_string(),
                    upstream: orcas_core::ConnectionState {
                        endpoint: "ws://fake-codex".to_string(),
                        status: "connected".to_string(),
                        detail: None,
                    },
                    client_count: 1,
                    known_threads: 1,
                    runtime: orcas_core::DaemonRuntimeMetadata {
                        pid: std::process::id(),
                        started_at: Utc::now(),
                        version: "test".to_string(),
                        build_fingerprint: "fake-planning-session-daemon".to_string(),
                        binary_path: "/usr/bin/orcasd".to_string(),
                        socket_path: socket_path.display().to_string(),
                        metadata_path: metadata_path.display().to_string(),
                        git_commit: Some("deadbeef".to_string()),
                    },
                };
                let _ = send_jsonrpc_response(&mut write_half, id, response);
            }
            ipc::methods::PLANNING_SESSION_REQUEST_RESEARCH => {
                let Some(params) = params else {
                    let _ = send_jsonrpc_error(
                        &mut write_half,
                        id,
                        -32602,
                        "invalid planning session research request",
                    );
                    continue;
                };
                let Ok(params) =
                    serde_json::from_value::<ipc::PlanningSessionRequestResearchRequest>(params)
                else {
                    let _ = send_jsonrpc_error(
                        &mut write_half,
                        id,
                        -32602,
                        "invalid planning session research request",
                    );
                    continue;
                };
                let mut counts = request_counts
                    .lock()
                    .expect("lock fake planning daemon request counts");
                let entry = counts.entry(params.session_id.clone()).or_insert(0);
                *entry += 1;
                if *entry > 1 {
                    let _ = send_jsonrpc_error(
                        &mut write_half,
                        id,
                        -32001,
                        format!(
                            "planning session {} already used its bounded research turn",
                            params.session_id
                        ),
                    );
                    continue;
                }
                let response = planning_session_research_response(&params, &socket_path);
                let _ = send_jsonrpc_response(&mut write_half, id, response);
            }
            _ => {
                let _ = send_jsonrpc_error(
                    &mut write_half,
                    id,
                    -32601,
                    format!("method not found: {method}"),
                );
            }
        }
    }
}

fn send_jsonrpc_response<T: serde::Serialize>(
    write_half: &mut std::os::unix::net::UnixStream,
    id: serde_json::Value,
    result: T,
) -> std::io::Result<()> {
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": serde_json::to_value(result).expect("serialize fake daemon response"),
    });
    send_jsonrpc_message(write_half, message)
}

fn send_jsonrpc_error(
    write_half: &mut std::os::unix::net::UnixStream,
    id: serde_json::Value,
    code: i64,
    message: impl Into<String>,
) -> std::io::Result<()> {
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        }
    });
    send_jsonrpc_message(write_half, message)
}

fn send_jsonrpc_message(
    write_half: &mut std::os::unix::net::UnixStream,
    message: serde_json::Value,
) -> std::io::Result<()> {
    let raw = serde_json::to_string(&message).expect("serialize json-rpc message");
    std::io::Write::write_all(write_half, raw.as_bytes())?;
    std::io::Write::write_all(write_half, b"\n")?;
    std::io::Write::flush(write_half)
}

fn planning_session_research_response(
    request: &ipc::PlanningSessionRequestResearchRequest,
    socket_path: &PathBuf,
) -> ipc::PlanningSessionRequestResearchResponse {
    let now = Utc::now();
    let assignment_id = format!("assignment-{}", request.session_id);
    let worker_session_id = format!("worker-session-{}", request.session_id);
    let report_id = format!("report-{}", request.session_id);
    let work_unit_id = format!("work-unit-{}", request.session_id);
    let worker_kind = request
        .worker_kind
        .clone()
        .unwrap_or_else(|| "codex".to_string());
    let session = PlanningSession {
        session_id: request.session_id.clone(),
        workstream_id: "fake-workstream".to_string(),
        status: PlanningSessionStatus::ResearchRequested,
        planning_thread_id: "fake-planning-thread".to_string(),
        base_plan_id: None,
        base_plan_version: None,
        research_assignment_id: Some(assignment_id.clone()),
        research_report_id: Some(report_id.clone()),
        draft_revision_proposal_id: None,
        approved_plan_id: None,
        approved_plan_version: None,
        latest_structured_summary: PlanningSessionStructuredSummary {
            objective: "Plan one bounded change.".to_string(),
            research_status: PlanningSessionResearchStatus::Requested,
            requirements: vec![],
            constraints: vec![],
            non_goals: vec![],
            open_questions: vec![],
            draft_plan_summary: None,
            ready_for_review: false,
        },
        created_at: now,
        created_by: request
            .requested_by
            .clone()
            .unwrap_or_else(|| "cli_operator".to_string()),
        updated_at: now,
        updated_by: request
            .requested_by
            .clone()
            .unwrap_or_else(|| "cli_operator".to_string()),
        request_note: request.request_note.clone(),
        reviewed_at: None,
        reviewed_by: None,
        review_note: None,
        superseded_by_session_id: None,
    };
    let assignment = Assignment {
        id: assignment_id.clone(),
        work_unit_id: work_unit_id.clone(),
        plan_id: None,
        plan_version: None,
        plan_item_id: None,
        execution_kind: PlanExecutionKind::DirectExecution,
        alignment_rationale: Some("bounded research turn".to_string()),
        worker_id: request.worker_id.clone(),
        worker_session_id: worker_session_id.clone(),
        instructions: request.request_note.clone().unwrap_or_else(|| {
            "Perform one bounded research turn for the planning session.".to_string()
        }),
        communication_seed: None,
        status: AssignmentStatus::Running,
        attempt_number: 1,
        created_at: now,
        updated_at: now,
    };
    let worker = Worker {
        id: request.worker_id.clone(),
        kind: worker_kind.clone(),
        status: WorkerStatus::Busy,
        current_assignment_id: Some(assignment_id.clone()),
    };
    let worker_session = WorkerSession {
        id: worker_session_id.clone(),
        worker_id: request.worker_id.clone(),
        backend_type: worker_kind,
        thread_id: Some(socket_path.display().to_string()),
        tracked_thread_id: None,
        active_turn_id: None,
        runtime_status: WorkerSessionRuntimeStatus::Running,
        attachability: WorkerSessionAttachability::NotAttachable,
        updated_at: now,
    };
    let report = Report {
        id: report_id,
        work_unit_id,
        assignment_id,
        worker_id: request.worker_id.clone(),
        disposition: ReportDisposition::Completed,
        summary: "bounded research completed".to_string(),
        findings: vec!["fake-daemon synthesized one bounded research turn".to_string()],
        blockers: vec![],
        questions: vec![],
        recommended_next_actions: vec![],
        confidence: ReportConfidence::High,
        raw_output: "bounded research completed".to_string(),
        parse_result: ReportParseResult::Parsed,
        needs_supervisor_review: false,
        created_at: now,
    };
    ipc::PlanningSessionRequestResearchResponse {
        session,
        assignment,
        worker,
        worker_session,
        report,
    }
}

fn seed_cli_planning_session(
    daemon: &TestDaemon,
    session_id: &str,
    workstream_id: &str,
    workstream_label: &str,
    planning_thread_id: &str,
    objective: &str,
    ready_for_review: bool,
    include_active_plan: bool,
) {
    let mut stored: StoredState = std::fs::read_to_string(&daemon.paths.state_file)
        .map(|raw| serde_json::from_str(&raw).expect("parse stored state"))
        .unwrap_or_default();
    let now = Utc::now();
    let workstream = orcas_core::collaboration::Workstream {
        id: workstream_id.to_string(),
        title: format!("CLI {workstream_label} Root"),
        objective: format!("Objective for {workstream_label}"),
        status: orcas_core::collaboration::WorkstreamStatus::Active,
        priority: "high".to_string(),
        created_at: now,
        updated_at: now,
    };
    let active_plan = if include_active_plan {
        let active_plan = orcas_core::planning::WorkstreamPlan::bootstrap_from_workstream(
            &workstream,
            &[],
            "cli_operator",
            now,
        );
        stored
            .collaboration
            .planning
            .workstream_plans
            .insert(workstream_id.to_string(), vec![active_plan.clone()]);
        Some(active_plan)
    } else {
        None
    };
    stored.collaboration.planning_sessions.insert(
        session_id.to_string(),
        orcas_core::collaboration::PlanningSession {
            session_id: session_id.to_string(),
            workstream_id: workstream_id.to_string(),
            status: if ready_for_review {
                orcas_core::collaboration::PlanningSessionStatus::AwaitingApproval
            } else {
                orcas_core::collaboration::PlanningSessionStatus::Chatting
            },
            planning_thread_id: planning_thread_id.to_string(),
            base_plan_id: active_plan.as_ref().map(|plan| plan.plan_id.clone()),
            base_plan_version: active_plan.as_ref().map(|plan| plan.version),
            research_assignment_id: None,
            research_report_id: None,
            draft_revision_proposal_id: None,
            approved_plan_id: None,
            approved_plan_version: None,
            latest_structured_summary:
                orcas_core::collaboration::PlanningSessionStructuredSummary {
                    objective: objective.to_string(),
                    research_status:
                        orcas_core::collaboration::PlanningSessionResearchStatus::NotRequested,
                    requirements: Vec::new(),
                    constraints: Vec::new(),
                    non_goals: Vec::new(),
                    open_questions: Vec::new(),
                    draft_plan_summary: None,
                    ready_for_review,
                },
            created_at: now,
            created_by: "cli_operator".to_string(),
            updated_at: now,
            updated_by: "cli_operator".to_string(),
            request_note: Some("CLI planning session fixture".to_string()),
            reviewed_at: None,
            reviewed_by: None,
            review_note: None,
            superseded_by_session_id: None,
        },
    );
    std::fs::write(
        &daemon.paths.state_file,
        serde_json::to_string_pretty(&stored).expect("serialize stored state"),
    )
    .expect("write state file");
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
    let fixture = AuthorityFixture::new();
    let workstream_id = authority::WorkstreamId::new().to_string();
    let work_unit_id = authority::WorkUnitId::new().to_string();
    let workstream = create_authority_workstream(
        &daemon,
        &fixture,
        &workstream_id,
        &format!("{test_name} root"),
    )
    .await;
    let work_unit = create_authority_workunit(
        &daemon,
        &fixture,
        &work_unit_id,
        &workstream.id,
        &format!("{test_name} unit"),
    )
    .await;
    let started = client
        .assignment_start(&ipc::AssignmentStartRequest {
            work_unit_id: work_unit.id.to_string(),
            worker_id: format!("worker-{test_name}"),
            worker_kind: Some("codex".to_string()),
            instructions: Some("Run the bounded report path.".to_string()),
            model: None,
            cwd: None,
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: orcas_core::planning::PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
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
    let fixture = AuthorityFixture::new();
    let workstream_id = authority::WorkstreamId::new().to_string();
    let work_unit_id = authority::WorkUnitId::new().to_string();
    let workstream = create_authority_workstream(
        &daemon,
        &fixture,
        &workstream_id,
        &format!("{test_name} root"),
    )
    .await;
    let work_unit = create_authority_workunit(
        &daemon,
        &fixture,
        &work_unit_id,
        &workstream.id,
        &format!("{test_name} unit"),
    )
    .await;
    let started = client
        .assignment_start(&ipc::AssignmentStartRequest {
            work_unit_id: work_unit.id.to_string(),
            worker_id: format!("worker-{test_name}"),
            worker_kind: Some("codex".to_string()),
            instructions: Some("Run the bounded proposal path.".to_string()),
            model: None,
            cwd: None,
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: orcas_core::planning::PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
        })
        .await
        .expect("start bounded proposal assignment");

    (fake_codex, fake_supervisor, daemon, started)
}

async fn spawn_codex_review_ready_daemon(
    test_name: &str,
) -> (
    FakeCodexAppServer,
    TestDaemon,
    ipc::CodexAssignmentCreateResponse,
) {
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
    let fixture = AuthorityFixture::new();
    let workstream = create_authority_workstream(
        &daemon,
        &fixture,
        &authority::WorkstreamId::new().to_string(),
        &format!("{test_name} root"),
    )
    .await;
    let work_unit = create_authority_workunit(
        &daemon,
        &fixture,
        &authority::WorkUnitId::new().to_string(),
        &workstream.id,
        &format!("{test_name} unit"),
    )
    .await;
    let thread = client
        .thread_start(&ipc::ThreadStartRequest {
            cwd: None,
            model: None,
            ephemeral: false,
        })
        .await
        .expect("start thread")
        .thread;
    let assignment = client
        .codex_assignment_create(&ipc::CodexAssignmentCreateRequest {
            codex_thread_id: thread.id,
            workstream_id: workstream.id.to_string(),
            work_unit_id: work_unit.id.to_string(),
            supervisor_id: "cli_operator".to_string(),
            assigned_by: "cli_operator".to_string(),
            send_policy: None,
            notes: Some("Seed one review queue item".to_string()),
        })
        .await
        .expect("create codex assignment");

    (fake_codex, daemon, assignment)
}

async fn pending_review_decision_id(daemon: &TestDaemon, assignment_id: &str) -> String {
    let client = daemon.connect().await;
    let decisions = client
        .supervisor_decision_list(&ipc::SupervisorDecisionListRequest {
            assignment_id: Some(assignment_id.to_string()),
            actionable_only: true,
            ..Default::default()
        })
        .await
        .expect("list seeded supervisor decisions");
    assert_eq!(decisions.decisions.len(), 1);
    decisions.decisions[0].decision_id.clone()
}

fn create_proposal_via_cli(daemon: &TestDaemon, started: &ipc::AssignmentStartResponse) -> String {
    let create_output = run_orcas(
        daemon,
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
    proposal_id
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
async fn real_cli_doctor_reports_current_runtime_and_persistence_paths() {
    let mut daemon = TestDaemon::spawn("cli-doctor").await;

    let output = run_orcas(&daemon, &["doctor"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let stdout = stdout(&output);
    assert_eq!(
        field_value(&stdout, "config"),
        Some(daemon.paths.config_file.to_string_lossy().as_ref())
    );
    assert_eq!(
        field_value(&stdout, "state"),
        Some(daemon.paths.state_file.to_string_lossy().as_ref())
    );
    assert_eq!(
        field_value(&stdout, "state_db"),
        Some(daemon.paths.state_db_file.to_string_lossy().as_ref())
    );
    assert_eq!(
        field_value(&stdout, "runtime_dir"),
        Some(daemon.paths.runtime_dir.to_string_lossy().as_ref())
    );
    assert_eq!(field_value(&stdout, "daemon_running"), Some("true"));
    assert!(field_value(&stdout, "codex_endpoint").is_some());
    assert!(field_value(&stdout, "connection_mode").is_some());

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_observe_hierarchy_via_workstream_get() {
    let mut daemon = TestDaemon::spawn("cli-workstream-get").await;
    let fixture = AuthorityFixture::new();
    let workstream =
        create_authority_workstream(&daemon, &fixture, "cli-workstream-get-root", "CLI Root").await;
    let work_unit = create_authority_workunit(
        &daemon,
        &fixture,
        "cli-workstream-get-unit",
        &workstream.id,
        "CLI Unit",
    )
    .await;

    let output = run_orcas(
        &daemon,
        &[
            "workstreams",
            "get",
            "--workstream",
            &workstream.id.to_string(),
        ],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let stdout = stdout(&output);
    assert!(stdout.contains("surface: authority"));
    assert!(stdout.contains(&format!("workstream_id: {}", workstream.id)));
    assert!(stdout.contains("title: CLI Root"));
    assert!(stdout.contains("objective: Objective for CLI Root"));
    assert!(stdout.contains("priority: high"));
    assert!(stdout.contains("status: Active"));
    assert!(stdout.contains("revision: 1"));
    assert!(stdout.contains(&format!("origin_node_id: {}", fixture.origin_node_id)));
    assert!(stdout.contains("work_units: 1"));
    assert!(stdout.contains(&format!(
        "work_unit\t{}\trev=1\tReady\tCLI Unit",
        work_unit.id
    )));
    assert!(stdout.contains("CLI Unit"));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_read_workunit_detail_after_real_setup() {
    let mut daemon = TestDaemon::spawn("cli-workunit-get").await;
    let fixture = AuthorityFixture::new();
    let workstream = create_authority_workstream(
        &daemon,
        &fixture,
        "cli-workunit-get-root",
        "CLI Detail Root",
    )
    .await;
    let work_unit = create_authority_workunit(
        &daemon,
        &fixture,
        "cli-workunit-get-unit",
        &workstream.id,
        "CLI Detail Unit",
    )
    .await;
    let tracked_thread = create_authority_tracked_thread(
        &daemon,
        &fixture,
        "cli-workunit-get-thread",
        &work_unit.id,
        "CLI Detail Thread",
    )
    .await;

    let output = run_orcas(
        &daemon,
        &["workunits", "get", "--workunit", &work_unit.id.to_string()],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let stdout = stdout(&output);
    assert!(stdout.contains("surface: authority"));
    assert!(stdout.contains(&format!("work_unit_id: {}", work_unit.id)));
    assert!(stdout.contains(&format!("workstream_id: {}", workstream.id)));
    assert!(stdout.contains("title: CLI Detail Unit"));
    assert!(stdout.contains("task_statement: Task for CLI Detail Unit"));
    assert!(stdout.contains("status: Ready"));
    assert!(stdout.contains("revision: 1"));
    assert!(stdout.contains(&format!("origin_node_id: {}", fixture.origin_node_id)));
    assert!(stdout.contains("tracked_threads: 1"));
    assert!(stdout.contains(&format!(
        "tracked_thread\t{}\trev=1\tCodex\tBound\tCLI Detail Thread",
        tracked_thread.id
    )));

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
    assert!(create_stdout.contains("surface: authority"));
    assert!(create_stdout.contains("revision: 1"));
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
    assert!(get_stdout.contains("surface: authority"));
    assert!(get_stdout.contains(&format!("workstream_id: {workstream_id}")));
    assert!(get_stdout.contains("title: CLI Created Root"));
    assert!(get_stdout.contains("objective: Create a workstream entirely through the CLI."));
    assert!(get_stdout.contains("status: Active"));
    assert!(get_stdout.contains("priority: high"));
    assert!(get_stdout.contains("revision: 1"));
    assert!(get_stdout.contains("origin_node_id: orcas-cli"));
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
    assert!(create_workunit_stdout.contains("surface: authority"));
    assert!(create_workunit_stdout.contains("revision: 1"));
    assert!(create_workunit_stdout.contains("status: Ready"));

    let get_workunit = run_orcas(&daemon, &["workunits", "get", "--workunit", &work_unit_id]);
    assert!(
        get_workunit.status.success(),
        "stderr: {}",
        stderr(&get_workunit)
    );
    let get_workunit_stdout = stdout(&get_workunit);
    assert!(get_workunit_stdout.contains("surface: authority"));
    assert!(get_workunit_stdout.contains(&format!("work_unit_id: {work_unit_id}")));
    assert!(get_workunit_stdout.contains(&format!("workstream_id: {workstream_id}")));
    assert!(get_workunit_stdout.contains("title: CLI Created Unit"));
    assert!(
        get_workunit_stdout.contains(
            "task_statement: Create and inspect this work unit entirely through the CLI."
        )
    );
    assert!(get_workunit_stdout.contains("status: Ready"));
    assert!(get_workunit_stdout.contains("revision: 1"));
    assert!(get_workunit_stdout.contains("origin_node_id: orcas-cli"));
    assert!(get_workunit_stdout.contains("tracked_threads: 0"));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_edit_and_delete_authority_workstream() {
    let mut daemon = TestDaemon::spawn("cli-workstream-edit-delete").await;

    let create_output = run_orcas(
        &daemon,
        &[
            "workstreams",
            "create",
            "--title",
            "CLI Edit Root",
            "--objective",
            "Create a workstream for edit and delete coverage.",
        ],
    );
    assert!(
        create_output.status.success(),
        "stderr: {}",
        stderr(&create_output)
    );
    let workstream_id = field_value(&stdout(&create_output), "workstream_id")
        .expect("workstream create should print workstream_id")
        .to_string();

    let edit_output = run_orcas(
        &daemon,
        &[
            "workstreams",
            "edit",
            "--workstream",
            &workstream_id,
            "--title",
            "CLI Edited Root",
            "--priority",
            "urgent",
            "--status",
            "blocked",
        ],
    );
    assert!(
        edit_output.status.success(),
        "stderr: {}",
        stderr(&edit_output)
    );
    let edit_stdout = stdout(&edit_output);
    assert!(edit_stdout.contains("surface: authority"));
    assert!(edit_stdout.contains(&format!("workstream_id: {workstream_id}")));
    assert!(edit_stdout.contains("revision: 2"));
    assert!(edit_stdout.contains("status: Blocked"));

    let client = daemon.connect().await;
    let hierarchy = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest::default())
        .await
        .expect("authority hierarchy after workstream edit");
    let stored = hierarchy
        .hierarchy
        .workstreams
        .iter()
        .find(|workstream| workstream.workstream.id.to_string() == workstream_id)
        .expect("edited workstream in hierarchy");
    assert_eq!(stored.workstream.title, "CLI Edited Root");
    assert_eq!(stored.workstream.priority, "urgent");
    assert_eq!(
        stored.workstream.status,
        orcas_core::WorkstreamStatus::Blocked
    );

    let delete_output = run_orcas(
        &daemon,
        &["workstreams", "delete", "--workstream", &workstream_id],
    );
    assert!(
        delete_output.status.success(),
        "stderr: {}",
        stderr(&delete_output)
    );
    let delete_stdout = stdout(&delete_output);
    assert!(delete_stdout.contains("surface: authority"));
    assert!(delete_stdout.contains(&format!("workstream_id: {workstream_id}")));
    assert!(delete_stdout.contains("deleted: true"));

    let hierarchy = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest::default())
        .await
        .expect("authority hierarchy after workstream delete");
    assert!(
        hierarchy
            .hierarchy
            .workstreams
            .iter()
            .all(|workstream| workstream.workstream.id.to_string() != workstream_id)
    );

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_can_create_edit_and_delete_tracked_thread_via_canonical_cli() {
    let mut daemon = TestDaemon::spawn("cli-tracked-thread-crud").await;

    let create_workstream = run_orcas(
        &daemon,
        &[
            "workstreams",
            "create",
            "--title",
            "CLI Tracked Thread Root",
            "--objective",
            "Create a workstream for tracked-thread CLI coverage.",
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
            "CLI Tracked Thread Unit",
            "--task",
            "Create a work unit for tracked-thread CRUD coverage.",
        ],
    );
    assert!(
        create_workunit.status.success(),
        "stderr: {}",
        stderr(&create_workunit)
    );
    let work_unit_id = field_value(&stdout(&create_workunit), "work_unit_id")
        .expect("workunit create should print work_unit_id")
        .to_string();

    let create_tracked_thread = run_orcas(
        &daemon,
        &[
            "tracked-threads",
            "create",
            "--workunit",
            &work_unit_id,
            "--title",
            "CLI Tracked Thread",
            "--root-dir",
            "/tmp/orcas-cli",
            "--notes",
            "Track this Codex binding through the canonical CLI surface.",
            "--upstream-thread",
            "thread-upstream-1",
            "--model",
            "gpt-5.4",
        ],
    );
    assert!(
        create_tracked_thread.status.success(),
        "stderr: {}",
        stderr(&create_tracked_thread)
    );
    let create_stdout = stdout(&create_tracked_thread);
    let tracked_thread_id = field_value(&create_stdout, "tracked_thread_id")
        .expect("tracked thread create should print tracked_thread_id")
        .to_string();
    assert!(create_stdout.contains("surface: authority"));
    assert!(create_stdout.contains("revision: 1"));
    assert!(create_stdout.contains("binding_state: Bound"));

    let get_output = run_orcas(
        &daemon,
        &[
            "tracked-threads",
            "get",
            "--tracked-thread",
            &tracked_thread_id,
        ],
    );
    assert!(
        get_output.status.success(),
        "stderr: {}",
        stderr(&get_output)
    );
    let get_stdout = stdout(&get_output);
    assert!(get_stdout.contains("surface: authority"));
    assert!(get_stdout.contains(&format!("tracked_thread_id: {tracked_thread_id}")));
    assert!(get_stdout.contains(&format!("work_unit_id: {work_unit_id}")));
    assert!(get_stdout.contains("title: CLI Tracked Thread"));
    assert!(get_stdout.contains("backend_kind: Codex"));
    assert!(get_stdout.contains("binding_state: Bound"));
    assert!(get_stdout.contains("preferred_cwd: /tmp/orcas-cli"));
    assert!(get_stdout.contains("upstream_thread_id: thread-upstream-1"));
    assert!(get_stdout.contains("preferred_model: gpt-5.4"));

    let edit_output = run_orcas(
        &daemon,
        &[
            "tracked-threads",
            "edit",
            "--tracked-thread",
            &tracked_thread_id,
            "--title",
            "CLI Tracked Thread Updated",
            "--binding-state",
            "bound",
            "--model",
            "gpt-5.5",
        ],
    );
    assert!(
        edit_output.status.success(),
        "stderr: {}",
        stderr(&edit_output)
    );
    let edit_stdout = stdout(&edit_output);
    assert!(edit_stdout.contains("surface: authority"));
    assert!(edit_stdout.contains(&format!("tracked_thread_id: {tracked_thread_id}")));
    assert!(edit_stdout.contains("revision: 2"));
    assert!(edit_stdout.contains("binding_state: Bound"));

    let client = daemon.connect().await;
    let tracked_thread = client
        .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
            tracked_thread_id: authority::TrackedThreadId::parse(tracked_thread_id.clone())
                .expect("tracked thread id"),
        })
        .await
        .expect("authority tracked thread get after edit")
        .tracked_thread;
    assert_eq!(tracked_thread.title, "CLI Tracked Thread Updated");
    assert_eq!(
        tracked_thread.binding_state,
        authority::TrackedThreadBindingState::Bound
    );
    assert_eq!(tracked_thread.preferred_model.as_deref(), Some("gpt-5.5"));

    let delete_output = run_orcas(
        &daemon,
        &[
            "tracked-threads",
            "delete",
            "--tracked-thread",
            &tracked_thread_id,
        ],
    );
    assert!(
        delete_output.status.success(),
        "stderr: {}",
        stderr(&delete_output)
    );
    let delete_stdout = stdout(&delete_output);
    assert!(delete_stdout.contains("surface: authority"));
    assert!(delete_stdout.contains(&format!("tracked_thread_id: {tracked_thread_id}")));
    assert!(delete_stdout.contains("deleted: true"));

    let hierarchy = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest::default())
        .await
        .expect("authority hierarchy after tracked thread delete");
    assert!(
        hierarchy
            .hierarchy
            .workstreams
            .iter()
            .flat_map(|workstream| workstream.work_units.iter())
            .flat_map(|work_unit| work_unit.tracked_threads.iter())
            .all(|tracked_thread| tracked_thread.id.to_string() != tracked_thread_id)
    );

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
async fn real_cli_planning_session_approve_stages_revision_proposal_without_applying_plan() {
    let _guard = planning_session_cli_test_lock().lock().await;
    let (_fake_codex, mut daemon) = spawn_planning_session_cli_daemon("cli-planning-approve").await;
    let workstream_label = "planning approve";
    let workstream_id = create_cli_workstream(&daemon, workstream_label);
    daemon.stop().await;
    let session_id = "planning_session_cli_approve";
    seed_cli_planning_session(
        &daemon,
        session_id,
        &workstream_id,
        workstream_label,
        "planning-thread-cli-approve",
        "Plan one bounded change.",
        true,
        true,
    );
    daemon.start().await;

    let approve_output = run_orcas(
        &daemon,
        &[
            "planning-sessions",
            "approve",
            "--session",
            &session_id,
            "--approved-by",
            "cli_operator",
            "--review-note",
            "Stage the revision proposal only",
        ],
    );
    assert!(
        approve_output.status.success(),
        "stderr: {}",
        stderr(&approve_output)
    );

    let approve_stdout = stdout(&approve_output);
    assert!(approve_stdout.contains("surface: planning_session"));
    assert!(approve_stdout.contains("planning_session_status: Approved"));
    assert!(approve_stdout.contains("planning_session_draft_revision_proposal_id:"));
    assert!(approve_stdout.contains("planning_session_approved_plan_id:"));
    assert!(approve_stdout.contains("planning_revision_proposal_id:"));
    assert!(approve_stdout.contains("planning_revision_proposal_status: Pending"));
    assert!(approve_stdout.contains(
        "planning_session_approval_effect: staged_revision_proposal_only; apply it through the existing plan revision approval path"
    ));

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_planning_session_request_research_succeeds_once_and_rejects_repeat() {
    let _guard = planning_session_cli_test_lock().lock().await;
    let daemon = spawn_fake_planning_session_cli_daemon("cli-planning-research").await;
    let session_id = "planning_session_cli_research";
    let envs = daemon.xdg_env();

    let first_output = run_orcas_with_env(
        &[
            "planning-sessions",
            "request-research",
            "--session",
            &session_id,
            "--worker",
            "worker-research",
            "--worker-kind",
            "codex",
            "--requested-by",
            "cli_operator",
            "--request-note",
            "Need one bounded research turn",
        ],
        &envs,
    );
    assert!(
        first_output.status.success(),
        "stderr: {}",
        stderr(&first_output)
    );
    let first_stdout = stdout(&first_output);
    assert!(first_stdout.contains("surface: planning_session"));
    assert!(first_stdout.contains("planning_session_research_assignment_id:"));
    assert!(first_stdout.contains("planning_session_research_report_id:"));
    assert!(first_stdout.contains("research_assignment_id:"));
    assert!(first_stdout.contains("research_report_id:"));
    assert!(first_stdout.contains(
        "planning_session_research_effect: bounded_research_turn_requested; repeated requests for this session will be rejected"
    ));

    let second_output = run_orcas_with_env(
        &[
            "planning-sessions",
            "request-research",
            "--session",
            &session_id,
            "--worker",
            "worker-research-2",
            "--worker-kind",
            "codex",
            "--requested-by",
            "cli_operator",
            "--request-note",
            "Should be rejected",
        ],
        &envs,
    );
    assert!(!second_output.status.success());
    assert!(stdout(&second_output).is_empty());
    assert!(
        stderr(&second_output).contains("already used its bounded research turn"),
        "stderr: {}",
        stderr(&second_output)
    );

    daemon.stop().await;
}

#[tokio::test]
async fn real_cli_planning_session_help_mentions_lifecycle_boundaries() {
    let _guard = planning_session_cli_test_lock().lock().await;
    let mut daemon = TestDaemon::spawn("cli-planning-help").await;

    let root_help = run_orcas(&daemon, &["planning-sessions", "--help"]);
    assert!(root_help.status.success(), "stderr: {}", stderr(&root_help));
    let root_stdout = stdout(&root_help);
    assert!(root_stdout.contains("Supervisor-owned planning session orchestration"));
    assert!(root_stdout.contains("approve"));
    assert!(root_stdout.contains("request-research"));
    assert!(root_stdout.contains("update-summary"));
    assert!(root_stdout.contains("mark-ready-for-review"));

    let approve_help = run_orcas(&daemon, &["planning-sessions", "approve", "--help"]);
    assert!(
        approve_help.status.success(),
        "stderr: {}",
        stderr(&approve_help)
    );
    let approve_stdout = stdout(&approve_help);
    assert!(
        approve_stdout
            .contains("Stage a canonical plan revision proposal from the session summary")
    );

    let research_help = run_orcas(
        &daemon,
        &["planning-sessions", "request-research", "--help"],
    );
    assert!(
        research_help.status.success(),
        "stderr: {}",
        stderr(&research_help)
    );
    let research_stdout = stdout(&research_help);
    assert!(research_stdout.contains("bounded one-turn research assignment"));

    let update_help = run_orcas(&daemon, &["planning-sessions", "update-summary", "--help"]);
    assert!(
        update_help.status.success(),
        "stderr: {}",
        stderr(&update_help)
    );
    let update_stdout = stdout(&update_help);
    assert!(update_stdout.contains("descriptive planning summary without changing approval state"));

    let ready_help = run_orcas(
        &daemon,
        &["planning-sessions", "mark-ready-for-review", "--help"],
    );
    assert!(
        ready_help.status.success(),
        "stderr: {}",
        stderr(&ready_help)
    );
    let ready_stdout = stdout(&ready_help);
    assert!(ready_stdout.contains("chat session into awaiting-approval"));

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
    assert!(workunit_stdout.contains("surface: authority"));
    assert!(workunit_stdout.contains(&format!("work_unit_id: {}", started.report.work_unit_id)));
    assert!(workunit_stdout.contains("status: "));
    assert!(workunit_stdout.contains("tracked_threads: 0"));

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

    let proposal_id = create_proposal_via_cli(&daemon, &started);

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

#[tokio::test(flavor = "multi_thread")]
async fn real_cli_can_discover_pending_review_item_via_queue_and_history() {
    let (_fake_codex, mut daemon, assignment) =
        spawn_codex_review_ready_daemon("cli-review-queue").await;
    let pending_decision_id =
        pending_review_decision_id(&daemon, &assignment.assignment.assignment_id).await;

    let queue_output = run_orcas(
        &daemon,
        &[
            "codex",
            "review",
            "queue",
            "--assignment",
            &assignment.assignment.assignment_id,
        ],
    );
    assert!(
        queue_output.status.success(),
        "stderr: {}",
        stderr(&queue_output)
    );
    let queue_stdout = stdout(&queue_output);
    assert!(queue_stdout.contains(&pending_decision_id));
    assert!(queue_stdout.contains("ProposedToHuman"));
    assert!(queue_stdout.contains("NextTurn/Bootstrap"));
    assert!(queue_stdout.contains(&format!(
        "assignment={}",
        assignment.assignment.assignment_id
    )));
    assert!(queue_stdout.contains(&format!("wu={}", assignment.assignment.work_unit_id)));

    let history_output = run_orcas(
        &daemon,
        &[
            "codex",
            "review",
            "history",
            "--assignment",
            &assignment.assignment.assignment_id,
        ],
    );
    assert!(
        history_output.status.success(),
        "stderr: {}",
        stderr(&history_output)
    );
    let history_stdout = stdout(&history_output);
    assert!(history_stdout.contains(&format!(
        "history_for_assignment: {}",
        assignment.assignment.assignment_id
    )));
    assert!(history_stdout.contains(&pending_decision_id));
    assert!(history_stdout.contains("NextTurn/Bootstrap"));
    assert!(history_stdout.contains("ProposedToHuman"));

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn real_cli_can_fetch_pending_review_item_detail_via_get() {
    let (_fake_codex, mut daemon, assignment) =
        spawn_codex_review_ready_daemon("cli-review-get").await;
    let pending_decision_id =
        pending_review_decision_id(&daemon, &assignment.assignment.assignment_id).await;

    let get_output = run_orcas(
        &daemon,
        &["codex", "review", "get", "--decision", &pending_decision_id],
    );
    assert!(
        get_output.status.success(),
        "stderr: {}",
        stderr(&get_output)
    );

    let get_stdout = stdout(&get_output);
    assert!(get_stdout.contains(&format!("decision_id: {}", pending_decision_id)));
    assert!(get_stdout.contains(&format!(
        "assignment_id: {}",
        assignment.assignment.assignment_id
    )));
    assert!(get_stdout.contains("kind: NextTurn"));
    assert!(get_stdout.contains("proposal_kind: Bootstrap"));
    assert!(get_stdout.contains("status: ProposedToHuman"));
    assert!(get_stdout.contains("actionable: yes"));
    assert!(get_stdout.contains(&format!(
        "workstream_id: {}",
        assignment.assignment.workstream_id
    )));
    assert!(get_stdout.contains(&format!(
        "work_unit_id: {}",
        assignment.assignment.work_unit_id
    )));
    assert!(get_stdout.contains("related_history:"));
    assert!(get_stdout.contains(&pending_decision_id));

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn real_cli_can_approve_pending_review_item() {
    let (_fake_codex, mut daemon, assignment) =
        spawn_codex_review_ready_daemon("cli-review-approve").await;
    let pending_decision_id =
        pending_review_decision_id(&daemon, &assignment.assignment.assignment_id).await;

    let approve_output = run_orcas(
        &daemon,
        &[
            "codex",
            "review",
            "approve",
            "--decision",
            &pending_decision_id,
            "--reviewed-by",
            "cli_operator",
            "--review-note",
            "Approve the bounded pending review item",
        ],
    );
    assert!(
        approve_output.status.success(),
        "stderr: {}",
        stderr(&approve_output)
    );

    let approve_stdout = stdout(&approve_output);
    assert!(approve_stdout.contains(&format!("decision_id: {}", pending_decision_id)));
    assert!(approve_stdout.contains(&format!(
        "assignment_id: {}",
        assignment.assignment.assignment_id
    )));
    assert!(approve_stdout.contains("kind: NextTurn"));
    assert!(approve_stdout.contains("proposal_kind: Bootstrap"));
    assert!(approve_stdout.contains("status: Sent"));
    assert!(approve_stdout.contains("actionable: no"));
    assert!(approve_stdout.contains("approved_at:"));
    assert!(approve_stdout.contains("sent_at:"));

    let get_output = run_orcas(
        &daemon,
        &["codex", "review", "get", "--decision", &pending_decision_id],
    );
    assert!(
        get_output.status.success(),
        "stderr: {}",
        stderr(&get_output)
    );

    let get_stdout = stdout(&get_output);
    assert!(get_stdout.contains(&format!("decision_id: {}", pending_decision_id)));
    assert!(get_stdout.contains("status: Sent"));
    assert!(get_stdout.contains("actionable: no"));
    assert!(get_stdout.contains("approved_at:"));
    assert!(get_stdout.contains("sent_at:"));
    assert!(get_stdout.contains("related_history:"));

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn real_cli_can_reject_pending_review_item() {
    let (_fake_codex, mut daemon, assignment) =
        spawn_codex_review_ready_daemon("cli-review-reject").await;
    let pending_decision_id =
        pending_review_decision_id(&daemon, &assignment.assignment.assignment_id).await;

    let reject_output = run_orcas(
        &daemon,
        &[
            "codex",
            "review",
            "reject",
            "--decision",
            &pending_decision_id,
            "--reviewed-by",
            "cli_operator",
            "--review-note",
            "Reject the bounded pending review item",
        ],
    );
    assert!(
        reject_output.status.success(),
        "stderr: {}",
        stderr(&reject_output)
    );

    let reject_stdout = stdout(&reject_output);
    assert!(reject_stdout.contains(&format!("decision_id: {}", pending_decision_id)));
    assert!(reject_stdout.contains(&format!(
        "assignment_id: {}",
        assignment.assignment.assignment_id
    )));
    assert!(reject_stdout.contains("kind: NextTurn"));
    assert!(reject_stdout.contains("proposal_kind: Bootstrap"));
    assert!(reject_stdout.contains("status: Rejected"));
    assert!(reject_stdout.contains("actionable: no"));
    assert!(reject_stdout.contains("rejected_at:"));

    let get_output = run_orcas(
        &daemon,
        &["codex", "review", "get", "--decision", &pending_decision_id],
    );
    assert!(
        get_output.status.success(),
        "stderr: {}",
        stderr(&get_output)
    );

    let get_stdout = stdout(&get_output);
    assert!(get_stdout.contains(&format!("decision_id: {}", pending_decision_id)));
    assert!(get_stdout.contains("status: Rejected"));
    assert!(get_stdout.contains("actionable: no"));
    assert!(get_stdout.contains("rejected_at:"));
    assert!(get_stdout.contains("related_history:"));

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn real_cli_review_queue_and_history_reflect_approval_transition() {
    let (_fake_codex, mut daemon, assignment) =
        spawn_codex_review_ready_daemon("cli-review-transition").await;
    let pending_decision_id =
        pending_review_decision_id(&daemon, &assignment.assignment.assignment_id).await;

    let queue_before = run_orcas(
        &daemon,
        &[
            "codex",
            "review",
            "queue",
            "--assignment",
            &assignment.assignment.assignment_id,
        ],
    );
    assert!(
        queue_before.status.success(),
        "stderr: {}",
        stderr(&queue_before)
    );
    let queue_before_stdout = stdout(&queue_before);
    assert!(queue_before_stdout.contains(&pending_decision_id));
    assert!(queue_before_stdout.contains("ProposedToHuman"));

    let approve_output = run_orcas(
        &daemon,
        &[
            "codex",
            "review",
            "approve",
            "--decision",
            &pending_decision_id,
            "--reviewed-by",
            "cli_operator",
            "--review-note",
            "Approve to clear the actionable queue",
        ],
    );
    assert!(
        approve_output.status.success(),
        "stderr: {}",
        stderr(&approve_output)
    );

    let queue_after = run_orcas(
        &daemon,
        &[
            "codex",
            "review",
            "queue",
            "--assignment",
            &assignment.assignment.assignment_id,
        ],
    );
    assert!(
        queue_after.status.success(),
        "stderr: {}",
        stderr(&queue_after)
    );
    let queue_after_stdout = stdout(&queue_after);
    assert!(!queue_after_stdout.contains(&pending_decision_id));

    let history_after = run_orcas(
        &daemon,
        &[
            "codex",
            "review",
            "history",
            "--assignment",
            &assignment.assignment.assignment_id,
        ],
    );
    assert!(
        history_after.status.success(),
        "stderr: {}",
        stderr(&history_after)
    );
    let history_after_stdout = stdout(&history_after);
    assert!(history_after_stdout.contains(&format!(
        "history_for_assignment: {}",
        assignment.assignment.assignment_id
    )));
    assert!(history_after_stdout.contains(&pending_decision_id));
    assert!(history_after_stdout.contains("Sent"));
    assert!(history_after_stdout.contains("NextTurn/Bootstrap"));

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn real_cli_reports_missing_review_item_with_nonzero_exit() {
    let (_fake_codex, mut daemon, _assignment) =
        spawn_codex_review_ready_daemon("cli-review-missing").await;

    let output = run_orcas(
        &daemon,
        &["codex", "review", "get", "--decision", "missing-decision"],
    );
    assert!(!output.status.success());
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).contains("unknown supervisor decision `missing-decision`"),
        "stderr: {}",
        stderr(&output)
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn real_cli_can_create_list_and_reject_proposal_after_real_assignment_setup() {
    let (_fake_codex, _fake_supervisor, mut daemon, started) =
        spawn_proposal_ready_daemon("cli-proposal-reject").await;

    let proposal_id = create_proposal_via_cli(&daemon, &started);

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

    let reject_output = run_orcas(
        &daemon,
        &[
            "proposals",
            "reject",
            "--proposal",
            &proposal_id,
            "--reviewed-by",
            "cli_operator",
            "--review-note",
            "Reject the bounded fake supervisor proposal",
        ],
    );
    assert!(
        reject_output.status.success(),
        "stderr: {}",
        stderr(&reject_output)
    );
    let reject_stdout = stdout(&reject_output);
    assert!(reject_stdout.contains(&format!("proposal_id: {proposal_id}")));
    assert!(reject_stdout.contains("status: Rejected"));
    assert!(reject_stdout.contains("reviewed_by: cli_operator"));
    assert!(reject_stdout.contains("review_note: Reject the bounded fake supervisor proposal"));

    let get_output = run_orcas(&daemon, &["proposals", "get", "--proposal", &proposal_id]);
    assert!(
        get_output.status.success(),
        "stderr: {}",
        stderr(&get_output)
    );
    let get_stdout = stdout(&get_output);
    assert!(get_stdout.contains(&format!("proposal_id: {proposal_id}")));
    assert!(get_stdout.contains("status: Rejected"));
    assert!(get_stdout.contains("reviewed_by: cli_operator"));

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn real_cli_rejects_approving_non_open_proposal_with_nonzero_exit() {
    let (_fake_codex, _fake_supervisor, mut daemon, started) =
        spawn_proposal_ready_daemon("cli-proposal-closed").await;

    let proposal_id = create_proposal_via_cli(&daemon, &started);

    let reject_output = run_orcas(
        &daemon,
        &[
            "proposals",
            "reject",
            "--proposal",
            &proposal_id,
            "--reviewed-by",
            "cli_operator",
            "--review-note",
            "Close the proposal before the negative case",
        ],
    );
    assert!(
        reject_output.status.success(),
        "stderr: {}",
        stderr(&reject_output)
    );

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
            "This should fail because the proposal is already closed",
        ],
    );
    assert!(!approve_output.status.success());
    assert!(stdout(&approve_output).is_empty());
    assert!(
        stderr(&approve_output).contains("is not open and cannot be approved"),
        "stderr: {}",
        stderr(&approve_output)
    );

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
    assert!(stderr(&output).contains("missing-workunit"));

    daemon.stop().await;
}
