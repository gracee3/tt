use chrono::Utc;

use crate::app::{
    BannerLevel, CollaborationFocus, DaemonConnectionPhase, TopLevelView, UiEvent, UserAction,
};
use crate::backend::BackendCommand;
use crate::test_harness::AppHarness;
use orcas_core::{
    Assignment, AssignmentStatus, ConnectionState, Decision, DecisionPolicy, DecisionType,
    DraftAssignment, ProposedDecision, RecentPrimaryHistory, Report, ReportConfidence,
    ReportDisposition, ReportParseResult, SupervisorAssignmentContext, SupervisorContextPack,
    SupervisorDependencyContext, SupervisorPackLimits, SupervisorPackTruncation,
    SupervisorProposal, SupervisorProposalFailure, SupervisorProposalFailureStage,
    SupervisorProposalRecord, SupervisorProposalStatus, SupervisorProposalTrigger,
    SupervisorProposalTriggerKind, SupervisorSourceReportContext, SupervisorStateAnchor,
    SupervisorSummary, SupervisorWorkUnitContext, SupervisorWorkerSessionContext,
    SupervisorWorkstreamContext, WorkUnit, WorkUnitStatus, WorkstreamStatus, ipc,
};

fn sample_thread_summary(id: &str, preview: &str, updated_at: i64) -> ipc::ThreadSummary {
    ipc::ThreadSummary {
        id: id.to_string(),
        preview: preview.to_string(),
        name: None,
        model_provider: "openai".to_string(),
        cwd: "/tmp/orcas".to_string(),
        status: "idle".to_string(),
        created_at: updated_at - 10,
        updated_at,
        scope: "orcas_managed".to_string(),
        recent_output: Some(preview.to_string()),
        recent_event: Some("thread idle".to_string()),
        turn_in_flight: false,
    }
}

fn sample_thread_view(id: &str, preview: &str, output: &str) -> ipc::ThreadView {
    ipc::ThreadView {
        summary: sample_thread_summary(id, preview, 200),
        turns: vec![ipc::TurnView {
            id: "turn-1".to_string(),
            status: "completed".to_string(),
            error_message: None,
            items: vec![ipc::ItemView {
                id: "item-1".to_string(),
                item_type: "agent_message".to_string(),
                status: Some("completed".to_string()),
                text: Some(output.to_string()),
            }],
        }],
    }
}

fn sample_turn_state(
    thread_id: &str,
    turn_id: &str,
    lifecycle: ipc::TurnLifecycleState,
    status: &str,
    attachable: bool,
) -> ipc::TurnStateView {
    ipc::TurnStateView {
        thread_id: thread_id.to_string(),
        turn_id: turn_id.to_string(),
        lifecycle,
        status: status.to_string(),
        attachable,
        live_stream: attachable,
        terminal: !matches!(lifecycle, ipc::TurnLifecycleState::Active),
        recent_output: Some("turn output".to_string()),
        recent_event: Some(format!("turn {status}")),
        updated_at: Utc::now(),
        error_message: None,
    }
}

fn sample_proposal_summary(
    latest_status: SupervisorProposalStatus,
    latest_decision_type: Option<DecisionType>,
) -> ipc::WorkUnitProposalSummary {
    ipc::WorkUnitProposalSummary {
        latest_proposal_id: "proposal-1".to_string(),
        latest_status,
        latest_proposed_decision_type: latest_decision_type,
        latest_created_at: Utc::now(),
        latest_reviewed_at: None,
        latest_has_approval_edits: false,
        latest_failure_stage: None,
        has_open_proposal: latest_status == SupervisorProposalStatus::Open,
        open_proposal_id: (latest_status == SupervisorProposalStatus::Open)
            .then(|| "proposal-1".to_string()),
        open_proposed_decision_type: (latest_status == SupervisorProposalStatus::Open)
            .then_some(latest_decision_type)
            .flatten(),
        has_generation_failed: latest_status == SupervisorProposalStatus::GenerationFailed,
        has_stale_or_superseded: matches!(
            latest_status,
            SupervisorProposalStatus::Stale | SupervisorProposalStatus::Superseded
        ),
    }
}

fn sample_proposal_record(
    id: &str,
    work_unit_id: &str,
    report_id: &str,
    assignment_id: &str,
    decision_type: DecisionType,
    status: SupervisorProposalStatus,
) -> SupervisorProposalRecord {
    let now = Utc::now();
    let proposal = SupervisorProposal {
        schema_version: "supervisor_proposal.v1".to_string(),
        summary: SupervisorSummary {
            headline: format!("Proposal {id}"),
            situation: "The work unit reached a bounded decision point.".to_string(),
            recommended_action: "Keep the next step reviewable.".to_string(),
            key_evidence: vec!["The latest report is explicit.".to_string()],
            risks: vec!["Avoid broadening scope.".to_string()],
            review_focus: vec!["Check boundedness.".to_string()],
        },
        proposed_decision: ProposedDecision {
            decision_type,
            target_work_unit_id: work_unit_id.to_string(),
            source_report_id: report_id.to_string(),
            rationale: "Bounded follow-up remains appropriate.".to_string(),
            expected_work_unit_status: match decision_type {
                DecisionType::Accept => "accepted",
                DecisionType::Continue | DecisionType::Redirect => "ready",
                DecisionType::MarkComplete => "completed",
                DecisionType::EscalateToHuman => "needs_human",
            }
            .to_string(),
            requires_assignment: matches!(
                decision_type,
                DecisionType::Continue | DecisionType::Redirect
            ),
        },
        draft_next_assignment: matches!(
            decision_type,
            DecisionType::Continue | DecisionType::Redirect
        )
        .then(|| DraftAssignment {
            target_work_unit_id: work_unit_id.to_string(),
            predecessor_assignment_id: assignment_id.to_string(),
            derived_from_decision_type: decision_type,
            preferred_worker_id: Some("worker-a".to_string()),
            worker_kind: Some("codex".to_string()),
            objective: "Resolve one bounded follow-up question.".to_string(),
            instructions: vec![
                "Inspect the remaining gap.".to_string(),
                "Report the result without broadening scope.".to_string(),
            ],
            acceptance_criteria: vec!["The bounded question is resolved.".to_string()],
            stop_conditions: vec!["Stop if human input is required.".to_string()],
            required_context_refs: vec![report_id.to_string()],
            expected_report_fields: vec!["summary".to_string(), "findings".to_string()],
            boundedness_note: "Stay within one bounded follow-up.".to_string(),
        }),
        confidence: ReportConfidence::High,
        warnings: Vec::new(),
        open_questions: Vec::new(),
    };

    SupervisorProposalRecord {
        id: id.to_string(),
        workstream_id: "ws-1".to_string(),
        primary_work_unit_id: work_unit_id.to_string(),
        source_report_id: report_id.to_string(),
        trigger: SupervisorProposalTrigger {
            kind: SupervisorProposalTriggerKind::HumanRequested,
            requested_at: now,
            requested_by: "tester".to_string(),
            source_report_id: report_id.to_string(),
            note: Some("review the next bounded step".to_string()),
        },
        status,
        created_at: now,
        reasoner_backend: "test".to_string(),
        reasoner_model: "test-supervisor".to_string(),
        reasoner_response_id: Some("resp-1".to_string()),
        reasoner_usage: None,
        reasoner_output_text: Some("raw structured output".to_string()),
        context_pack: SupervisorContextPack {
            schema_version: "supervisor_context_pack.v1".to_string(),
            generated_at: now,
            trigger: SupervisorProposalTrigger {
                kind: SupervisorProposalTriggerKind::HumanRequested,
                requested_at: now,
                requested_by: "tester".to_string(),
                source_report_id: report_id.to_string(),
                note: Some("review the next bounded step".to_string()),
            },
            pack_limits: SupervisorPackLimits {
                max_related_work_units: 4,
                max_prior_reports: 4,
                max_prior_decisions: 4,
                max_artifacts: 0,
                max_raw_report_chars: 512,
            },
            truncation: SupervisorPackTruncation::default(),
            state_anchor: SupervisorStateAnchor {
                workstream_id: "ws-1".to_string(),
                primary_work_unit_id: work_unit_id.to_string(),
                source_report_id: report_id.to_string(),
                source_report_created_at: now,
                current_assignment_id: Some(assignment_id.to_string()),
                primary_work_unit_updated_at: now,
                latest_decision_id: None,
                latest_decision_created_at: None,
            },
            decision_policy: DecisionPolicy {
                supported_decisions: vec![
                    DecisionType::Accept,
                    DecisionType::Continue,
                    DecisionType::Redirect,
                    DecisionType::MarkComplete,
                    DecisionType::EscalateToHuman,
                ],
                allowed_decisions: vec![
                    DecisionType::Accept,
                    DecisionType::Continue,
                    DecisionType::Redirect,
                    DecisionType::MarkComplete,
                    DecisionType::EscalateToHuman,
                ],
                disallowed_decisions: Vec::new(),
                disallowed_reasons_by_decision: std::collections::BTreeMap::new(),
                assignment_required_for: vec![DecisionType::Continue, DecisionType::Redirect],
                assignment_forbidden_for: vec![
                    DecisionType::Accept,
                    DecisionType::MarkComplete,
                    DecisionType::EscalateToHuman,
                ],
                human_review_required: true,
            },
            workstream: SupervisorWorkstreamContext {
                id: "ws-1".to_string(),
                title: "Collaboration hardening".to_string(),
                objective: "Harden collaboration snapshot semantics.".to_string(),
                status: "active".to_string(),
                priority: "high".to_string(),
                success_criteria: Vec::new(),
                constraints: Vec::new(),
                summary: Some("Keep proposal visibility read-only.".to_string()),
                open_work_unit_count: 2,
                blocked_work_unit_count: 0,
                completed_work_unit_count: 0,
            },
            primary_work_unit: SupervisorWorkUnitContext {
                id: work_unit_id.to_string(),
                title: "Snapshot wiring".to_string(),
                task_statement: "Wire collaboration summaries into the snapshot.".to_string(),
                status: "awaiting_decision".to_string(),
                dependencies: Vec::new(),
                current_assignment_id: Some(assignment_id.to_string()),
                latest_report_id: Some(report_id.to_string()),
                acceptance_criteria: Vec::new(),
                stop_conditions: Vec::new(),
                result_summary: None,
            },
            source_report: SupervisorSourceReportContext {
                id: report_id.to_string(),
                assignment_id: assignment_id.to_string(),
                worker_id: "worker-a".to_string(),
                worker_session_id: Some("session-1".to_string()),
                submitted_at: now,
                disposition: ReportDisposition::Partial,
                summary: "Snapshot path is implemented, review is required.".to_string(),
                findings: vec!["Event summaries need one more pass.".to_string()],
                blockers: Vec::new(),
                questions: vec!["Should summaries include objective?".to_string()],
                recommended_next_actions: vec!["Supervisor decide continue.".to_string()],
                confidence: ReportConfidence::Medium,
                parse_result: ReportParseResult::Ambiguous,
                needs_supervisor_review: true,
                raw_output_excerpt: "noise + json".to_string(),
            },
            current_assignment: SupervisorAssignmentContext {
                id: assignment_id.to_string(),
                status: "awaiting_decision".to_string(),
                attempt_number: 2,
                worker_id: "worker-a".to_string(),
                worker_session_id: "session-1".to_string(),
                instructions: "Second bounded pass".to_string(),
                created_at: now,
                updated_at: now,
            },
            worker_session: SupervisorWorkerSessionContext {
                id: "session-1".to_string(),
                worker_id: "worker-a".to_string(),
                backend_type: "codex".to_string(),
                thread_id: Some("thread-1".to_string()),
                active_turn_id: None,
                runtime_status: "completed".to_string(),
                attachability: "not_attachable".to_string(),
                updated_at: now,
            },
            dependency_context: SupervisorDependencyContext::default(),
            related_work_units: Vec::new(),
            recent_primary_history: RecentPrimaryHistory::default(),
            relevant_artifacts: Vec::new(),
            operator_request: None,
        },
        proposal: Some(proposal),
        approval_edits: None,
        approved_proposal: None,
        generation_failure: None,
        validated_at: Some(now),
        reviewed_at: None,
        reviewed_by: None,
        review_note: None,
        approved_decision_id: None,
        approved_assignment_id: None,
    }
}

