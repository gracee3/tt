use orcas_core::authority;
use orcas_core::ipc;

#[derive(Debug, Clone)]
pub struct WorkstreamsDashboardData {
    pub hierarchy: authority::HierarchySnapshot,
    pub snapshot: ipc::StateSnapshot,
    pub planning_sessions: Vec<orcas_core::PlanningSession>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedThreadRuntimeStatusView {
    pub headline: String,
    pub detail: String,
    pub assignment_id: Option<String>,
    pub codex_thread_id: Option<String>,
    pub supervisor_waiting: bool,
}

#[derive(Debug, Clone)]
pub struct LiveThreadLinkageView {
    pub tracked_thread: Option<authority::TrackedThreadSummary>,
    pub codex_assignment: Option<ipc::CodexThreadAssignmentSummary>,
    pub assignment: Option<ipc::AssignmentSummary>,
    pub open_decision: Option<ipc::SupervisorTurnDecisionSummary>,
}

pub fn inferred_live_thread_for_assignment(
    assignment: &ipc::AssignmentSummary,
    dashboard: &WorkstreamsDashboardData,
) -> Option<ipc::ThreadSummary> {
    assignment
        .codex_thread_id
        .as_ref()
        .and_then(|thread_id| {
            dashboard
                .snapshot
                .threads
                .iter()
                .find(|thread| thread.id == *thread_id)
                .cloned()
        })
        .or_else(|| {
            assignment
                .tracked_thread_id
                .as_ref()
                .and_then(|tracked_thread_id| {
                    dashboard
                        .hierarchy
                        .workstreams
                        .iter()
                        .flat_map(|workstream| workstream.work_units.iter())
                        .flat_map(|work_unit| work_unit.tracked_threads.iter())
                        .find(|tracked_thread| tracked_thread.id.as_str() == tracked_thread_id)
                        .and_then(|tracked_thread| tracked_thread.upstream_thread_id.as_ref())
                        .and_then(|thread_id| {
                            dashboard
                                .snapshot
                                .threads
                                .iter()
                                .find(|thread| thread.id == *thread_id)
                                .cloned()
                        })
                })
        })
        .or_else(|| {
            dashboard
                .snapshot
                .collaboration
                .codex_thread_assignments
                .iter()
                .find(|candidate| candidate.assignment_id == assignment.id)
                .and_then(|candidate| {
                    dashboard
                        .snapshot
                        .threads
                        .iter()
                        .find(|thread| thread.id == candidate.codex_thread_id)
                        .cloned()
                })
        })
        .or_else(|| {
            dashboard
                .snapshot
                .threads
                .iter()
                .find(|thread| thread.preview.contains(&assignment.id))
                .cloned()
        })
}

pub fn humanize_snake_case(raw: &str) -> String {
    raw.split('_')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn tracked_thread_runtime_status(
    thread: &authority::TrackedThreadSummary,
    dashboard: &WorkstreamsDashboardData,
) -> TrackedThreadRuntimeStatusView {
    let live_thread = thread
        .upstream_thread_id
        .as_ref()
        .and_then(|upstream_thread_id| {
            dashboard
                .snapshot
                .threads
                .iter()
                .find(|candidate| candidate.id == *upstream_thread_id)
                .cloned()
        });
    let codex_assignment = dashboard
        .snapshot
        .collaboration
        .codex_thread_assignments
        .iter()
        .filter(|assignment| assignment.work_unit_id == thread.work_unit_id.as_str())
        .find(|assignment| {
            thread.upstream_thread_id.as_deref() == Some(assignment.codex_thread_id.as_str())
                || assignment.active
        })
        .cloned();
    let assignment = codex_assignment
        .as_ref()
        .and_then(|codex_assignment| {
            dashboard
                .snapshot
                .collaboration
                .assignments
                .iter()
                .find(|assignment| assignment.id == codex_assignment.assignment_id)
                .cloned()
        })
        .or_else(|| {
            let assignments = dashboard
                .snapshot
                .collaboration
                .assignments
                .iter()
                .filter(|assignment| assignment.work_unit_id == thread.work_unit_id.as_str())
                .cloned()
                .collect::<Vec<_>>();
            (assignments.len() == 1)
                .then(|| assignments.into_iter().next())
                .flatten()
        });
    let open_decision = codex_assignment.as_ref().and_then(|codex_assignment| {
        dashboard
            .snapshot
            .collaboration
            .supervisor_turn_decisions
            .iter()
            .filter(|decision| {
                decision.assignment_id == codex_assignment.assignment_id && decision.open
            })
            .max_by_key(|decision| decision.created_at)
            .cloned()
    });

    let headline = if assignment.as_ref().is_some_and(|assignment| {
        assignment.status == orcas_core::AssignmentStatus::AwaitingDecision
    }) || open_decision.as_ref().is_some_and(|decision| {
        matches!(
            decision.status,
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman
                | orcas_core::SupervisorTurnDecisionStatus::Approved
        )
    }) {
        "Waiting for supervisor".to_string()
    } else if assignment
        .as_ref()
        .is_some_and(|assignment| assignment.status == orcas_core::AssignmentStatus::Running)
        || codex_assignment.as_ref().is_some_and(|assignment| {
            assignment.status == orcas_core::CodexThreadAssignmentStatus::Active
        })
    {
        "In progress".to_string()
    } else if assignment
        .as_ref()
        .is_some_and(|assignment| assignment.status == orcas_core::AssignmentStatus::Created)
    {
        "Queued".to_string()
    } else if codex_assignment.as_ref().is_some_and(|assignment| {
        assignment.status == orcas_core::CodexThreadAssignmentStatus::Paused
    }) {
        "Paused".to_string()
    } else if assignment.as_ref().is_some_and(|assignment| {
        matches!(
            assignment.status,
            orcas_core::AssignmentStatus::Closed
                | orcas_core::AssignmentStatus::Interrupted
                | orcas_core::AssignmentStatus::Failed
                | orcas_core::AssignmentStatus::Lost
        )
    }) {
        humanize_snake_case(
            &serde_json::to_string(
                &assignment
                    .as_ref()
                    .expect("assignment status checked above")
                    .status,
            )
            .unwrap_or_default()
            .trim_matches('"')
            .to_string(),
        )
    } else if codex_assignment.as_ref().is_some_and(|assignment| {
        matches!(
            assignment.status,
            orcas_core::CodexThreadAssignmentStatus::Completed
                | orcas_core::CodexThreadAssignmentStatus::Released
        )
    }) {
        "Stopped".to_string()
    } else if thread.deleted_at.is_some() {
        "Removed".to_string()
    } else if live_thread
        .as_ref()
        .is_some_and(|thread| thread.turn_in_flight || thread.status == "active")
    {
        "In progress".to_string()
    } else if thread.binding_state == authority::TrackedThreadBindingState::Missing {
        "Missing".to_string()
    } else if thread.binding_state == authority::TrackedThreadBindingState::Detached {
        "Detached".to_string()
    } else {
        "Tracked only".to_string()
    };

    let mut detail_parts = Vec::new();
    if let Some(codex_assignment) = codex_assignment.as_ref() {
        detail_parts.push(format!(
            "codex {}",
            humanize_snake_case(
                serde_json::to_string(&codex_assignment.status)
                    .unwrap_or_default()
                    .trim_matches('"')
            )
        ));
        detail_parts.push(format!(
            "bootstrap {}",
            humanize_snake_case(
                serde_json::to_string(&codex_assignment.bootstrap_state)
                    .unwrap_or_default()
                    .trim_matches('"')
            )
        ));
    }
    let supervisor_waiting = assignment.as_ref().is_some_and(|assignment| {
        assignment.status == orcas_core::AssignmentStatus::AwaitingDecision
    }) || open_decision.is_some();

    if let Some(live_thread) = live_thread.as_ref() {
        if supervisor_waiting && live_thread.status == "idle" {
            detail_parts.push("report submitted".to_string());
        } else {
            detail_parts.push(format!("thread {}", live_thread.status));
        }
        if live_thread.turn_in_flight {
            detail_parts.push("turn in flight".to_string());
        }
    }
    if let Some(assignment) = assignment.as_ref() {
        detail_parts.push(format!(
            "assignment {}",
            humanize_snake_case(
                serde_json::to_string(&assignment.status)
                    .unwrap_or_default()
                    .trim_matches('"')
            )
        ));
    }
    if let Some(decision) = open_decision.as_ref() {
        detail_parts.push(format!(
            "decision {}",
            humanize_snake_case(
                serde_json::to_string(&decision.status)
                    .unwrap_or_default()
                    .trim_matches('"')
            )
        ));
    }
    if detail_parts.is_empty() {
        detail_parts.push(format!(
            "binding {}",
            humanize_snake_case(
                serde_json::to_string(&thread.binding_state)
                    .unwrap_or_default()
                    .trim_matches('"')
            )
        ));
        if let Some(workspace_status) = thread.workspace_status {
            detail_parts.push(format!(
                "workspace {}",
                humanize_snake_case(
                    serde_json::to_string(&workspace_status)
                        .unwrap_or_default()
                        .trim_matches('"')
                )
            ));
        }
    }

    TrackedThreadRuntimeStatusView {
        headline,
        detail: detail_parts.join(" · "),
        assignment_id: assignment.map(|assignment| assignment.id),
        codex_thread_id: codex_assignment.map(|assignment| assignment.codex_thread_id),
        supervisor_waiting,
    }
}

pub fn live_thread_linkage(
    thread: &ipc::ThreadSummary,
    dashboard: &WorkstreamsDashboardData,
) -> LiveThreadLinkageView {
    let tracked_thread = dashboard
        .hierarchy
        .workstreams
        .iter()
        .flat_map(|workstream| workstream.work_units.iter())
        .flat_map(|work_unit| work_unit.tracked_threads.iter())
        .find(|tracked_thread| {
            tracked_thread.upstream_thread_id.as_deref() == Some(thread.id.as_str())
        })
        .cloned();
    let codex_assignment = dashboard
        .snapshot
        .collaboration
        .codex_thread_assignments
        .iter()
        .find(|assignment| assignment.codex_thread_id == thread.id)
        .cloned();
    let assignment = codex_assignment.as_ref().and_then(|codex_assignment| {
        dashboard
            .snapshot
            .collaboration
            .assignments
            .iter()
            .find(|assignment| assignment.id == codex_assignment.assignment_id)
            .cloned()
    });
    let open_decision = codex_assignment.as_ref().and_then(|codex_assignment| {
        dashboard
            .snapshot
            .collaboration
            .supervisor_turn_decisions
            .iter()
            .filter(|decision| {
                decision.assignment_id == codex_assignment.assignment_id && decision.open
            })
            .max_by_key(|decision| decision.created_at)
            .cloned()
    });

    LiveThreadLinkageView {
        tracked_thread,
        codex_assignment,
        assignment,
        open_decision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use orcas_core::authority::{
        TrackedThreadBackendKind, TrackedThreadBindingState, TrackedThreadId, WorkUnitId,
    };
    use orcas_core::events::ConnectionState;
    use orcas_core::ipc::{
        AssignmentSummary, CodexThreadAssignmentSummary, DaemonRuntimeMetadata, OperatorInboxState,
        SessionState, StateSnapshot, SupervisorTurnDecisionSummary,
    };

    fn tracked_thread() -> authority::TrackedThreadSummary {
        authority::TrackedThreadSummary {
            id: TrackedThreadId::parse("tt-1").expect("tracked thread id"),
            work_unit_id: WorkUnitId::parse("wu-1").expect("work unit id"),
            title: "Thread".to_string(),
            backend_kind: TrackedThreadBackendKind::Codex,
            upstream_thread_id: Some("codex-thread-1".to_string()),
            binding_state: TrackedThreadBindingState::Bound,
            workspace_strategy: None,
            workspace_status: None,
            revision: authority::Revision::initial(),
            updated_at: Utc::now(),
            deleted_at: None,
        }
    }

    fn empty_snapshot() -> StateSnapshot {
        StateSnapshot {
            daemon: ipc::DaemonStatusResponse {
                socket_path: "/tmp/orcasd.sock".to_string(),
                metadata_path: "/tmp/orcasd.json".to_string(),
                codex_endpoint: "ws://127.0.0.1:4500".to_string(),
                codex_binary_path: "/tmp/codex".to_string(),
                upstream: ConnectionState {
                    endpoint: "ws://127.0.0.1:4500".to_string(),
                    status: "connected".to_string(),
                    detail: None,
                },
                client_count: 0,
                known_threads: 0,
                runtime: DaemonRuntimeMetadata {
                    pid: 1,
                    started_at: Utc::now(),
                    version: "test".to_string(),
                    build_fingerprint: "test".to_string(),
                    binary_path: "/tmp/orcasd".to_string(),
                    socket_path: "/tmp/orcasd.sock".to_string(),
                    metadata_path: "/tmp/orcasd.json".to_string(),
                    git_commit: None,
                },
            },
            session: SessionState {
                active_thread_id: None,
                active_turns: Vec::new(),
            },
            threads: Vec::new(),
            active_thread: None,
            collaboration: ipc::CollaborationSnapshot::default(),
            operator_inbox: OperatorInboxState::default(),
            recent_events: Vec::new(),
        }
    }

    #[test]
    fn reports_waiting_for_supervisor_when_assignment_awaits_decision() {
        let mut snapshot = empty_snapshot();
        snapshot
            .collaboration
            .codex_thread_assignments
            .push(CodexThreadAssignmentSummary {
                assignment_id: "assign-1".to_string(),
                codex_thread_id: "codex-thread-1".to_string(),
                workstream_id: "ws-1".to_string(),
                work_unit_id: "wu-1".to_string(),
                supervisor_id: "sup-1".to_string(),
                assigned_by: "daemon".to_string(),
                assigned_at: Utc::now(),
                updated_at: Utc::now(),
                status: orcas_core::CodexThreadAssignmentStatus::Active,
                send_policy: orcas_core::CodexThreadSendPolicy::HumanApprovalRequired,
                bootstrap_state: orcas_core::CodexThreadBootstrapState::Sent,
                latest_basis_turn_id: None,
                latest_decision_id: None,
                notes: None,
                active: true,
            });
        snapshot.collaboration.assignments.push(AssignmentSummary {
            id: "assign-1".to_string(),
            work_unit_id: "wu-1".to_string(),
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: Default::default(),
            alignment_rationale: None,
            worker_id: "worker-1".to_string(),
            worker_session_id: "session-1".to_string(),
            codex_thread_id: None,
            tracked_thread_id: None,
            status: orcas_core::AssignmentStatus::AwaitingDecision,
            attempt_number: 1,
            updated_at: Utc::now(),
        });
        let dashboard = WorkstreamsDashboardData {
            hierarchy: authority::HierarchySnapshot::default(),
            snapshot,
            planning_sessions: Vec::new(),
        };

        let status = tracked_thread_runtime_status(&tracked_thread(), &dashboard);
        assert_eq!(status.headline, "Waiting for supervisor");
    }

    #[test]
    fn reports_in_progress_when_assignment_running() {
        let mut snapshot = empty_snapshot();
        snapshot.collaboration.assignments.push(AssignmentSummary {
            id: "assign-1".to_string(),
            work_unit_id: "wu-1".to_string(),
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: Default::default(),
            alignment_rationale: None,
            worker_id: "worker-1".to_string(),
            worker_session_id: "session-1".to_string(),
            codex_thread_id: None,
            tracked_thread_id: None,
            status: orcas_core::AssignmentStatus::Running,
            attempt_number: 1,
            updated_at: Utc::now(),
        });
        snapshot
            .collaboration
            .codex_thread_assignments
            .push(CodexThreadAssignmentSummary {
                assignment_id: "assign-1".to_string(),
                codex_thread_id: "codex-thread-1".to_string(),
                workstream_id: "ws-1".to_string(),
                work_unit_id: "wu-1".to_string(),
                supervisor_id: "sup-1".to_string(),
                assigned_by: "daemon".to_string(),
                assigned_at: Utc::now(),
                updated_at: Utc::now(),
                status: orcas_core::CodexThreadAssignmentStatus::Active,
                send_policy: orcas_core::CodexThreadSendPolicy::SupervisorMaySend,
                bootstrap_state: orcas_core::CodexThreadBootstrapState::Sent,
                latest_basis_turn_id: None,
                latest_decision_id: None,
                notes: None,
                active: true,
            });

        let dashboard = WorkstreamsDashboardData {
            hierarchy: authority::HierarchySnapshot::default(),
            snapshot,
            planning_sessions: Vec::new(),
        };

        let status = tracked_thread_runtime_status(&tracked_thread(), &dashboard);
        assert_eq!(status.headline, "In progress");
    }

    #[test]
    fn reports_waiting_for_supervisor_when_open_decision_exists() {
        let mut snapshot = empty_snapshot();
        snapshot
            .collaboration
            .codex_thread_assignments
            .push(CodexThreadAssignmentSummary {
                assignment_id: "assign-1".to_string(),
                codex_thread_id: "codex-thread-1".to_string(),
                workstream_id: "ws-1".to_string(),
                work_unit_id: "wu-1".to_string(),
                supervisor_id: "sup-1".to_string(),
                assigned_by: "daemon".to_string(),
                assigned_at: Utc::now(),
                updated_at: Utc::now(),
                status: orcas_core::CodexThreadAssignmentStatus::Active,
                send_policy: orcas_core::CodexThreadSendPolicy::HumanApprovalRequired,
                bootstrap_state: orcas_core::CodexThreadBootstrapState::Sent,
                latest_basis_turn_id: None,
                latest_decision_id: Some("decision-1".to_string()),
                notes: None,
                active: true,
            });
        snapshot
            .collaboration
            .supervisor_turn_decisions
            .push(SupervisorTurnDecisionSummary {
                decision_id: "decision-1".to_string(),
                assignment_id: "assign-1".to_string(),
                codex_thread_id: "codex-thread-1".to_string(),
                workstream_id: Some("ws-1".to_string()),
                work_unit_id: Some("wu-1".to_string()),
                supervisor_id: Some("sup-1".to_string()),
                basis_turn_id: None,
                kind: orcas_core::SupervisorTurnDecisionKind::NextTurn,
                proposal_kind: orcas_core::SupervisorTurnProposalKind::ContinueAfterTurn,
                proposed_text: None,
                rationale_summary: "Need review".to_string(),
                status: orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
                created_at: Utc::now(),
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: None,
                open: true,
            });
        let dashboard = WorkstreamsDashboardData {
            hierarchy: authority::HierarchySnapshot::default(),
            snapshot,
            planning_sessions: Vec::new(),
        };

        let status = tracked_thread_runtime_status(&tracked_thread(), &dashboard);
        assert_eq!(status.headline, "Waiting for supervisor");
    }

    #[test]
    fn inferred_live_thread_prefers_assignment_codex_thread_id() {
        let mut snapshot = empty_snapshot();
        snapshot.threads.push(ipc::ThreadSummary {
            id: "codex-thread-1".to_string(),
            preview: "live lane".to_string(),
            name: None,
            model_provider: "openai:gpt-5.4".to_string(),
            cwd: "/tmp/lane".to_string(),
            status: "idle".to_string(),
            created_at: 0,
            updated_at: 0,
            scope: "user".to_string(),
            archived: false,
            loaded_status: ipc::ThreadLoadedStatus::Idle,
            active_flags: Vec::new(),
            monitor_state: ipc::ThreadMonitorState::default(),
            turn_in_flight: false,
            active_turn_id: None,
            last_seen_turn_id: None,
            recent_output: None,
            recent_event: Some("turn completed".to_string()),
            last_sync_at: Utc::now(),
            source_kind: None,
            raw_summary: None,
        });
        let assignment = AssignmentSummary {
            id: "assign-1".to_string(),
            work_unit_id: "wu-1".to_string(),
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: Default::default(),
            alignment_rationale: None,
            worker_id: "worker-1".to_string(),
            worker_session_id: "session-1".to_string(),
            codex_thread_id: Some("codex-thread-1".to_string()),
            tracked_thread_id: None,
            status: orcas_core::AssignmentStatus::AwaitingDecision,
            attempt_number: 1,
            updated_at: Utc::now(),
        };
        let dashboard = WorkstreamsDashboardData {
            hierarchy: authority::HierarchySnapshot::default(),
            snapshot,
            planning_sessions: Vec::new(),
        };

        let thread = inferred_live_thread_for_assignment(&assignment, &dashboard)
            .expect("thread should resolve from assignment summary");
        assert_eq!(thread.id, "codex-thread-1");
    }
}
