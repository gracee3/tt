#![allow(unused_crate_dependencies)]

mod harness;

use chrono::Utc;

use harness::TestDaemon;
use orcas_core::authority::{
    self, CommandActor, CommandMetadata, CorrelationId, DeleteTarget, OriginNodeId, Revision,
};
use orcas_core::{OrcasError, WorkUnitStatus, WorkstreamStatus, ipc};

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

async fn create_workstream(
    client: &orcasd::OrcasIpcClient,
    fixture: &AuthorityFixture,
    workstream_id: &str,
    title: &str,
) -> authority::WorkstreamRecord {
    client
        .authority_workstream_create(&ipc::AuthorityWorkstreamCreateRequest {
            command: authority::CreateWorkstream {
                metadata: fixture.metadata("ws-create"),
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

async fn create_workunit(
    client: &orcasd::OrcasIpcClient,
    fixture: &AuthorityFixture,
    work_unit_id: &str,
    workstream_id: &authority::WorkstreamId,
    title: &str,
) -> authority::WorkUnitRecord {
    client
        .authority_workunit_create(&ipc::AuthorityWorkunitCreateRequest {
            command: authority::CreateWorkUnit {
                metadata: fixture.metadata("wu-create"),
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

async fn create_tracked_thread(
    client: &orcasd::OrcasIpcClient,
    fixture: &AuthorityFixture,
    tracked_thread_id: &str,
    work_unit_id: &authority::WorkUnitId,
    title: &str,
) -> authority::TrackedThreadRecord {
    client
        .authority_tracked_thread_create(&ipc::AuthorityTrackedThreadCreateRequest {
            command: authority::CreateTrackedThread {
                metadata: fixture.metadata("tt-create"),
                tracked_thread_id: authority::TrackedThreadId::parse(tracked_thread_id)
                    .expect("tracked thread id"),
                work_unit_id: work_unit_id.clone(),
                title: title.to_string(),
                notes: Some("socket authority test".to_string()),
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

#[tokio::test]
async fn authority_create_and_get_round_trip_over_real_socket() {
    let mut daemon = TestDaemon::spawn("authority-round-trip").await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();

    let workstream =
        create_workstream(&client, &fixture, "authority-ws-round-trip", "Round Trip").await;

    assert_eq!(workstream.title, "Round Trip");
    assert_eq!(workstream.status, WorkstreamStatus::Active);
    assert_eq!(workstream.revision, Revision::initial());

    let fetched = client
        .authority_workstream_get(&ipc::AuthorityWorkstreamGetRequest {
            workstream_id: workstream.id.clone(),
        })
        .await
        .expect("get created authority workstream");

    assert_eq!(fetched.workstream.id, workstream.id);
    assert_eq!(fetched.workstream.title, "Round Trip");
    assert_eq!(fetched.workstream.objective, "Objective for Round Trip");
    assert!(fetched.work_units.is_empty());

    daemon.stop().await;
}

#[tokio::test]
async fn authority_hierarchy_round_trip_over_real_socket() {
    let mut daemon = TestDaemon::spawn("authority-hierarchy").await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();

    let workstream = create_workstream(
        &client,
        &fixture,
        "authority-ws-hierarchy",
        "Hierarchy Root",
    )
    .await;
    let work_unit = create_workunit(
        &client,
        &fixture,
        "authority-wu-hierarchy",
        &workstream.id,
        "Hierarchy Unit",
    )
    .await;
    let tracked_thread = create_tracked_thread(
        &client,
        &fixture,
        "authority-tt-hierarchy",
        &work_unit.id,
        "Hierarchy Thread",
    )
    .await;

    let fetched_workstream = client
        .authority_workstream_get(&ipc::AuthorityWorkstreamGetRequest {
            workstream_id: workstream.id.clone(),
        })
        .await
        .expect("get workstream hierarchy");
    assert_eq!(fetched_workstream.work_units.len(), 1);
    assert_eq!(fetched_workstream.work_units[0].id, work_unit.id);

    let fetched_work_unit = client
        .authority_workunit_get(&ipc::AuthorityWorkunitGetRequest {
            work_unit_id: work_unit.id.clone(),
        })
        .await
        .expect("get work unit hierarchy");
    assert_eq!(fetched_work_unit.tracked_threads.len(), 1);
    assert_eq!(fetched_work_unit.tracked_threads[0].id, tracked_thread.id);

    let tracked_threads = client
        .authority_tracked_thread_list(&ipc::AuthorityTrackedThreadListRequest {
            work_unit_id: work_unit.id.clone(),
            include_deleted: false,
        })
        .await
        .expect("list tracked threads");
    assert_eq!(tracked_threads.tracked_threads.len(), 1);
    assert_eq!(
        tracked_threads.tracked_threads[0].binding_state,
        authority::TrackedThreadBindingState::Bound
    );

    let hierarchy = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest::default())
        .await
        .expect("authority hierarchy")
        .hierarchy;
    assert_eq!(hierarchy.workstreams.len(), 1);
    assert_eq!(hierarchy.workstreams[0].workstream.id, workstream.id);
    assert_eq!(hierarchy.workstreams[0].work_units.len(), 1);
    assert_eq!(
        hierarchy.workstreams[0].work_units[0].work_unit.id,
        work_unit.id
    );
    assert_eq!(
        hierarchy.workstreams[0].work_units[0].tracked_threads[0].id,
        tracked_thread.id
    );

    daemon.stop().await;
}

#[tokio::test]
async fn authority_update_and_revision_behavior_over_real_socket() {
    let mut daemon = TestDaemon::spawn("authority-update").await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();

    let created = create_workstream(&client, &fixture, "authority-ws-update", "Original").await;

    let edited = client
        .authority_workstream_edit(&ipc::AuthorityWorkstreamEditRequest {
            command: authority::EditWorkstream {
                metadata: fixture.metadata("ws-edit"),
                workstream_id: created.id.clone(),
                expected_revision: created.revision,
                changes: authority::WorkstreamPatch {
                    title: Some("Updated".to_string()),
                    objective: Some("Updated objective".to_string()),
                    status: Some(WorkstreamStatus::Completed),
                    priority: Some("urgent".to_string()),
                },
            },
        })
        .await
        .expect("edit workstream")
        .workstream;

    assert_eq!(edited.id, created.id);
    assert_eq!(edited.revision, created.revision.next());
    assert_eq!(edited.title, "Updated");
    assert_eq!(edited.objective, "Updated objective");
    assert_eq!(edited.status, WorkstreamStatus::Completed);
    assert_eq!(edited.priority, "urgent");

    let listed = client
        .authority_workstream_list(&ipc::AuthorityWorkstreamListRequest {
            include_deleted: false,
        })
        .await
        .expect("list workstreams");
    assert_eq!(listed.workstreams.len(), 1);
    assert_eq!(listed.workstreams[0].revision, edited.revision);
    assert_eq!(listed.workstreams[0].title, "Updated");

    let stale_edit = client
        .authority_workstream_edit(&ipc::AuthorityWorkstreamEditRequest {
            command: authority::EditWorkstream {
                metadata: fixture.metadata("ws-edit-stale"),
                workstream_id: created.id.clone(),
                expected_revision: Revision::initial(),
                changes: authority::WorkstreamPatch {
                    title: Some("Stale".to_string()),
                    objective: None,
                    status: None,
                    priority: None,
                },
            },
        })
        .await
        .expect_err("stale revision should fail");
    assert!(
        matches!(stale_edit, OrcasError::Protocol(message) if message.contains("revision mismatch"))
    );

    daemon.stop().await;
}

#[tokio::test]
async fn authority_delete_or_cascade_behavior_over_real_socket() {
    let mut daemon = TestDaemon::spawn("authority-delete").await;
    let client = daemon.connect().await;
    let fixture = AuthorityFixture::new();

    let workstream =
        create_workstream(&client, &fixture, "authority-ws-delete", "Delete Root").await;
    let work_unit = create_workunit(
        &client,
        &fixture,
        "authority-wu-delete",
        &workstream.id,
        "Delete Unit",
    )
    .await;
    let tracked_thread = create_tracked_thread(
        &client,
        &fixture,
        "authority-tt-delete",
        &work_unit.id,
        "Delete Thread",
    )
    .await;

    let delete_plan = client
        .authority_delete_plan(&ipc::AuthorityDeletePlanRequest {
            target: DeleteTarget::Workstream {
                workstream_id: workstream.id.clone(),
            },
        })
        .await
        .expect("delete plan")
        .delete_plan;
    assert_eq!(delete_plan.expected_revision, workstream.revision);
    assert_eq!(delete_plan.affected_work_units, 1);
    assert_eq!(delete_plan.affected_tracked_threads, 1);
    assert!(delete_plan.has_upstream_bindings);

    let deleted = client
        .authority_workstream_delete(&ipc::AuthorityWorkstreamDeleteRequest {
            command: authority::DeleteWorkstream {
                metadata: fixture.metadata("ws-delete"),
                workstream_id: workstream.id.clone(),
                expected_revision: delete_plan.expected_revision,
                delete_token: delete_plan.confirmation_token.clone(),
            },
        })
        .await
        .expect("delete workstream")
        .workstream;
    assert!(deleted.deleted_at.is_some());
    assert_eq!(deleted.revision, workstream.revision.next());

    let visible_workstreams = client
        .authority_workstream_list(&ipc::AuthorityWorkstreamListRequest {
            include_deleted: false,
        })
        .await
        .expect("list live workstreams");
    assert!(visible_workstreams.workstreams.is_empty());

    let all_workstreams = client
        .authority_workstream_list(&ipc::AuthorityWorkstreamListRequest {
            include_deleted: true,
        })
        .await
        .expect("list deleted workstreams");
    assert_eq!(all_workstreams.workstreams.len(), 1);
    assert!(all_workstreams.workstreams[0].deleted_at.is_some());

    let live_work_units = client
        .authority_workunit_list(&ipc::AuthorityWorkunitListRequest {
            workstream_id: Some(workstream.id.clone()),
            include_deleted: false,
        })
        .await
        .expect("list live work units");
    assert!(live_work_units.work_units.is_empty());

    let all_work_units = client
        .authority_workunit_list(&ipc::AuthorityWorkunitListRequest {
            workstream_id: Some(workstream.id.clone()),
            include_deleted: true,
        })
        .await
        .expect("list deleted work units");
    assert_eq!(all_work_units.work_units.len(), 1);
    assert_eq!(all_work_units.work_units[0].id, work_unit.id);
    assert!(all_work_units.work_units[0].deleted_at.is_some());

    let live_threads = client
        .authority_tracked_thread_list(&ipc::AuthorityTrackedThreadListRequest {
            work_unit_id: work_unit.id.clone(),
            include_deleted: false,
        })
        .await
        .expect("list live tracked threads");
    assert!(live_threads.tracked_threads.is_empty());

    let all_threads = client
        .authority_tracked_thread_list(&ipc::AuthorityTrackedThreadListRequest {
            work_unit_id: work_unit.id.clone(),
            include_deleted: true,
        })
        .await
        .expect("list deleted tracked threads");
    assert_eq!(all_threads.tracked_threads.len(), 1);
    assert_eq!(all_threads.tracked_threads[0].id, tracked_thread.id);
    assert!(all_threads.tracked_threads[0].deleted_at.is_some());

    let live_hierarchy = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest {
            include_deleted: false,
        })
        .await
        .expect("live hierarchy")
        .hierarchy;
    assert!(live_hierarchy.workstreams.is_empty());

    let deleted_hierarchy = client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest {
            include_deleted: true,
        })
        .await
        .expect("deleted hierarchy")
        .hierarchy;
    assert_eq!(deleted_hierarchy.workstreams.len(), 1);
    assert_eq!(
        deleted_hierarchy.workstreams[0].workstream.id,
        workstream.id
    );
    assert!(
        deleted_hierarchy.workstreams[0]
            .workstream
            .deleted_at
            .is_some()
    );
    assert_eq!(deleted_hierarchy.workstreams[0].work_units.len(), 1);
    assert!(
        deleted_hierarchy.workstreams[0].work_units[0]
            .work_unit
            .deleted_at
            .is_some()
    );
    assert_eq!(
        deleted_hierarchy.workstreams[0].work_units[0].tracked_threads[0].id,
        tracked_thread.id
    );
    assert!(
        deleted_hierarchy.workstreams[0].work_units[0].tracked_threads[0]
            .deleted_at
            .is_some()
    );

    daemon.stop().await;
}

#[tokio::test]
async fn authority_persists_across_restart_and_reconnect() {
    let mut daemon = TestDaemon::spawn("authority-restart").await;
    let fixture = AuthorityFixture::new();

    let first_client = daemon.connect().await;
    let workstream = create_workstream(
        &first_client,
        &fixture,
        "authority-ws-restart",
        "Restart Root",
    )
    .await;
    let work_unit = create_workunit(
        &first_client,
        &fixture,
        "authority-wu-restart",
        &workstream.id,
        "Restart Unit",
    )
    .await;
    let tracked_thread = create_tracked_thread(
        &first_client,
        &fixture,
        "authority-tt-restart",
        &work_unit.id,
        "Restart Thread",
    )
    .await;

    daemon.restart().await;

    let second_client = daemon.connect().await;
    let workstream_get = second_client
        .authority_workstream_get(&ipc::AuthorityWorkstreamGetRequest {
            workstream_id: workstream.id.clone(),
        })
        .await
        .expect("get workstream after restart");
    assert_eq!(workstream_get.workstream.id, workstream.id);
    assert_eq!(workstream_get.work_units.len(), 1);
    assert_eq!(workstream_get.work_units[0].id, work_unit.id);

    let work_unit_get = second_client
        .authority_workunit_get(&ipc::AuthorityWorkunitGetRequest {
            work_unit_id: work_unit.id.clone(),
        })
        .await
        .expect("get work unit after restart");
    assert_eq!(work_unit_get.work_unit.id, work_unit.id);
    assert_eq!(work_unit_get.tracked_threads.len(), 1);
    assert_eq!(work_unit_get.tracked_threads[0].id, tracked_thread.id);

    let tracked_thread_get = second_client
        .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
            tracked_thread_id: tracked_thread.id.clone(),
        })
        .await
        .expect("get tracked thread after restart");
    assert_eq!(tracked_thread_get.tracked_thread.id, tracked_thread.id);
    assert_eq!(
        tracked_thread_get.tracked_thread.preferred_model.as_deref(),
        Some("gpt-5.4")
    );

    let hierarchy = second_client
        .authority_hierarchy_get(&ipc::AuthorityHierarchyGetRequest::default())
        .await
        .expect("hierarchy after restart")
        .hierarchy;
    assert_eq!(hierarchy.workstreams.len(), 1);
    assert_eq!(hierarchy.workstreams[0].workstream.id, workstream.id);
    assert_eq!(
        hierarchy.workstreams[0].work_units[0].work_unit.id,
        work_unit.id
    );
    assert_eq!(
        hierarchy.workstreams[0].work_units[0].tracked_threads[0].id,
        tracked_thread.id
    );

    daemon.stop().await;
}