fn sample_collaboration_snapshot() -> ipc::CollaborationSnapshot {
    ipc::CollaborationSnapshot {
        workstreams: vec![
            ipc::WorkstreamSummary {
                id: "ws-1".to_string(),
                title: "Collaboration hardening".to_string(),
                objective: "Harden collaboration snapshot semantics.".to_string(),
                status: WorkstreamStatus::Active,
                priority: "high".to_string(),
                updated_at: Utc::now(),
            },
            ipc::WorkstreamSummary {
                id: "ws-2".to_string(),
                title: "Deferred work".to_string(),
                objective: "Hold future scope.".to_string(),
                status: WorkstreamStatus::Blocked,
                priority: "low".to_string(),
                updated_at: Utc::now(),
            },
        ],
        work_units: vec![
            ipc::WorkUnitSummary {
                id: "wu-1".to_string(),
                workstream_id: "ws-1".to_string(),
                title: "Snapshot wiring".to_string(),
                status: WorkUnitStatus::AwaitingDecision,
                dependency_count: 0,
                current_assignment_id: Some("assignment-2".to_string()),
                latest_report_id: Some("report-2".to_string()),
                proposal: Some(ipc::WorkUnitProposalSummary {
                    has_generation_failed: true,
                    has_stale_or_superseded: false,
                    ..sample_proposal_summary(
                        SupervisorProposalStatus::Open,
                        Some(DecisionType::Continue),
                    )
                }),
                updated_at: Utc::now(),
            },
            ipc::WorkUnitSummary {
                id: "wu-2".to_string(),
                workstream_id: "ws-1".to_string(),
                title: "Event wiring".to_string(),
                status: WorkUnitStatus::Ready,
                dependency_count: 1,
                current_assignment_id: Some("assignment-3".to_string()),
                latest_report_id: Some("report-3".to_string()),
                proposal: Some(ipc::WorkUnitProposalSummary {
                    latest_failure_stage: Some(SupervisorProposalFailureStage::Backend),
                    ..sample_proposal_summary(SupervisorProposalStatus::GenerationFailed, None)
                }),
                updated_at: Utc::now(),
            },
            ipc::WorkUnitSummary {
                id: "wu-3".to_string(),
                workstream_id: "ws-2".to_string(),
                title: "Out of scope".to_string(),
                status: WorkUnitStatus::Blocked,
                dependency_count: 2,
                current_assignment_id: None,
                latest_report_id: None,
                proposal: None,
                updated_at: Utc::now(),
            },
        ],
        assignments: vec![
            ipc::AssignmentSummary {
                id: "assignment-2".to_string(),
                work_unit_id: "wu-1".to_string(),
                worker_id: "worker-a".to_string(),
                worker_session_id: "session-1".to_string(),
                status: AssignmentStatus::AwaitingDecision,
                attempt_number: 2,
                updated_at: Utc::now(),
            },
            ipc::AssignmentSummary {
                id: "assignment-3".to_string(),
                work_unit_id: "wu-2".to_string(),
                worker_id: "worker-a".to_string(),
                worker_session_id: "session-1".to_string(),
                status: AssignmentStatus::Created,
                attempt_number: 3,
                updated_at: Utc::now(),
            },
        ],
        reports: vec![
            ipc::ReportSummary {
                id: "report-2".to_string(),
                work_unit_id: "wu-1".to_string(),
                assignment_id: "assignment-2".to_string(),
                worker_id: "worker-a".to_string(),
                disposition: ReportDisposition::Partial,
                summary: "Snapshot path is implemented, review is required.".to_string(),
                confidence: ReportConfidence::Medium,
                parse_result: ReportParseResult::Ambiguous,
                needs_supervisor_review: true,
                created_at: Utc::now(),
            },
            ipc::ReportSummary {
                id: "report-3".to_string(),
                work_unit_id: "wu-2".to_string(),
                assignment_id: "assignment-3".to_string(),
                worker_id: "worker-a".to_string(),
                disposition: ReportDisposition::Completed,
                summary: "Clean report for event wiring.".to_string(),
                confidence: ReportConfidence::High,
                parse_result: ReportParseResult::Parsed,
                needs_supervisor_review: false,
                created_at: Utc::now(),
            },
        ],
        decisions: vec![ipc::DecisionSummary {
            id: "decision-1".to_string(),
            work_unit_id: "wu-1".to_string(),
            report_id: Some("report-2".to_string()),
            decision_type: DecisionType::Continue,
            rationale: "Need one more bounded pass.".to_string(),
            created_at: Utc::now(),
        }],
    }
}

