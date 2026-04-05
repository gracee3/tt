#![allow(warnings)]

mod fake_runtime;
mod harness;

use chrono::Utc;
use tokio::time::{Duration, Instant, sleep};

use fake_runtime::FakeTTAppServer;
use harness::TestDaemon;
use tt_core::authority::{self, CommandActor, CommandMetadata, CorrelationId, OriginNodeId};
use tt_core::{AssignmentStatus, WorkUnitStatus, WorkstreamStatus, ipc};

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
    client: &ttd::TTIpcClient,
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
                execution_scope: None,
            },
        })
        .await
        .expect("create authority workstream")
        .workstream
}

async fn create_authority_workunit(
    client: &ttd::TTIpcClient,
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

async fn delete_plan(
    client: &ttd::TTIpcClient,
    target: authority::DeleteTarget,
) -> authority::DeletePlan {
    client
        .authority_delete_plan(&ipc::AuthorityDeletePlanRequest { target })
        .await
        .expect("load authority delete plan")
        .delete_plan
}

#[tokio::test]
async fn assignment_start_bridges_authority_work_unit_into_state_and_updates_it() {
    let fake_tt = FakeTTAppServer::spawn().await;
    let mut daemon = TestDaemon::spawn_with_env(
        "authority-assignment",
        vec![(
            "TT_RUNTIME_LISTEN_URL".to_string(),
            fake_tt.endpoint.clone(),
        )],
    )
    .await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();
    let (mut events, _) = client
        .subscribe_events(false)
        .await
        .expect("subscribe to daemon events");

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
            .all(|summary| summary.id != work_unit.id.as_str())
    );

    let started = client
        .assignment_start(&ipc::AssignmentStartRequest {
            work_unit_id: work_unit.id.to_string(),
            worker_id: "worker-1".to_string(),
            worker_kind: Some("tt".to_string()),
            instructions: Some("Handle the projected authority task".to_string()),
            model: None,
            cwd: None,
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: tt_core::PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
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
    let bridged_work_unit_event = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit: summary }
                if *action == ipc::CollaborationLifecycleAction::Updated
                    && summary.id == work_unit.id.as_str()
                    && summary.source_kind
                        == ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
        )
    })
    .await;
    match bridged_work_unit_event.event {
        ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Updated);
            assert_eq!(work_unit.id, "authority-assignment-wu");
            assert_eq!(
                work_unit.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
            );
        }
        other => panic!("expected bridged work-unit lifecycle event, got {other:?}"),
    }

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
            .expect("bridged work unit should remain visible");
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
        .expect("bridged work unit should remain visible");
    let bridged_workstream = settled_snapshot
        .snapshot
        .collaboration
        .workstreams
        .iter()
        .find(|summary| summary.id == workstream.id.as_str())
        .expect("bridged workstream should remain visible");
    assert_eq!(
        bridged_workstream.source_kind,
        ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
    );
    assert_eq!(projected_work_unit.status, WorkUnitStatus::AwaitingDecision);
    assert_eq!(
        projected_work_unit.source_kind,
        ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
    );
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

