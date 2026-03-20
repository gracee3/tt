use chrono::Utc;

use crate::app::{
    BannerLevel, CollaborationFocus, DaemonConnectionPhase, DaemonLifecycleState, MainFooterState,
    MainHierarchySelection, ProgramView, ReviewSelection, TopLevelView, UiEvent, UserAction,
};
use crate::backend::BackendCommand;
use crate::codex::{
    CodexOutputPreview, CodexSessionId, CodexSessionState, CodexThreadSessionSummary,
    CodexThreadSessions,
};
use crate::test_harness::AppHarness;
use crate::view_model;
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
        archived: false,
        loaded_status: ipc::ThreadLoadedStatus::Idle,
        active_flags: Vec::new(),
        active_turn_id: None,
        last_seen_turn_id: Some("turn-1".to_string()),
        recent_output: Some(preview.to_string()),
        recent_event: Some("thread idle".to_string()),
        turn_in_flight: false,
        monitor_state: ipc::ThreadMonitorState::Detached,
        last_sync_at: Utc::now(),
        source_kind: None,
        raw_summary: None,
    }
}

fn sample_thread_view(id: &str, preview: &str, output: &str) -> ipc::ThreadView {
    ipc::ThreadView {
        summary: sample_thread_summary(id, preview, 200),
        history_loaded: true,
        turns: vec![ipc::TurnView {
            id: "turn-1".to_string(),
            status: "completed".to_string(),
            error_message: None,
            error_summary: None,
            started_at: None,
            completed_at: None,
            latest_diff: None,
            latest_plan_snapshot: None,
            token_usage_snapshot: None,
            items: vec![ipc::ItemView {
                id: "item-1".to_string(),
                item_type: "agent_message".to_string(),
                status: Some("completed".to_string()),
                text: Some(output.to_string()),
                summary: Some(output.to_string()),
                payload: None,
            }],
        }],
    }
}

fn sample_codex_assignment_summary(
    thread_id: &str,
    status: orcas_core::CodexThreadAssignmentStatus,
) -> ipc::CodexThreadAssignmentSummary {
    ipc::CodexThreadAssignmentSummary {
        assignment_id: "cta-1".to_string(),
        codex_thread_id: thread_id.to_string(),
        workstream_id: "ws-1".to_string(),
        work_unit_id: "wu-1".to_string(),
        supervisor_id: "supervisor-a".to_string(),
        assigned_by: "operator".to_string(),
        assigned_at: Utc::now(),
        updated_at: Utc::now(),
        status,
        send_policy: orcas_core::CodexThreadSendPolicy::HumanApprovalRequired,
        bootstrap_state: orcas_core::CodexThreadBootstrapState::Pending,
        latest_basis_turn_id: Some("turn-1".to_string()),
        latest_decision_id: None,
        notes: Some("watch this thread".to_string()),
        active: matches!(
            status,
            orcas_core::CodexThreadAssignmentStatus::Proposed
                | orcas_core::CodexThreadAssignmentStatus::Active
        ),
    }
}

fn sample_supervisor_turn_decision_summary(
    thread_id: &str,
    status: orcas_core::SupervisorTurnDecisionStatus,
) -> ipc::SupervisorTurnDecisionSummary {
    sample_supervisor_turn_decision_summary_with_kind(
        thread_id,
        status,
        orcas_core::SupervisorTurnDecisionKind::NextTurn,
        orcas_core::SupervisorTurnProposalKind::Bootstrap,
    )
}

fn sample_supervisor_turn_decision_summary_with_kind(
    thread_id: &str,
    status: orcas_core::SupervisorTurnDecisionStatus,
    kind: orcas_core::SupervisorTurnDecisionKind,
    proposal_kind: orcas_core::SupervisorTurnProposalKind,
) -> ipc::SupervisorTurnDecisionSummary {
    let proposed_text = match kind {
        orcas_core::SupervisorTurnDecisionKind::InterruptActiveTurn => None,
        orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn => Some(
            "Please focus on the current bounded step and call out blockers before broadening scope."
                .to_string(),
        ),
        orcas_core::SupervisorTurnDecisionKind::NoAction => None,
        _ => Some("Please summarize status and take the next bounded step.".to_string()),
    };
    let rationale_summary = match kind {
        orcas_core::SupervisorTurnDecisionKind::InterruptActiveTurn => {
            "Operator requested review of interrupting the active turn.".to_string()
        }
        orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn => {
            "Operator requested review of steering the active turn.".to_string()
        }
        orcas_core::SupervisorTurnDecisionKind::NoAction => {
            "Operator deliberately chose to wait on the current idle-thread basis.".to_string()
        }
        _ => "Thread is idle under an active assignment and needs bootstrap review.".to_string(),
    };
    ipc::SupervisorTurnDecisionSummary {
        decision_id: "std-1".to_string(),
        assignment_id: "cta-1".to_string(),
        codex_thread_id: thread_id.to_string(),
        workstream_id: Some("ws-1".to_string()),
        work_unit_id: Some("wu-1".to_string()),
        supervisor_id: Some("supervisor-a".to_string()),
        basis_turn_id: Some("turn-1".to_string()),
        kind,
        proposal_kind,
        proposed_text,
        rationale_summary,
        status,
        created_at: Utc::now(),
        approved_at: None,
        rejected_at: None,
        sent_at: None,
        superseded_by: None,
        sent_turn_id: None,
        notes: Some("human approval required".to_string()),
        open: matches!(
            status,
            orcas_core::SupervisorTurnDecisionStatus::Draft
                | orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman
        ),
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

async fn type_steer_text(harness: &mut AppHarness, text: &str) {
    for ch in text.chars() {
        harness.dispatch(UserAction::SteerComposeAppend(ch)).await;
    }
}

async fn type_multiline_steer_text(harness: &mut AppHarness, lines: &[&str]) {
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            harness
                .dispatch(UserAction::SteerComposeInsertNewline)
                .await;
        }
        type_steer_text(harness, line).await;
    }
}

async fn clear_steer_text(harness: &mut AppHarness, text_len: usize) {
    for _ in 0..text_len {
        harness.dispatch(UserAction::SteerComposeBackspace).await;
    }
}

async fn type_main_footer_text(harness: &mut AppHarness, text: &str) {
    for ch in text.chars() {
        harness.dispatch(UserAction::MainFooterAppend(ch)).await;
    }
}

async fn clear_main_footer_text(harness: &mut AppHarness, text_len: usize) {
    for _ in 0..text_len {
        harness.dispatch(UserAction::MainFooterBackspace).await;
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
                source_kind: ipc::PlanningSummarySourceKind::Collaboration,
                updated_at: Utc::now(),
            },
            ipc::WorkstreamSummary {
                id: "ws-2".to_string(),
                title: "Deferred work".to_string(),
                objective: "Hold future scope.".to_string(),
                status: WorkstreamStatus::Blocked,
                priority: "low".to_string(),
                source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
                source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
                source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
                source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
        codex_thread_assignments: Vec::new(),
        supervisor_turn_decisions: Vec::new(),
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

fn sample_disconnected_snapshot() -> ipc::StateSnapshot {
    let mut snapshot = sample_snapshot();
    snapshot.daemon.upstream.status = "disconnected".to_string();
    snapshot
}

fn sample_main_surface_snapshot() -> ipc::StateSnapshot {
    let mut snapshot = sample_snapshot();
    snapshot.collaboration.codex_thread_assignments = vec![
        sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ),
        ipc::CodexThreadAssignmentSummary {
            work_unit_id: "wu-2".to_string(),
            assignment_id: "cta-2".to_string(),
            codex_thread_id: "thread-2".to_string(),
            status: orcas_core::CodexThreadAssignmentStatus::Active,
            notes: Some("follow event state".to_string()),
            ..sample_codex_assignment_summary(
                "thread-2",
                orcas_core::CodexThreadAssignmentStatus::Active,
            )
        },
    ];
    snapshot
}

fn sample_review_snapshot() -> ipc::StateSnapshot {
    let mut snapshot = sample_main_surface_snapshot();
    snapshot.collaboration.supervisor_turn_decisions.push(
        sample_supervisor_turn_decision_summary_with_kind(
            "thread-1",
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
            orcas_core::SupervisorTurnDecisionKind::NextTurn,
            orcas_core::SupervisorTurnProposalKind::Bootstrap,
        ),
    );
    snapshot
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
async fn thread_list_and_detail_show_codex_assignment_state() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Paused,
        ));

    let harness = AppHarness::new(snapshot).await.unwrap();
    let list = harness.thread_list_vm();
    assert_eq!(list.rows[0].assignment_badge.as_deref(), Some("paused"));

    let summary = harness.thread_summary_vm();
    assert!(
        summary
            .lines
            .iter()
            .any(|line| line.contains("assignment: cta-1 [paused]"))
    );

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("assignment cta-1 [paused]"))
    );
    assert!(
        detail.lines.iter().any(|line| {
            line.contains("workstream=ws-1  work_unit=wu-1  supervisor=supervisor-a")
        })
    );
}