fn sample_snapshot() -> ipc::StateSnapshot {
    ipc::StateSnapshot {
        daemon: ipc::DaemonStatusResponse {
            socket_path: "/tmp/orcasd.sock".to_string(),
            metadata_path: "/tmp/orcasd.json".to_string(),
            codex_endpoint: "ws://127.0.0.1:4500".to_string(),
            codex_binary_path: "/home/emmy/git/codex/codex-rs/target/debug/codex".to_string(),
            upstream: ConnectionState {
                endpoint: "ws://127.0.0.1:4500".to_string(),
                status: "connected".to_string(),
                detail: None,
            },
            client_count: 1,
            known_threads: 2,
            runtime: ipc::DaemonRuntimeMetadata {
                pid: 4242,
                started_at: Utc::now(),
                version: "0.1.0".to_string(),
                build_fingerprint: "abc123".to_string(),
                binary_path: "/tmp/orcasd".to_string(),
                socket_path: "/tmp/orcasd.sock".to_string(),
                metadata_path: "/tmp/orcasd.json".to_string(),
                git_commit: None,
            },
        },
        session: ipc::SessionState {
            active_thread_id: Some("thread-1".to_string()),
            active_turns: Vec::new(),
        },
        threads: vec![
            sample_thread_summary("thread-1", "hello", 200),
            sample_thread_summary("thread-2", "later", 150),
        ],
        active_thread: Some(sample_thread_view("thread-1", "hello", "world")),
        collaboration: sample_collaboration_snapshot(),
        recent_events: vec![ipc::EventSummary {
            timestamp: Utc::now(),
            kind: "thread".to_string(),
            message: "loaded thread-1".to_string(),
            thread_id: Some("thread-1".to_string()),
            turn_id: None,
        }],
    }
}

fn sample_workunit_detail(work_unit_id: &str) -> ipc::WorkunitGetResponse {
    let now = Utc::now();
    match work_unit_id {
        "wu-1" => {
            let mut failed = sample_proposal_record(
                "proposal-failed",
                "wu-1",
                "report-1",
                "assignment-1",
                DecisionType::Continue,
                SupervisorProposalStatus::GenerationFailed,
            );
            failed.proposal = None;
            failed.generation_failure = Some(SupervisorProposalFailure {
                stage: SupervisorProposalFailureStage::Backend,
                message: "request timed out".to_string(),
            });
            let open = sample_proposal_record(
                "proposal-1",
                "wu-1",
                "report-2",
                "assignment-2",
                DecisionType::Continue,
                SupervisorProposalStatus::Open,
            );
            ipc::WorkunitGetResponse {
                work_unit: WorkUnit {
                    id: "wu-1".to_string(),
                    workstream_id: "ws-1".to_string(),
                    title: "Snapshot wiring".to_string(),
                    task_statement: "Wire collaboration summaries into the snapshot.".to_string(),
                    status: WorkUnitStatus::AwaitingDecision,
                    dependencies: Vec::new(),
                    latest_report_id: Some("report-2".to_string()),
                    current_assignment_id: Some("assignment-2".to_string()),
                    created_at: now,
                    updated_at: now,
                },
                assignments: vec![
                    Assignment {
                        id: "assignment-1".to_string(),
                        work_unit_id: "wu-1".to_string(),
                        worker_id: "worker-a".to_string(),
                        worker_session_id: "session-1".to_string(),
                        instructions: "Initial snapshot pass".to_string(),
                        communication_seed: None,
                        status: AssignmentStatus::Closed,
                        attempt_number: 1,
                        created_at: now,
                        updated_at: now,
                    },
                    Assignment {
                        id: "assignment-2".to_string(),
                        work_unit_id: "wu-1".to_string(),
                        worker_id: "worker-a".to_string(),
                        worker_session_id: "session-1".to_string(),
                        instructions: "Second bounded pass".to_string(),
                        communication_seed: None,
                        status: AssignmentStatus::AwaitingDecision,
                        attempt_number: 2,
                        created_at: now,
                        updated_at: now,
                    },
                ],
                reports: vec![
                    Report {
                        id: "report-1".to_string(),
                        work_unit_id: "wu-1".to_string(),
                        assignment_id: "assignment-1".to_string(),
                        worker_id: "worker-a".to_string(),
                        disposition: ReportDisposition::Completed,
                        summary: "Initial snapshot path landed cleanly.".to_string(),
                        findings: vec!["Snapshot summaries added.".to_string()],
                        blockers: Vec::new(),
                        questions: Vec::new(),
                        recommended_next_actions: vec!["Review event model".to_string()],
                        confidence: ReportConfidence::High,
                        raw_output: "{}".to_string(),
                        parse_result: ReportParseResult::Parsed,
                        needs_supervisor_review: false,
                        created_at: now,
                    },
                    Report {
                        id: "report-2".to_string(),
                        work_unit_id: "wu-1".to_string(),
                        assignment_id: "assignment-2".to_string(),
                        worker_id: "worker-a".to_string(),
                        disposition: ReportDisposition::Partial,
                        summary: "Snapshot path is implemented, review is required.".to_string(),
                        findings: vec!["Event summaries need one more pass.".to_string()],
                        blockers: Vec::new(),
                        questions: vec!["Should summaries include objective?".to_string()],
                        recommended_next_actions: vec!["Supervisor decide continue.".to_string()],
                        confidence: ReportConfidence::Medium,
                        raw_output: "noise + json".to_string(),
                        parse_result: ReportParseResult::Ambiguous,
                        needs_supervisor_review: true,
                        created_at: now,
                    },
                ],
                decisions: vec![Decision {
                    id: "decision-1".to_string(),
                    work_unit_id: "wu-1".to_string(),
                    report_id: Some("report-2".to_string()),
                    decision_type: DecisionType::Continue,
                    rationale: "Need one more bounded pass.".to_string(),
                    created_at: now,
                }],
                proposals: vec![failed, open],
            }
        }
        "wu-2" => ipc::WorkunitGetResponse {
            work_unit: WorkUnit {
                id: "wu-2".to_string(),
                workstream_id: "ws-1".to_string(),
                title: "Event wiring".to_string(),
                task_statement: "Surface collaboration events in the daemon event stream."
                    .to_string(),
                status: WorkUnitStatus::Ready,
                dependencies: vec!["wu-1".to_string()],
                latest_report_id: Some("report-3".to_string()),
                current_assignment_id: Some("assignment-3".to_string()),
                created_at: now,
                updated_at: now,
            },
            assignments: vec![Assignment {
                id: "assignment-3".to_string(),
                work_unit_id: "wu-2".to_string(),
                worker_id: "worker-a".to_string(),
                worker_session_id: "session-1".to_string(),
                instructions: "Prepare event surface".to_string(),
                communication_seed: None,
                status: AssignmentStatus::Created,
                attempt_number: 3,
                created_at: now,
                updated_at: now,
            }],
            reports: vec![Report {
                id: "report-3".to_string(),
                work_unit_id: "wu-2".to_string(),
                assignment_id: "assignment-3".to_string(),
                worker_id: "worker-a".to_string(),
                disposition: ReportDisposition::Completed,
                summary: "Clean report for event wiring.".to_string(),
                findings: Vec::new(),
                blockers: Vec::new(),
                questions: Vec::new(),
                recommended_next_actions: Vec::new(),
                confidence: ReportConfidence::High,
                raw_output: "{}".to_string(),
                parse_result: ReportParseResult::Parsed,
                needs_supervisor_review: false,
                created_at: now,
            }],
            decisions: Vec::new(),
            proposals: Vec::new(),
        },
        _ => panic!("unknown sample work unit"),
    }
}

#[tokio::test]
async fn initial_snapshot_load_populates_state() {
    let harness = AppHarness::new(sample_snapshot()).await.unwrap();
    let connection = harness.connection_vm();
    let overview = harness.overview_vm();
    let threads = harness.thread_list_vm();
    let workstreams = harness.workstream_list_vm();
    let work_units = harness.work_unit_list_vm();

    assert_eq!(harness.current_view(), TopLevelView::Overview);
    assert_eq!(connection.daemon_phase, DaemonConnectionPhase::Connected);
    assert_eq!(connection.upstream_status, "connected");
    assert!(
        overview
            .connection
            .lines
            .iter()
            .any(|line| line.contains("daemon: connected"))
    );
    assert!(
        overview
            .recent_events
            .lines
            .iter()
            .any(|line| line.contains("loaded thread-1"))
    );
    assert_eq!(threads.rows.len(), 2);
    assert!(threads.rows[0].selected);
    assert_eq!(workstreams.rows.len(), 2);
    assert!(workstreams.rows[0].selected);
    assert_eq!(work_units.rows.len(), 2);
}

#[tokio::test]
async fn event_stream_updates_connection_state() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::UpstreamStatusChanged {
                upstream: ConnectionState {
                    endpoint: "ws://127.0.0.1:4500".to_string(),
                    status: "connect_failed".to_string(),
                    detail: Some("boom".to_string()),
                },
            },
        ))
        .await
        .unwrap();

    let connection = harness.connection_vm();
    assert_eq!(connection.upstream_status, "connect_failed");
    assert_eq!(connection.upstream_detail.as_deref(), Some("boom"));
}

