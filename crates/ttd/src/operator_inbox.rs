use std::collections::BTreeMap;
use std::time::Duration;

#[cfg(test)]
use chrono::{DateTime, Utc};

use tokio::sync::watch;
use tokio::time::timeout;
use tt_core::collaboration::{
    CollaborationState, PlanningSession, PlanningSessionStatus, SupervisorTurnDecision,
    SupervisorTurnDecisionStatus,
};
use tt_core::ipc;
use tt_core::planning::{PlanRevisionProposal, PlanRevisionProposalStatus};
use tt_core::supervisor::SupervisorProposalRecord;

#[cfg(test)]
use chrono::TimeZone;
#[cfg(test)]
use tt_core::WorkUnit;
#[cfg(test)]
use tt_core::collaboration::PlanningSessionStructuredSummary;
#[cfg(test)]
use tt_core::planning::{PlanId, PlanRevisionApplyFailureKind, PlanRevisionApplyPhase};
#[cfg(test)]
use tt_core::supervisor::{
    DecisionPolicy, RecentPrimaryHistory, RelatedWorkUnitContext, SupervisorAssignmentContext,
    SupervisorContextPack, SupervisorDependencyContext, SupervisorOperatorRequest,
    SupervisorPackLimits, SupervisorPackTruncation, SupervisorProposalFailure,
    SupervisorProposalFailureStage, SupervisorProposalTrigger, SupervisorProposalTriggerKind,
    SupervisorSourceReportContext, SupervisorStateAnchor, SupervisorWorkUnitContext,
    SupervisorWorkerSessionContext, SupervisorWorkstreamContext, SupervisorWorkstreamPlanContext,
};
#[cfg(test)]
use tt_core::{
    DecisionType, ReportConfidence, ReportDisposition, ReportParseResult,
    SupervisorTurnDecisionKind, WorkUnitStatus, Workstream,
};

pub fn rebuild_operator_inbox_state(
    collaboration: &CollaborationState,
    previous: Option<&ipc::OperatorInboxState>,
) -> ipc::OperatorInboxState {
    let previous = previous.cloned().unwrap_or_default();
    let mut next_sequence = previous.checkpoint.current_sequence;
    let previous_items = previous
        .items
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect::<BTreeMap<_, _>>();
    let current_items = derive_current_operator_inbox_items(collaboration);

    let mut changes = previous.changes;
    let mut changed = false;
    let mut retained = Vec::with_capacity(current_items.len());

    for mut item in current_items {
        match previous_items.get(&item.id) {
            Some(previous_item) if operator_inbox_items_equivalent(previous_item, &item) => {
                item.sequence = previous_item.sequence;
            }
            Some(_) | None => {
                next_sequence += 1;
                item.sequence = next_sequence;
                changes.push(ipc::OperatorInboxChange {
                    sequence: next_sequence,
                    kind: ipc::OperatorInboxChangeKind::Upsert,
                    item: item.clone(),
                    changed_at: item.updated_at,
                });
                changed = true;
            }
        }
        retained.push(item);
    }

    let mut removed = previous_items
        .into_iter()
        .filter(|(item_id, _)| !retained.iter().any(|item| item.id == *item_id))
        .map(|(_, item)| item)
        .collect::<Vec<_>>();
    removed.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then_with(|| left.id.cmp(&right.id))
    });
    for item in removed {
        next_sequence += 1;
        changes.push(ipc::OperatorInboxChange {
            sequence: next_sequence,
            kind: ipc::OperatorInboxChangeKind::Removed,
            item,
            changed_at: chrono::Utc::now(),
        });
        changed = true;
    }

    retained.sort_by(|left, right| {
        right
            .sequence
            .cmp(&left.sequence)
            .then_with(|| left.id.cmp(&right.id))
    });
    changes.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then_with(|| left.item.id.cmp(&right.item.id))
    });

    ipc::OperatorInboxState {
        items: retained,
        checkpoint: ipc::OperatorInboxCheckpoint {
            current_sequence: next_sequence,
            updated_at: if changed {
                chrono::Utc::now()
            } else {
                previous.checkpoint.updated_at
            },
        },
        changes,
    }
}

fn operator_inbox_items_equivalent(
    left: &ipc::OperatorInboxItem,
    right: &ipc::OperatorInboxItem,
) -> bool {
    left.id == right.id
        && left.source_kind == right.source_kind
        && left.actionable_object_id == right.actionable_object_id
        && left.workstream_id == right.workstream_id
        && left.work_unit_id == right.work_unit_id
        && left.title == right.title
        && left.summary == right.summary
        && left.status == right.status
        && left.available_actions == right.available_actions
        && left.created_at == right.created_at
        && left.updated_at == right.updated_at
        && left.resolved_at == right.resolved_at
        && left.rationale == right.rationale
        && left.provenance == right.provenance
}

pub fn build_operator_inbox_state(collaboration: &CollaborationState) -> ipc::OperatorInboxState {
    rebuild_operator_inbox_state(collaboration, None)
}

pub fn operator_inbox_checkpoint(state: &ipc::OperatorInboxState) -> ipc::OperatorInboxCheckpoint {
    state.checkpoint.clone()
}

pub async fn wait_for_operator_inbox_checkpoint(
    mut checkpoint_rx: watch::Receiver<ipc::OperatorInboxCheckpoint>,
    after_sequence: u64,
    timeout_ms: Option<u64>,
) -> Result<ipc::OperatorInboxCheckpoint, String> {
    let wait = async {
        loop {
            let checkpoint = checkpoint_rx.borrow_and_update().clone();
            if checkpoint.current_sequence > after_sequence {
                return Ok(checkpoint);
            }
            checkpoint_rx
                .changed()
                .await
                .map_err(|_| "operator inbox checkpoint stream closed".to_string())?;
        }
    };
    match timeout_ms {
        Some(timeout_ms) => timeout(Duration::from_millis(timeout_ms), wait)
            .await
            .map_err(|_| {
                format!(
                    "timed out waiting for operator inbox checkpoint after sequence {after_sequence}"
                )
            })?,
        None => wait.await,
    }
}

pub fn operator_inbox_changes_after(
    state: &ipc::OperatorInboxState,
    after_sequence: u64,
    limit: Option<usize>,
) -> Vec<ipc::OperatorInboxChange> {
    let mut changes = state
        .changes
        .iter()
        .filter(|change| change.sequence > after_sequence)
        .cloned()
        .collect::<Vec<_>>();
    changes.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then_with(|| left.item.id.cmp(&right.item.id))
    });
    if let Some(limit) = limit {
        changes.truncate(limit);
    }
    changes
}

pub fn operator_inbox_replay_items(state: &ipc::OperatorInboxState) -> Vec<ipc::OperatorInboxItem> {
    let mut items = state.items.clone();
    items.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then_with(|| left.id.cmp(&right.id))
    });
    items
}

pub fn operator_inbox_mirror_checkpoint_for_peer(
    mirrors: &BTreeMap<String, ipc::OperatorInboxMirrorCheckpoint>,
    peer_id: &str,
) -> ipc::OperatorInboxMirrorCheckpoint {
    mirrors
        .get(peer_id)
        .cloned()
        .unwrap_or_else(|| ipc::OperatorInboxMirrorCheckpoint {
            peer_id: peer_id.to_string(),
            ..Default::default()
        })
}

pub fn update_operator_inbox_export_checkpoint(
    mirrors: &mut BTreeMap<String, ipc::OperatorInboxMirrorCheckpoint>,
    peer_id: &str,
    checkpoint: &ipc::OperatorInboxCheckpoint,
    after_sequence: u64,
    changes: &[ipc::OperatorInboxChange],
) -> Result<ipc::OperatorInboxMirrorCheckpoint, String> {
    let mut mirror = operator_inbox_mirror_checkpoint_for_peer(mirrors, peer_id);
    let exported_sequence = changes
        .last()
        .map(|change| change.sequence)
        .unwrap_or_else(|| after_sequence.min(checkpoint.current_sequence));
    mirror.peer_id = peer_id.to_string();
    mirror.last_exported_sequence = mirror.last_exported_sequence.max(exported_sequence);
    mirror.updated_at = chrono::Utc::now();
    mirrors.insert(peer_id.to_string(), mirror.clone());
    Ok(mirror)
}

pub fn update_operator_inbox_ack_checkpoint(
    mirrors: &mut BTreeMap<String, ipc::OperatorInboxMirrorCheckpoint>,
    peer_id: &str,
    through_sequence: u64,
) -> Result<ipc::OperatorInboxMirrorCheckpoint, String> {
    let mut mirror = operator_inbox_mirror_checkpoint_for_peer(mirrors, peer_id);
    if through_sequence < mirror.last_acked_sequence {
        return Err(format!(
            "inbox ack for peer `{peer_id}` cannot move backward from {} to {through_sequence}",
            mirror.last_acked_sequence
        ));
    }
    if through_sequence > mirror.last_exported_sequence {
        return Err(format!(
            "inbox ack for peer `{peer_id}` cannot exceed exported sequence {}",
            mirror.last_exported_sequence
        ));
    }
    mirror.peer_id = peer_id.to_string();
    mirror.last_acked_sequence = through_sequence;
    mirror.updated_at = chrono::Utc::now();
    mirrors.insert(peer_id.to_string(), mirror.clone());
    Ok(mirror)
}