#[tokio::test]
async fn detached_codex_session_surfaces_in_thread_views() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .inject_ui_event(UiEvent::CodexSessionsChanged {
            sessions: std::iter::once((
                "thread-1".to_string(),
                CodexThreadSessions {
                    thread_id: "thread-1".to_string(),
                    sessions: vec![CodexThreadSessionSummary {
                        session_id: CodexSessionId::from(1_u64),
                        thread_id: "thread-1".to_string(),
                        state: CodexSessionState::Detached { pid: 4242 },
                        created_at: std::time::Instant::now(),
                        last_activity_at: None,
                        output_preview: CodexOutputPreview::default(),
                    }],
                },
            ))
            .collect(),
        })
        .await;

    let list = harness.thread_list_vm();
    assert_eq!(list.rows[0].session_badge.as_deref(), Some("detached"));

    let summary = harness.thread_summary_vm();
    assert!(
        summary
            .lines
            .iter()
            .any(|line| line.contains("codex session: detached"))
    );

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("Codex Session: detached"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("actions: c reattach live Codex session"))
    );
}

#[tokio::test]
async fn codex_session_preview_and_history_surface_in_thread_detail() {
    let now = std::time::Instant::now();
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .inject_ui_event(UiEvent::CodexSessionsChanged {
            sessions: std::iter::once((
                "thread-1".to_string(),
                CodexThreadSessions {
                    thread_id: "thread-1".to_string(),
                    sessions: vec![
                        CodexThreadSessionSummary {
                            session_id: CodexSessionId::from(2_u64),
                            thread_id: "thread-1".to_string(),
                            state: CodexSessionState::Detached { pid: 4242 },
                            created_at: now,
                            last_activity_at: Some(now),
                            output_preview: CodexOutputPreview {
                                lines: vec![
                                    "running tests".to_string(),
                                    "waiting for approval".to_string(),
                                ],
                                truncated: true,
                                control_sequences_removed: true,
                            },
                        },
                        CodexThreadSessionSummary {
                            session_id: CodexSessionId::from(1_u64),
                            thread_id: "thread-1".to_string(),
                            state: CodexSessionState::Exited {
                                result: crate::codex::session::CodexExit {
                                    success: true,
                                    code: Some(0),
                                },
                            },
                            created_at: now,
                            last_activity_at: Some(now),
                            output_preview: CodexOutputPreview::default(),
                        },
                    ],
                },
            ))
            .collect(),
        })
        .await;

    let summary = harness.thread_summary_vm();
    assert!(
        summary
            .lines
            .iter()
            .any(|line| line.contains("codex action: c reattach live Codex session"))
    );

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("recent PTY output (best effort):"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("running tests"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("local session history:"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("session 1  exited"))
    );
}

#[tokio::test]
async fn thread_list_and_detail_show_supervisor_decision_state() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    snapshot
        .collaboration
        .supervisor_turn_decisions
        .push(sample_supervisor_turn_decision_summary(
            "thread-1",
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
        ));

    let harness = AppHarness::new(snapshot).await.unwrap();
    let list = harness.thread_list_vm();
    assert_eq!(
        list.rows[0].decision_badge.as_deref(),
        Some("pending human approval")
    );

    let summary = harness.thread_summary_vm();
    assert!(
        summary
            .lines
            .iter()
            .any(|line| line.contains("decision: std-1 [pending human approval]"))
    );

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("decision std-1 [pending human approval]"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("actions: w record no action  a approve/send  d reject"))
    );
}

#[tokio::test]
async fn pending_next_turn_decision_shows_record_no_action_action() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    snapshot.collaboration.supervisor_turn_decisions.push(
        sample_supervisor_turn_decision_summary_with_kind(
            "thread-1",
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
            orcas_core::SupervisorTurnDecisionKind::NextTurn,
            orcas_core::SupervisorTurnProposalKind::Bootstrap,
        ),
    );

    let harness = AppHarness::new(snapshot).await.unwrap();
    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("actions: w record no action  a approve/send  d reject"))
    );
}

#[tokio::test]
async fn record_no_action_dispatches_backend_command_for_pending_next_turn() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    snapshot.collaboration.supervisor_turn_decisions.push(
        sample_supervisor_turn_decision_summary_with_kind(
            "thread-1",
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
            orcas_core::SupervisorTurnDecisionKind::NextTurn,
            orcas_core::SupervisorTurnProposalKind::Bootstrap,
        ),
    );
    let mut harness = AppHarness::new(snapshot).await.unwrap();

    harness
        .dispatch(UserAction::RecordNoActionForSelectedThread)
        .await;

    let commands = harness.recorded_commands().await;
    assert!(commands.iter().any(|command| {
        matches!(
            command,
            BackendCommand::RecordNoActionSupervisorDecision { decision_id }
                if decision_id == "std-1"
        )
    }));

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("[waiting on current basis]"))
    );
}

#[tokio::test]
async fn recorded_no_action_shows_manual_refresh_action_when_valid() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    snapshot.collaboration.supervisor_turn_decisions.push(
        sample_supervisor_turn_decision_summary_with_kind(
            "thread-1",
            orcas_core::SupervisorTurnDecisionStatus::Recorded,
            orcas_core::SupervisorTurnDecisionKind::NoAction,
            orcas_core::SupervisorTurnProposalKind::Bootstrap,
        ),
    );

    let harness = AppHarness::new(snapshot).await.unwrap();
    let list = harness.thread_list_vm();
    assert_eq!(
        list.rows[0].decision_badge.as_deref(),
        Some("waiting on current basis")
    );

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("decision std-1 [waiting on current basis]"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("actions: m manual refresh"))
    );
}

#[tokio::test]
async fn manual_refresh_dispatches_backend_command_when_current_basis_is_waiting() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    snapshot.collaboration.supervisor_turn_decisions.push(
        sample_supervisor_turn_decision_summary_with_kind(
            "thread-1",
            orcas_core::SupervisorTurnDecisionStatus::Recorded,
            orcas_core::SupervisorTurnDecisionKind::NoAction,
            orcas_core::SupervisorTurnProposalKind::Bootstrap,
        ),
    );
    let mut harness = AppHarness::new(snapshot).await.unwrap();

    harness
        .dispatch(UserAction::ManualRefreshForSelectedThread)
        .await;

    let commands = harness.recorded_commands().await;
    assert!(commands.iter().any(|command| {
        matches!(
            command,
            BackendCommand::ManualRefreshSupervisorDecision { assignment_id }
                if assignment_id == "cta-1"
        )
    }));

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("[pending human approval]"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("proposal=manual refresh"))
    );
}

#[tokio::test]
async fn approve_selected_supervisor_decision_refreshes_thread_state() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .supervisor_turn_decisions
        .push(sample_supervisor_turn_decision_summary(
            "thread-1",
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
        ));
    let mut harness = AppHarness::new(snapshot).await.unwrap();

    harness
        .dispatch(UserAction::ApproveSelectedSupervisorDecision)
        .await;

    let commands = harness.recorded_commands().await;
    assert!(commands.iter().any(|command| {
        matches!(
            command,
            BackendCommand::ApproveSupervisorDecision { decision_id }
                if decision_id == "std-1"
        )
    }));

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("decision std-1 [sent]"))
    );
}