#[tokio::test]
async fn active_turn_state_drives_prompt_in_flight_and_thread_badge() {
    let mut snapshot = sample_snapshot();
    snapshot.session.active_turns = vec![ipc::ActiveTurn {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-7".to_string(),
        status: "in_progress".to_string(),
        updated_at: Utc::now(),
    }];

    let harness = AppHarness::new(snapshot).await.unwrap();
    let overview = harness.overview_vm();
    let threads = harness.thread_list_vm();

    assert!(harness.prompt_in_flight());
    assert!(
        overview
            .active_work
            .lines
            .iter()
            .any(|line| line.contains("thread-1 / turn-7 [in_progress]"))
    );
    assert_eq!(threads.rows[0].status, "active");
    assert_eq!(
        threads.rows[0].turn_badge.as_deref(),
        Some("active attachable")
    );
}

#[tokio::test]
async fn thread_selection_loads_detail() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_thread(sample_thread_view("thread-2", "later", "second output"))
        .await;
    harness
        .set_turn(ipc::TurnAttachResponse {
            turn: Some(sample_turn_state(
                "thread-2",
                "turn-1",
                ipc::TurnLifecycleState::Completed,
                "completed",
                false,
            )),
            attached: false,
            reason: Some("turn already completed; only terminal state is queryable".to_string()),
        })
        .await;
    harness.dispatch(UserAction::SelectNextThread).await;

    let threads = harness.thread_list_vm();
    let detail = harness.thread_detail_vm();
    assert!(threads.rows[1].selected);
    assert!(detail.title.contains("thread-2"));
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("lifecycle=completed"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("second output"))
    );
}

#[tokio::test]
async fn streamed_deltas_accumulate_in_selected_thread() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::TurnUpdated {
                thread_id: "thread-1".to_string(),
                turn: ipc::TurnView {
                    id: "turn-2".to_string(),
                    status: "in_progress".to_string(),
                    error_message: None,
                    items: Vec::new(),
                },
            },
        ))
        .await
        .unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::OutputDelta {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-2".to_string(),
                item_id: "item-2".to_string(),
                delta: "hello ".to_string(),
            },
        ))
        .await
        .unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::OutputDelta {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-2".to_string(),
                item_id: "item-2".to_string(),
                delta: "world".to_string(),
            },
        ))
        .await
        .unwrap();

    let detail = harness.thread_detail_vm();
    assert!(detail.lines.iter().any(|line| line.contains("hello world")));
}

#[tokio::test]
async fn completed_turn_clears_in_progress_marker() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_active_turns(vec![sample_turn_state(
            "thread-1",
            "turn-1",
            ipc::TurnLifecycleState::Active,
            "in_progress",
            true,
        )])
        .await;
    harness.dispatch(UserAction::Refresh).await;
    assert!(harness.prompt_in_flight());

    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::TurnUpdated {
                thread_id: "thread-1".to_string(),
                turn: ipc::TurnView {
                    id: "turn-1".to_string(),
                    status: "completed".to_string(),
                    error_message: None,
                    items: Vec::new(),
                },
            },
        ))
        .await
        .unwrap();

    assert!(!harness.prompt_in_flight());
}

#[tokio::test]
async fn prompt_submission_is_disabled_in_read_only_console() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness.dispatch(UserAction::SubmitPrompt).await;

    let banner = harness.state().banner.clone().expect("banner");
    let commands = harness.recorded_commands().await;
    assert_eq!(banner.level, BannerLevel::Info);
    assert!(banner.message.contains("read-only"));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, BackendCommand::SubmitPrompt { .. }))
    );
}

#[tokio::test]
async fn backend_failure_surfaces_in_banner_state() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness.fail_snapshot_once("cannot load snapshot").await;
    harness.dispatch(UserAction::Refresh).await;

    let banner = harness.state().banner.clone().unwrap();
    assert_eq!(banner.level, BannerLevel::Warning);
    assert!(banner.message.contains("Reconnecting"));
    assert_eq!(
        harness.state().daemon_phase,
        DaemonConnectionPhase::Reconnecting
    );
}

#[tokio::test]
async fn reconnect_recovers_with_snapshot_then_resubscribe() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    let mut recovered = sample_snapshot();
    recovered.threads = vec![sample_thread_summary("thread-2", "recovered", 300)];
    recovered.session.active_thread_id = Some("thread-2".to_string());
    recovered.active_thread = Some(sample_thread_view("thread-2", "recovered", "after restart"));
    recovered.collaboration.workstreams = vec![ipc::WorkstreamSummary {
        id: "ws-9".to_string(),
        title: "Recovered collaboration".to_string(),
        objective: "Reload collaboration snapshot.".to_string(),
        status: WorkstreamStatus::Active,
        priority: "high".to_string(),
        updated_at: Utc::now(),
    }];
    recovered.collaboration.work_units = vec![ipc::WorkUnitSummary {
        id: "wu-9".to_string(),
        workstream_id: "ws-9".to_string(),
        title: "Recovered unit".to_string(),
        status: WorkUnitStatus::Ready,
        dependency_count: 0,
        current_assignment_id: None,
        latest_report_id: None,
        proposal: None,
        updated_at: Utc::now(),
    }];
    recovered.collaboration.assignments = Vec::new();
    recovered.collaboration.reports = Vec::new();
    recovered.collaboration.decisions = Vec::new();
    harness.replace_snapshot(recovered).await;

    harness.disconnect_events().await;
    harness.process().await;

    assert_eq!(
        harness.state().daemon_phase,
        DaemonConnectionPhase::Reconnecting
    );
    assert_eq!(harness.snapshot_requests().await, 1);
    assert_eq!(harness.subscribe_requests().await, 1);

    harness.force_reconnect_now();
    harness.process().await;

    let connection = harness.connection_vm();
    let detail = harness.thread_detail_vm();
    let workstreams = harness.workstream_list_vm();
    let work_units = harness.work_unit_list_vm();
    assert_eq!(connection.daemon_phase, DaemonConnectionPhase::Connected);
    assert_eq!(harness.snapshot_requests().await, 2);
    assert_eq!(harness.subscribe_requests().await, 2);
    assert_eq!(harness.thread_list_vm().rows.len(), 1);
    assert!(detail.title.contains("thread-2"));
    assert_eq!(workstreams.rows.len(), 1);
    assert_eq!(workstreams.rows[0].title, "Recovered collaboration");
    assert_eq!(work_units.rows.len(), 1);
    assert_eq!(work_units.rows[0].title, "Recovered unit");
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("after restart"))
    );
}

#[tokio::test]
async fn collaboration_snapshot_drives_rendering() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let workstream_detail = harness.workstream_detail_vm();
    let work_units = harness.work_unit_list_vm();
    let assignments = harness.assignment_list_vm();
    let detail = harness.collaboration_detail_vm();
    let history = harness.collaboration_history_vm();

    assert!(
        workstream_detail
            .lines
            .iter()
            .any(|line| line.contains("Harden collaboration snapshot semantics."))
    );
    assert!(
        work_units
            .rows
            .iter()
            .any(|row| row.title == "Snapshot wiring" && row.needs_supervisor_review)
    );
    assert!(
        assignments
            .rows
            .iter()
            .any(|row| row.id == "assignment-2" && row.worker_session_id == "session-1")
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("report: report-2 parse=ambiguous review=true"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("decision_rationale: Need one more bounded pass."))
    );
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("assignment-1 [closed]"))
    );
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("report-2 [partial ambiguous review=true]"))
    );
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("proposal-1 [open] decision=continue"))
    );
}

#[tokio::test]
async fn proposal_state_renders_distinct_from_report_and_decision_state() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness.dispatch(UserAction::Refresh).await;
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;

    let work_units = harness.work_unit_list_vm();
    let detail = harness.collaboration_detail_vm();
    let history = harness.collaboration_history_vm();
    let rendered = harness.render_text(160, 42);

    assert!(work_units.rows.iter().any(|row| {
        row.title == "Snapshot wiring"
            && row.latest_report_parse_result == "ambiguous"
            && row.proposal_status.contains("open/continue")
            && row.latest_decision == "continue"
    }));
    assert!(work_units.rows.iter().any(|row| {
        row.title == "Event wiring" && row.proposal_status.contains("generation_failed/backend")
    }));
    assert!(detail.lines.iter().any(|line| {
        line.contains(
            "proposal: proposal-1 status=open latest_decision=continue open=true stale_or_superseded=false failed=true edits=false",
        )
    }));
    assert!(history.lines.iter().any(|line| line == "Proposals"));
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("proposal-1 [open] decision=continue"))
    );
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("proposal-failed [generation_failed] decision=-"))
    );
    assert!(rendered.contains("proposal=open/continue"));
    assert!(rendered.contains("proposal=generation_failed/backend"));
}

