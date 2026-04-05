#![allow(warnings)]

mod harness;

use chrono::Utc;
use std::time::Duration;
use tokio::time::timeout;

use harness::TestDaemon;
use tt_core::authority::{self, CommandActor, CommandMetadata, CorrelationId, OriginNodeId};
use tt_core::{WorkUnitStatus, WorkstreamStatus, ipc};

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
                metadata: fixture.metadata("bridge-ws-create"),
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
                metadata: fixture.metadata("bridge-wu-create"),
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

async fn create_authority_tracked_thread(
    client: &ttd::TTIpcClient,
    fixture: &AuthorityFixture,
    tracked_thread_id: &str,
    work_unit_id: &authority::WorkUnitId,
    title: &str,
) -> authority::TrackedThreadRecord {
    client
        .authority_tracked_thread_create(&ipc::AuthorityTrackedThreadCreateRequest {
            command: authority::CreateTrackedThread {
                metadata: fixture.metadata("bridge-thread-create"),
                tracked_thread_id: authority::TrackedThreadId::parse(tracked_thread_id)
                    .expect("tracked thread id"),
                work_unit_id: work_unit_id.clone(),
                title: title.to_string(),
                notes: Some(format!("Notes for {title}")),
                backend_kind: authority::TrackedThreadBackendKind::TT,
                upstream_thread_id: Some(format!("upstream-{tracked_thread_id}")),
                preferred_cwd: Some("/tmp/tt".to_string()),
                preferred_model: Some("gpt-5.4".to_string()),
                workspace: None,
            },
        })
        .await
        .expect("create authority tracked thread")
        .tracked_thread
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
async fn authority_hierarchy_is_canonical_while_events_expose_authority_projection_summaries() {
    let mut daemon = TestDaemon::spawn("authority-bridge").await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();

    let (mut events, snapshot) = client
        .subscribe_events(true)
        .await
        .expect("subscribe to daemon events");
    let snapshot = snapshot.expect("snapshot should be returned");
    assert!(snapshot.collaboration.workstreams.is_empty());
    assert!(snapshot.collaboration.work_units.is_empty());

    let workstream = create_authority_workstream(
        &client,
        &fixture,
        "authority-bridge-ws",
        "Bridged Workstream",
    )
    .await;
    let work_unit = create_authority_workunit(
        &client,
        &fixture,
        "authority-bridge-wu",
        &workstream.id,
        "Bridged Work Unit",
    )
    .await;

    let workstream_event = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream: summary }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && summary.id == workstream.id.as_str()
        )
    })
    .await;
    match workstream_event.event {
        ipc::DaemonEvent::WorkstreamLifecycle { action, workstream } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Created);
            assert_eq!(workstream.id, workstream.id);
            assert_eq!(workstream.title, "Bridged Workstream");
            assert_eq!(
                workstream.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected workstream lifecycle event, got {other:?}"),
    }

    let work_unit_event = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit: summary }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && summary.id == work_unit.id.as_str()
        )
    })
    .await;
    match work_unit_event.event {
        ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Created);
            assert_eq!(work_unit.id, work_unit.id);
            assert_eq!(work_unit.workstream_id, workstream.id.as_str());
            assert_eq!(work_unit.title, "Bridged Work Unit");
            assert_eq!(
                work_unit.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected work unit lifecycle event, got {other:?}"),
    }

    let projected = client
        .state_get()
        .await
        .expect("state/get after authority create");
    assert!(
        projected
            .snapshot
            .collaboration
            .workstreams
            .iter()
            .all(|summary| summary.id != workstream.id.as_str())
    );
    assert!(
        projected
            .snapshot
            .collaboration
            .work_units
            .iter()
            .all(|summary| summary.id != work_unit.id.as_str())
    );

    let hierarchy = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest::default())
        .await
        .expect("authority hierarchy after create");
    assert!(hierarchy.hierarchy.workstreams.iter().any(|node| {
        node.workstream.id == workstream.id
            && node
                .work_units
                .iter()
                .any(|summary| summary.work_unit.id == work_unit.id)
    }));

    daemon.stop().await;
}