#[tokio::test]
async fn reject_selected_supervisor_decision_refreshes_thread_state() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .supervisor_turn_decisions
        .push(sample_supervisor_turn_decision_summary(
            "thread-1",
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
        ));
    let mut harness = AppHarness::new(snapshot).await.unwrap();

    harness
        .dispatch(UserAction::RejectSelectedSupervisorDecision)
        .await;

    let commands = harness.recorded_commands().await;
    assert!(commands.iter().any(|command| {
        matches!(
            command,
            BackendCommand::RejectSupervisorDecision { decision_id }
                if decision_id == "std-1"
        )
    }));

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("decision std-1 [rejected]"))
    );
}

#[tokio::test]
async fn assigned_active_thread_without_open_decision_shows_propose_interrupt_action() {
    let mut snapshot = sample_snapshot();
    snapshot.threads[0].status = "active".to_string();
    snapshot.threads[0].loaded_status = ipc::ThreadLoadedStatus::Active;
    snapshot.threads[0].active_turn_id = Some("turn-1".to_string());
    snapshot.threads[0].turn_in_flight = true;
    snapshot.active_thread = Some(sample_thread_view("thread-1", "hello", "turn output"));
    if let Some(active_thread) = snapshot.active_thread.as_mut() {
        active_thread.summary.status = "active".to_string();
        active_thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
        active_thread.summary.active_turn_id = Some("turn-1".to_string());
        active_thread.summary.turn_in_flight = true;
        active_thread.turns[0].status = "in_progress".to_string();
    }
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));

    let harness = AppHarness::new(snapshot).await.unwrap();
    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("actions: s compose steer  i propose interrupt"))
    );
}

#[tokio::test]
async fn assigned_active_thread_without_open_decision_shows_propose_steer_action() {
    let mut snapshot = sample_snapshot();
    snapshot.threads[0].status = "active".to_string();
    snapshot.threads[0].loaded_status = ipc::ThreadLoadedStatus::Active;
    snapshot.threads[0].active_turn_id = Some("turn-1".to_string());
    snapshot.threads[0].turn_in_flight = true;
    snapshot.active_thread = Some(sample_thread_view("thread-1", "hello", "turn output"));
    if let Some(active_thread) = snapshot.active_thread.as_mut() {
        active_thread.summary.status = "active".to_string();
        active_thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
        active_thread.summary.active_turn_id = Some("turn-1".to_string());
        active_thread.summary.turn_in_flight = true;
        active_thread.turns[0].status = "in_progress".to_string();
    }
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));

    let harness = AppHarness::new(snapshot).await.unwrap();
    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("actions: s compose steer  i propose interrupt"))
    );
}

#[tokio::test]
async fn propose_edit_and_review_steer_text_flow_works_end_to_end() {
    let mut snapshot = sample_snapshot();
    snapshot.threads[0].status = "active".to_string();
    snapshot.threads[0].loaded_status = ipc::ThreadLoadedStatus::Active;
    snapshot.threads[0].active_turn_id = Some("turn-1".to_string());
    snapshot.threads[0].turn_in_flight = true;
    snapshot.active_thread = Some(sample_thread_view("thread-1", "hello", "turn output"));
    if let Some(active_thread) = snapshot.active_thread.as_mut() {
        active_thread.summary.status = "active".to_string();
        active_thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
        active_thread.summary.active_turn_id = Some("turn-1".to_string());
        active_thread.summary.turn_in_flight = true;
        active_thread.turns[0].status = "in_progress".to_string();
    }
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    let mut harness = AppHarness::new(snapshot).await.unwrap();
    let initial_command_count = harness.recorded_commands().await.len();

    harness
        .dispatch(UserAction::ProposeSteerForSelectedThread)
        .await;
    assert_eq!(
        harness.recorded_commands().await.len(),
        initial_command_count
    );
    let compose_detail = harness.thread_detail_vm();
    assert!(
        compose_detail
            .lines
            .iter()
            .any(|line| line.contains("Steer Compose: new steer proposal"))
    );
    assert!(
        compose_detail
            .lines
            .iter()
            .any(|line| line.contains("ctrl+s create steer"))
    );
    type_multiline_steer_text(&mut harness, &["focus tests", "then summarize blockers"]).await;
    harness.dispatch(UserAction::SubmitSteerCompose).await;

    let commands = harness.recorded_commands().await;
    assert!(commands.iter().any(|command| {
        matches!(
            command,
            BackendCommand::ProposeSteerSupervisorDecision { assignment_id, proposed_text }
                if assignment_id == "cta-1"
                    && proposed_text == "focus tests\nthen summarize blockers"
        )
    }));

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("steer active turn"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("pending steer approval"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("proposed focus tests then summarize blockers"))
    );

    harness
        .dispatch(UserAction::EditPendingSteerForSelectedThread)
        .await;
    let edit_detail = harness.thread_detail_vm();
    assert!(
        edit_detail
            .lines
            .iter()
            .any(|line| line.contains("Steer Compose: editing pending steer"))
    );
    assert!(
        edit_detail
            .lines
            .iter()
            .any(|line| line.contains("focus tests"))
    );
    clear_steer_text(
        &mut harness,
        "focus tests\nthen summarize blockers".chars().count(),
    )
    .await;
    type_multiline_steer_text(&mut harness, &["focus logs", "then summarize risks"]).await;
    harness.dispatch(UserAction::SubmitSteerCompose).await;

    let commands = harness.recorded_commands().await;
    assert!(commands.iter().any(|command| {
        matches!(
            command,
            BackendCommand::ReplacePendingSteerSupervisorDecision { decision_id, proposed_text }
                if decision_id == "std-1"
                    && proposed_text == "focus logs\nthen summarize risks"
        )
    }));

    let revised_detail = harness.thread_detail_vm();
    assert!(
        revised_detail
            .lines
            .iter()
            .any(|line| line.contains("proposed focus logs then summarize risks"))
    );
    assert!(
        revised_detail
            .lines
            .iter()
            .any(|line| line.contains("Decision History:"))
    );
    assert!(
        revised_detail
            .lines
            .iter()
            .any(|line| line.contains("superseded by std-2"))
    );

    harness
        .dispatch(UserAction::ApproveSelectedSupervisorDecision)
        .await;
    let sent_detail = harness.thread_detail_vm();
    assert!(
        sent_detail
            .lines
            .iter()
            .any(|line| line.contains("decision std-2 [sent]"))
    );
}

#[tokio::test]
async fn steer_decisions_render_distinctly_from_interrupt_and_next_turn() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    snapshot.collaboration.supervisor_turn_decisions.push(
        sample_supervisor_turn_decision_summary_with_kind(
            "thread-1",
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
            orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn,
            orcas_core::SupervisorTurnProposalKind::OperatorSteer,
        ),
    );

    let harness = AppHarness::new(snapshot).await.unwrap();
    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("decision std-1 [pending steer approval]"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("kind=steer active turn  proposal=operator steer"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("editable=yes revision_state=pending steer"))
    );
}

#[tokio::test]
async fn submit_steer_compose_rejects_empty_text_without_backend_command() {
    let mut snapshot = sample_snapshot();
    snapshot.threads[0].status = "active".to_string();
    snapshot.threads[0].loaded_status = ipc::ThreadLoadedStatus::Active;
    snapshot.threads[0].active_turn_id = Some("turn-1".to_string());
    snapshot.threads[0].turn_in_flight = true;
    snapshot.active_thread = Some(sample_thread_view("thread-1", "hello", "turn output"));
    if let Some(active_thread) = snapshot.active_thread.as_mut() {
        active_thread.summary.status = "active".to_string();
        active_thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
        active_thread.summary.active_turn_id = Some("turn-1".to_string());
        active_thread.summary.turn_in_flight = true;
        active_thread.turns[0].status = "in_progress".to_string();
    }
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    let mut harness = AppHarness::new(snapshot).await.unwrap();
    let initial_command_count = harness.recorded_commands().await.len();

    harness
        .dispatch(UserAction::ProposeSteerForSelectedThread)
        .await;
    harness.dispatch(UserAction::SubmitSteerCompose).await;

    assert_eq!(
        harness.recorded_commands().await.len(),
        initial_command_count
    );
    let banner = harness.state().banner.clone().expect("banner");
    assert_eq!(banner.level, BannerLevel::Warning);
    assert!(banner.message.contains("must not be empty"));
}

