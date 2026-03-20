#![allow(unused_crate_dependencies)]

mod fake_codex;
mod harness;

use chrono::Utc;
use tokio::time::{Duration, Instant, sleep};

use fake_codex::FakeCodexAppServer;
use harness::TestDaemon;
use orcas_core::authority::{self, CommandActor, CommandMetadata, CorrelationId, OriginNodeId};
use orcas_core::{AssignmentStatus, WorkUnitStatus, WorkstreamStatus, ipc};

struct AuthorityFixture {
    origin_node_id: OriginNodeId,
    actor: CommandActor,
}

impl AuthorityFixture {
    fn new() -> Self {
        Self {
            origin_node_id: OriginNodeId::new(),
            actor: CommandActor::parse("integration_test").expect("command actor"),
        }
    }

    fn metadata(&self, label: &str) -> CommandMetadata {
        CommandMetadata {
            command_id: authority::CommandId::new(),
            issued_at: Utc::now(),
            origin_node_id: self.origin_node_id.clone(),
            actor: self.actor.clone(),
            correlation_id: Some(
                CorrelationId::parse(format!("corr-{label}")).expect("correlation id"),
            ),
        }
    }
}

async fn create_authority_workstream(
    client: &orcasd::OrcasIpcClient,
    fixture: &AuthorityFixture,
    workstream_id: &str,
    title: &str,
) -> authority::WorkstreamRecord {
    client
        .authority_workstream_create(&ipc::AuthorityWorkstreamCreateRequest {
            command: authority::CreateWorkstream {
                metadata: fixture.metadata("assignment-ws-create"),
                workstream_id: authority::WorkstreamId::parse(workstream_id)
                    .expect("workstream id"),
                title: title.to_string(),
                objective: format!("Objective for {title}"),
                status: WorkstreamStatus::Active,
                priority: "high".to_string(),
            },
        })
        .await
        .expect("create authority workstream")
        .workstream
}

async fn create_authority_workunit(
    client: &orcasd::OrcasIpcClient,
    fixture: &AuthorityFixture,
    work_unit_id: &str,
    workstream_id: &authority::WorkstreamId,
    title: &str,
) -> authority::WorkUnitRecord {
    client
        .authority_workunit_create(&ipc::AuthorityWorkunitCreateRequest {
            command: authority::CreateWorkUnit {
                metadata: fixture.metadata("assignment-wu-create"),
                work_unit_id: authority::WorkUnitId::parse(work_unit_id).expect("work unit id"),
                workstream_id: workstream_id.clone(),
                title: title.to_string(),
                task_statement: format!("Task for {title}"),
                status: WorkUnitStatus::Ready,
            },
        })
        .await
        .expect("create authority work unit")
        .work_unit
}

#[tokio::test]
async fn assignment_start_consumes_projected_authority_work_unit_and_updates_state() {
    let fake_codex = FakeCodexAppServer::spawn().await;
    let mut daemon = TestDaemon::spawn_with_env(
        "authority-assignment",
        vec![(
            "ORCAS_CODEX_LISTEN_URL".to_string(),
            fake_codex.endpoint.clone(),
        )],
    )
    .await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();

    let workstream = create_authority_workstream(
        &client,
        &fixture,
        "authority-assignment-ws",
        "Assignment Root",
    )
    .await;
    let work_unit = create_authority_workunit(
        &client,
        &fixture,
        "authority-assignment-wu",
        &workstream.id,
        "Assignment Unit",
    )
    .await;

    let projected_before = client
        .state_get()
        .await
        .expect("state/get before assignment start");
    assert!(
        projected_before
            .snapshot
            .collaboration
            .work_units
            .iter()
            .any(|summary| {
                summary.id == work_unit.id.as_str() && summary.status == WorkUnitStatus::Ready
            })
    );

    let started = client
        .assignment_start(&ipc::AssignmentStartRequest {
            work_unit_id: work_unit.id.to_string(),
            worker_id: "worker-1".to_string(),
            worker_kind: Some("codex".to_string()),
            instructions: Some("Handle the projected authority task".to_string()),
            model: None,
            cwd: None,
        })
        .await
        .expect("assignment start over projected authority work unit");
    assert_eq!(started.assignment.work_unit_id, work_unit.id.as_str());
    assert_eq!(
        started.assignment.status,
        AssignmentStatus::AwaitingDecision
    );
    assert_eq!(started.worker.id, "worker-1");
    assert_eq!(started.report.work_unit_id, work_unit.id.as_str());
    assert_eq!(started.report.assignment_id, started.assignment.id);

    let follow_up_client = daemon.connect().await;
    let assignment_id = started.assignment.id.clone();

    let deadline = Instant::now() + Duration::from_secs(10);
    let settled_snapshot = loop {
        let snapshot = follow_up_client
            .state_get()
            .await
            .expect("state/get while waiting for bounded assignment transition");
        let assignment_summary = snapshot
            .snapshot
            .collaboration
            .assignments
            .iter()
            .find(|summary| summary.id == assignment_id)
            .expect("assignment should remain visible");
        let work_unit_summary = snapshot
            .snapshot
            .collaboration
            .work_units
            .iter()
            .find(|summary| summary.id == work_unit.id.as_str())
            .expect("projected work unit should remain visible");
        if assignment_summary.status == AssignmentStatus::AwaitingDecision
            && work_unit_summary.status == WorkUnitStatus::AwaitingDecision
        {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for assignment to settle into awaiting decision"
        );
        sleep(Duration::from_millis(100)).await;
    };

    let assignment = follow_up_client
        .assignment_get(&ipc::AssignmentGetRequest {
            assignment_id: assignment_id.clone(),
        })
        .await
        .expect("assignment should be queryable after start");
    assert_eq!(assignment.assignment.id, assignment_id);
    assert_eq!(assignment.assignment.work_unit_id, work_unit.id.as_str());
    assert_eq!(
        assignment.assignment.status,
        AssignmentStatus::AwaitingDecision
    );
    let report = assignment
        .report
        .expect("assignment should have produced a persisted report");
    assert_eq!(report.id, started.report.id);
    assert_eq!(report.work_unit_id, work_unit.id.as_str());
    assert_eq!(report.assignment_id, started.assignment.id);

    let settled_assignment = settled_snapshot
        .snapshot
        .collaboration
        .assignments
        .iter()
        .find(|summary| summary.id == assignment_id)
        .expect("assignment should remain visible");
    assert_eq!(
        settled_assignment.status,
        AssignmentStatus::AwaitingDecision
    );
    let projected_work_unit = settled_snapshot
        .snapshot
        .collaboration
        .work_units
        .iter()
        .find(|summary| summary.id == work_unit.id.as_str())
        .expect("projected work unit should remain visible");
    assert_eq!(projected_work_unit.status, WorkUnitStatus::AwaitingDecision);
    assert_eq!(
        projected_work_unit.current_assignment_id.as_deref(),
        Some(assignment_id.as_str())
    );
    assert!(
        settled_snapshot
            .snapshot
            .recent_events
            .iter()
            .any(|event| { event.kind == "assignment" && event.message.contains(&assignment_id) })
    );
    assert!(settled_snapshot.snapshot.recent_events.iter().any(|event| {
        event.kind == "work_unit" && event.message.contains(work_unit.id.as_str())
    }));
    assert!(
        settled_snapshot
            .snapshot
            .recent_events
            .iter()
            .any(|event| { event.kind == "report" && event.message.contains(&started.report.id) })
    );

    daemon.stop().await;
}