pub fn resolve_operator_inbox_action_route(
    item: &ipc::OperatorInboxItem,
    action_kind: ipc::OperatorInboxActionKind,
) -> Result<ipc::OperatorInboxActionRoute, String> {
    if !item.available_actions.contains(&action_kind) {
        return Err(format!(
            "action `{:?}` is not available for inbox item `{}`",
            action_kind, item.id
        ));
    }

    match (item.source_kind, action_kind) {
        (
            ipc::OperatorInboxSourceKind::SupervisorProposal,
            ipc::OperatorInboxActionKind::Approve,
        ) => Ok(ipc::OperatorInboxActionRoute::Proposal {
            item_id: item.id.clone(),
            proposal_id: item.actionable_object_id.clone(),
            method: ipc::methods::PROPOSAL_APPROVE.to_string(),
        }),
        (
            ipc::OperatorInboxSourceKind::SupervisorProposal,
            ipc::OperatorInboxActionKind::Reject,
        ) => Ok(ipc::OperatorInboxActionRoute::Proposal {
            item_id: item.id.clone(),
            proposal_id: item.actionable_object_id.clone(),
            method: ipc::methods::PROPOSAL_REJECT.to_string(),
        }),
        (
            ipc::OperatorInboxSourceKind::SupervisorDecision,
            ipc::OperatorInboxActionKind::ApproveAndSend,
        ) => Ok(ipc::OperatorInboxActionRoute::SupervisorDecision {
            item_id: item.id.clone(),
            decision_id: item.actionable_object_id.clone(),
            method: ipc::methods::SUPERVISOR_DECISION_APPROVE_AND_SEND.to_string(),
        }),
        (
            ipc::OperatorInboxSourceKind::SupervisorDecision,
            ipc::OperatorInboxActionKind::Reject,
        ) => Ok(ipc::OperatorInboxActionRoute::SupervisorDecision {
            item_id: item.id.clone(),
            decision_id: item.actionable_object_id.clone(),
            method: ipc::methods::SUPERVISOR_DECISION_REJECT.to_string(),
        }),
        (
            ipc::OperatorInboxSourceKind::SupervisorDecision,
            ipc::OperatorInboxActionKind::RecordNoAction,
        ) => Ok(ipc::OperatorInboxActionRoute::SupervisorDecision {
            item_id: item.id.clone(),
            decision_id: item.actionable_object_id.clone(),
            method: ipc::methods::SUPERVISOR_DECISION_RECORD_NO_ACTION.to_string(),
        }),
        (
            ipc::OperatorInboxSourceKind::SupervisorDecision,
            ipc::OperatorInboxActionKind::ManualRefresh,
        ) => Ok(ipc::OperatorInboxActionRoute::SupervisorDecision {
            item_id: item.id.clone(),
            decision_id: item.actionable_object_id.clone(),
            method: ipc::methods::SUPERVISOR_DECISION_MANUAL_REFRESH.to_string(),
        }),
        (ipc::OperatorInboxSourceKind::PlanningSession, ipc::OperatorInboxActionKind::Approve) => {
            Ok(ipc::OperatorInboxActionRoute::PlanningSession {
                item_id: item.id.clone(),
                session_id: item.actionable_object_id.clone(),
                method: ipc::methods::PLANNING_SESSION_APPROVE.to_string(),
            })
        }
        (ipc::OperatorInboxSourceKind::PlanningSession, ipc::OperatorInboxActionKind::Reject) => {
            Ok(ipc::OperatorInboxActionRoute::PlanningSession {
                item_id: item.id.clone(),
                session_id: item.actionable_object_id.clone(),
                method: ipc::methods::PLANNING_SESSION_REJECT.to_string(),
            })
        }
        (
            ipc::OperatorInboxSourceKind::PlanningSession,
            ipc::OperatorInboxActionKind::Supersede,
        ) => Ok(ipc::OperatorInboxActionRoute::PlanningSession {
            item_id: item.id.clone(),
            session_id: item.actionable_object_id.clone(),
            method: ipc::methods::PLANNING_SESSION_SUPERSEDE.to_string(),
        }),
        (
            ipc::OperatorInboxSourceKind::PlanRevisionProposal,
            ipc::OperatorInboxActionKind::Approve,
        ) => Ok(ipc::OperatorInboxActionRoute::PlanRevisionProposal {
            item_id: item.id.clone(),
            proposal_id: item.actionable_object_id.clone(),
            method: ipc::methods::PROPOSAL_APPROVE.to_string(),
        }),
        (
            ipc::OperatorInboxSourceKind::PlanRevisionProposal,
            ipc::OperatorInboxActionKind::Reject,
        ) => Ok(ipc::OperatorInboxActionRoute::PlanRevisionProposal {
            item_id: item.id.clone(),
            proposal_id: item.actionable_object_id.clone(),
            method: ipc::methods::PROPOSAL_REJECT.to_string(),
        }),
        (
            ipc::OperatorInboxSourceKind::PlanRevisionProposal,
            ipc::OperatorInboxActionKind::Reconcile,
        ) => Ok(ipc::OperatorInboxActionRoute::PlanRevisionProposal {
            item_id: item.id.clone(),
            proposal_id: item.actionable_object_id.clone(),
            method: ipc::methods::PROPOSAL_RECONCILE.to_string(),
        }),
        (
            ipc::OperatorInboxSourceKind::PlanRevisionProposal,
            ipc::OperatorInboxActionKind::Retry,
        ) => Ok(ipc::OperatorInboxActionRoute::PlanRevisionProposal {
            item_id: item.id.clone(),
            proposal_id: item.actionable_object_id.clone(),
            method: ipc::methods::PROPOSAL_APPROVE.to_string(),
        }),
        (
            ipc::OperatorInboxSourceKind::PlanningSession,
            ipc::OperatorInboxActionKind::MarkReadyForReview,
        ) => Ok(ipc::OperatorInboxActionRoute::PlanningSession {
            item_id: item.id.clone(),
            session_id: item.actionable_object_id.clone(),
            method: ipc::methods::PLANNING_SESSION_MARK_READY_FOR_REVIEW.to_string(),
        }),
        _ => Err(format!(
            "action `{:?}` is not routed for inbox item `{}`",
            action_kind, item.id
        )),
    }
}

fn derive_current_operator_inbox_items(
    collaboration: &CollaborationState,
) -> Vec<ipc::OperatorInboxItem> {
    let mut items = collaboration
        .supervisor_proposals
        .values()
        .filter_map(|proposal| supervisor_proposal_inbox_item(proposal))
        .chain(
            collaboration
                .supervisor_turn_decisions
                .values()
                .filter_map(|decision| supervisor_decision_inbox_item(collaboration, decision)),
        )
        .chain(
            collaboration
                .planning_sessions
                .values()
                .filter_map(planning_session_inbox_item),
        )
        .chain(
            collaboration
                .planning
                .revision_proposals
                .values()
                .filter_map(|proposal| plan_revision_inbox_item(collaboration, proposal)),
        )
        .collect::<Vec<_>>();

    items.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    items
}

pub fn list_operator_inbox_items(
    state: &ipc::OperatorInboxState,
    request: &ipc::OperatorInboxListRequest,
) -> Vec<ipc::OperatorInboxItem> {
    let mut items = state.items.clone();
    items.retain(|item| {
        request
            .workstream_id
            .as_ref()
            .map(|workstream_id| item.workstream_id.as_deref() == Some(workstream_id.as_str()))
            .unwrap_or(true)
            && request
                .work_unit_id
                .as_ref()
                .map(|work_unit_id| item.work_unit_id.as_deref() == Some(work_unit_id.as_str()))
                .unwrap_or(true)
            && request
                .source_kind
                .map(|source_kind| item.source_kind == source_kind)
                .unwrap_or(true)
            && request
                .status
                .map(|status| item.status == status)
                .unwrap_or(true)
            && (request.status.is_some()
                || request.include_closed
                || item.status == ipc::OperatorInboxItemStatus::Open)
            && (!request.actionable_only || operator_inbox_item_is_actionable(item))
    });
    items.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(limit) = request.limit {
        items.truncate(limit);
    }
    items
}

pub fn get_operator_inbox_item(
    state: &ipc::OperatorInboxState,
    item_id: &str,
) -> Option<ipc::OperatorInboxItem> {
    state.items.iter().find(|item| item.id == item_id).cloned()
}

pub fn operator_inbox_item_is_actionable(item: &ipc::OperatorInboxItem) -> bool {
    item.status == ipc::OperatorInboxItemStatus::Open && !item.available_actions.is_empty()
}