#[tokio::test]
async fn proposal_lifecycle_event_refreshes_selected_work_unit_detail() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let mut updated_detail = sample_workunit_detail("wu-1");
    let approved = updated_detail
        .proposals
        .iter_mut()
        .find(|proposal| proposal.id == "proposal-1")
        .expect("proposal");
    approved.status = SupervisorProposalStatus::Approved;
    approved.reviewed_at = Some(Utc::now());
    approved.approved_proposal = approved.proposal.clone();

    let mut updated_work_unit = sample_snapshot()
        .collaboration
        .work_units
        .into_iter()
        .find(|work_unit| work_unit.id == "wu-1")
        .expect("work unit");
    updated_work_unit.proposal = Some(ipc::WorkUnitProposalSummary {
        latest_status: SupervisorProposalStatus::Approved,
        latest_has_approval_edits: true,
        latest_reviewed_at: Some(Utc::now()),
        has_open_proposal: false,
        open_proposal_id: None,
        open_proposed_decision_type: None,
        has_generation_failed: true,
        has_stale_or_superseded: false,
        ..sample_proposal_summary(
            SupervisorProposalStatus::Approved,
            Some(DecisionType::Continue),
        )
    });

    harness.set_workunit_detail(updated_detail).await;
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::ProposalLifecycle {
                action: ipc::ProposalLifecycleAction::Approved,
                proposal: ipc::ProposalSummary {
                    id: "proposal-1".to_string(),
                    primary_work_unit_id: "wu-1".to_string(),
                    source_report_id: "report-2".to_string(),
                    status: SupervisorProposalStatus::Approved,
                    proposed_decision_type: Some(DecisionType::Continue),
                    created_at: Utc::now(),
                    reviewed_at: Some(Utc::now()),
                    has_approval_edits: true,
                    generation_failure_stage: None,
                    reasoner_model: "test-supervisor".to_string(),
                },
                work_unit: updated_work_unit,
            },
        ))
        .await
        .unwrap();

    let detail = harness.collaboration_detail_vm();
    let history = harness.collaboration_history_vm();

    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("status=approved latest_decision=continue open=false"))
    );
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("proposal-1 [approved] decision=continue edits=false"))
    );
}

#[tokio::test]
async fn collaboration_events_refresh_summaries_incrementally() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::WorkstreamLifecycle {
                action: ipc::CollaborationLifecycleAction::Created,
                workstream: ipc::WorkstreamSummary {
                    id: "ws-3".to_string(),
                    title: "Fresh stream".to_string(),
                    objective: "Add new read-only surface.".to_string(),
                    status: WorkstreamStatus::Active,
                    priority: "medium".to_string(),
                    updated_at: Utc::now(),
                },
            },
        ))
        .await
        .unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::WorkUnitLifecycle {
                action: ipc::CollaborationLifecycleAction::Created,
                work_unit: ipc::WorkUnitSummary {
                    id: "wu-4".to_string(),
                    workstream_id: "ws-3".to_string(),
                    title: "Render panel".to_string(),
                    status: WorkUnitStatus::Running,
                    dependency_count: 0,
                    current_assignment_id: Some("assignment-4".to_string()),
                    latest_report_id: Some("report-4".to_string()),
                    proposal: None,
                    updated_at: Utc::now(),
                },
            },
        ))
        .await
        .unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::AssignmentLifecycle {
                action: ipc::AssignmentLifecycleAction::Started,
                assignment: ipc::AssignmentSummary {
                    id: "assignment-4".to_string(),
                    work_unit_id: "wu-4".to_string(),
                    worker_id: "worker-b".to_string(),
                    worker_session_id: "session-4".to_string(),
                    status: AssignmentStatus::Running,
                    attempt_number: 1,
                    updated_at: Utc::now(),
                },
            },
        ))
        .await
        .unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::ReportRecorded {
                report: ipc::ReportSummary {
                    id: "report-4".to_string(),
                    work_unit_id: "wu-4".to_string(),
                    assignment_id: "assignment-4".to_string(),
                    worker_id: "worker-b".to_string(),
                    disposition: ReportDisposition::Completed,
                    summary: "Panel rendering is visible.".to_string(),
                    confidence: ReportConfidence::High,
                    parse_result: ReportParseResult::Parsed,
                    needs_supervisor_review: false,
                    created_at: Utc::now(),
                },
            },
        ))
        .await
        .unwrap();
    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::DecisionApplied {
                decision: ipc::DecisionSummary {
                    id: "decision-4".to_string(),
                    work_unit_id: "wu-4".to_string(),
                    report_id: Some("report-4".to_string()),
                    decision_type: DecisionType::MarkComplete,
                    rationale: "Read-only visibility is good enough.".to_string(),
                    created_at: Utc::now(),
                },
            },
        ))
        .await
        .unwrap();

    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    for _ in 0..3 {
        if harness
            .workstream_detail_vm()
            .title
            .contains("Fresh stream")
        {
            break;
        }
        harness.dispatch(UserAction::SelectPreviousInView).await;
    }

    let workstreams = harness.workstream_list_vm();
    let work_units = harness.work_unit_list_vm();
    let assignments = harness.assignment_list_vm();
    let detail = harness.collaboration_detail_vm();

    assert!(
        workstreams
            .rows
            .iter()
            .any(|row| row.title == "Fresh stream")
    );
    assert!(
        harness
            .workstream_detail_vm()
            .title
            .contains("Fresh stream")
    );
    assert!(
        work_units
            .rows
            .iter()
            .any(|row| { row.title == "Render panel" && row.latest_decision == "mark_complete" })
    );
    assert!(
        assignments
            .rows
            .iter()
            .any(|row| row.id == "assignment-4" && row.worker_id == "worker-b")
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("decision_rationale: Read-only visibility is good enough."))
    );
}

#[tokio::test]
async fn parse_result_and_supervisor_review_display_are_distinct() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness.dispatch(UserAction::Refresh).await;
    let work_units = harness.work_unit_list_vm();
    let detail = harness.collaboration_detail_vm();
    let history = harness.collaboration_history_vm();

    assert!(work_units.rows.iter().any(|row| {
        row.title == "Snapshot wiring"
            && row.latest_report_parse_result == "ambiguous"
            && row.needs_supervisor_review
    }));
    assert!(work_units.rows.iter().any(|row| {
        row.title == "Event wiring"
            && row.latest_report_parse_result == "parsed"
            && !row.needs_supervisor_review
    }));
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("report: report-2 parse=ambiguous review=true"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("report: report-2 parse=ambiguous review=true"))
    );
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("report-2 [partial ambiguous review=true]"))
    );
}

#[tokio::test]
async fn reused_worker_session_does_not_imply_same_assignment_continuity() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness.dispatch(UserAction::Refresh).await;
    let assignments = harness.assignment_list_vm();
    let detail = harness.collaboration_detail_vm();
    let history = harness.collaboration_history_vm();

    assert!(
        assignments
            .rows
            .iter()
            .any(|row| { row.id == "assignment-2" && row.worker_session_id == "session-1" })
    );
    assert!(
        assignments
            .rows
            .iter()
            .any(|row| { row.id == "assignment-3" && row.worker_session_id == "session-1" })
    );
    assert!(detail.lines.iter().any(|line| {
        line.contains(
            "assignment: assignment-2 [awaiting_decision] worker=worker-a session=session-1",
        )
    }));
    assert!(history.lines.iter().any(|line| {
        line.contains("assignment-1 [closed] attempt=1 worker=worker-a session=session-1")
    }));
    assert!(history.lines.iter().any(|line| {
        line.contains(
            "assignment-2 [awaiting_decision] attempt=2 worker=worker-a session=session-1",
        )
    }));
}