#[tokio::test]
async fn deleted_authority_rows_are_hidden_from_state_even_after_assignment_bridge() {
    let fake_tt = FakeTTAppServer::spawn().await;
    let mut daemon = TestDaemon::spawn_with_env(
        "authority-assignment-delete",
        vec![(
            "TT_RUNTIME_LISTEN_URL".to_string(),
            fake_tt.endpoint.clone(),
        )],
    )
    .await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();

    let workstream = create_authority_workstream(
        &client,
        &fixture,
        "authority-assignment-delete-ws",
        "Delete Root",
    )
    .await;
    let work_unit = create_authority_workunit(
        &client,
        &fixture,
        "authority-assignment-delete-wu",
        &workstream.id,
        "Delete Unit",
    )
    .await;

    client
        .assignment_start(&ipc::AssignmentStartRequest {
            work_unit_id: work_unit.id.to_string(),
            worker_id: "worker-bridge".to_string(),
            worker_kind: Some("tt".to_string()),
            instructions: Some("Bridge then delete".to_string()),
            model: None,
            cwd: None,
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: tt_core::PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
        })
        .await
        .expect("assignment start should bridge authority work unit");

    let bridged = client.state_get().await.expect("state/get after bridge");
    assert!(
        bridged
            .snapshot
            .collaboration
            .work_units
            .iter()
            .any(|summary| {
                summary.id == work_unit.id.as_str()
                    && summary.source_kind
                        == ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
            })
    );

    let delete_plan = delete_plan(
        &client,
        authority::DeleteTarget::Workstream {
            workstream_id: workstream.id.clone(),
        },
    )
    .await;
    client
        .authority_workstream_delete(&ipc::AuthorityWorkstreamDeleteRequest {
            command: authority::DeleteWorkstream {
                metadata: fixture.metadata("assignment-ws-delete"),
                workstream_id: workstream.id.clone(),
                expected_revision: delete_plan.expected_revision,
                delete_token: delete_plan.confirmation_token,
            },
        })
        .await
        .expect("delete bridged authority workstream");

    let snapshot_after_delete = client
        .state_get()
        .await
        .expect("state/get after bridged authority delete");
    assert!(
        snapshot_after_delete
            .snapshot
            .collaboration
            .workstreams
            .iter()
            .all(|summary| summary.id != workstream.id.as_str())
    );
    assert!(
        snapshot_after_delete
            .snapshot
            .collaboration
            .work_units
            .iter()
            .all(|summary| summary.id != work_unit.id.as_str())
    );

    let hierarchy_after_delete = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest {
            include_deleted: false,
        })
        .await
        .expect("authority hierarchy after bridged delete");
    assert!(hierarchy_after_delete.hierarchy.workstreams.is_empty());

    daemon.stop().await;
}

#[tokio::test]
async fn assignment_bridge_and_authority_hierarchy_remain_coherent_after_restart() {
    let fake_tt = FakeTTAppServer::spawn().await;
    let mut daemon = TestDaemon::spawn_with_env(
        "authority-assignment-restart",
        vec![(
            "TT_RUNTIME_LISTEN_URL".to_string(),
            fake_tt.endpoint.clone(),
        )],
    )
    .await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();

    let workstream = create_authority_workstream(
        &client,
        &fixture,
        "authority-assignment-restart-ws",
        "Restart Root",
    )
    .await;
    let work_unit = create_authority_workunit(
        &client,
        &fixture,
        "authority-assignment-restart-wu",
        &workstream.id,
        "Restart Unit",
    )
    .await;

    let started = client
        .assignment_start(&ipc::AssignmentStartRequest {
            work_unit_id: work_unit.id.to_string(),
            worker_id: "worker-restart".to_string(),
            worker_kind: Some("tt".to_string()),
            instructions: Some("Bridge before restart".to_string()),
            model: None,
            cwd: None,
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: tt_core::PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
        })
        .await
        .expect("assignment start should bridge authority work unit");

    daemon.restart().await;

    let reconnected = daemon.connect().await;
    let state_after_restart = reconnected
        .state_get()
        .await
        .expect("state/get after restart");
    let bridged_workstream = state_after_restart
        .snapshot
        .collaboration
        .workstreams
        .iter()
        .find(|summary| summary.id == workstream.id.as_str())
        .expect("bridged workstream should remain visible after restart");
    assert_eq!(
        bridged_workstream.source_kind,
        ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
    );
    let bridged_work_unit = state_after_restart
        .snapshot
        .collaboration
        .work_units
        .iter()
        .find(|summary| summary.id == work_unit.id.as_str())
        .expect("bridged work unit should remain visible after restart");
    assert_eq!(
        bridged_work_unit.source_kind,
        ipc::PlanningSummarySourceKind::AuthorityCompatibilityBridge
    );
    assert_eq!(
        bridged_work_unit.current_assignment_id.as_deref(),
        Some(started.assignment.id.as_str())
    );

    let hierarchy_after_restart = reconnected
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest {
            include_deleted: false,
        })
        .await
        .expect("authority hierarchy after restart");
    let authority_workstream = hierarchy_after_restart
        .hierarchy
        .workstreams
        .iter()
        .find(|node| node.workstream.id == workstream.id)
        .expect("authority workstream should remain canonical after restart");
    assert!(
        authority_workstream
            .work_units
            .iter()
            .any(|node| node.work_unit.id == work_unit.id)
    );

    daemon.stop().await;
}