fn supervisor_proposal_inbox_item(
    proposal: &SupervisorProposalRecord,
) -> Option<ipc::OperatorInboxItem> {
    if proposal.status == tt_core::SupervisorProposalStatus::GenerationFailed {
        return None;
    }

    let (status, available_actions, resolved_at, title, summary) = match proposal.status {
        tt_core::SupervisorProposalStatus::Open => (
            ipc::OperatorInboxItemStatus::Open,
            vec![
                ipc::OperatorInboxActionKind::Approve,
                ipc::OperatorInboxActionKind::Reject,
            ],
            None,
            "Supervisor proposal awaiting review".to_string(),
            format!(
                "Proposal `{}` for work unit `{}` is awaiting review.",
                proposal.id, proposal.primary_work_unit_id
            ),
        ),
        tt_core::SupervisorProposalStatus::Approved => (
            ipc::OperatorInboxItemStatus::Resolved,
            Vec::new(),
            proposal.reviewed_at,
            "Supervisor proposal approved".to_string(),
            format!("Proposal `{}` was approved.", proposal.id),
        ),
        tt_core::SupervisorProposalStatus::Rejected => (
            ipc::OperatorInboxItemStatus::Resolved,
            Vec::new(),
            proposal.reviewed_at,
            "Supervisor proposal rejected".to_string(),
            format!("Proposal `{}` was rejected.", proposal.id),
        ),
        tt_core::SupervisorProposalStatus::Superseded => (
            ipc::OperatorInboxItemStatus::Superseded,
            Vec::new(),
            proposal.reviewed_at,
            "Supervisor proposal superseded".to_string(),
            format!(
                "Proposal `{}` was superseded by a newer proposal.",
                proposal.id
            ),
        ),
        tt_core::SupervisorProposalStatus::Stale => (
            ipc::OperatorInboxItemStatus::Stale,
            Vec::new(),
            proposal.reviewed_at,
            "Supervisor proposal stale".to_string(),
            format!("Proposal `{}` is stale.", proposal.id),
        ),
        tt_core::SupervisorProposalStatus::GenerationFailed => unreachable!(),
    };

    Some(ipc::OperatorInboxItem {
        id: format!("supervisor_proposal::{}", proposal.id),
        sequence: 0,
        source_kind: ipc::OperatorInboxSourceKind::SupervisorProposal,
        actionable_object_id: proposal.id.clone(),
        workstream_id: Some(proposal.workstream_id.clone()),
        work_unit_id: Some(proposal.primary_work_unit_id.clone()),
        title,
        summary,
        status,
        available_actions,
        created_at: proposal.created_at,
        updated_at: proposal.reviewed_at.unwrap_or(proposal.created_at),
        resolved_at,
        rationale: proposal.review_note.clone(),
        provenance: Some(format!(
            "trigger={:?}; source_report_id={}",
            proposal.trigger.kind, proposal.source_report_id
        )),
    })
}

fn supervisor_decision_inbox_item(
    collaboration: &CollaborationState,
    decision: &SupervisorTurnDecision,
) -> Option<ipc::OperatorInboxItem> {
    if decision.status == SupervisorTurnDecisionStatus::Draft {
        return None;
    }
    let assignment = collaboration
        .tt_thread_assignments
        .get(&decision.assignment_id);
    let workstream_id = assignment.map(|assignment| assignment.workstream_id.clone());
    let work_unit_id = assignment.map(|assignment| assignment.work_unit_id.clone());

    let (status, available_actions, resolved_at, title, summary) = match decision.status {
        SupervisorTurnDecisionStatus::ProposedToHuman => (
            ipc::OperatorInboxItemStatus::Open,
            vec![
                ipc::OperatorInboxActionKind::ApproveAndSend,
                ipc::OperatorInboxActionKind::Reject,
                ipc::OperatorInboxActionKind::RecordNoAction,
                ipc::OperatorInboxActionKind::ManualRefresh,
            ],
            None,
            "Supervisor decision awaiting review".to_string(),
            format!(
                "Decision `{}` for assignment `{}` is awaiting human review.",
                decision.decision_id, decision.assignment_id
            ),
        ),
        SupervisorTurnDecisionStatus::Approved
        | SupervisorTurnDecisionStatus::Recorded
        | SupervisorTurnDecisionStatus::Sent
        | SupervisorTurnDecisionStatus::Rejected => (
            ipc::OperatorInboxItemStatus::Resolved,
            Vec::new(),
            decision
                .approved_at
                .or(decision.rejected_at)
                .or(decision.sent_at),
            format!("Supervisor decision {:?}", decision.status).replace('"', ""),
            format!(
                "Decision `{}` for assignment `{}` was {:?}.",
                decision.decision_id, decision.assignment_id, decision.status
            ),
        ),
        SupervisorTurnDecisionStatus::Superseded => (
            ipc::OperatorInboxItemStatus::Superseded,
            Vec::new(),
            decision
                .sent_at
                .or(decision.approved_at)
                .or(decision.rejected_at),
            "Supervisor decision superseded".to_string(),
            format!(
                "Decision `{}` was superseded by `{}`.",
                decision.decision_id,
                decision.superseded_by.as_deref().unwrap_or("unknown")
            ),
        ),
        SupervisorTurnDecisionStatus::Stale => (
            ipc::OperatorInboxItemStatus::Stale,
            Vec::new(),
            decision
                .sent_at
                .or(decision.approved_at)
                .or(decision.rejected_at),
            "Supervisor decision stale".to_string(),
            format!("Decision `{}` is stale.", decision.decision_id),
        ),
        SupervisorTurnDecisionStatus::Draft => unreachable!(),
    };

    Some(ipc::OperatorInboxItem {
        id: format!("supervisor_decision::{}", decision.decision_id),
        sequence: 0,
        source_kind: ipc::OperatorInboxSourceKind::SupervisorDecision,
        actionable_object_id: decision.decision_id.clone(),
        workstream_id,
        work_unit_id,
        title,
        summary,
        status,
        available_actions,
        created_at: decision.created_at,
        updated_at: decision
            .approved_at
            .or(decision.rejected_at)
            .or(decision.sent_at)
            .unwrap_or(decision.created_at),
        resolved_at,
        rationale: Some(decision.rationale_summary.clone()),
        provenance: Some(format!(
            "proposal_kind={:?}; kind={:?}; basis_turn_id={}",
            decision.proposal_kind,
            decision.kind,
            decision.basis_turn_id.as_deref().unwrap_or("none")
        )),
    })
}

fn planning_session_inbox_item(session: &PlanningSession) -> Option<ipc::OperatorInboxItem> {
    let actionable = session.latest_structured_summary.ready_for_review
        || matches!(session.status, PlanningSessionStatus::AwaitingApproval);
    let terminal = matches!(
        session.status,
        PlanningSessionStatus::Approved
            | PlanningSessionStatus::Rejected
            | PlanningSessionStatus::Superseded
            | PlanningSessionStatus::Aborted
    );
    if !actionable && !terminal {
        return None;
    }

    let (status, available_actions, resolved_at, title, summary) = match session.status {
        PlanningSessionStatus::AwaitingApproval
        | PlanningSessionStatus::Draft
        | PlanningSessionStatus::Chatting
        | PlanningSessionStatus::ResearchRequested => (
            ipc::OperatorInboxItemStatus::Open,
            vec![
                ipc::OperatorInboxActionKind::Approve,
                ipc::OperatorInboxActionKind::Reject,
                ipc::OperatorInboxActionKind::Supersede,
            ],
            None,
            "Planning session ready for review".to_string(),
            format!(
                "Planning session `{}` for workstream `{}` is ready for approval.",
                session.session_id, session.workstream_id
            ),
        ),
        PlanningSessionStatus::Approved
        | PlanningSessionStatus::Rejected
        | PlanningSessionStatus::Aborted => (
            ipc::OperatorInboxItemStatus::Resolved,
            Vec::new(),
            session.reviewed_at.or(Some(session.updated_at)),
            format!("Planning session {:?}", session.status).replace('"', ""),
            format!(
                "Planning session `{}` for workstream `{}` was {:?}.",
                session.session_id, session.workstream_id, session.status
            ),
        ),
        PlanningSessionStatus::Superseded => (
            ipc::OperatorInboxItemStatus::Superseded,
            Vec::new(),
            session.reviewed_at.or(Some(session.updated_at)),
            "Planning session superseded".to_string(),
            format!(
                "Planning session `{}` was superseded by `{}`.",
                session.session_id,
                session
                    .superseded_by_session_id
                    .as_deref()
                    .unwrap_or("unknown")
            ),
        ),
    };

    Some(ipc::OperatorInboxItem {
        id: format!("planning_session::{}", session.session_id),
        sequence: 0,
        source_kind: ipc::OperatorInboxSourceKind::PlanningSession,
        actionable_object_id: session.session_id.clone(),
        workstream_id: Some(session.workstream_id.clone()),
        work_unit_id: None,
        title,
        summary,
        status,
        available_actions,
        created_at: session.created_at,
        updated_at: session.updated_at,
        resolved_at,
        rationale: session.review_note.clone(),
        provenance: Some(format!(
            "planning_thread_id={}; base_plan_id={}",
            session.planning_thread_id,
            session
                .base_plan_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "none".to_string())
        )),
    })
}