#[tokio::test]
async fn collaboration_history_shows_failed_interrupted_and_lost_states_explicitly() {
    let mut snapshot = sample_snapshot();
    snapshot.collaboration.work_units = vec![ipc::WorkUnitSummary {
        id: "wu-f".to_string(),
        workstream_id: "ws-1".to_string(),
        title: "Runtime truth".to_string(),
        status: WorkUnitStatus::AwaitingDecision,
        dependency_count: 0,
        current_assignment_id: Some("assignment-i".to_string()),
        latest_report_id: Some("report-i".to_string()),
        proposal: None,
        updated_at: Utc::now(),
    }];
    snapshot.collaboration.assignments = vec![ipc::AssignmentSummary {
        id: "assignment-i".to_string(),
        work_unit_id: "wu-f".to_string(),
        worker_id: "worker-a".to_string(),
        worker_session_id: "session-2".to_string(),
        status: AssignmentStatus::Interrupted,
        attempt_number: 2,
        updated_at: Utc::now(),
    }];
    snapshot.collaboration.reports = vec![ipc::ReportSummary {
        id: "report-i".to_string(),
        work_unit_id: "wu-f".to_string(),
        assignment_id: "assignment-i".to_string(),
        worker_id: "worker-a".to_string(),
        disposition: ReportDisposition::Interrupted,
        summary: "Interrupted raw output retained.".to_string(),
        confidence: ReportConfidence::Unknown,
        parse_result: ReportParseResult::Invalid,
        needs_supervisor_review: true,
        created_at: Utc::now(),
    }];
    snapshot.collaboration.decisions = vec![ipc::DecisionSummary {
        id: "decision-i".to_string(),
        work_unit_id: "wu-f".to_string(),
        report_id: Some("report-i".to_string()),
        decision_type: DecisionType::EscalateToHuman,
        rationale: "Supervisor review is required.".to_string(),
        created_at: Utc::now(),
    }];

    let mut harness = AppHarness::new(snapshot).await.unwrap();
    harness
        .set_workunit_detail(ipc::WorkunitGetResponse {
            work_unit: WorkUnit {
                id: "wu-f".to_string(),
                workstream_id: "ws-1".to_string(),
                title: "Runtime truth".to_string(),
                task_statement: "Show honest failure and interruption states.".to_string(),
                status: WorkUnitStatus::AwaitingDecision,
                dependencies: Vec::new(),
                latest_report_id: Some("report-i".to_string()),
                current_assignment_id: Some("assignment-i".to_string()),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            assignments: vec![
                Assignment {
                    id: "assignment-f".to_string(),
                    work_unit_id: "wu-f".to_string(),
                    worker_id: "worker-a".to_string(),
                    worker_session_id: "session-1".to_string(),
                    instructions: "Failed start".to_string(),
                    communication_seed: None,
                    status: AssignmentStatus::Failed,
                    attempt_number: 1,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                },
                Assignment {
                    id: "assignment-i".to_string(),
                    work_unit_id: "wu-f".to_string(),
                    worker_id: "worker-a".to_string(),
                    worker_session_id: "session-2".to_string(),
                    instructions: "Interrupted run".to_string(),
                    communication_seed: None,
                    status: AssignmentStatus::Interrupted,
                    attempt_number: 2,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                },
            ],
            reports: vec![Report {
                id: "report-i".to_string(),
                work_unit_id: "wu-f".to_string(),
                assignment_id: "assignment-i".to_string(),
                worker_id: "worker-a".to_string(),
                disposition: ReportDisposition::Interrupted,
                summary: "Interrupted raw output retained.".to_string(),
                findings: Vec::new(),
                blockers: vec!["Supervisor must decide the next step.".to_string()],
                questions: Vec::new(),
                recommended_next_actions: Vec::new(),
                confidence: ReportConfidence::Unknown,
                raw_output: "partial".to_string(),
                parse_result: ReportParseResult::Invalid,
                needs_supervisor_review: true,
                created_at: Utc::now(),
            }],
            decisions: vec![Decision {
                id: "decision-i".to_string(),
                work_unit_id: "wu-f".to_string(),
                report_id: Some("report-i".to_string()),
                decision_type: DecisionType::EscalateToHuman,
                rationale: "Supervisor review is required.".to_string(),
                created_at: Utc::now(),
            }],
            proposals: Vec::new(),
        })
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let detail = harness.collaboration_detail_vm();
    let history = harness.collaboration_history_vm();

    assert!(detail.lines.iter().any(|line| {
        line.contains("assignment: assignment-i [interrupted] worker=worker-a session=session-2")
    }));
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("report: report-i parse=invalid review=true"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("report: report-i parse=invalid review=true"))
    );
    assert!(history.lines.iter().any(|line| {
        line.contains("assignment-f [failed] attempt=1 worker=worker-a session=session-1")
    }));
    assert!(history.lines.iter().any(|line| line.contains(
        "assignment-i [interrupted] attempt=2 worker=worker-a session=session-2 current"
    )));
}

#[tokio::test]
async fn focus_switches_collaboration_navigation_without_overwriting_thread_state() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    harness.dispatch(UserAction::SelectNextInView).await;
    harness.dispatch(UserAction::CycleCollaborationFocus).await;

    let status = harness.collaboration_status_vm();
    let detail = harness.workstream_detail_vm();
    let threads = harness.thread_list_vm();

    assert_eq!(status.focus, CollaborationFocus::WorkUnits);
    assert!(detail.title.contains("Deferred work"));
    assert!(threads.rows[0].selected);
}

#[tokio::test]
async fn collaboration_focus_cycle_order_is_workstreams_then_work_units_then_workstreams() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;

    assert_eq!(
        harness.collaboration_focus(),
        CollaborationFocus::Workstreams
    );
    harness.dispatch(UserAction::CycleCollaborationFocus).await;
    assert_eq!(harness.collaboration_focus(), CollaborationFocus::WorkUnits);
    harness.dispatch(UserAction::CycleCollaborationFocus).await;
    assert_eq!(
        harness.collaboration_focus(),
        CollaborationFocus::Workstreams
    );
}

#[tokio::test]
async fn top_level_view_navigation_switches_between_overview_threads_and_collaboration() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();

    assert_eq!(harness.current_view(), TopLevelView::Overview);
    harness.dispatch(UserAction::CycleView).await;
    assert_eq!(harness.current_view(), TopLevelView::Threads);
    harness.dispatch(UserAction::CycleView).await;
    assert_eq!(harness.current_view(), TopLevelView::Collaboration);
    harness.dispatch(UserAction::CycleView).await;
    assert_eq!(harness.current_view(), TopLevelView::Supervisor);
    harness.dispatch(UserAction::CycleView).await;
    assert_eq!(harness.current_view(), TopLevelView::Overview);
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Overview))
        .await;
    assert_eq!(harness.current_view(), TopLevelView::Overview);
}

#[tokio::test]
async fn supervisor_view_loads_models_and_renders_available_models() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();

    harness
        .dispatch(UserAction::ShowView(TopLevelView::Supervisor))
        .await;

    let rendered = harness.render_text(160, 42);
    assert!(rendered.contains("Supervisor"));
    assert!(rendered.contains("Available Models"));
    assert!(rendered.contains("codex-small"));
    assert_eq!(harness.state().daemon_models.len(), 2);
    assert!(
        harness
            .recorded_commands()
            .await
            .contains(&BackendCommand::LoadModels)
    );
}

#[tokio::test]
async fn supervisor_stop_daemon_dispatches_stop_request_command() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();

    harness
        .dispatch(UserAction::ShowView(TopLevelView::Supervisor))
        .await;
    harness.dispatch(UserAction::StopDaemon).await;

    let commands = harness.recorded_commands().await;
    assert!(commands.contains(&BackendCommand::StopDaemon));
    assert_eq!(
        harness
            .state()
            .banner
            .as_ref()
            .map(|banner| banner.message.as_str()),
        Some("Daemon stop requested.")
    );
}

#[tokio::test]
async fn reconnect_keeps_selected_top_level_view_and_collaboration_focus() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    harness.dispatch(UserAction::CycleCollaborationFocus).await;

    harness.replace_snapshot(sample_snapshot()).await;
    harness.disconnect_events().await;
    harness.process().await;
    harness.force_reconnect_now();
    harness.process().await;

    assert_eq!(harness.current_view(), TopLevelView::Collaboration);
    assert_eq!(harness.collaboration_focus(), CollaborationFocus::WorkUnits);
}

#[tokio::test]
async fn j_and_k_only_move_the_focused_list_selection() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();

    let initial_thread = harness.selected_thread_id().map(str::to_string);
    let initial_workstream = harness.selected_workstream_id().map(str::to_string);
    let initial_work_unit = harness.selected_work_unit_id().map(str::to_string);

    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    harness.dispatch(UserAction::SelectNextInView).await;

    assert_eq!(harness.current_view(), TopLevelView::Collaboration);
    assert_eq!(
        harness.collaboration_focus(),
        CollaborationFocus::Workstreams
    );
    assert_eq!(harness.selected_thread_id(), initial_thread.as_deref());
    assert_ne!(
        harness.selected_workstream_id(),
        initial_workstream.as_deref()
    );
    assert_ne!(
        harness.selected_work_unit_id(),
        initial_work_unit.as_deref()
    );

    let workstream_after_move = harness.selected_workstream_id().map(str::to_string);
    let thread_after_workstream_move = harness.selected_thread_id().map(str::to_string);
    harness.dispatch(UserAction::CycleCollaborationFocus).await;
    harness.dispatch(UserAction::SelectPreviousInView).await;

    assert_eq!(harness.collaboration_focus(), CollaborationFocus::WorkUnits);
    assert_eq!(
        harness.selected_thread_id(),
        thread_after_workstream_move.as_deref()
    );
    assert_eq!(
        harness.selected_workstream_id(),
        workstream_after_move.as_deref()
    );
    assert_eq!(harness.selected_work_unit_id(), Some("wu-3"));
}