#[tokio::test]
async fn multiline_compose_cancel_does_not_mutate_backend_state() {
    let mut snapshot = sample_snapshot();
    snapshot.threads[0].status = "active".to_string();
    snapshot.threads[0].loaded_status = ipc::ThreadLoadedStatus::Active;
    snapshot.threads[0].active_turn_id = Some("turn-1".to_string());
    snapshot.threads[0].turn_in_flight = true;
    snapshot.active_thread = Some(sample_thread_view("thread-1", "hello", "turn output"));
    if let Some(active_thread) = snapshot.active_thread.as_mut() {
        active_thread.summary.status = "active".to_string();
        active_thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
        active_thread.summary.active_turn_id = Some("turn-1".to_string());
        active_thread.summary.turn_in_flight = true;
        active_thread.turns[0].status = "in_progress".to_string();
    }
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    let mut harness = AppHarness::new(snapshot).await.unwrap();
    let initial_command_count = harness.recorded_commands().await.len();

    harness
        .dispatch(UserAction::ProposeSteerForSelectedThread)
        .await;
    type_multiline_steer_text(&mut harness, &["line one", "line two"]).await;
    harness.dispatch(UserAction::CancelSteerCompose).await;

    assert_eq!(
        harness.recorded_commands().await.len(),
        initial_command_count
    );
    let detail = harness.thread_detail_vm();
    assert!(
        !detail
            .lines
            .iter()
            .any(|line| line.contains("Steer Compose:"))
    );
}

#[tokio::test]
async fn non_pending_steer_decisions_cannot_enter_edit_mode() {
    for status in [
        orcas_core::SupervisorTurnDecisionStatus::Sent,
        orcas_core::SupervisorTurnDecisionStatus::Rejected,
        orcas_core::SupervisorTurnDecisionStatus::Stale,
        orcas_core::SupervisorTurnDecisionStatus::Superseded,
    ] {
        let mut snapshot = sample_snapshot();
        snapshot
            .collaboration
            .codex_thread_assignments
            .push(sample_codex_assignment_summary(
                "thread-1",
                orcas_core::CodexThreadAssignmentStatus::Active,
            ));
        snapshot.collaboration.supervisor_turn_decisions.push(
            sample_supervisor_turn_decision_summary_with_kind(
                "thread-1",
                status,
                orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn,
                orcas_core::SupervisorTurnProposalKind::OperatorSteer,
            ),
        );
        let mut harness = AppHarness::new(snapshot).await.unwrap();
        let initial_command_count = harness.recorded_commands().await.len();
        harness
            .dispatch(UserAction::EditPendingSteerForSelectedThread)
            .await;
        assert_eq!(
            harness.recorded_commands().await.len(),
            initial_command_count
        );
        assert!(harness.state().steer_compose.is_none());
        let banner = harness.state().banner.clone().expect("banner");
        assert!(
            banner
                .message
                .contains("no editable pending steer decision")
        );
    }
}

#[tokio::test]
async fn decision_history_includes_superseded_steer_revision_chain() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    let mut original = sample_supervisor_turn_decision_summary_with_kind(
        "thread-1",
        orcas_core::SupervisorTurnDecisionStatus::Superseded,
        orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn,
        orcas_core::SupervisorTurnProposalKind::OperatorSteer,
    );
    original.decision_id = "std-1".to_string();
    original.superseded_by = Some("std-2".to_string());
    original.proposed_text = Some("first steer revision".to_string());
    let mut replacement = sample_supervisor_turn_decision_summary_with_kind(
        "thread-1",
        orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
        orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn,
        orcas_core::SupervisorTurnProposalKind::OperatorSteer,
    );
    replacement.decision_id = "std-2".to_string();
    replacement.proposed_text = Some("second steer revision".to_string());
    replacement.created_at = Utc::now() + chrono::TimeDelta::seconds(1);
    snapshot
        .collaboration
        .supervisor_turn_decisions
        .extend([original, replacement]);

    let harness = AppHarness::new(snapshot).await.unwrap();
    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("Decision History:"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("std-2 [pending steer approval]"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("std-1 [superseded]"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("superseded by std-2"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("text first steer revision"))
    );
}

#[tokio::test]
async fn propose_interrupt_dispatches_backend_command_only_when_valid() {
    let mut snapshot = sample_snapshot();
    snapshot.threads[0].status = "active".to_string();
    snapshot.threads[0].loaded_status = ipc::ThreadLoadedStatus::Active;
    snapshot.threads[0].active_turn_id = Some("turn-1".to_string());
    snapshot.threads[0].turn_in_flight = true;
    snapshot.active_thread = Some(sample_thread_view("thread-1", "hello", "turn output"));
    if let Some(active_thread) = snapshot.active_thread.as_mut() {
        active_thread.summary.status = "active".to_string();
        active_thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
        active_thread.summary.active_turn_id = Some("turn-1".to_string());
        active_thread.summary.turn_in_flight = true;
        active_thread.turns[0].status = "in_progress".to_string();
    }
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    let mut harness = AppHarness::new(snapshot).await.unwrap();

    harness
        .dispatch(UserAction::ProposeInterruptForSelectedThread)
        .await;

    let commands = harness.recorded_commands().await;
    assert!(commands.iter().any(|command| {
        matches!(
            command,
            BackendCommand::ProposeInterruptSupervisorDecision { assignment_id }
                if assignment_id == "cta-1"
        )
    }));

    let detail = harness.thread_detail_vm();
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("interrupt active turn"))
    );
    assert!(
        detail
            .lines
            .iter()
            .any(|line| line.contains("pending interrupt approval"))
    );
}

#[tokio::test]
async fn interrupt_action_is_hidden_for_idle_or_conflicting_thread() {
    let mut snapshot = sample_snapshot();
    snapshot
        .collaboration
        .codex_thread_assignments
        .push(sample_codex_assignment_summary(
            "thread-1",
            orcas_core::CodexThreadAssignmentStatus::Active,
        ));
    let harness = AppHarness::new(snapshot.clone()).await.unwrap();
    let idle_detail = harness.thread_detail_vm();
    assert!(
        !idle_detail
            .lines
            .iter()
            .any(|line| line.contains("actions: s compose steer  i propose interrupt"))
    );

    snapshot.threads[0].status = "active".to_string();
    snapshot.threads[0].loaded_status = ipc::ThreadLoadedStatus::Active;
    snapshot.threads[0].active_turn_id = Some("turn-1".to_string());
    snapshot.threads[0].turn_in_flight = true;
    snapshot.active_thread = Some(sample_thread_view("thread-1", "hello", "turn output"));
    if let Some(active_thread) = snapshot.active_thread.as_mut() {
        active_thread.summary.status = "active".to_string();
        active_thread.summary.loaded_status = ipc::ThreadLoadedStatus::Active;
        active_thread.summary.active_turn_id = Some("turn-1".to_string());
        active_thread.summary.turn_in_flight = true;
        active_thread.turns[0].status = "in_progress".to_string();
    }
    snapshot.collaboration.supervisor_turn_decisions.push(
        sample_supervisor_turn_decision_summary_with_kind(
            "thread-1",
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
            orcas_core::SupervisorTurnDecisionKind::NextTurn,
            orcas_core::SupervisorTurnProposalKind::Bootstrap,
        ),
    );
    let harness = AppHarness::new(snapshot).await.unwrap();
    let conflicting_detail = harness.thread_detail_vm();
    assert!(
        !conflicting_detail
            .lines
            .iter()
            .any(|line| line.contains("actions: s compose steer  i propose interrupt"))
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
                    error_summary: None,
                    started_at: None,
                    completed_at: None,
                    latest_diff: None,
                    latest_plan_snapshot: None,
                    token_usage_snapshot: None,
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
                    error_summary: None,
                    started_at: None,
                    completed_at: None,
                    latest_diff: None,
                    latest_plan_snapshot: None,
                    token_usage_snapshot: None,
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
        source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
        source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
                    source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
                    source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
        source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
async fn supervisor_stop_daemon_transition_stops_daemon() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();

    harness
        .dispatch(UserAction::ShowView(TopLevelView::Supervisor))
        .await;
    harness.dispatch_no_wait(UserAction::StopDaemon);
    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Stopping
    );

    harness.process().await;
    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Stopped
    );
}