fn plan_revision_inbox_item(
    collaboration: &CollaborationState,
    proposal: &PlanRevisionProposal,
) -> Option<ipc::OperatorInboxItem> {
    let source_proposal = proposal
        .source_supervisor_proposal_id
        .as_ref()
        .and_then(|proposal_id| collaboration.supervisor_proposals.get(proposal_id));
    let workstream_id = Some(proposal.workstream_id.clone());
    let work_unit_id = source_proposal.map(|source| source.primary_work_unit_id.clone());
    let status_and_actions = match proposal.status {
        PlanRevisionProposalStatus::Pending => Some((
            ipc::OperatorInboxItemStatus::Open,
            vec![
                ipc::OperatorInboxActionKind::Approve,
                ipc::OperatorInboxActionKind::Reject,
            ],
            None,
            "Plan revision awaiting review".to_string(),
            format!(
                "Plan revision `{}` for workstream `{}` is pending review.",
                proposal.proposal_id, proposal.workstream_id
            ),
        )),
        PlanRevisionProposalStatus::Applying => Some((
            ipc::OperatorInboxItemStatus::Resolved,
            Vec::new(),
            proposal.apply_finished_at.or(proposal.reviewed_at),
            "Plan revision applying".to_string(),
            format!(
                "Plan revision `{}` is currently applying.",
                proposal.proposal_id
            ),
        )),
        PlanRevisionProposalStatus::ApplyFailed => {
            let mut available_actions = Vec::new();
            if proposal.recovery.can_reconcile()
                || proposal.recovery.reconcile_available
                || proposal.recovery.operator_intervention_required
            {
                available_actions.push(ipc::OperatorInboxActionKind::Reconcile);
            }
            if proposal.recovery.can_retry() {
                available_actions.push(ipc::OperatorInboxActionKind::Retry);
            }
            if available_actions.is_empty() {
                None
            } else {
                Some((
                    ipc::OperatorInboxItemStatus::Open,
                    available_actions,
                    None,
                    "Plan revision requires recovery".to_string(),
                    format!(
                        "Plan revision `{}` failed and needs operator follow-up.",
                        proposal.proposal_id
                    ),
                ))
            }
        }
        PlanRevisionProposalStatus::Approved | PlanRevisionProposalStatus::Applied => Some((
            ipc::OperatorInboxItemStatus::Resolved,
            Vec::new(),
            proposal.apply_finished_at.or(proposal.reviewed_at),
            "Plan revision applied".to_string(),
            format!(
                "Plan revision `{}` was applied to the active plan.",
                proposal.proposal_id
            ),
        )),
        PlanRevisionProposalStatus::Rejected => Some((
            ipc::OperatorInboxItemStatus::Resolved,
            Vec::new(),
            proposal.apply_finished_at.or(proposal.reviewed_at),
            "Plan revision rejected".to_string(),
            format!("Plan revision `{}` was rejected.", proposal.proposal_id),
        )),
        PlanRevisionProposalStatus::Superseded => Some((
            ipc::OperatorInboxItemStatus::Superseded,
            Vec::new(),
            proposal.apply_finished_at.or(proposal.reviewed_at),
            "Plan revision superseded".to_string(),
            format!("Plan revision `{}` was superseded.", proposal.proposal_id),
        )),
    }?;

    let (status, available_actions, resolved_at, title, summary) = status_and_actions;
    let mut provenance_bits = vec![format!("base_plan_id={}", proposal.base_plan_id)];
    if let Some(source_id) = proposal.source_supervisor_proposal_id.as_ref() {
        provenance_bits.push(format!("source_supervisor_proposal_id={source_id}"));
    }
    if let Some(failure_kind) = proposal.recovery.failure_kind {
        provenance_bits.push(format!("failure_kind={failure_kind:?}"));
    }

    Some(ipc::OperatorInboxItem {
        id: format!("plan_revision_proposal::{}", proposal.proposal_id),
        sequence: 0,
        source_kind: ipc::OperatorInboxSourceKind::PlanRevisionProposal,
        actionable_object_id: proposal.proposal_id.to_string(),
        workstream_id,
        work_unit_id,
        title,
        summary,
        status,
        available_actions,
        created_at: proposal.created_at,
        updated_at: proposal
            .apply_finished_at
            .or(proposal.reviewed_at)
            .unwrap_or(proposal.created_at),
        resolved_at,
        rationale: proposal
            .review_note
            .clone()
            .or_else(|| proposal.apply_error.clone()),
        provenance: Some(provenance_bits.join("; ")),
    })
}

#[cfg(test)]
fn sample_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 5, 6, 7, 8, 9)
        .single()
        .expect("valid timestamp")
}