#[tokio::test]
async fn authority_hierarchy_remains_canonical_after_restart() {
    let mut daemon = TestDaemon::spawn("authority-bridge-restart").await;
    let fixture = AuthorityFixture::new();

    let first_client = daemon.connect().await;
    let workstream = create_authority_workstream(
        &first_client,
        &fixture,
        "authority-bridge-ws-restart",
        "Restart Bridge Workstream",
    )
    .await;
    let work_unit = create_authority_workunit(
        &first_client,
        &fixture,
        "authority-bridge-wu-restart",
        &workstream.id,
        "Restart Bridge Work Unit",
    )
    .await;

    daemon.restart().await;

    let second_client = daemon.connect().await;
    let projected = second_client
        .state_get()
        .await
        .expect("state/get after restart");

    assert!(
        projected
            .snapshot
            .collaboration
            .workstreams
            .iter()
            .all(|summary| summary.id != workstream.id.as_str())
    );
    assert!(
        projected
            .snapshot
            .collaboration
            .work_units
            .iter()
            .all(|summary| summary.id != work_unit.id.as_str())
    );

    let hierarchy = second_client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest::default())
        .await
        .expect("authority hierarchy after restart");
    assert!(hierarchy.hierarchy.workstreams.iter().any(|node| {
        node.workstream.id == workstream.id
            && node
                .work_units
                .iter()
                .any(|summary| summary.work_unit.id == work_unit.id)
    }));

    daemon.stop().await;
}