#[tokio::test]
async fn workstream_navigation_updates_selected_work_unit_and_rendered_context() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    harness.dispatch(UserAction::SelectNextInView).await;

    let status = harness.collaboration_status_vm();
    let workstreams = harness.workstream_list_vm();
    let work_units = harness.work_unit_list_vm();
    let history = harness.collaboration_history_vm();
    let rendered = harness.render_text(160, 42);

    assert_eq!(status.focus, CollaborationFocus::Workstreams);
    assert!(
        workstreams
            .rows
            .iter()
            .any(|row| row.title == "Deferred work" && row.selected)
    );
    assert_eq!(work_units.rows.len(), 1);
    assert_eq!(work_units.rows[0].title, "Out of scope");
    assert!(work_units.rows[0].selected);
    assert!(history.title.contains("Out of scope"));
    assert!(rendered.contains("focus=workstreams"));
    assert!(rendered.contains("Workstreams <focus>"));
    assert!(rendered.contains("> Deferred work [blocked]"));
    assert!(rendered.contains("> Out of scope"));
    assert!(rendered.contains("[blocked]"));
    assert!(rendered.contains("selected stream: Deferred work"));
}

#[tokio::test]
async fn work_unit_navigation_refreshes_detail_history_and_fetch_command() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    harness.dispatch(UserAction::CycleCollaborationFocus).await;
    harness.dispatch(UserAction::SelectNextInView).await;

    let status = harness.collaboration_status_vm();
    let work_units = harness.work_unit_list_vm();
    let detail = harness.collaboration_detail_vm();
    let history = harness.collaboration_history_vm();
    let commands = harness.recorded_commands().await;
    let rendered = harness.render_text(160, 42);

    assert_eq!(status.focus, CollaborationFocus::WorkUnits);
    assert!(
        work_units
            .rows
            .iter()
            .any(|row| row.title == "Event wiring" && row.selected)
    );
    assert!(detail.title.contains("Work Unit wu-2"));
    assert!(history.title.contains("Event wiring"));
    assert!(commands.contains(&BackendCommand::GetWorkUnit {
        work_unit_id: "wu-2".to_string(),
    }));
    assert!(rendered.contains("focus=work_units"));
    assert!(rendered.contains("Work Units <focus>"));
    assert!(rendered.contains("> Event wiring"));
    assert!(rendered.contains("[ready]"));
    assert!(rendered.contains("assignment-3"));
    assert!(rendered.contains("[created]"));
}

#[tokio::test]
async fn late_detail_for_non_selected_work_unit_does_not_overwrite_visible_history() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    harness.dispatch(UserAction::CycleCollaborationFocus).await;
    harness.dispatch(UserAction::SelectNextInView).await;
    assert_eq!(harness.selected_work_unit_id(), Some("wu-2"));
    harness.dispatch(UserAction::SelectPreviousInView).await;
    assert_eq!(harness.selected_work_unit_id(), Some("wu-1"));

    harness
        .inject_ui_event(UiEvent::WorkUnitDetailLoaded(sample_workunit_detail(
            "wu-2",
        )))
        .await;

    let detail = harness.collaboration_detail_vm();
    let history = harness.collaboration_history_vm();
    assert!(detail.title.contains("Work Unit wu-1"));
    assert!(history.title.contains("Snapshot wiring"));
    assert!(
        !history
            .lines
            .iter()
            .any(|line| line.contains("assignment-3 [created]"))
    );
}

#[tokio::test]
async fn collaboration_detail_does_not_overwrite_thread_detail_and_vice_versa() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_thread(sample_thread_view("thread-2", "later", "second output"))
        .await;
    harness
        .set_turn(ipc::TurnAttachResponse {
            turn: Some(sample_turn_state(
                "thread-2",
                "turn-1",
                ipc::TurnLifecycleState::Completed,
                "completed",
                false,
            )),
            attached: false,
            reason: Some("turn already completed; only terminal state is queryable".to_string()),
        })
        .await;
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Threads))
        .await;
    harness.dispatch(UserAction::SelectNextInView).await;

    harness
        .set_workunit_detail(sample_workunit_detail("wu-2"))
        .await;
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    harness.dispatch(UserAction::CycleCollaborationFocus).await;
    harness.dispatch(UserAction::SelectNextInView).await;

    let collaboration_detail = harness.collaboration_detail_vm();
    let collaboration_history = harness.collaboration_history_vm();
    assert!(collaboration_detail.title.contains("Work Unit wu-2"));
    assert!(collaboration_history.title.contains("Event wiring"));

    harness
        .dispatch(UserAction::ShowView(TopLevelView::Threads))
        .await;
    let thread_detail = harness.thread_detail_vm();
    assert!(thread_detail.title.contains("thread-2"));
    assert!(
        thread_detail
            .lines
            .iter()
            .any(|line| line.contains("second output"))
    );

    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    let collaboration_history_again = harness.collaboration_history_vm();
    assert!(collaboration_history_again.title.contains("Event wiring"));
    assert!(
        !collaboration_history_again
            .lines
            .iter()
            .any(|line| line.contains("second output"))
    );
}

#[tokio::test]
async fn selected_work_unit_history_renders_assignment_report_and_decision_chain() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let history = harness.collaboration_history_vm();

    assert!(history.title.contains("Snapshot wiring"));
    assert!(history.lines.iter().any(|line| line == "Assignments"));
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("assignment-1 [closed]"))
    );
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("assignment-2 [awaiting_decision]"))
    );
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("report-1 [completed parsed review=false]"))
    );
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("decision-1 [continue]"))
    );
}

#[tokio::test]
async fn reconnect_refreshes_history_for_selected_work_unit() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    let mut recovered = sample_snapshot();
    recovered.collaboration.workstreams = vec![ipc::WorkstreamSummary {
        id: "ws-9".to_string(),
        title: "Recovered collaboration".to_string(),
        objective: "Reload collaboration snapshot.".to_string(),
        status: WorkstreamStatus::Active,
        priority: "high".to_string(),
        updated_at: Utc::now(),
    }];
    recovered.collaboration.work_units = vec![ipc::WorkUnitSummary {
        id: "wu-9".to_string(),
        workstream_id: "ws-9".to_string(),
        title: "Recovered unit".to_string(),
        status: WorkUnitStatus::AwaitingDecision,
        dependency_count: 0,
        current_assignment_id: Some("assignment-9".to_string()),
        latest_report_id: Some("report-9".to_string()),
        proposal: None,
        updated_at: Utc::now(),
    }];
    recovered.collaboration.assignments = vec![ipc::AssignmentSummary {
        id: "assignment-9".to_string(),
        work_unit_id: "wu-9".to_string(),
        worker_id: "worker-r".to_string(),
        worker_session_id: "session-9".to_string(),
        status: AssignmentStatus::Failed,
        attempt_number: 1,
        updated_at: Utc::now(),
    }];
    recovered.collaboration.reports = vec![ipc::ReportSummary {
        id: "report-9".to_string(),
        work_unit_id: "wu-9".to_string(),
        assignment_id: "assignment-9".to_string(),
        worker_id: "worker-r".to_string(),
        disposition: ReportDisposition::Failed,
        summary: "Recovered history summary.".to_string(),
        confidence: ReportConfidence::Unknown,
        parse_result: ReportParseResult::Invalid,
        needs_supervisor_review: true,
        created_at: Utc::now(),
    }];
    recovered.collaboration.decisions = vec![ipc::DecisionSummary {
        id: "decision-9".to_string(),
        work_unit_id: "wu-9".to_string(),
        report_id: Some("report-9".to_string()),
        decision_type: DecisionType::EscalateToHuman,
        rationale: "Recovered issue needs review.".to_string(),
        created_at: Utc::now(),
    }];
    harness.replace_snapshot(recovered).await;
    harness
        .set_workunit_detail(ipc::WorkunitGetResponse {
            work_unit: WorkUnit {
                id: "wu-9".to_string(),
                workstream_id: "ws-9".to_string(),
                title: "Recovered unit".to_string(),
                task_statement: "Recovered task.".to_string(),
                status: WorkUnitStatus::AwaitingDecision,
                dependencies: Vec::new(),
                latest_report_id: Some("report-9".to_string()),
                current_assignment_id: Some("assignment-9".to_string()),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            assignments: vec![Assignment {
                id: "assignment-9".to_string(),
                work_unit_id: "wu-9".to_string(),
                worker_id: "worker-r".to_string(),
                worker_session_id: "session-9".to_string(),
                instructions: "Recovered work".to_string(),
                communication_seed: None,
                status: AssignmentStatus::Failed,
                attempt_number: 1,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }],
            reports: vec![Report {
                id: "report-9".to_string(),
                work_unit_id: "wu-9".to_string(),
                assignment_id: "assignment-9".to_string(),
                worker_id: "worker-r".to_string(),
                disposition: ReportDisposition::Failed,
                summary: "Recovered history summary.".to_string(),
                findings: Vec::new(),
                blockers: vec!["Needs operator review".to_string()],
                questions: Vec::new(),
                recommended_next_actions: Vec::new(),
                confidence: ReportConfidence::Unknown,
                raw_output: "raw".to_string(),
                parse_result: ReportParseResult::Invalid,
                needs_supervisor_review: true,
                created_at: Utc::now(),
            }],
            decisions: vec![Decision {
                id: "decision-9".to_string(),
                work_unit_id: "wu-9".to_string(),
                report_id: Some("report-9".to_string()),
                decision_type: DecisionType::EscalateToHuman,
                rationale: "Recovered issue needs review.".to_string(),
                created_at: Utc::now(),
            }],
            proposals: Vec::new(),
        })
        .await;

    harness.disconnect_events().await;
    harness.process().await;
    harness.force_reconnect_now();
    harness.process().await;

    let history = harness.collaboration_history_vm();
    assert!(history.title.contains("Recovered unit"));
    assert!(history.lines.iter().any(|line| {
        line.contains("assignment-9 [failed] attempt=1 worker=worker-r session=session-9 current")
    }));
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("report-9 [failed invalid review=true]"))
    );
    assert!(
        history
            .lines
            .iter()
            .any(|line| line.contains("decision-9 [escalate_to_human]"))
    );
    assert!(
        !history
            .lines
            .iter()
            .any(|line| line.contains("assignment-2 [awaiting_decision]"))
    );
    assert!(
        !history
            .lines
            .iter()
            .any(|line| line.contains("report-2 [partial ambiguous review=true]"))
    );
}