#[cfg(test)]
fn sample_workstream(workstream_id: &str) -> Workstream {
    let now = sample_now();
    Workstream {
        id: workstream_id.to_string(),
        title: "Sample workstream".to_string(),
        objective: "Exercise inbox derivation".to_string(),
        status: tt_core::collaboration::WorkstreamStatus::Active,
        priority: "normal".to_string(),
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
fn sample_work_unit(work_unit_id: &str, workstream_id: &str) -> WorkUnit {
    let now = sample_now();
    WorkUnit {
        id: work_unit_id.to_string(),
        workstream_id: workstream_id.to_string(),
        title: "Sample work unit".to_string(),
        task_statement: "Exercise inbox derivation.".to_string(),
        status: WorkUnitStatus::AwaitingDecision,
        dependencies: Vec::new(),
        latest_report_id: None,
        current_assignment_id: None,
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
fn sample_context_pack(workstream_id: &str, work_unit_id: &str) -> SupervisorContextPack {
    let now = sample_now();
    SupervisorContextPack {
        schema_version: "supervisor_context_pack.v2".to_string(),
        generated_at: now,
        trigger: SupervisorProposalTrigger {
            kind: SupervisorProposalTriggerKind::ReportRecorded,
            requested_at: now,
            requested_by: "daemon".to_string(),
            source_report_id: "report-1".to_string(),
            note: None,
        },
        pack_limits: SupervisorPackLimits {
            max_related_work_units: 4,
            max_prior_reports: 3,
            max_prior_decisions: 3,
            max_artifacts: 2,
            max_raw_report_chars: 2_048,
        },
        truncation: SupervisorPackTruncation::default(),
        state_anchor: SupervisorStateAnchor {
            workstream_id: workstream_id.to_string(),
            primary_work_unit_id: work_unit_id.to_string(),
            source_report_id: "report-1".to_string(),
            source_report_created_at: now,
            current_assignment_id: Some("assignment-1".to_string()),
            primary_work_unit_updated_at: now,
            latest_decision_id: None,
            latest_decision_created_at: None,
        },
        decision_policy: DecisionPolicy {
            supported_decisions: vec![DecisionType::Continue, DecisionType::EscalateToHuman],
            allowed_decisions: vec![DecisionType::Continue, DecisionType::EscalateToHuman],
            disallowed_decisions: Vec::new(),
            disallowed_reasons_by_decision: Default::default(),
            assignment_required_for: vec![DecisionType::Continue],
            assignment_forbidden_for: vec![DecisionType::EscalateToHuman],
            human_review_required: true,
        },
        workstream: SupervisorWorkstreamContext {
            id: workstream_id.to_string(),
            title: "Sample workstream".to_string(),
            objective: "Exercise inbox derivation".to_string(),
            status: "active".to_string(),
            priority: "normal".to_string(),
            success_criteria: vec!["Reviewable".to_string()],
            constraints: Vec::new(),
            summary: Some("Sample summary".to_string()),
            open_work_unit_count: 1,
            blocked_work_unit_count: 0,
            completed_work_unit_count: 0,
        },
        workstream_plan: Some(SupervisorWorkstreamPlanContext {
            active_plan: tt_core::planning::WorkstreamPlan {
                plan_id: PlanId::parse("plan-1").expect("plan id"),
                workstream_id: workstream_id.to_string(),
                version: 1,
                status: tt_core::planning::PlanStatus::Active,
                title: "Sample plan".to_string(),
                overview: Some("Sample overview".to_string()),
                goals: Vec::new(),
                plan_items: Vec::new(),
                success_criteria: Vec::new(),
                constraints: Vec::new(),
                exploration_policy: Default::default(),
                current_focus_item_id: None,
                created_at: now,
                updated_at: now,
                created_by: "tester".to_string(),
                updated_by: "tester".to_string(),
                superseded_by_plan_id: None,
                source_revision_proposal_id: None,
            },
            recent_assessments: Vec::new(),
            pending_revision_proposals: Vec::new(),
        }),
        primary_work_unit: SupervisorWorkUnitContext {
            id: work_unit_id.to_string(),
            title: "Sample work unit".to_string(),
            task_statement: "Exercise inbox derivation.".to_string(),
            status: "awaiting_decision".to_string(),
            dependencies: Vec::new(),
            current_assignment_id: Some("assignment-1".to_string()),
            latest_report_id: Some("report-1".to_string()),
            acceptance_criteria: Vec::new(),
            stop_conditions: Vec::new(),
            result_summary: None,
        },
        source_report: SupervisorSourceReportContext {
            id: "report-1".to_string(),
            assignment_id: "assignment-1".to_string(),
            worker_id: "worker-1".to_string(),
            worker_session_id: Some("worker-session-1".to_string()),
            submitted_at: now,
            disposition: ReportDisposition::Completed,
            summary: "Sample report".to_string(),
            findings: Vec::new(),
            blockers: Vec::new(),
            questions: Vec::new(),
            recommended_next_actions: Vec::new(),
            confidence: ReportConfidence::High,
            parse_result: ReportParseResult::Parsed,
            needs_supervisor_review: true,
            raw_output_excerpt: "excerpt".to_string(),
        },
        current_assignment: SupervisorAssignmentContext {
            id: "assignment-1".to_string(),
            status: "active".to_string(),
            attempt_number: 1,
            plan_id: Some("plan-1".to_string()),
            plan_version: Some(1),
            plan_item_id: None,
            execution_kind: tt_core::planning::PlanExecutionKind::DirectExecution,
            alignment_rationale: Some("sample".to_string()),
            worker_id: "worker-1".to_string(),
            worker_session_id: "worker-session-1".to_string(),
            instructions: "Do the thing".to_string(),
            created_at: now,
            updated_at: now,
        },
        worker_session: SupervisorWorkerSessionContext {
            id: "worker-session-1".to_string(),
            worker_id: "worker-1".to_string(),
            backend_type: "local".to_string(),
            thread_id: Some("thread-1".to_string()),
            active_turn_id: None,
            runtime_status: "active".to_string(),
            attachability: "attachable".to_string(),
            updated_at: now,
        },
        dependency_context: SupervisorDependencyContext::default(),
        related_work_units: Vec::<RelatedWorkUnitContext>::new(),
        recent_primary_history: RecentPrimaryHistory::default(),
        relevant_artifacts: Vec::new(),
        operator_request: Some(SupervisorOperatorRequest {
            summary: "sample operator request".to_string(),
            focus: None,
            constraints: Vec::new(),
        }),
    }
}

#[cfg(test)]
fn sample_supervisor_proposal_record(
    status: tt_core::SupervisorProposalStatus,
) -> SupervisorProposalRecord {
    let now = sample_now();
    SupervisorProposalRecord {
        id: "proposal-1".to_string(),
        workstream_id: "ws-1".to_string(),
        primary_work_unit_id: "wu-1".to_string(),
        source_report_id: "report-1".to_string(),
        trigger: SupervisorProposalTrigger {
            kind: SupervisorProposalTriggerKind::ReportRecorded,
            requested_at: now,
            requested_by: "daemon".to_string(),
            source_report_id: "report-1".to_string(),
            note: None,
        },
        status,
        created_at: now,
        reasoner_backend: "responses".to_string(),
        reasoner_model: "gpt-5.4".to_string(),
        reasoner_response_id: Some("response-1".to_string()),
        reasoner_usage: None,
        reasoner_output_text: None,
        context_pack: sample_context_pack("ws-1", "wu-1"),
        prompt_render: None,
        response_artifact: None,
        proposal: None,
        approval_edits: None,
        approved_proposal: None,
        generation_failure: if status == tt_core::SupervisorProposalStatus::GenerationFailed {
            Some(SupervisorProposalFailure {
                stage: SupervisorProposalFailureStage::Backend,
                message: "backend failed".to_string(),
            })
        } else {
            None
        },
        validated_at: None,
        reviewed_at: match status {
            tt_core::SupervisorProposalStatus::Open
            | tt_core::SupervisorProposalStatus::GenerationFailed => None,
            _ => Some(now),
        },
        reviewed_by: match status {
            tt_core::SupervisorProposalStatus::Open
            | tt_core::SupervisorProposalStatus::GenerationFailed => None,
            _ => Some("reviewer".to_string()),
        },
        review_note: match status {
            tt_core::SupervisorProposalStatus::Open
            | tt_core::SupervisorProposalStatus::GenerationFailed => None,
            _ => Some("reviewed".to_string()),
        },
        approved_decision_id: None,
        approved_assignment_id: None,
    }
}

#[cfg(test)]
fn sample_decision_record(status: SupervisorTurnDecisionStatus) -> SupervisorTurnDecision {
    let now = sample_now();
    SupervisorTurnDecision {
        decision_id: "decision-1".to_string(),
        assignment_id: "assignment-1".to_string(),
        tt_thread_id: "thread-1".to_string(),
        basis_turn_id: Some("turn-1".to_string()),
        kind: SupervisorTurnDecisionKind::NextTurn,
        proposal_kind: tt_core::collaboration::SupervisorTurnProposalKind::ManualRefresh,
        proposed_text: Some("continue please".to_string()),
        rationale_summary: "human review needed".to_string(),
        status,
        created_at: now,
        approved_at: matches!(
            status,
            SupervisorTurnDecisionStatus::Approved
                | SupervisorTurnDecisionStatus::Recorded
                | SupervisorTurnDecisionStatus::Sent
        )
        .then_some(now),
        rejected_at: matches!(status, SupervisorTurnDecisionStatus::Rejected).then_some(now),
        sent_at: matches!(status, SupervisorTurnDecisionStatus::Sent).then_some(now),
        superseded_by: matches!(status, SupervisorTurnDecisionStatus::Superseded)
            .then_some("decision-2".to_string()),
        sent_turn_id: matches!(status, SupervisorTurnDecisionStatus::Sent)
            .then_some("turn-2".to_string()),
        notes: Some("note".to_string()),
    }
}

#[cfg(test)]
fn sample_planning_session(
    status: PlanningSessionStatus,
    ready_for_review: bool,
) -> PlanningSession {
    let now = sample_now();
    PlanningSession {
        session_id: "session-1".to_string(),
        workstream_id: "ws-1".to_string(),
        status,
        planning_thread_id: "thread-1".to_string(),
        base_plan_id: Some(PlanId::parse("plan-1").expect("plan id")),
        base_plan_version: Some(1),
        research_assignment_id: None,
        research_report_id: None,
        draft_revision_proposal_id: None,
        approved_plan_id: None,
        approved_plan_version: None,
        latest_structured_summary: PlanningSessionStructuredSummary {
            objective: "Confirm the plan".to_string(),
            requirements: vec!["be reviewable".to_string()],
            constraints: Vec::new(),
            non_goals: Vec::new(),
            open_questions: Vec::new(),
            research_status: tt_core::collaboration::PlanningSessionResearchStatus::NotRequested,
            draft_plan_summary: Some("draft summary".to_string()),
            ready_for_review,
        },
        created_at: now,
        created_by: "tester".to_string(),
        updated_at: now,
        updated_by: "tester".to_string(),
        request_note: Some("review this".to_string()),
        reviewed_at: matches!(
            status,
            PlanningSessionStatus::Approved
                | PlanningSessionStatus::Rejected
                | PlanningSessionStatus::Superseded
                | PlanningSessionStatus::Aborted
        )
        .then_some(now),
        reviewed_by: matches!(
            status,
            PlanningSessionStatus::Approved
                | PlanningSessionStatus::Rejected
                | PlanningSessionStatus::Superseded
                | PlanningSessionStatus::Aborted
        )
        .then_some("reviewer".to_string()),
        review_note: Some("session note".to_string()),
        superseded_by_session_id: matches!(status, PlanningSessionStatus::Superseded)
            .then_some("session-2".to_string()),
    }
}

#[cfg(test)]
fn sample_plan_revision_proposal(
    status: PlanRevisionProposalStatus,
    recovery: tt_core::planning::PlanRevisionRecoveryState,
) -> PlanRevisionProposal {
    let now = sample_now();
    PlanRevisionProposal {
        proposal_id: tt_core::planning::PlanRevisionProposalId::parse("revision-1")
            .expect("proposal id"),
        workstream_id: "ws-1".to_string(),
        base_plan_id: PlanId::parse("plan-1").expect("plan id"),
        base_plan_version: 1,
        rationale: "revise the plan".to_string(),
        urgency: "high".to_string(),
        expected_benefit: "exercise inbox".to_string(),
        tradeoffs: vec!["more work".to_string()],
        ops: vec![tt_core::planning::PlanRevisionOp::UpdateSuccessCriteria {
            success_criteria: vec!["done".to_string()],
        }],
        status,
        created_at: now,
        created_by: "tester".to_string(),
        reviewed_at: matches!(
            status,
            PlanRevisionProposalStatus::Approved
                | PlanRevisionProposalStatus::Applied
                | PlanRevisionProposalStatus::Rejected
                | PlanRevisionProposalStatus::Superseded
                | PlanRevisionProposalStatus::ApplyFailed
                | PlanRevisionProposalStatus::Applying
        )
        .then_some(now),
        reviewed_by: matches!(
            status,
            PlanRevisionProposalStatus::Approved
                | PlanRevisionProposalStatus::Applied
                | PlanRevisionProposalStatus::Rejected
                | PlanRevisionProposalStatus::Superseded
                | PlanRevisionProposalStatus::ApplyFailed
                | PlanRevisionProposalStatus::Applying
        )
        .then_some("reviewer".to_string()),
        review_note: Some("revision note".to_string()),
        apply_started_at: matches!(
            status,
            PlanRevisionProposalStatus::Applying
                | PlanRevisionProposalStatus::ApplyFailed
                | PlanRevisionProposalStatus::Applied
        )
        .then_some(now),
        apply_finished_at: matches!(
            status,
            PlanRevisionProposalStatus::Applied
                | PlanRevisionProposalStatus::Rejected
                | PlanRevisionProposalStatus::Superseded
                | PlanRevisionProposalStatus::ApplyFailed
        )
        .then_some(now),
        apply_error: matches!(status, PlanRevisionProposalStatus::ApplyFailed)
            .then_some("apply failed".to_string()),
        recovery,
        applied_plan_id: matches!(status, PlanRevisionProposalStatus::Applied)
            .then_some(PlanId::parse("plan-2").expect("plan id")),
        applied_plan_version: matches!(status, PlanRevisionProposalStatus::Applied).then_some(2),
        source_supervisor_proposal_id: Some("proposal-1".to_string()),
    }
}

#[cfg(test)]
fn sample_collaboration() -> CollaborationState {
    let mut collaboration = CollaborationState::default();
    let workstream = sample_workstream("ws-1");
    let work_unit = sample_work_unit("wu-1", "ws-1");
    collaboration
        .workstreams
        .insert(workstream.id.clone(), workstream);
    collaboration
        .work_units
        .insert(work_unit.id.clone(), work_unit);
    collaboration.supervisor_proposals.insert(
        "proposal-1".to_string(),
        sample_supervisor_proposal_record(tt_core::SupervisorProposalStatus::Open),
    );
    collaboration.supervisor_turn_decisions.insert(
        "decision-1".to_string(),
        sample_decision_record(SupervisorTurnDecisionStatus::ProposedToHuman),
    );
    collaboration.planning_sessions.insert(
        "session-1".to_string(),
        sample_planning_session(PlanningSessionStatus::AwaitingApproval, true),
    );
    collaboration.planning.revision_proposals.insert(
        "revision-1".to_string(),
        sample_plan_revision_proposal(PlanRevisionProposalStatus::ApplyFailed, {
            let mut recovery = tt_core::planning::PlanRevisionRecoveryState::default();
            recovery.phase = PlanRevisionApplyPhase::FailedAfterDownstream;
            recovery.failure_kind = Some(PlanRevisionApplyFailureKind::OperatorRequired);
            recovery.reconcile_available = true;
            recovery.operator_intervention_required = true;
            recovery.failure_message = Some("needs reconciliation".to_string());
            recovery.downstream_apply_started = true;
            recovery.downstream_apply_completed = true;
            recovery
        }),
    );
    collaboration.supervisor_proposals.insert(
        "proposal-passive".to_string(),
        sample_supervisor_proposal_record(tt_core::SupervisorProposalStatus::GenerationFailed),
    );
    collaboration.planning_sessions.insert(
        "session-passive".to_string(),
        sample_planning_session(PlanningSessionStatus::Chatting, false),
    );
    collaboration
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tokio::sync::watch;
    use tt_core::store::StoredState;

    fn apply_inbox_changes(
        mut items: BTreeMap<String, ipc::OperatorInboxItem>,
        changes: &[ipc::OperatorInboxChange],
    ) -> Vec<ipc::OperatorInboxItem> {
        for change in changes {
            match change.kind {
                ipc::OperatorInboxChangeKind::Upsert => {
                    items.insert(change.item.id.clone(), change.item.clone());
                }
                ipc::OperatorInboxChangeKind::Removed => {
                    items.remove(&change.item.id);
                }
            }
        }
        let mut items = items.into_values().collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .sequence
                .cmp(&left.sequence)
                .then_with(|| left.id.cmp(&right.id))
        });
        items
    }

    #[test]
    fn open_supervisor_proposal_appears_in_inbox() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);

        let proposal = inbox
            .items
            .iter()
            .find(|item| item.id == "supervisor_proposal::proposal-1")
            .expect("proposal item");
        assert_eq!(
            proposal.source_kind,
            ipc::OperatorInboxSourceKind::SupervisorProposal
        );
        assert_eq!(proposal.status, ipc::OperatorInboxItemStatus::Open);
        assert!(
            proposal
                .available_actions
                .contains(&ipc::OperatorInboxActionKind::Approve)
        );
        assert!(
            proposal
                .available_actions
                .contains(&ipc::OperatorInboxActionKind::Reject)
        );
    }

    #[test]
    fn terminal_supervisor_proposals_close_the_inbox_item() {
        for (status, expected) in [
            (
                tt_core::SupervisorProposalStatus::Approved,
                ipc::OperatorInboxItemStatus::Resolved,
            ),
            (
                tt_core::SupervisorProposalStatus::Rejected,
                ipc::OperatorInboxItemStatus::Resolved,
            ),
            (
                tt_core::SupervisorProposalStatus::Superseded,
                ipc::OperatorInboxItemStatus::Superseded,
            ),
            (
                tt_core::SupervisorProposalStatus::Stale,
                ipc::OperatorInboxItemStatus::Stale,
            ),
        ] {
            let mut collaboration = sample_collaboration();
            collaboration.supervisor_proposals.insert(
                "proposal-1".to_string(),
                sample_supervisor_proposal_record(status),
            );
            let inbox = build_operator_inbox_state(&collaboration);
            let proposal = inbox
                .items
                .iter()
                .find(|item| item.id == "supervisor_proposal::proposal-1")
                .expect("proposal item");
            assert_eq!(proposal.status, expected);
            assert!(proposal.available_actions.is_empty());
        }
    }

    #[test]
    fn pending_supervisor_decision_appears_and_resolves() {
        let mut collaboration = sample_collaboration();
        collaboration.supervisor_turn_decisions.insert(
            "decision-1".to_string(),
            sample_decision_record(SupervisorTurnDecisionStatus::ProposedToHuman),
        );
        let inbox = build_operator_inbox_state(&collaboration);
        let decision = inbox
            .items
            .iter()
            .find(|item| item.id == "supervisor_decision::decision-1")
            .expect("decision item");
        assert_eq!(decision.status, ipc::OperatorInboxItemStatus::Open);
        assert!(
            decision
                .available_actions
                .contains(&ipc::OperatorInboxActionKind::ApproveAndSend)
        );

        collaboration.supervisor_turn_decisions.insert(
            "decision-1".to_string(),
            sample_decision_record(SupervisorTurnDecisionStatus::Recorded),
        );
        let resolved = build_operator_inbox_state(&collaboration);
        let decision = resolved
            .items
            .iter()
            .find(|item| item.id == "supervisor_decision::decision-1")
            .expect("decision item");
        assert_eq!(decision.status, ipc::OperatorInboxItemStatus::Resolved);
        assert!(decision.available_actions.is_empty());
    }

    #[test]
    fn planning_session_ready_for_review_appears_in_inbox() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);
        let session = inbox
            .items
            .iter()
            .find(|item| item.id == "planning_session::session-1")
            .expect("planning session item");
        assert_eq!(
            session.source_kind,
            ipc::OperatorInboxSourceKind::PlanningSession
        );
        assert_eq!(session.status, ipc::OperatorInboxItemStatus::Open);
        assert!(
            session
                .available_actions
                .contains(&ipc::OperatorInboxActionKind::Approve)
        );
        assert!(
            session
                .available_actions
                .contains(&ipc::OperatorInboxActionKind::Reject)
        );
    }

    #[test]
    fn reconcile_required_plan_revision_failure_appears_in_inbox() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);
        let revision = inbox
            .items
            .iter()
            .find(|item| item.id == "plan_revision_proposal::revision-1")
            .expect("revision item");
        assert_eq!(
            revision.source_kind,
            ipc::OperatorInboxSourceKind::PlanRevisionProposal
        );
        assert_eq!(revision.status, ipc::OperatorInboxItemStatus::Open);
        assert!(
            revision
                .available_actions
                .contains(&ipc::OperatorInboxActionKind::Reconcile)
        );
    }

    #[test]
    fn passive_records_do_not_appear() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);
        assert!(
            inbox
                .items
                .iter()
                .all(|item| item.id != "supervisor_proposal::proposal-passive"
                    && item.id != "planning_session::session-passive")
        );
    }

    #[test]
    fn inbox_round_trips_through_durable_state_and_rebuilds() {
        let collaboration = sample_collaboration();
        let stored = StoredState {
            registry: Default::default(),
            thread_views: Default::default(),
            turn_states: Default::default(),
            collaboration: collaboration.clone(),
            operator_inbox: build_operator_inbox_state(&collaboration),
            operator_inbox_mirrors: Default::default(),
        };
        let encoded = serde_json::to_value(&stored).expect("serialize");
        let decoded: StoredState = serde_json::from_value(encoded).expect("deserialize");
        assert_eq!(
            decoded.operator_inbox.items.len(),
            stored.operator_inbox.items.len()
        );
        assert_eq!(
            decoded.operator_inbox.items[0].id,
            stored.operator_inbox.items[0].id
        );
        assert_eq!(
            build_operator_inbox_state(&decoded.collaboration).items,
            decoded.operator_inbox.items
        );
    }

    #[test]
    fn inbox_query_filters_are_stable_and_predictable() {
        let collaboration = sample_collaboration();
        let request = ipc::OperatorInboxListRequest {
            include_closed: false,
            actionable_only: true,
            source_kind: Some(ipc::OperatorInboxSourceKind::PlanningSession),
            ..Default::default()
        };
        let items =
            list_operator_inbox_items(&build_operator_inbox_state(&collaboration), &request);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "planning_session::session-1");
    }

    #[test]
    fn inbox_export_after_sequence_is_ordered_and_deterministic() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);

        let all_changes = operator_inbox_changes_after(&inbox, 0, None);
        assert!(
            all_changes
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
        assert_eq!(all_changes, operator_inbox_changes_after(&inbox, 0, None));

        let partial = operator_inbox_changes_after(&inbox, 1, Some(2));
        assert!(partial.iter().all(|change| change.sequence > 1));
        assert!(
            partial
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
    }

    #[tokio::test]
    async fn inbox_wait_resolves_when_checkpoint_advances() {
        let inbox = build_operator_inbox_state(&sample_collaboration());
        let (tx, rx) = watch::channel(inbox.checkpoint.clone());
        let waiter = tokio::spawn(async move {
            wait_for_operator_inbox_checkpoint(rx, inbox.checkpoint.current_sequence, Some(250))
                .await
        });

        tx.send(ipc::OperatorInboxCheckpoint {
            current_sequence: inbox.checkpoint.current_sequence + 1,
            updated_at: sample_now(),
        })
        .expect("advance checkpoint");

        let checkpoint = waiter.await.expect("join waiter").expect("wait succeeds");
        assert_eq!(
            checkpoint.current_sequence,
            inbox.checkpoint.current_sequence + 1
        );
    }

    #[tokio::test]
    async fn inbox_wait_times_out_without_material_change() {
        let inbox = build_operator_inbox_state(&sample_collaboration());
        let (_, rx) = watch::channel(inbox.checkpoint.clone());

        let result =
            wait_for_operator_inbox_checkpoint(rx, inbox.checkpoint.current_sequence, Some(25))
                .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mirror_loop_checkpoint_wait_export_ack_behaves_predictably() {
        let initial_collaboration = sample_collaboration();
        let base = build_operator_inbox_state(&initial_collaboration);
        let (tx, rx) = watch::channel(base.checkpoint.clone());
        let waiter = tokio::spawn(async move {
            wait_for_operator_inbox_checkpoint(rx, base.checkpoint.current_sequence, Some(500))
                .await
        });

        let mut next_collaboration = initial_collaboration.clone();
        next_collaboration.supervisor_proposals.insert(
            "proposal-1".to_string(),
            sample_supervisor_proposal_record(tt_core::SupervisorProposalStatus::Approved),
        );
        let next = rebuild_operator_inbox_state(&next_collaboration, Some(&base));
        tx.send(next.checkpoint.clone())
            .expect("advance checkpoint");
        let checkpoint = waiter.await.expect("join waiter").expect("wait succeeds");
        assert_eq!(checkpoint, next.checkpoint);

        let mut mirrors = BTreeMap::new();
        let export = update_operator_inbox_export_checkpoint(
            &mut mirrors,
            "peer-loop",
            &checkpoint,
            base.checkpoint.current_sequence,
            &operator_inbox_changes_after(&next, base.checkpoint.current_sequence, None),
        )
        .expect("export");
        assert_eq!(export.last_exported_sequence, checkpoint.current_sequence);

        let ack = update_operator_inbox_ack_checkpoint(
            &mut mirrors,
            "peer-loop",
            export.last_exported_sequence,
        )
        .expect("ack");
        assert_eq!(ack.last_acked_sequence, export.last_exported_sequence);
        assert_eq!(
            operator_inbox_mirror_checkpoint_for_peer(&mirrors, "peer-loop"),
            ack
        );
    }

    #[tokio::test]
    async fn sequential_checkpoint_updates_advance_monotonically() {
        let inbox = build_operator_inbox_state(&sample_collaboration());
        let (tx, rx) = watch::channel(inbox.checkpoint.clone());
        let waiter = tokio::spawn(async move {
            wait_for_operator_inbox_checkpoint(rx, inbox.checkpoint.current_sequence, Some(500))
                .await
        });

        tx.send(ipc::OperatorInboxCheckpoint {
            current_sequence: inbox.checkpoint.current_sequence + 1,
            updated_at: sample_now(),
        })
        .expect("advance 1");
        let first = waiter.await.expect("join waiter").expect("wait succeeds");
        assert_eq!(
            first.current_sequence,
            inbox.checkpoint.current_sequence + 1
        );

        let rx = tx.subscribe();
        let waiter = tokio::spawn(async move {
            wait_for_operator_inbox_checkpoint(rx, first.current_sequence, Some(500)).await
        });

        tx.send(ipc::OperatorInboxCheckpoint {
            current_sequence: inbox.checkpoint.current_sequence + 2,
            updated_at: sample_now(),
        })
        .expect("advance 2");

        let second = waiter.await.expect("join waiter").expect("wait succeeds");
        assert_eq!(
            second.current_sequence,
            inbox.checkpoint.current_sequence + 2
        );
    }

    #[test]
    fn peer_scoped_mirror_checkpoint_persists_across_restart() {
        let mut mirrors = BTreeMap::new();
        mirrors.insert(
            "peer-1".to_string(),
            ipc::OperatorInboxMirrorCheckpoint {
                peer_id: "peer-1".to_string(),
                last_exported_sequence: 7,
                last_acked_sequence: 5,
                updated_at: sample_now(),
            },
        );
        let stored = StoredState {
            registry: Default::default(),
            thread_views: Default::default(),
            turn_states: Default::default(),
            collaboration: Default::default(),
            operator_inbox: Default::default(),
            operator_inbox_mirrors: mirrors.clone(),
        };
        let encoded = serde_json::to_value(&stored).expect("serialize");
        let decoded: StoredState = serde_json::from_value(encoded).expect("deserialize");
        assert_eq!(decoded.operator_inbox_mirrors, mirrors);
    }

    #[test]
    fn mirror_ack_cannot_move_backward_or_exceed_exported_sequence() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);
        let mut mirrors = BTreeMap::new();

        let export = update_operator_inbox_export_checkpoint(
            &mut mirrors,
            "peer-1",
            &inbox.checkpoint,
            0,
            &operator_inbox_changes_after(&inbox, 0, Some(2)),
        )
        .expect("export checkpoint");
        assert_eq!(export.last_exported_sequence, 2);

        let ack = update_operator_inbox_ack_checkpoint(&mut mirrors, "peer-1", 2)
            .expect("ack checkpoint");
        assert_eq!(ack.last_acked_sequence, 2);

        assert!(update_operator_inbox_ack_checkpoint(&mut mirrors, "peer-1", 1).is_err());
        assert!(update_operator_inbox_ack_checkpoint(&mut mirrors, "peer-1", 3).is_err());
    }

    #[test]
    fn bootstrap_replay_and_incremental_catch_up_match_rebuilt_view() {
        let initial_collaboration = sample_collaboration();
        let base = build_operator_inbox_state(&initial_collaboration);
        let bootstrap_items = operator_inbox_replay_items(&base);

        let mut next_collaboration = initial_collaboration.clone();
        next_collaboration.supervisor_proposals.insert(
            "proposal-1".to_string(),
            sample_supervisor_proposal_record(tt_core::SupervisorProposalStatus::Approved),
        );
        next_collaboration
            .planning_sessions
            .remove("session-passive");
        let next = rebuild_operator_inbox_state(&next_collaboration, Some(&base));
        let incremental =
            operator_inbox_changes_after(&next, base.checkpoint.current_sequence, None);

        let projected = apply_inbox_changes(
            bootstrap_items
                .into_iter()
                .map(|item| (item.id.clone(), item))
                .collect(),
            &incremental,
        );
        assert_eq!(projected, next.items);
    }

    #[test]
    fn overlapping_export_windows_behave_predictably() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);
        let first = operator_inbox_changes_after(&inbox, 0, Some(2));
        let second = operator_inbox_changes_after(&inbox, 1, Some(3));

        assert_eq!(first.len(), 2);
        assert!(second.iter().all(|change| change.sequence > 1));
        assert!(second.len() >= first.len().saturating_sub(1));
    }

    #[test]
    fn removed_inbox_items_mirror_correctly() {
        let initial_collaboration = sample_collaboration();
        let base = build_operator_inbox_state(&initial_collaboration);

        let mut next_collaboration = initial_collaboration.clone();
        next_collaboration.supervisor_proposals.remove("proposal-1");
        let next = rebuild_operator_inbox_state(&next_collaboration, Some(&base));
        let changes = operator_inbox_changes_after(&next, base.checkpoint.current_sequence, None);

        assert!(changes.iter().any(
            |change| change.kind == ipc::OperatorInboxChangeKind::Removed
                && change.item.id == "supervisor_proposal::proposal-1"
        ));
        let projected = apply_inbox_changes(
            base.items
                .clone()
                .into_iter()
                .map(|item| (item.id.clone(), item))
                .collect(),
            &changes,
        );
        assert_eq!(projected, next.items);
    }

    #[test]
    fn inbox_identity_is_stable_across_rebuilds() {
        let collaboration = sample_collaboration();
        let initial = build_operator_inbox_state(&collaboration);
        let rebuilt = rebuild_operator_inbox_state(&collaboration, Some(&initial));

        assert_eq!(initial.items, rebuilt.items);
        assert_eq!(
            initial.checkpoint.current_sequence,
            rebuilt.checkpoint.current_sequence
        );
        assert!(
            operator_inbox_changes_after(&rebuilt, initial.checkpoint.current_sequence, None)
                .is_empty()
        );
    }

    #[test]
    fn inbox_change_feed_is_ordered_and_cursorable() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);

        let first_two = operator_inbox_changes_after(&inbox, 0, Some(2));
        assert_eq!(first_two.len(), 2);
        assert!(first_two[0].sequence < first_two[1].sequence);

        let tail = operator_inbox_changes_after(&inbox, first_two[1].sequence, None);
        assert!(
            tail.iter()
                .all(|change| change.sequence > first_two[1].sequence)
        );
        assert!(!tail.is_empty());
    }

    #[test]
    fn inbox_checkpoint_survives_restart() {
        let collaboration = sample_collaboration();
        let stored = StoredState {
            registry: Default::default(),
            thread_views: Default::default(),
            turn_states: Default::default(),
            collaboration: collaboration.clone(),
            operator_inbox: build_operator_inbox_state(&collaboration),
            operator_inbox_mirrors: Default::default(),
        };
        let encoded = serde_json::to_value(&stored).expect("serialize");
        let decoded: StoredState = serde_json::from_value(encoded).expect("deserialize");

        assert_eq!(
            decoded.operator_inbox.checkpoint,
            stored.operator_inbox.checkpoint
        );
        assert_eq!(
            decoded.operator_inbox.changes,
            stored.operator_inbox.changes
        );
    }

    #[test]
    fn overlapping_cursor_reads_remain_predictable() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);

        let first_page = operator_inbox_changes_after(&inbox, 0, Some(1));
        let second_page = operator_inbox_changes_after(&inbox, first_page[0].sequence, Some(2));

        assert_eq!(first_page.len(), 1);
        assert!(
            second_page
                .iter()
                .all(|change| change.sequence > first_page[0].sequence)
        );
        assert!(
            !second_page
                .iter()
                .any(|change| change.sequence == first_page[0].sequence)
        );
    }

    #[test]
    fn terminal_state_changes_appear_in_incremental_feed() {
        let initial_collaboration = sample_collaboration();
        let initial = build_operator_inbox_state(&initial_collaboration);

        let mut next_collaboration = initial_collaboration.clone();
        next_collaboration.supervisor_proposals.insert(
            "proposal-1".to_string(),
            sample_supervisor_proposal_record(tt_core::SupervisorProposalStatus::Approved),
        );
        let next = rebuild_operator_inbox_state(&next_collaboration, Some(&initial));
        let changes =
            operator_inbox_changes_after(&next, initial.checkpoint.current_sequence, None);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ipc::OperatorInboxChangeKind::Upsert);
        assert_eq!(
            changes[0].item.id,
            "supervisor_proposal::proposal-1".to_string()
        );
        assert_eq!(
            changes[0].item.status,
            ipc::OperatorInboxItemStatus::Resolved
        );

        let projected = apply_inbox_changes(
            initial
                .items
                .clone()
                .into_iter()
                .map(|item| (item.id.clone(), item))
                .collect(),
            &changes,
        );
        assert_eq!(projected, next.items);
    }

    #[test]
    fn action_routing_maps_supported_actions_to_underlying_methods() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);

        let proposal = get_operator_inbox_item(&inbox, "supervisor_proposal::proposal-1")
            .expect("proposal item");
        let route =
            resolve_operator_inbox_action_route(&proposal, ipc::OperatorInboxActionKind::Approve)
                .expect("approve route");
        assert_eq!(
            route,
            ipc::OperatorInboxActionRoute::Proposal {
                item_id: proposal.id.clone(),
                proposal_id: proposal.actionable_object_id.clone(),
                method: ipc::methods::PROPOSAL_APPROVE.to_string(),
            }
        );

        let route =
            resolve_operator_inbox_action_route(&proposal, ipc::OperatorInboxActionKind::Reject)
                .expect("reject route");
        assert_eq!(
            route,
            ipc::OperatorInboxActionRoute::Proposal {
                item_id: proposal.id.clone(),
                proposal_id: proposal.actionable_object_id.clone(),
                method: ipc::methods::PROPOSAL_REJECT.to_string(),
            }
        );

        let decision = get_operator_inbox_item(&inbox, "supervisor_decision::decision-1")
            .expect("decision item");
        let route = resolve_operator_inbox_action_route(
            &decision,
            ipc::OperatorInboxActionKind::ApproveAndSend,
        )
        .expect("approve-and-send route");
        assert_eq!(
            route,
            ipc::OperatorInboxActionRoute::SupervisorDecision {
                item_id: decision.id.clone(),
                decision_id: decision.actionable_object_id.clone(),
                method: ipc::methods::SUPERVISOR_DECISION_APPROVE_AND_SEND.to_string(),
            }
        );

        let route = resolve_operator_inbox_action_route(
            &decision,
            ipc::OperatorInboxActionKind::RecordNoAction,
        )
        .expect("record-no-action route");
        assert_eq!(
            route,
            ipc::OperatorInboxActionRoute::SupervisorDecision {
                item_id: decision.id.clone(),
                decision_id: decision.actionable_object_id.clone(),
                method: ipc::methods::SUPERVISOR_DECISION_RECORD_NO_ACTION.to_string(),
            }
        );

        let session = get_operator_inbox_item(&inbox, "planning_session::session-1")
            .expect("planning session item");
        let route =
            resolve_operator_inbox_action_route(&session, ipc::OperatorInboxActionKind::Approve)
                .expect("planning session approve route");
        assert_eq!(
            route,
            ipc::OperatorInboxActionRoute::PlanningSession {
                item_id: session.id.clone(),
                session_id: session.actionable_object_id.clone(),
                method: ipc::methods::PLANNING_SESSION_APPROVE.to_string(),
            }
        );

        let route =
            resolve_operator_inbox_action_route(&session, ipc::OperatorInboxActionKind::Supersede)
                .expect("planning session supersede route");
        assert_eq!(
            route,
            ipc::OperatorInboxActionRoute::PlanningSession {
                item_id: session.id.clone(),
                session_id: session.actionable_object_id.clone(),
                method: ipc::methods::PLANNING_SESSION_SUPERSEDE.to_string(),
            }
        );

        let revision = get_operator_inbox_item(&inbox, "plan_revision_proposal::revision-1")
            .expect("revision item");
        let route =
            resolve_operator_inbox_action_route(&revision, ipc::OperatorInboxActionKind::Reconcile)
                .expect("reconcile route");
        assert_eq!(
            route,
            ipc::OperatorInboxActionRoute::PlanRevisionProposal {
                item_id: revision.id.clone(),
                proposal_id: revision.actionable_object_id.clone(),
                method: ipc::methods::PROPOSAL_RECONCILE.to_string(),
            }
        );

        let mut retryable_collaboration = sample_collaboration();
        let mut retryable_revision =
            sample_plan_revision_proposal(PlanRevisionProposalStatus::ApplyFailed, {
                let mut recovery = tt_core::planning::PlanRevisionRecoveryState::default();
                recovery.phase = PlanRevisionApplyPhase::FailedBeforeDownstream;
                recovery.failure_kind = Some(PlanRevisionApplyFailureKind::RetryableInfrastructure);
                recovery.retry_safe = true;
                recovery.reconcile_available = false;
                recovery.operator_intervention_required = false;
                recovery.failure_message = Some("retryable".to_string());
                recovery
            });
        retryable_revision.proposal_id =
            tt_core::planning::PlanRevisionProposalId::parse("revision-retry")
                .expect("revision id");
        retryable_collaboration
            .planning
            .revision_proposals
            .insert("revision-retry".to_string(), retryable_revision);
        let retryable_inbox = build_operator_inbox_state(&retryable_collaboration);
        let retryable =
            get_operator_inbox_item(&retryable_inbox, "plan_revision_proposal::revision-retry")
                .expect("retryable revision item");
        assert!(
            retryable
                .available_actions
                .contains(&ipc::OperatorInboxActionKind::Retry)
        );
        let route =
            resolve_operator_inbox_action_route(&retryable, ipc::OperatorInboxActionKind::Retry)
                .expect("retry route");
        assert_eq!(
            route,
            ipc::OperatorInboxActionRoute::PlanRevisionProposal {
                item_id: retryable.id.clone(),
                proposal_id: retryable.actionable_object_id.clone(),
                method: ipc::methods::PROPOSAL_APPROVE.to_string(),
            }
        );
    }

    #[test]
    fn passive_records_do_not_appear_in_the_incremental_feed() {
        let collaboration = sample_collaboration();
        let inbox = build_operator_inbox_state(&collaboration);
        let change_ids = operator_inbox_changes_after(&inbox, 0, None)
            .into_iter()
            .map(|change| change.item.id)
            .collect::<Vec<_>>();

        assert!(
            !change_ids
                .iter()
                .any(|id| id == "supervisor_proposal::proposal-passive")
        );
        assert!(
            !change_ids
                .iter()
                .any(|id| id == "planning_session::session-passive")
        );
    }
}