#[tokio::test]
async fn authority_mutations_emit_post_commit_events_for_all_authority_entities() {
    let mut daemon = TestDaemon::spawn("authority-events").await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();
    let (mut events, _) = client
        .subscribe_events(false)
        .await
        .expect("subscribe to daemon events");

    let workstream = create_authority_workstream(
        &client,
        &fixture,
        "authority-events-ws",
        "Evented Workstream",
    )
    .await;
    let workstream_created = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && workstream.id == "authority-events-ws"
        )
    })
    .await;
    match workstream_created.event {
        ipc::DaemonEvent::WorkstreamLifecycle { action, workstream } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Created);
            assert_eq!(workstream.id, "authority-events-ws");
            assert_eq!(workstream.title, "Evented Workstream");
            assert_eq!(
                workstream.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected workstream create event, got {other:?}"),
    }
    let hierarchy = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest {
            include_deleted: false,
        })
        .await
        .expect("hierarchy after workstream create");
    assert!(
        hierarchy
            .hierarchy
            .workstreams
            .iter()
            .any(|node| node.workstream.id == workstream.id)
    );

    let edited_workstream = client
        .authority_workstream_edit(&ipc::AuthorityWorkstreamEditRequest {
            command: authority::EditWorkstream {
                metadata: fixture.metadata("bridge-ws-edit"),
                workstream_id: workstream.id.clone(),
                expected_revision: workstream.revision,
                changes: authority::WorkstreamPatch {
                    title: Some("Evented Workstream Updated".to_string()),
                    objective: None,
                    status: Some(WorkstreamStatus::Completed),
                    priority: None,
                    execution_scope: None,
                },
            },
        })
        .await
        .expect("edit authority workstream")
        .workstream;
    let workstream_updated = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Updated
                    && workstream.id == "authority-events-ws"
        )
    })
    .await;
    match workstream_updated.event {
        ipc::DaemonEvent::WorkstreamLifecycle { action, workstream } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Updated);
            assert_eq!(workstream.title, "Evented Workstream Updated");
            assert_eq!(workstream.status, WorkstreamStatus::Completed);
            assert_eq!(
                workstream.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected workstream update event, got {other:?}"),
    }

    let work_unit = create_authority_workunit(
        &client,
        &fixture,
        "authority-events-wu",
        &edited_workstream.id,
        "Evented Work Unit",
    )
    .await;
    let work_unit_created = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit }
                if *action == ipc::CollaborationLifecycleAction::Created
                    && work_unit.id == "authority-events-wu"
        )
    })
    .await;
    match work_unit_created.event {
        ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Created);
            assert_eq!(work_unit.id, "authority-events-wu");
            assert_eq!(work_unit.workstream_id, edited_workstream.id.as_str());
            assert_eq!(
                work_unit.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected work unit create event, got {other:?}"),
    }

    let edited_work_unit = client
        .authority_workunit_edit(&ipc::AuthorityWorkunitEditRequest {
            command: authority::EditWorkUnit {
                metadata: fixture.metadata("bridge-wu-edit"),
                work_unit_id: work_unit.id.clone(),
                expected_revision: work_unit.revision,
                changes: authority::WorkUnitPatch {
                    title: Some("Evented Work Unit Updated".to_string()),
                    task_statement: None,
                    status: Some(WorkUnitStatus::Running),
                },
            },
        })
        .await
        .expect("edit authority work unit")
        .work_unit;
    let work_unit_updated = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit }
                if *action == ipc::CollaborationLifecycleAction::Updated
                    && work_unit.id == "authority-events-wu"
        )
    })
    .await;
    match work_unit_updated.event {
        ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Updated);
            assert_eq!(work_unit.title, "Evented Work Unit Updated");
            assert_eq!(work_unit.status, WorkUnitStatus::Running);
            assert_eq!(
                work_unit.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected work unit update event, got {other:?}"),
    }

    let tracked_thread = create_authority_tracked_thread(
        &client,
        &fixture,
        "authority-events-thread",
        &edited_work_unit.id,
        "Tracked Thread",
    )
    .await;
    let tracked_thread_created = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::TrackedThreadLifecycle {
                action,
                tracked_thread,
            } if *action == ipc::CollaborationLifecycleAction::Created
                && tracked_thread.id == authority::TrackedThreadId::parse("authority-events-thread")
                    .expect("tracked thread id")
        )
    })
    .await;
    match tracked_thread_created.event {
        ipc::DaemonEvent::TrackedThreadLifecycle {
            action,
            tracked_thread,
        } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Created);
            assert_eq!(
                tracked_thread.id,
                authority::TrackedThreadId::parse("authority-events-thread")
                    .expect("tracked thread id")
            );
            assert_eq!(tracked_thread.work_unit_id, edited_work_unit.id);
            assert_eq!(tracked_thread.title, "Tracked Thread");
        }
        other => panic!("expected tracked thread create event, got {other:?}"),
    }
    let tracked_thread_detail = client
        .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
            tracked_thread_id: tracked_thread.id.clone(),
        })
        .await
        .expect("tracked thread detail after create");
    assert_eq!(tracked_thread_detail.tracked_thread.id, tracked_thread.id);

    let edited_tracked_thread = client
        .authority_tracked_thread_edit(&ipc::AuthorityTrackedThreadEditRequest {
            command: authority::EditTrackedThread {
                metadata: fixture.metadata("bridge-thread-edit"),
                tracked_thread_id: tracked_thread.id.clone(),
                expected_revision: tracked_thread.revision,
                changes: authority::TrackedThreadPatch {
                    title: Some("Tracked Thread Updated".to_string()),
                    notes: Some(Some("Updated notes".to_string())),
                    backend_kind: None,
                    upstream_thread_id: None,
                    binding_state: Some(authority::TrackedThreadBindingState::Bound),
                    preferred_cwd: None,
                    preferred_model: None,
                    last_seen_turn_id: None,
                    workspace: None,
                },
            },
        })
        .await
        .expect("edit authority tracked thread")
        .tracked_thread;
    let tracked_thread_updated = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::TrackedThreadLifecycle {
                action,
                tracked_thread,
            } if *action == ipc::CollaborationLifecycleAction::Updated
                && tracked_thread.id == authority::TrackedThreadId::parse("authority-events-thread")
                    .expect("tracked thread id")
        )
    })
    .await;
    match tracked_thread_updated.event {
        ipc::DaemonEvent::TrackedThreadLifecycle {
            action,
            tracked_thread,
        } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Updated);
            assert_eq!(tracked_thread.title, "Tracked Thread Updated");
            assert_eq!(
                tracked_thread.binding_state,
                authority::TrackedThreadBindingState::Bound
            );
        }
        other => panic!("expected tracked thread update event, got {other:?}"),
    }

    let tracked_thread_delete_plan = delete_plan(
        &client,
        authority::DeleteTarget::TrackedThread {
            tracked_thread_id: edited_tracked_thread.id.clone(),
        },
    )
    .await;
    client
        .authority_tracked_thread_delete(&ipc::AuthorityTrackedThreadDeleteRequest {
            command: authority::DeleteTrackedThread {
                metadata: fixture.metadata("bridge-thread-delete"),
                tracked_thread_id: edited_tracked_thread.id.clone(),
                expected_revision: tracked_thread_delete_plan.expected_revision,
                delete_token: tracked_thread_delete_plan.confirmation_token,
            },
        })
        .await
        .expect("delete authority tracked thread");
    let tracked_thread_deleted = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::TrackedThreadLifecycle {
                action,
                tracked_thread,
            } if *action == ipc::CollaborationLifecycleAction::Deleted
                && tracked_thread.id == authority::TrackedThreadId::parse("authority-events-thread")
                    .expect("tracked thread id")
        )
    })
    .await;
    match tracked_thread_deleted.event {
        ipc::DaemonEvent::TrackedThreadLifecycle {
            action,
            tracked_thread,
        } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Deleted);
            assert_eq!(tracked_thread.id, edited_tracked_thread.id);
            assert!(tracked_thread.deleted_at.is_some());
        }
        other => panic!("expected tracked thread delete event, got {other:?}"),
    }
    let hierarchy_after_thread_delete = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest {
            include_deleted: false,
        })
        .await
        .expect("hierarchy after tracked thread delete");
    assert!(
        hierarchy_after_thread_delete
            .hierarchy
            .workstreams
            .iter()
            .flat_map(|workstream| workstream.work_units.iter())
            .all(|work_unit| work_unit
                .tracked_threads
                .iter()
                .all(|summary| summary.id != edited_tracked_thread.id))
    );

    let work_unit_delete_plan = delete_plan(
        &client,
        authority::DeleteTarget::WorkUnit {
            work_unit_id: edited_work_unit.id.clone(),
        },
    )
    .await;
    client
        .authority_workunit_delete(&ipc::AuthorityWorkunitDeleteRequest {
            command: authority::DeleteWorkUnit {
                metadata: fixture.metadata("bridge-wu-delete"),
                work_unit_id: edited_work_unit.id.clone(),
                expected_revision: work_unit_delete_plan.expected_revision,
                delete_token: work_unit_delete_plan.confirmation_token,
            },
        })
        .await
        .expect("delete authority work unit");
    let work_unit_deleted = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit }
                if *action == ipc::CollaborationLifecycleAction::Deleted
                    && work_unit.id == "authority-events-wu"
        )
    })
    .await;
    match work_unit_deleted.event {
        ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Deleted);
            assert_eq!(work_unit.id, edited_work_unit.id.as_str());
            assert_eq!(
                work_unit.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected work unit delete event, got {other:?}"),
    }

    let workstream_delete_plan = delete_plan(
        &client,
        authority::DeleteTarget::Workstream {
            workstream_id: edited_workstream.id.clone(),
        },
    )
    .await;
    client
        .authority_workstream_delete(&ipc::AuthorityWorkstreamDeleteRequest {
            command: authority::DeleteWorkstream {
                metadata: fixture.metadata("bridge-ws-delete"),
                workstream_id: edited_workstream.id.clone(),
                expected_revision: workstream_delete_plan.expected_revision,
                delete_token: workstream_delete_plan.confirmation_token,
            },
        })
        .await
        .expect("delete authority workstream");
    let workstream_deleted = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream }
                if *action == ipc::CollaborationLifecycleAction::Deleted
                    && workstream.id == "authority-events-ws"
        )
    })
    .await;
    match workstream_deleted.event {
        ipc::DaemonEvent::WorkstreamLifecycle { action, workstream } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Deleted);
            assert_eq!(workstream.id, edited_workstream.id.as_str());
            assert_eq!(
                workstream.source_kind,
                ipc::PlanningSummarySourceKind::AuthorityProjection
            );
        }
        other => panic!("expected workstream delete event, got {other:?}"),
    }

    let snapshot_after_delete = client
        .state_get()
        .await
        .expect("state/get after authority deletes");
    assert!(
        snapshot_after_delete
            .snapshot
            .collaboration
            .workstreams
            .iter()
            .all(|summary| summary.id != edited_workstream.id.as_str())
    );
    assert!(
        snapshot_after_delete
            .snapshot
            .collaboration
            .work_units
            .iter()
            .all(|summary| summary.id != edited_work_unit.id.as_str())
    );
    let hierarchy_after_delete = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest {
            include_deleted: false,
        })
        .await
        .expect("hierarchy after workstream delete");
    assert!(hierarchy_after_delete.hierarchy.workstreams.is_empty());

    daemon.stop().await;
}