#[tokio::test]
async fn supervisor_start_daemon_transition_starts_daemon() {
    let mut harness = AppHarness::new(sample_disconnected_snapshot())
        .await
        .unwrap();

    harness
        .dispatch(UserAction::ShowView(TopLevelView::Supervisor))
        .await;
    harness.dispatch_no_wait(UserAction::StartDaemon);
    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Starting
    );
    assert_eq!(
        harness.state().daemon_lifecycle_error.as_deref().is_none(),
        true
    );
    harness.process().await;
    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Running
    );
    let commands = harness.recorded_commands().await;
    assert!(commands.contains(&BackendCommand::StartDaemon));
}

#[tokio::test]
async fn supervisor_restart_daemon_dispatches_restart_request_sequence() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Supervisor))
        .await;
    harness.dispatch_no_wait(UserAction::RestartDaemon);
    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Restarting
    );

    harness.process().await;
    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Running
    );
    let commands = harness.recorded_commands().await;
    let stop_index = commands
        .iter()
        .position(|command| command == &BackendCommand::StopDaemon);
    let start_index = commands
        .iter()
        .position(|command| command == &BackendCommand::StartDaemon);
    assert!(matches!((stop_index, start_index), (Some(_), Some(_))));
    assert!(
        stop_index.unwrap() < start_index.unwrap(),
        "restart should stop then start (commands were {commands:?})"
    );
}

#[tokio::test]
async fn supervisor_start_failure_surfaces_error_state() {
    let mut harness = AppHarness::new(sample_disconnected_snapshot())
        .await
        .unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Supervisor))
        .await;

    harness.fail_next_command("start failed").await;
    harness.dispatch(UserAction::StartDaemon).await;

    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Failed
    );
    assert!(
        harness
            .state()
            .daemon_lifecycle_error
            .as_deref()
            .is_some_and(|error| error.contains("start failed"))
    );
    assert!(
        harness
            .state()
            .banner
            .as_ref()
            .is_some_and(|banner| banner.message.contains("Daemon start failed"))
    );
}

#[tokio::test]
async fn supervisor_restart_start_failure_surfaces_error_state() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Supervisor))
        .await;

    harness.dispatch_no_wait(UserAction::RestartDaemon);
    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Restarting
    );
    harness
        .inject_ui_event(UiEvent::DaemonStopped { stopping: true })
        .await;
    harness
        .inject_ui_event(UiEvent::DaemonStartFailed("start timed out".to_string()))
        .await;
    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Failed
    );
    assert!(
        harness
            .state()
            .daemon_lifecycle_error
            .as_deref()
            .is_some_and(|error| error == "start timed out")
    );
}

#[tokio::test]
async fn supervisor_stop_rejected_on_already_stopped_keeps_state() {
    let mut harness = AppHarness::new(sample_disconnected_snapshot())
        .await
        .unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Supervisor))
        .await;
    harness.dispatch(UserAction::StopDaemon).await;

    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Stopped
    );
    assert_eq!(
        harness.state().daemon_lifecycle_error.as_deref(),
        Some("daemon already stopped")
    );
}

#[tokio::test]
async fn supervisor_redundant_lifecycle_keys_are_ignored_while_inflight() {
    let mut harness = AppHarness::new(sample_disconnected_snapshot())
        .await
        .unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Supervisor))
        .await;

    harness.dispatch_no_wait(UserAction::StartDaemon);
    harness.dispatch_no_wait(UserAction::StopDaemon);
    harness.dispatch_no_wait(UserAction::RestartDaemon);
    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Starting
    );

    harness.process().await;
    assert_eq!(
        harness.state().daemon_lifecycle,
        DaemonLifecycleState::Running
    );
    let commands = harness.recorded_commands().await;
    let start_count = commands
        .iter()
        .filter(|command| **command == BackendCommand::StartDaemon)
        .count();
    assert_eq!(start_count, 1);
}

#[tokio::test]
async fn supervisor_footer_shows_global_and_supervisor_actions_once_each() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Supervisor))
        .await;

    let keys_line = harness
        .render_text(160, 42)
        .lines()
        .find(|line| line.contains("keys:"))
        .map(str::to_string)
        .unwrap_or_default();
    assert!(keys_line.contains("left/right"));
    assert!(!keys_line.contains("tab focus"));
    assert_eq!(keys_line.matches("x stop daemon").count(), 1);
    assert!(keys_line.contains("s start daemon"));
    assert!(keys_line.contains("R restart daemon"));
    assert!(keys_line.contains("m refresh models"));
}

#[tokio::test]
async fn collaboration_footer_shows_tab_for_focus_and_arrows_for_selection() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowView(TopLevelView::Collaboration))
        .await;

    let keys_line = harness
        .render_text(160, 42)
        .lines()
        .find(|line| line.contains("keys:"))
        .map(str::to_string)
        .unwrap_or_default();
    assert!(keys_line.contains("left/right"));
    assert!(keys_line.contains("tab focus"));
    assert!(keys_line.contains("up/down"));
    assert!(!keys_line.contains("j/k"));
    assert!(!keys_line.contains("h/l"));
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
async fn arrow_keys_only_move_the_focused_list_selection() {
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
        source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
        source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
                    source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
        source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
        source_kind: ipc::PlanningSummarySourceKind::Collaboration,
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
    assert!(expanded.contains("Status"), "{expanded}");
    assert!(expanded.contains("Hierarchy"), "{expanded}");
    assert!(expanded.contains("Composer"), "{expanded}");

    for (width, height) in [(120, 40), (100, 30), (80, 24)] {
        harness
            .dispatch(UserAction::ShowView(TopLevelView::Overview))
            .await;
        let overview = harness.render_text(width, height);
        assert!(
            overview.contains("Status"),
            "missing main-status header at {width}x{height}\n{overview}"
        );
        assert!(
            overview.contains("Hierarchy"),
            "missing main hierarchy at {width}x{height}\n{overview}"
        );
        assert!(
            overview.contains("Composer"),
            "missing main composer area at {width}x{height}\n{overview}"
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
async fn main_surface_renders_expected_three_row_structure() {
    let mut harness = AppHarness::new(sample_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let rendered = harness.render_text(160, 42);
    assert!(rendered.contains("Status"), "{rendered}");
    assert!(rendered.contains("Program"), "{rendered}");
    assert!(rendered.contains("Updates"), "{rendered}");
    assert!(rendered.contains("Hierarchy"), "{rendered}");
    assert!(rendered.contains("Composer"), "{rendered}");
}

#[tokio::test]
async fn main_hierarchy_groups_workstream_work_unit_and_thread_rows() {
    let harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    let hierarchy = harness.main_hierarchy_vm();
    assert!(hierarchy.rows.iter().any(|row| {
        row.kind == view_model::HierarchyRowKind::Workstream
            && row.label == "Collaboration hardening"
            && row.depth == 0
    }));
    assert!(hierarchy.rows.iter().any(|row| {
        row.kind == view_model::HierarchyRowKind::WorkUnit
            && row.label == "Snapshot wiring"
            && row.depth == 1
    }));
    assert!(hierarchy.rows.iter().any(|row| {
        row.kind == view_model::HierarchyRowKind::Thread
            && row.label == "thread-1"
            && row.depth == 2
    }));
    assert!(hierarchy.rows.iter().any(|row| {
        row.kind == view_model::HierarchyRowKind::Thread
            && row.label == "thread-2"
            && row.depth == 2
    }));
}

#[tokio::test]
async fn main_hierarchy_expand_and_collapse_change_visible_rows_correctly() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    let initial_rows = harness.main_hierarchy_vm().rows;
    assert!(initial_rows.iter().any(|row| row.label == "thread-1"));

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    assert_eq!(
        harness.state().main_view.selected,
        Some(MainHierarchySelection::WorkUnit {
            workstream_id: "ws-1".to_string(),
            work_unit_id: "wu-1".to_string(),
        })
    );

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    let collapsed_rows = harness.main_hierarchy_vm().rows;
    assert!(!collapsed_rows.iter().any(|row| row.label == "thread-1"));

    harness.dispatch(UserAction::ExpandSelectedInView).await;
    let expanded_rows = harness.main_hierarchy_vm().rows;
    assert!(expanded_rows.iter().any(|row| row.label == "thread-1"));
}

#[tokio::test]
async fn main_selection_updates_detail_panel_by_row_kind() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let initial = harness.main_vm();
    assert!(initial.detail_panel.title.contains("Tracked Thread"));

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    let work_unit = harness.main_vm();
    assert!(work_unit.detail_panel.title.contains("Work Unit wu-1"));

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    let collapsed_work_unit = harness.main_vm();
    assert!(
        collapsed_work_unit
            .detail_panel
            .title
            .contains("Work Unit wu-1")
    );

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    let workstream = harness.main_vm();
    assert!(workstream.detail_panel.title.contains("Workstream"));
}

#[tokio::test]
async fn main_header_and_footer_surface_runtime_summary_and_prompt_region() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();
    harness
        .inject_ui_event(UiEvent::ReconnectScheduled {
            attempt: 3,
            delay_ms: 500,
        })
        .await;

    let main = harness.main_vm();
    assert_eq!(harness.state().main_view.program_view, ProgramView::Main);
    assert!(
        main.header
            .status_segments
            .iter()
            .any(|segment| segment.label == "reconnect" && segment.value == "3")
    );
    assert!(
        main.footer_prompt
            .prompt_lines
            .iter()
            .any(|line| line.contains("mode: Inspect"))
    );
    assert!(main.footer_prompt.hint_line.contains("n new"));
}

#[tokio::test]
async fn snapshot_refresh_keeps_main_selection_stable() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    assert_eq!(
        harness.state().main_view.selected,
        Some(MainHierarchySelection::WorkUnit {
            workstream_id: "ws-1".to_string(),
            work_unit_id: "wu-1".to_string(),
        })
    );

    harness
        .replace_snapshot(sample_main_surface_snapshot())
        .await;
    harness.dispatch(UserAction::Refresh).await;

    assert_eq!(
        harness.state().main_view.selected,
        Some(MainHierarchySelection::WorkUnit {
            workstream_id: "ws-1".to_string(),
            work_unit_id: "wu-1".to_string(),
        })
    );
    let rendered = harness.render_text(160, 42);
    assert!(rendered.contains("Hierarchy"), "{rendered}");
}