#[tokio::test]
async fn event_refresh_does_not_leave_invalid_parent_child_selection() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    harness.dispatch(UserAction::SelectNextInView).await;
    assert_eq!(harness.selected_workstream_id(), Some("ws-2"));
    assert_eq!(harness.selected_work_unit_id(), Some("wu-3"));

    harness
        .inject_event(ipc::DaemonEventEnvelope::new(
            ipc::DaemonEvent::WorkUnitLifecycle {
                action: ipc::CollaborationLifecycleAction::Updated,
                work_unit: ipc::WorkUnitSummary {
                    id: "wu-3".to_string(),
                    workstream_id: "ws-1".to_string(),
                    title: "Out of scope".to_string(),
                    status: WorkUnitStatus::Blocked,
                    dependency_count: 2,
                    current_assignment_id: None,
                    latest_report_id: None,
                    proposal: None,
                    updated_at: Utc::now(),
                },
            },
        ))
        .await
        .unwrap();

    assert_eq!(harness.selected_workstream_id(), Some("ws-2"));
    assert_eq!(harness.selected_work_unit_id(), None);
    assert!(
        harness
            .workstream_detail_vm()
            .lines
            .iter()
            .any(|line| line.contains("units: total=0"))
    );
}

#[tokio::test]
async fn reconnect_reconciles_collaboration_selection_to_authoritative_snapshot() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;
    harness.dispatch(UserAction::SelectNextInView).await;

    let mut recovered = sample_snapshot();
    recovered.collaboration.workstreams = vec![ipc::WorkstreamSummary {
        id: "ws-r".to_string(),
        title: "Recovered".to_string(),
        objective: "Replace stale selection.".to_string(),
        status: WorkstreamStatus::Active,
        priority: "high".to_string(),
        updated_at: Utc::now(),
    }];
    recovered.collaboration.work_units = vec![ipc::WorkUnitSummary {
        id: "wu-r".to_string(),
        workstream_id: "ws-r".to_string(),
        title: "Recovered unit".to_string(),
        status: WorkUnitStatus::AwaitingDecision,
        dependency_count: 0,
        current_assignment_id: Some("assignment-r".to_string()),
        latest_report_id: Some("report-r".to_string()),
        proposal: Some(ipc::WorkUnitProposalSummary {
            latest_failure_stage: Some(SupervisorProposalFailureStage::Backend),
            ..sample_proposal_summary(SupervisorProposalStatus::GenerationFailed, None)
        }),
        updated_at: Utc::now(),
    }];
    recovered.collaboration.assignments = vec![ipc::AssignmentSummary {
        id: "assignment-r".to_string(),
        work_unit_id: "wu-r".to_string(),
        worker_id: "worker-r".to_string(),
        worker_session_id: "session-r".to_string(),
        status: AssignmentStatus::Failed,
        attempt_number: 1,
        updated_at: Utc::now(),
    }];
    recovered.collaboration.reports = vec![ipc::ReportSummary {
        id: "report-r".to_string(),
        work_unit_id: "wu-r".to_string(),
        assignment_id: "assignment-r".to_string(),
        worker_id: "worker-r".to_string(),
        disposition: ReportDisposition::Failed,
        summary: "Recovered failure state.".to_string(),
        confidence: ReportConfidence::Unknown,
        parse_result: ReportParseResult::Invalid,
        needs_supervisor_review: true,
        created_at: Utc::now(),
    }];
    recovered.collaboration.decisions = vec![ipc::DecisionSummary {
        id: "decision-r".to_string(),
        work_unit_id: "wu-r".to_string(),
        report_id: Some("report-r".to_string()),
        decision_type: DecisionType::EscalateToHuman,
        rationale: "Recovered review required.".to_string(),
        created_at: Utc::now(),
    }];
    harness.replace_snapshot(recovered).await;

    harness.disconnect_events().await;
    harness.process().await;
    harness.force_reconnect_now();
    harness.process().await;

    assert_eq!(
        harness.state().selected_workstream_id.as_deref(),
        Some("ws-r")
    );
    assert_eq!(
        harness.state().selected_work_unit_id.as_deref(),
        Some("wu-r")
    );

    let rendered = harness.render_text(160, 42);
    assert!(rendered.contains("Recovered [active]"));
    assert!(rendered.contains("Recovered unit"));
    assert!(rendered.contains("[awaiting_decision]"));
    assert!(rendered.contains("proposal=generation_failed/backend"));
    assert!(!rendered.contains("Deferred work"));
    assert!(!rendered.contains("Out of scope"));
}

#[tokio::test]
async fn compact_layout_keeps_focus_selection_and_state_labels_visible_across_sizes() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let expanded = harness.render_text(160, 42);
    assert!(expanded.contains("Connection"), "{expanded}");
    assert!(expanded.contains("Recent Events"), "{expanded}");

    for (width, height) in [(120, 40), (100, 30), (80, 24)] {
        harness
            .dispatch(UserAction::ShowView(TopLevelView::Overview))
            .await;
        let overview = harness.render_text(width, height);
        assert!(
            overview.contains("Connection"),
            "missing overview connection panel at {width}x{height}\n{overview}"
        );
        assert!(
            overview.contains("Active Work"),
            "missing overview active-work panel at {width}x{height}\n{overview}"
        );

        harness
            .dispatch(UserAction::ShowView(TopLevelView::Threads))
            .await;
        let threads = harness.render_text(width, height);
        assert!(
            threads.contains("Threads"),
            "missing threads list at {width}x{height}\n{threads}"
        );
        assert!(
            threads.contains("Thread Activity"),
            "missing thread activity at {width}x{height}\n{threads}"
        );

        harness
            .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
            .await;
        let collaboration = harness.render_text(width, height);
        assert!(
            collaboration.contains("Workstreams"),
            "missing workstreams at {width}x{height}\n{collaboration}"
        );
        assert!(
            collaboration.contains("Work Units"),
            "missing work units at {width}x{height}\n{collaboration}"
        );
        assert!(
            collaboration.contains("Snapshot wiring"),
            "missing selected work-unit detail at {width}x{height}\n{collaboration}"
        );
    }
}

#[tokio::test]
async fn small_terminal_render_keeps_collaboration_surface_stable() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness.dispatch(UserAction::Refresh).await;
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;

    let rendered = harness.render_text(110, 34);

    assert!(rendered.contains("Workstreams"));
    assert!(rendered.contains("History Snapshot wiring"));
    assert!(rendered.contains("Collaboration"));
    assert!(rendered.contains("Snapshot wiring"));
    assert!(rendered.contains("assignment-2"));
}