#[tokio::test]
async fn authority_parent_deletes_emit_cascaded_child_delete_events() {
    let mut daemon = TestDaemon::spawn("authority-cascade").await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();
    let (mut events, _) = client
        .subscribe_events(false)
        .await
        .expect("subscribe to daemon events");

    let workstream = create_authority_workstream(
        &client,
        &fixture,
        "authority-cascade-root-ws",
        "Cascade Root Workstream",
    )
    .await;
    let work_unit = create_authority_workunit(
        &client,
        &fixture,
        "authority-cascade-root-wu",
        &workstream.id,
        "Cascade Root Work Unit",
    )
    .await;
    let tracked_thread = create_authority_tracked_thread(
        &client,
        &fixture,
        "authority-cascade-root-tt",
        &work_unit.id,
        "Cascade Root Tracked Thread",
    )
    .await;

    let work_unit_delete_plan = delete_plan(
        &client,
        authority::DeleteTarget::WorkUnit {
            work_unit_id: work_unit.id.clone(),
        },
    )
    .await;
    client
        .authority_workunit_delete(&ipc::AuthorityWorkunitDeleteRequest {
            command: authority::DeleteWorkUnit {
                metadata: fixture.metadata("cascade-wu-delete"),
                work_unit_id: work_unit.id.clone(),
                expected_revision: work_unit_delete_plan.expected_revision,
                delete_token: work_unit_delete_plan.confirmation_token,
            },
        })
        .await
        .expect("delete authority work unit");

    let root_work_unit_deleted = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit: summary }
                if *action == ipc::CollaborationLifecycleAction::Deleted
                    && summary.id == work_unit.id.as_str()
        )
    })
    .await;
    assert!(matches!(
        root_work_unit_deleted.event,
        ipc::DaemonEvent::WorkUnitLifecycle {
            action: ipc::CollaborationLifecycleAction::Deleted,
            ..
        }
    ));

    let root_tracked_thread_deleted = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::TrackedThreadLifecycle {
                action,
                tracked_thread: summary,
            } if *action == ipc::CollaborationLifecycleAction::Deleted
                && summary.id == tracked_thread.id
        )
    })
    .await;
    match root_tracked_thread_deleted.event {
        ipc::DaemonEvent::TrackedThreadLifecycle {
            action,
            tracked_thread,
        } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Deleted);
            assert_eq!(
                tracked_thread.id,
                authority::TrackedThreadId::parse("authority-cascade-root-tt")
                    .expect("tracked thread id")
            );
            assert!(tracked_thread.deleted_at.is_some());
        }
        other => panic!("expected tracked thread work-unit cascade delete event, got {other:?}"),
    }

    let workstream = create_authority_workstream(
        &client,
        &fixture,
        "authority-cascade-ws",
        "Cascade Workstream",
    )
    .await;
    let work_unit = create_authority_workunit(
        &client,
        &fixture,
        "authority-cascade-wu",
        &workstream.id,
        "Cascade Work Unit",
    )
    .await;
    let tracked_thread = create_authority_tracked_thread(
        &client,
        &fixture,
        "authority-cascade-tt",
        &work_unit.id,
        "Cascade Tracked Thread",
    )
    .await;

    let workstream_delete_plan = delete_plan(
        &client,
        authority::DeleteTarget::Workstream {
            workstream_id: workstream.id.clone(),
        },
    )
    .await;
    client
        .authority_workstream_delete(&ipc::AuthorityWorkstreamDeleteRequest {
            command: authority::DeleteWorkstream {
                metadata: fixture.metadata("cascade-ws-delete"),
                workstream_id: workstream.id.clone(),
                expected_revision: workstream_delete_plan.expected_revision,
                delete_token: workstream_delete_plan.confirmation_token,
            },
        })
        .await
        .expect("delete authority workstream");

    let workstream_deleted = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream: summary }
                if *action == ipc::CollaborationLifecycleAction::Deleted
                    && summary.id == workstream.id.as_str()
        )
    })
    .await;
    assert!(matches!(
        workstream_deleted.event,
        ipc::DaemonEvent::WorkstreamLifecycle {
            action: ipc::CollaborationLifecycleAction::Deleted,
            ..
        }
    ));

    let work_unit_deleted = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit: summary }
                if *action == ipc::CollaborationLifecycleAction::Deleted
                    && summary.id == work_unit.id.as_str()
        )
    })
    .await;
    assert!(matches!(
        work_unit_deleted.event,
        ipc::DaemonEvent::WorkUnitLifecycle {
            action: ipc::CollaborationLifecycleAction::Deleted,
            ..
        }
    ));

    let tracked_thread_deleted = TestDaemon::next_event_matching(&mut events, |envelope| {
        matches!(
            &envelope.event,
            ipc::DaemonEvent::TrackedThreadLifecycle {
                action,
                tracked_thread: summary,
            } if *action == ipc::CollaborationLifecycleAction::Deleted
                && summary.id == tracked_thread.id
        )
    })
    .await;
    match tracked_thread_deleted.event {
        ipc::DaemonEvent::TrackedThreadLifecycle {
            action,
            tracked_thread,
        } => {
            assert_eq!(action, ipc::CollaborationLifecycleAction::Deleted);
            assert_eq!(
                tracked_thread.id,
                authority::TrackedThreadId::parse("authority-cascade-tt")
                    .expect("tracked thread id")
            );
            assert!(tracked_thread.deleted_at.is_some());
        }
        other => panic!("expected tracked thread cascade delete event, got {other:?}"),
    }

    daemon.stop().await;
}

#[tokio::test]
async fn failed_authority_mutation_does_not_emit_lifecycle_event() {
    let mut daemon = TestDaemon::spawn("authority-fail").await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();

    let workstream = create_authority_workstream(
        &client,
        &fixture,
        "authority-events-stale-ws",
        "Stale Revision Workstream",
    )
    .await;
    let (mut events, _) = client
        .subscribe_events(false)
        .await
        .expect("subscribe to daemon events");

    let error = client
        .authority_workstream_edit(&ipc::AuthorityWorkstreamEditRequest {
            command: authority::EditWorkstream {
                metadata: fixture.metadata("bridge-ws-stale-edit"),
                workstream_id: workstream.id.clone(),
                expected_revision: authority::Revision::new(999),
                changes: authority::WorkstreamPatch {
                    title: Some("Should Fail".to_string()),
                    objective: None,
                    status: None,
                    priority: None,
                    execution_scope: None,
                },
            },
        })
        .await
        .expect_err("stale authority edit should fail");
    assert!(error.to_string().contains("revision"));

    let no_event = timeout(Duration::from_millis(250), events.recv()).await;
    assert!(
        no_event.is_err(),
        "failed authority mutation unexpectedly emitted an event"
    );

    daemon.stop().await;
}