#[tokio::test]
async fn reconnect_invalidates_stale_authority_state_then_reloads_hierarchy() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    harness.dispatch(UserAction::EditSelectedMainEntity).await;
    harness.dispatch(UserAction::EditSelectedMainEntity).await;
    assert!(matches!(
        harness.state().authority_main.footer,
        MainFooterState::EditTrackedThread(_)
    ));
    assert!(
        !harness
            .state()
            .authority_main
            .tracked_thread_details
            .is_empty()
    );
    let expected_selection = harness.state().main_view.selected.clone();

    harness
        .replace_snapshot(sample_main_surface_snapshot())
        .await;
    harness.disconnect_events().await;
    harness.process().await;

    assert_eq!(
        harness.state().daemon_phase,
        DaemonConnectionPhase::Reconnecting
    );
    assert!(
        harness
            .state()
            .authority_main
            .hierarchy
            .workstreams
            .is_empty()
    );
    assert!(harness.state().authority_main.workstream_details.is_empty());
    assert!(harness.state().authority_main.work_unit_details.is_empty());
    assert!(
        harness
            .state()
            .authority_main
            .tracked_thread_details
            .is_empty()
    );
    assert!(matches!(
        harness.state().authority_main.footer,
        MainFooterState::Inspect
    ));
    assert_eq!(
        harness.state().main_view.pending_selection,
        expected_selection
    );

    harness.force_reconnect_now();
    harness.process().await;

    assert_eq!(
        harness.state().daemon_phase,
        DaemonConnectionPhase::Connected
    );
    assert!(
        !harness
            .state()
            .authority_main
            .hierarchy
            .workstreams
            .is_empty()
    );
    assert_eq!(harness.state().main_view.selected, expected_selection);
}

#[tokio::test]
async fn main_footer_mode_transitions_and_contextual_hints_are_explicit() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    let inspect = harness.main_vm().footer_prompt;
    assert!(inspect.hint_line.contains("n new"));
    assert!(inspect.hint_line.contains("e edit"));
    assert!(inspect.hint_line.contains("d delete"));

    harness.dispatch(UserAction::CreateWorkstream).await;
    assert!(matches!(
        harness.state().authority_main.footer,
        MainFooterState::CreateWorkstream(_)
    ));
    let footer = harness.main_vm().footer_prompt;
    assert!(
        footer
            .prompt_lines
            .iter()
            .any(|line| line.contains("CreateWorkstream"))
    );
    assert!(footer.hint_line.contains("ctrl+s submit"));

    harness.dispatch(UserAction::CancelMainFooter).await;
    assert!(matches!(
        harness.state().authority_main.footer,
        MainFooterState::Inspect
    ));

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    let work_unit_footer = harness.main_vm().footer_prompt;
    assert!(work_unit_footer.hint_line.contains("t tracked-thread"));
}

#[tokio::test]
async fn create_workstream_flow_routes_through_authority_backend() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    harness.dispatch(UserAction::CreateWorkstream).await;
    type_main_footer_text(&mut harness, "Local authority").await;
    harness.dispatch(UserAction::MainFooterNextField).await;
    type_main_footer_text(&mut harness, "/repo/orcas").await;
    harness.dispatch(UserAction::SubmitMainFooter).await;

    let hierarchy = harness.main_hierarchy_vm();
    assert!(
        hierarchy
            .rows
            .iter()
            .any(|row| row.label == "Local authority")
    );
    assert!(matches!(
        harness
            .recorded_commands()
            .await
            .iter()
            .find(|command| matches!(command, BackendCommand::CreateAuthorityWorkstream { .. })),
        Some(_)
    ));
}

#[tokio::test]
async fn create_work_unit_under_selected_workstream_routes_through_authority_backend() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness
        .dispatch(UserAction::CreateWorkUnitForSelection)
        .await;
    type_main_footer_text(&mut harness, "SQLite projector").await;
    harness.dispatch(UserAction::SubmitMainFooter).await;

    let hierarchy = harness.main_hierarchy_vm();
    assert!(
        hierarchy
            .rows
            .iter()
            .any(|row| row.label == "SQLite projector")
    );
    assert!(matches!(
        harness
            .recorded_commands()
            .await
            .iter()
            .find(|command| matches!(command, BackendCommand::CreateAuthorityWorkUnit { .. })),
        Some(_)
    ));
}

#[tokio::test]
async fn create_tracked_thread_under_selected_work_unit_routes_through_authority_backend() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness
        .dispatch(UserAction::CreateTrackedThreadForSelection)
        .await;
    type_main_footer_text(&mut harness, "operator lane").await;
    harness.dispatch(UserAction::MainFooterNextField).await;
    type_main_footer_text(&mut harness, "/repo/orcas").await;
    harness.dispatch(UserAction::SubmitMainFooter).await;

    let hierarchy = harness.main_hierarchy_vm();
    assert!(
        hierarchy
            .rows
            .iter()
            .any(|row| row.label == "operator lane")
    );
    assert!(matches!(
        harness
            .recorded_commands()
            .await
            .iter()
            .find(|command| matches!(command, BackendCommand::CreateAuthorityTrackedThread { .. })),
        Some(_)
    ));
}

#[tokio::test]
async fn edit_selected_main_entity_loads_authority_detail_before_opening_form() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    harness.fail_next_command("detail unavailable").await;
    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness.dispatch(UserAction::EditSelectedMainEntity).await;
    assert!(matches!(
        harness.state().authority_main.footer,
        MainFooterState::Inspect
    ));
    let banner = harness
        .state()
        .banner
        .as_ref()
        .expect("warning banner should be visible while detail loads");
    assert_eq!(banner.level, BannerLevel::Warning);
    assert!(banner.message.contains("detail is still loading"));
    assert!(matches!(
        harness.recorded_commands().await.last(),
        Some(BackendCommand::GetAuthorityWorkUnit { .. })
    ));
}

#[tokio::test]
async fn edit_workstream_work_unit_and_tracked_thread_flow_through_authority_backend() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    harness.dispatch(UserAction::EditSelectedMainEntity).await;
    assert!(matches!(
        harness.recorded_commands().await.last(),
        Some(BackendCommand::GetAuthorityTrackedThread { .. })
    ));
    harness.dispatch(UserAction::EditSelectedMainEntity).await;
    clear_main_footer_text(&mut harness, "thread-1".len()).await;
    type_main_footer_text(&mut harness, "tracked local").await;
    harness.dispatch(UserAction::MainFooterNextField).await;
    clear_main_footer_text(&mut harness, "/tmp/orcas".len()).await;
    type_main_footer_text(&mut harness, "/repo/tracked").await;
    harness.dispatch(UserAction::SubmitMainFooter).await;
    assert!(
        harness
            .main_hierarchy_vm()
            .rows
            .iter()
            .any(|row| row.label == "tracked local")
    );

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness.dispatch(UserAction::EditSelectedMainEntity).await;
    assert!(matches!(
        harness.recorded_commands().await.last(),
        Some(BackendCommand::GetAuthorityWorkUnit { .. })
    ));
    harness.dispatch(UserAction::EditSelectedMainEntity).await;
    clear_main_footer_text(&mut harness, "Snapshot wiring".len()).await;
    type_main_footer_text(&mut harness, "Snapshot reducer").await;
    harness.dispatch(UserAction::SubmitMainFooter).await;
    assert!(
        harness
            .main_hierarchy_vm()
            .rows
            .iter()
            .any(|row| row.label == "Snapshot reducer")
    );

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness.dispatch(UserAction::EditSelectedMainEntity).await;
    assert!(matches!(
        harness.recorded_commands().await.last(),
        Some(BackendCommand::GetAuthorityWorkstream { .. })
    ));
    harness.dispatch(UserAction::EditSelectedMainEntity).await;
    clear_main_footer_text(&mut harness, "Collaboration hardening".len()).await;
    type_main_footer_text(&mut harness, "Authority hardening").await;
    harness.dispatch(UserAction::MainFooterNextField).await;
    clear_main_footer_text(
        &mut harness,
        "Harden collaboration snapshot semantics.".len(),
    )
    .await;
    type_main_footer_text(&mut harness, "/repo/orcas").await;
    harness.dispatch(UserAction::SubmitMainFooter).await;
    assert!(
        harness
            .main_hierarchy_vm()
            .rows
            .iter()
            .any(|row| row.label == "Authority hardening")
    );

    let commands = harness.recorded_commands().await;
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, BackendCommand::EditAuthorityTrackedThread { .. }))
    );
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, BackendCommand::EditAuthorityWorkUnit { .. }))
    );
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, BackendCommand::EditAuthorityWorkstream { .. }))
    );
}

#[tokio::test]
async fn delete_flows_are_confirmation_gated_and_reselect_sensibly() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    harness.dispatch(UserAction::DeleteSelectedMainEntity).await;
    assert!(matches!(
        harness.state().authority_main.footer,
        MainFooterState::ConfirmDelete(_)
    ));
    let footer = harness.main_vm().footer_prompt;
    assert!(
        footer
            .prompt_lines
            .iter()
            .any(|line| line.contains("ConfirmDelete"))
    );
    harness.dispatch(UserAction::SubmitMainFooter).await;
    assert!(
        !harness
            .main_hierarchy_vm()
            .rows
            .iter()
            .any(|row| row.label == "thread-1")
    );

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness.dispatch(UserAction::DeleteSelectedMainEntity).await;
    type_main_footer_text(&mut harness, "Snapshot wiring").await;
    harness.dispatch(UserAction::SubmitMainFooter).await;
    assert!(
        !harness
            .main_hierarchy_vm()
            .rows
            .iter()
            .any(|row| row.label == "Snapshot wiring")
    );
    assert_eq!(
        harness.state().main_view.selected,
        Some(MainHierarchySelection::Thread {
            workstream_id: "ws-1".to_string(),
            work_unit_id: "wu-2".to_string(),
            thread_id: "thread-2".to_string(),
        })
    );

    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness.dispatch(UserAction::CollapseSelectedInView).await;
    harness.dispatch(UserAction::DeleteSelectedMainEntity).await;
    type_main_footer_text(&mut harness, "Collaboration hardening").await;
    harness.dispatch(UserAction::SubmitMainFooter).await;
    assert_eq!(
        harness.state().main_view.selected,
        Some(MainHierarchySelection::WorkUnit {
            workstream_id: "ws-2".to_string(),
            work_unit_id: "wu-3".to_string(),
        })
    );

    let commands = harness.recorded_commands().await;
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, BackendCommand::DeleteAuthorityTrackedThread { .. }))
    );
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, BackendCommand::DeleteAuthorityWorkUnit { .. }))
    );
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, BackendCommand::DeleteAuthorityWorkstream { .. }))
    );
}

#[tokio::test]
async fn main_refresh_keeps_authority_mutations_visible_after_requery() {
    let mut harness = AppHarness::new(sample_main_surface_snapshot())
        .await
        .unwrap();

    harness.dispatch(UserAction::CreateWorkstream).await;
    type_main_footer_text(&mut harness, "Reloaded authority").await;
    harness.dispatch(UserAction::MainFooterNextField).await;
    type_main_footer_text(&mut harness, "/repo/reloaded").await;
    harness.dispatch(UserAction::SubmitMainFooter).await;
    harness.dispatch(UserAction::Refresh).await;

    assert!(
        harness
            .main_hierarchy_vm()
            .rows
            .iter()
            .any(|row| row.label == "Reloaded authority")
    );
}

#[tokio::test]
async fn review_surface_renders_queue_detail_and_header_counts() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness
        .set_workunit_detail(sample_workunit_detail("wu-2"))
        .await;
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let rendered = harness.render_text(160, 42);
    assert!(rendered.contains("Review Queue"), "{rendered}");
    assert!(rendered.contains("Review Summary"), "{rendered}");
    assert!(rendered.contains("Review Actions"), "{rendered}");
    assert!(rendered.contains("mode=sectioned"), "{rendered}");
    assert!(rendered.contains("decisions=1"), "{rendered}");
    assert!(rendered.contains("proposals=1"), "{rendered}");
    assert!(rendered.contains("failures=1"), "{rendered}");
}

#[tokio::test]
async fn review_queue_contains_decision_proposal_failure_and_review_rows() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness
        .set_workunit_detail(sample_workunit_detail("wu-2"))
        .await;
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let review = harness.review_queue_vm();
    assert!(
        review
            .rows
            .iter()
            .any(|row| row.kind == view_model::ReviewRowKind::Decision)
    );
    assert!(
        review
            .rows
            .iter()
            .any(|row| row.kind == view_model::ReviewRowKind::Proposal)
    );
    assert!(
        review
            .rows
            .iter()
            .any(|row| row.kind == view_model::ReviewRowKind::Failure)
    );
    assert!(
        review
            .rows
            .iter()
            .any(|row| row.kind == view_model::ReviewRowKind::ReviewRequired)
    );
    assert_eq!(
        review
            .sections
            .iter()
            .map(|section| section.label.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Open Decisions",
            "Open Proposals",
            "Failures",
            "Review Required",
        ]
    );
}

#[tokio::test]
async fn review_selection_updates_detail_and_queue_selection() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness
        .set_workunit_detail(sample_workunit_detail("wu-2"))
        .await;
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let initial = harness.review_vm();
    assert!(initial.detail_panel.title.contains("Decision"));
    assert_eq!(
        harness.state().review_view.selected,
        Some(ReviewSelection::Decision {
            decision_id: "std-1".to_string(),
        })
    );

    harness.dispatch(UserAction::SelectNextInView).await;
    let proposal = harness.review_vm();
    assert!(proposal.detail_panel.title.contains("Proposal"));

    harness.dispatch(UserAction::SelectNextInView).await;
    let failure = harness.review_vm();
    assert!(failure.detail_panel.title.contains("Failure"));

    harness.dispatch(UserAction::SelectNextInView).await;
    let review_required = harness.review_vm();
    assert!(review_required.detail_panel.title.contains("Review"));
}

#[tokio::test]
async fn tab_switching_between_main_and_review_is_stable() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    assert_eq!(harness.state().main_view.program_view, ProgramView::Review);
    let review = harness.render_text(160, 42);
    assert!(review.contains("Review Queue"), "{review}");

    harness.dispatch(UserAction::CycleProgramView).await;
    assert_eq!(harness.state().main_view.program_view, ProgramView::Main);
    let main = harness.render_text(160, 42);
    assert!(main.contains("Hierarchy"), "{main}");
    assert!(!main.contains("Review Queue"), "{main}");
}

#[tokio::test]
async fn review_selection_survives_snapshot_refresh() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness
        .set_workunit_detail(sample_workunit_detail("wu-2"))
        .await;
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;
    harness.dispatch(UserAction::SelectNextInView).await;
    assert_eq!(
        harness.state().review_view.selected,
        Some(ReviewSelection::Proposal {
            work_unit_id: "wu-1".to_string(),
            proposal_id: "proposal-1".to_string(),
        })
    );

    harness.replace_snapshot(sample_review_snapshot()).await;
    harness.dispatch(UserAction::Refresh).await;

    assert_eq!(
        harness.state().review_view.selected,
        Some(ReviewSelection::Proposal {
            work_unit_id: "wu-1".to_string(),
            proposal_id: "proposal-1".to_string(),
        })
    );
    let rendered = harness.render_text(160, 42);
    assert!(rendered.contains("Review Queue"), "{rendered}");
}

#[tokio::test]
async fn actionable_review_decision_exposes_approve_and_reject_affordances() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness
        .set_workunit_detail(sample_workunit_detail("wu-2"))
        .await;
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let review = harness.review_vm();
    assert_eq!(
        review.footer.actions,
        vec![
            view_model::ReviewActionViewModel {
                key: "a".to_string(),
                label: "approve and send".to_string(),
            },
            view_model::ReviewActionViewModel {
                key: "d".to_string(),
                label: "reject".to_string(),
            },
        ]
    );
    assert!(review.footer.hint_line.contains("a approve"));
    assert!(review.footer.hint_line.contains("d reject"));
}

#[tokio::test]
async fn approve_from_review_updates_queue_detail_and_header_state() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness
        .set_workunit_detail(sample_workunit_detail("wu-2"))
        .await;
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    harness
        .dispatch(UserAction::ApproveSelectedSupervisorDecision)
        .await;

    let commands = harness.recorded_commands().await;
    assert!(commands.iter().any(|command| {
        matches!(
            command,
            BackendCommand::ApproveSupervisorDecision { decision_id }
                if decision_id == "std-1"
        )
    }));
    assert_eq!(
        harness.state().review_view.selected,
        Some(ReviewSelection::Proposal {
            work_unit_id: "wu-1".to_string(),
            proposal_id: "proposal-1".to_string(),
        })
    );
    assert_eq!(harness.review_queue_vm().rows.len(), 3);
    assert!(
        harness
            .review_vm()
            .header
            .summary_lines
            .iter()
            .any(|line| line.contains("decisions=0"))
    );
    assert_eq!(
        harness
            .state()
            .collaboration
            .supervisor_turn_decisions
            .iter()
            .find(|decision| decision.decision_id == "std-1")
            .map(|decision| decision.status),
        Some(orcas_core::SupervisorTurnDecisionStatus::Sent)
    );
    assert!(harness.review_vm().detail_panel.title.contains("Proposal"));
}

#[tokio::test]
async fn reject_from_review_updates_queue_detail_and_header_state() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness
        .set_workunit_detail(sample_workunit_detail("wu-2"))
        .await;
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    harness
        .dispatch(UserAction::RejectSelectedSupervisorDecision)
        .await;

    let commands = harness.recorded_commands().await;
    assert!(commands.iter().any(|command| {
        matches!(
            command,
            BackendCommand::RejectSupervisorDecision { decision_id }
                if decision_id == "std-1"
        )
    }));
    assert_eq!(
        harness.state().review_view.selected,
        Some(ReviewSelection::Proposal {
            work_unit_id: "wu-1".to_string(),
            proposal_id: "proposal-1".to_string(),
        })
    );
    assert_eq!(harness.review_queue_vm().rows.len(), 3);
    assert!(
        harness
            .review_vm()
            .header
            .summary_lines
            .iter()
            .any(|line| line.contains("decisions=0"))
    );
    assert_eq!(
        harness
            .state()
            .collaboration
            .supervisor_turn_decisions
            .iter()
            .find(|decision| decision.decision_id == "std-1")
            .map(|decision| decision.status),
        Some(orcas_core::SupervisorTurnDecisionStatus::Rejected)
    );
    assert!(harness.review_vm().detail_panel.title.contains("Proposal"));
}

#[tokio::test]
async fn non_actionable_review_rows_do_not_expose_invalid_actions() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness
        .set_workunit_detail(sample_workunit_detail("wu-2"))
        .await;
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;
    harness.dispatch(UserAction::SelectNextInView).await;

    let review = harness.review_vm();
    assert!(review.footer.actions.is_empty());
    assert!(!review.footer.hint_line.contains("a approve"));
    assert!(!review.footer.hint_line.contains("d reject"));
}

#[tokio::test]
async fn review_mutation_failure_preserves_selection_and_surfaces_feedback() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .set_workunit_detail(sample_workunit_detail("wu-1"))
        .await;
    harness
        .set_workunit_detail(sample_workunit_detail("wu-2"))
        .await;
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;
    harness.fail_next_command("backend exploded").await;

    harness
        .dispatch(UserAction::ApproveSelectedSupervisorDecision)
        .await;

    assert_eq!(
        harness.state().review_view.selected,
        Some(ReviewSelection::Decision {
            decision_id: "std-1".to_string(),
        })
    );
    assert_eq!(
        harness.state().banner.as_ref().map(|banner| banner.level),
        Some(BannerLevel::Error)
    );
    assert!(
        harness
            .state()
            .banner
            .as_ref()
            .is_some_and(|banner| banner.message.contains("supervisor approve failed"))
    );
    assert!(
        harness
            .review_vm()
            .header
            .summary_lines
            .iter()
            .any(|line| line.contains("decisions=1"))
    );
}

#[tokio::test]
async fn review_queue_sections_render_expected_categories() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;

    let rendered = harness.render_text(160, 42);
    assert!(rendered.contains("Review Queue [sectioned]"), "{rendered}");
    assert!(rendered.contains("Open Decisions"), "{rendered}");
    assert!(rendered.contains("Open Proposals"), "{rendered}");
    assert!(rendered.contains("Failures"), "{rendered}");
    assert!(rendered.contains("Review Required"), "{rendered}");
}

#[tokio::test]
async fn proposal_detail_fallback_is_informative_without_cached_detail() {
    let mut harness = AppHarness::new(sample_review_snapshot()).await.unwrap();
    harness
        .dispatch(UserAction::ShowProgramView(ProgramView::Review))
        .await;
    harness.dispatch(UserAction::Refresh).await;
    harness.dispatch(UserAction::SelectNextInView).await;

    let review = harness.review_vm();
    assert!(review.detail_panel.title.contains("Proposal"));
    assert!(
        review
            .detail_panel
            .lines
            .iter()
            .any(|line| line.contains("Detailed proposal pack is not cached yet"))
    );
    assert!(
        review
            .detail_panel
            .lines
            .iter()
            .any(|line| line.contains("decision: continue"))
    );
    assert!(
        review
            .detail_panel
            .lines
            .iter()
            .any(|line| line.contains("operator_read: supervisor has an open proposal context"))
    );
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
