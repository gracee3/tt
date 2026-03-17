use crate::app::AppState;
use orcas_core::{
    CodexThreadAssignmentStatus, CodexThreadBootstrapState, CodexThreadSendPolicy, ipc,
};

use super::shared::{PanelViewModel, abbreviate, compact_line, lifecycle_label, timestamp_label};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRowViewModel {
    pub id: String,
    pub status: String,
    pub turn_badge: Option<String>,
    pub assignment_badge: Option<String>,
    pub decision_badge: Option<String>,
    pub preview: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadListViewModel {
    pub rows: Vec<ThreadRowViewModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDetailViewModel {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadsViewModel {
    pub list: ThreadListViewModel,
    pub summary: PanelViewModel,
    pub detail: ThreadDetailViewModel,
}

pub fn thread_list(state: &AppState) -> ThreadListViewModel {
    ThreadListViewModel {
        rows: state
            .threads
            .iter()
            .map(|thread| ThreadRowViewModel {
                id: thread.id.clone(),
                status: thread_status_label(state, thread),
                turn_badge: thread_turn_badge(state, &thread.id),
                assignment_badge: thread_assignment_badge(state, &thread.id),
                decision_badge: thread_decision_badge(state, &thread.id),
                preview: abbreviate(&thread.preview.replace('\n', " "), 40),
                selected: state.selected_thread_id.as_deref() == Some(thread.id.as_str()),
            })
            .collect(),
    }
}

pub fn thread_summary(state: &AppState) -> PanelViewModel {
    let Some(thread_id) = state.selected_thread_id.as_ref() else {
        return PanelViewModel {
            title: "Selected Thread".to_string(),
            lines: vec!["No thread selected.".to_string()],
        };
    };

    let Some(summary) = state.threads.iter().find(|thread| thread.id == *thread_id) else {
        return PanelViewModel {
            title: format!("Selected Thread {thread_id}"),
            lines: vec!["Selected thread is no longer present.".to_string()],
        };
    };

    let mut lines = vec![
        format!("status: {}", thread_status_label(state, summary)),
        format!("loaded: {}", loaded_status_label(summary.loaded_status)),
        format!("monitor: {}", monitor_state_label(summary.monitor_state)),
        format!("cwd: {}", summary.cwd),
        format!("provider: {}", summary.model_provider),
        format!("scope: {}", summary.scope),
    ];

    if let Some(source_kind) = summary.source_kind.as_ref() {
        lines.push(format!("source: {source_kind}"));
    }
    if let Some(turn_id) = summary.active_turn_id.as_ref() {
        lines.push(format!("active turn: {turn_id}"));
    }
    if let Some(turn_id) = summary.last_seen_turn_id.as_ref() {
        lines.push(format!("last seen turn: {turn_id}"));
    }
    if let Some(assignment) = thread_assignment_for_display(state, thread_id) {
        lines.push(format!(
            "assignment: {} [{}]",
            assignment.assignment_id,
            codex_assignment_status_label(assignment.status)
        ));
        lines.push(format!(
            "binding: stream={} unit={} supervisor={}",
            assignment.workstream_id, assignment.work_unit_id, assignment.supervisor_id
        ));
        lines.push(format!(
            "policy: {}  bootstrap: {}",
            codex_send_policy_label(assignment.send_policy),
            codex_bootstrap_state_label(assignment.bootstrap_state)
        ));
    } else {
        lines.push("assignment: unassigned".to_string());
    }
    if let Some(decision) = thread_decision_for_display(state, thread_id) {
        lines.push(format!(
            "decision: {} [{}]",
            decision.decision_id,
            supervisor_decision_status_label(decision)
        ));
        lines.push(format!(
            "review: {}  proposal: {}",
            supervisor_decision_kind_label(decision.kind),
            supervisor_proposal_kind_label(decision.proposal_kind)
        ));
    } else {
        lines.push("decision: none".to_string());
    }

    if let Some(turn_state) = latest_turn_state_for_thread(state, thread_id) {
        lines.push(format!(
            "latest turn: {} [{}] attachable={} terminal={}",
            turn_state.turn_id,
            lifecycle_label(&turn_state.lifecycle),
            turn_state.attachable,
            turn_state.terminal
        ));
        if let Some(event) = turn_state.recent_event.as_ref() {
            lines.push(format!("event: {}", abbreviate(&compact_line(event), 88)));
        }
        if let Some(output) = turn_state.recent_output.as_ref() {
            lines.push(format!("output: {}", abbreviate(&compact_line(output), 88)));
        }
    } else {
        lines.push("latest turn: no active lifecycle state loaded".to_string());
    }

    if let Some(output) = summary.recent_output.as_ref() {
        lines.push(format!(
            "recent output: {}",
            abbreviate(&compact_line(output), 88)
        ));
    }
    if let Some(event) = summary.recent_event.as_ref() {
        lines.push(format!(
            "recent event: {}",
            abbreviate(&compact_line(event), 88)
        ));
    }

    lines.push(format!(
        "detail: {}",
        state
            .thread_details
            .get(thread_id)
            .map(|thread| {
                let history = if thread.history_loaded {
                    "history loaded"
                } else {
                    "summary only"
                };
                format!("{} turns cached, {history}", thread.turns.len())
            })
            .unwrap_or_else(|| "loading on demand".to_string())
    ));

    PanelViewModel {
        title: format!("Selected Thread {}", summary.id),
        lines,
    }
}

pub fn thread_detail(state: &AppState) -> ThreadDetailViewModel {
    let Some(thread_id) = state.selected_thread_id.as_ref() else {
        return ThreadDetailViewModel {
            title: "Thread Activity".to_string(),
            lines: vec!["No thread selected.".to_string()],
        };
    };

    let Some(thread) = state.thread_details.get(thread_id) else {
        return ThreadDetailViewModel {
            title: format!("Thread Activity {thread_id}"),
            lines: vec!["Loading thread details...".to_string()],
        };
    };

    let mut lines = Vec::new();
    if let Some(assignment) = thread_assignment_for_display(state, thread_id) {
        lines.push(format!(
            "assignment {} [{}]",
            assignment.assignment_id,
            codex_assignment_status_label(assignment.status)
        ));
        lines.push(format!(
            "  workstream={}  work_unit={}  supervisor={}",
            assignment.workstream_id, assignment.work_unit_id, assignment.supervisor_id
        ));
        lines.push(format!(
            "  policy={}  bootstrap={}",
            codex_send_policy_label(assignment.send_policy),
            codex_bootstrap_state_label(assignment.bootstrap_state)
        ));
        lines.push(format!(
            "  assigned by {} at {}",
            assignment.assigned_by,
            timestamp_label(assignment.assigned_at)
        ));
        if let Some(turn_id) = assignment.latest_basis_turn_id.as_ref() {
            lines.push(format!("  latest basis turn {turn_id}"));
        }
        if let Some(notes) = assignment.notes.as_ref() {
            lines.push(format!("  notes {}", abbreviate(&compact_line(notes), 84)));
        }
        lines.push(String::new());
    } else {
        lines.push("Assignment: unassigned".to_string());
        lines.push(String::new());
    }

    if let Some(decision) = thread_decision_for_display(state, thread_id) {
        lines.push(format!(
            "decision {} [{}]",
            decision.decision_id,
            supervisor_decision_status_label(decision)
        ));
        lines.push(format!(
            "  kind={}  proposal={}  basis={}",
            supervisor_decision_kind_label(decision.kind),
            supervisor_proposal_kind_label(decision.proposal_kind),
            decision.basis_turn_id.as_deref().unwrap_or("-"),
        ));
        lines.push(format!(
            "  rationale {}",
            abbreviate(&compact_line(&decision.rationale_summary), 84)
        ));
        if let Some(text) = decision.proposed_text.as_ref() {
            lines.push(format!(
                "  proposed {}",
                abbreviate(&compact_line(text), 84)
            ));
        }
        lines.push(format!(
            "  created {}  approved {}  rejected {}  sent {}",
            timestamp_label(decision.created_at),
            decision
                .approved_at
                .map(timestamp_label)
                .unwrap_or_else(|| "-".to_string()),
            decision
                .rejected_at
                .map(timestamp_label)
                .unwrap_or_else(|| "-".to_string()),
            decision
                .sent_at
                .map(timestamp_label)
                .unwrap_or_else(|| "-".to_string()),
        ));
        if let Some(turn_id) = decision.sent_turn_id.as_ref() {
            lines.push(format!("  sent turn {turn_id}"));
        }
        if let Some(notes) = decision.notes.as_ref() {
            lines.push(format!("  notes {}", abbreviate(&compact_line(notes), 84)));
        }
        if decision.kind == orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn {
            lines.push(format!(
                "  editable={} revision_state={}",
                if steer_decision_editable(state, decision) {
                    "yes"
                } else {
                    "no"
                },
                steer_revision_state_label(decision)
            ));
        }
        if decision.status == orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman {
            if decision.kind == orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn {
                if steer_compose_for_thread(state, thread_id).is_some_and(|compose| {
                    compose.replace_decision_id.as_deref() == Some(decision.decision_id.as_str())
                }) {
                    lines.push(
                        "  actions: ctrl+s save steer  esc cancel  enter newline".to_string(),
                    );
                } else {
                    lines.push(
                        "  actions: e edit pending steer  a approve/send  d reject".to_string(),
                    );
                }
            } else {
                lines.push("  actions: a approve/send  d reject".to_string());
            }
        }
        lines.push(String::new());
    } else {
        lines.push("Decision: none".to_string());
        if steer_compose_for_thread(state, thread_id).is_some() {
            lines.push("  actions: ctrl+s create steer  esc cancel  enter newline".to_string());
        } else if thread_steer_proposable(state, thread_id)
            || thread_interrupt_proposable(state, thread_id)
        {
            lines.push("  actions: s compose steer  i propose interrupt".to_string());
        }
        lines.push(String::new());
    }

    if let Some(compose) = steer_compose_for_thread(state, thread_id) {
        lines.push(if compose.replace_decision_id.is_some() {
            "Steer Compose: editing pending steer".to_string()
        } else {
            "Steer Compose: new steer proposal".to_string()
        });
        let (line_index, column, _) = compose_cursor_position(compose);
        lines.push(format!(
            "  cursor line={} col={} chars={}",
            line_index + 1,
            column + 1,
            compose.buffer.chars().count()
        ));
        for line in render_compose_lines(compose, 6, 84) {
            lines.push(line);
        }
        lines.push(
            "  actions: ctrl+s save steer  esc cancel  enter newline  arrows move  backspace/delete edit"
                .to_string(),
        );
        lines.push(String::new());
    }

    let history = thread_decision_history(state, thread_id);
    if !history.is_empty() {
        lines.push("Decision History:".to_string());
        for decision in history {
            lines.push(format!(
                "  {} [{}] kind={} proposal={} basis={}",
                decision.decision_id,
                supervisor_decision_status_label(decision),
                supervisor_decision_kind_label(decision.kind),
                supervisor_proposal_kind_label(decision.proposal_kind),
                decision.basis_turn_id.as_deref().unwrap_or("-"),
            ));
            lines.push(format!(
                "    created={} sent={} rejected={}",
                timestamp_label(decision.created_at),
                decision
                    .sent_at
                    .map(timestamp_label)
                    .unwrap_or_else(|| "-".to_string()),
                decision
                    .rejected_at
                    .map(timestamp_label)
                    .unwrap_or_else(|| "-".to_string()),
            ));
            if let Some(superseded_by) = decision.superseded_by.as_ref() {
                lines.push(format!("    superseded by {}", superseded_by));
            }
            lines.push(format!(
                "    rationale {}",
                abbreviate(&compact_line(&decision.rationale_summary), 76)
            ));
            if let Some(text) = decision.proposed_text.as_ref() {
                for preview in render_decision_text_preview(text, 2, 76) {
                    lines.push(preview);
                }
            }
        }
        lines.push(String::new());
    }

    if thread.turns.is_empty() {
        lines.push("No turns loaded.".to_string());
    } else {
        for turn in thread.turns.iter().rev().take(4) {
            lines.push(format!("turn {} [{}]", turn.id, turn.status));
            if let Some(diff) = turn.latest_diff.as_ref() {
                lines.push(format!("  diff {}", abbreviate(&compact_line(diff), 84)));
            }
            if let Some(turn_state) = turn_state_for_turn(state, thread_id, &turn.id) {
                lines.push(format!(
                    "  lifecycle={} attachable={} live_stream={} terminal={}",
                    lifecycle_label(&turn_state.lifecycle),
                    turn_state.attachable,
                    turn_state.live_stream,
                    turn_state.terminal
                ));
                if let Some(event) = turn_state.recent_event.as_ref() {
                    lines.push(format!("  event {}", abbreviate(&compact_line(event), 84)));
                }
                if let Some(output) = turn_state.recent_output.as_ref() {
                    lines.push(format!(
                        "  output {}",
                        abbreviate(&compact_line(output), 84)
                    ));
                }
            }

            if turn.items.is_empty() {
                lines.push("  no items".to_string());
                continue;
            }

            for item in turn.items.iter().rev().take(3) {
                let status = item.status.clone().unwrap_or_else(|| "unknown".to_string());
                let text = item
                    .text
                    .as_ref()
                    .or(item.summary.as_ref())
                    .map(|text| abbreviate(&compact_line(text), 84))
                    .unwrap_or_else(|| "-".to_string());
                lines.push(format!("  {} [{}] {}", item.item_type, status, text));
            }
        }
    }

    ThreadDetailViewModel {
        title: format!("Thread Activity {}", thread.summary.id),
        lines,
    }
}

pub fn threads_view(state: &AppState) -> ThreadsViewModel {
    ThreadsViewModel {
        list: thread_list(state),
        summary: thread_summary(state),
        detail: thread_detail(state),
    }
}

fn thread_status_label(state: &AppState, thread: &ipc::ThreadSummary) -> String {
    latest_turn_state_for_thread(state, &thread.id)
        .map(|turn| lifecycle_label(&turn.lifecycle).to_string())
        .unwrap_or_else(|| thread.status.clone())
}

fn thread_turn_badge(state: &AppState, thread_id: &str) -> Option<String> {
    latest_turn_state_for_thread(state, thread_id).map(|turn| {
        if turn.attachable && turn.live_stream {
            format!("{} attachable", lifecycle_label(&turn.lifecycle))
        } else {
            lifecycle_label(&turn.lifecycle).to_string()
        }
    })
}

fn thread_assignment_badge(state: &AppState, thread_id: &str) -> Option<String> {
    let assignment = current_thread_assignment(state, thread_id)?;
    Some(codex_assignment_status_label(assignment.status).to_string())
}

fn thread_decision_badge(state: &AppState, thread_id: &str) -> Option<String> {
    let decision = thread_decision_for_display(state, thread_id)?;
    Some(supervisor_decision_status_label(decision).to_string())
}

fn loaded_status_label(status: ipc::ThreadLoadedStatus) -> &'static str {
    match status {
        ipc::ThreadLoadedStatus::NotLoaded => "not loaded",
        ipc::ThreadLoadedStatus::Idle => "idle",
        ipc::ThreadLoadedStatus::Active => "active",
        ipc::ThreadLoadedStatus::SystemError => "system error",
        ipc::ThreadLoadedStatus::Unknown => "unknown",
    }
}

fn monitor_state_label(state: ipc::ThreadMonitorState) -> &'static str {
    match state {
        ipc::ThreadMonitorState::Detached => "history only",
        ipc::ThreadMonitorState::Attaching => "attaching",
        ipc::ThreadMonitorState::Attached => "live attached",
        ipc::ThreadMonitorState::Errored => "attach errored",
    }
}

fn codex_assignment_status_label(status: CodexThreadAssignmentStatus) -> &'static str {
    match status {
        CodexThreadAssignmentStatus::Proposed => "proposed",
        CodexThreadAssignmentStatus::Active => "assigned",
        CodexThreadAssignmentStatus::Paused => "paused",
        CodexThreadAssignmentStatus::Completed => "completed",
        CodexThreadAssignmentStatus::Released => "released",
    }
}

fn codex_send_policy_label(policy: CodexThreadSendPolicy) -> &'static str {
    match policy {
        CodexThreadSendPolicy::HumanApprovalRequired => "human approval required",
        CodexThreadSendPolicy::SupervisorMaySend => "supervisor may send",
    }
}

fn codex_bootstrap_state_label(state: CodexThreadBootstrapState) -> &'static str {
    match state {
        CodexThreadBootstrapState::NotNeeded => "not needed",
        CodexThreadBootstrapState::Pending => "pending",
        CodexThreadBootstrapState::Proposed => "proposed",
        CodexThreadBootstrapState::Sent => "sent",
    }
}

fn supervisor_decision_kind_label(kind: orcas_core::SupervisorTurnDecisionKind) -> &'static str {
    match kind {
        orcas_core::SupervisorTurnDecisionKind::NextTurn => "next turn",
        orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn => "steer active turn",
        orcas_core::SupervisorTurnDecisionKind::InterruptActiveTurn => "interrupt active turn",
        orcas_core::SupervisorTurnDecisionKind::NoAction => "no action",
    }
}

fn supervisor_proposal_kind_label(kind: orcas_core::SupervisorTurnProposalKind) -> &'static str {
    match kind {
        orcas_core::SupervisorTurnProposalKind::Bootstrap => "bootstrap",
        orcas_core::SupervisorTurnProposalKind::ContinueAfterTurn => "continue after turn",
        orcas_core::SupervisorTurnProposalKind::ManualRefresh => "manual refresh",
        orcas_core::SupervisorTurnProposalKind::OperatorSteer => "operator steer",
        orcas_core::SupervisorTurnProposalKind::OperatorInterrupt => "operator interrupt",
    }
}

fn supervisor_decision_status_label(decision: &ipc::SupervisorTurnDecisionSummary) -> &'static str {
    match (decision.kind, decision.status) {
        (
            orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn,
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
        ) => "pending steer approval",
        (
            orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn,
            orcas_core::SupervisorTurnDecisionStatus::Stale,
        ) => "stale steer proposal",
        (
            orcas_core::SupervisorTurnDecisionKind::InterruptActiveTurn,
            orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman,
        ) => "pending interrupt approval",
        (
            orcas_core::SupervisorTurnDecisionKind::InterruptActiveTurn,
            orcas_core::SupervisorTurnDecisionStatus::Stale,
        ) => "stale interrupt proposal",
        (_, orcas_core::SupervisorTurnDecisionStatus::Draft) => "draft",
        (_, orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman) => "pending human approval",
        (_, orcas_core::SupervisorTurnDecisionStatus::Approved) => "approved",
        (_, orcas_core::SupervisorTurnDecisionStatus::Rejected) => "rejected",
        (_, orcas_core::SupervisorTurnDecisionStatus::Sent) => "sent",
        (_, orcas_core::SupervisorTurnDecisionStatus::Superseded) => "superseded",
        (_, orcas_core::SupervisorTurnDecisionStatus::Stale) => "stale proposal",
    }
}

fn steer_revision_state_label(decision: &ipc::SupervisorTurnDecisionSummary) -> &'static str {
    match decision.status {
        orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman => "pending steer",
        orcas_core::SupervisorTurnDecisionStatus::Superseded => "superseded revision",
        orcas_core::SupervisorTurnDecisionStatus::Stale => "stale revision",
        orcas_core::SupervisorTurnDecisionStatus::Rejected => "rejected revision",
        orcas_core::SupervisorTurnDecisionStatus::Sent => "sent revision",
        _ => "non-editable revision",
    }
}

fn thread_steer_proposable(state: &AppState, thread_id: &str) -> bool {
    thread_active_turn_decision_proposable(state, thread_id)
}

fn thread_interrupt_proposable(state: &AppState, thread_id: &str) -> bool {
    thread_active_turn_decision_proposable(state, thread_id)
}

fn thread_active_turn_decision_proposable(state: &AppState, thread_id: &str) -> bool {
    let Some(assignment) = current_thread_assignment(state, thread_id) else {
        return false;
    };
    if assignment.status != CodexThreadAssignmentStatus::Active {
        return false;
    }
    let has_active_turn = state
        .thread_details
        .get(thread_id)
        .map(|thread| thread.summary.active_turn_id.is_some())
        .or_else(|| {
            state
                .threads
                .iter()
                .find(|thread| thread.id == thread_id)
                .map(|thread| thread.active_turn_id.is_some())
        })
        .unwrap_or(false);
    if !has_active_turn {
        return false;
    }
    !state
        .collaboration
        .supervisor_turn_decisions
        .iter()
        .any(|decision| decision.assignment_id == assignment.assignment_id && decision.open)
}

fn current_thread_assignment<'a>(
    state: &'a AppState,
    thread_id: &str,
) -> Option<&'a ipc::CodexThreadAssignmentSummary> {
    state
        .collaboration
        .codex_thread_assignments
        .iter()
        .find(|assignment| {
            assignment.codex_thread_id == thread_id
                && matches!(
                    assignment.status,
                    CodexThreadAssignmentStatus::Proposed
                        | CodexThreadAssignmentStatus::Active
                        | CodexThreadAssignmentStatus::Paused
                )
        })
}

fn steer_compose_for_thread<'a>(
    state: &'a AppState,
    thread_id: &str,
) -> Option<&'a crate::app::SteerComposeState> {
    state
        .steer_compose
        .as_ref()
        .filter(|compose| compose.thread_id == thread_id)
}

fn steer_decision_editable(
    _state: &AppState,
    decision: &ipc::SupervisorTurnDecisionSummary,
) -> bool {
    decision.kind == orcas_core::SupervisorTurnDecisionKind::SteerActiveTurn
        && decision.status == orcas_core::SupervisorTurnDecisionStatus::ProposedToHuman
}

fn latest_thread_assignment<'a>(
    state: &'a AppState,
    thread_id: &str,
) -> Option<&'a ipc::CodexThreadAssignmentSummary> {
    state
        .collaboration
        .codex_thread_assignments
        .iter()
        .filter(|assignment| assignment.codex_thread_id == thread_id)
        .max_by(|left, right| left.updated_at.cmp(&right.updated_at))
}

fn thread_assignment_for_display<'a>(
    state: &'a AppState,
    thread_id: &str,
) -> Option<&'a ipc::CodexThreadAssignmentSummary> {
    current_thread_assignment(state, thread_id)
        .or_else(|| latest_thread_assignment(state, thread_id))
}

fn thread_decision_for_display<'a>(
    state: &'a AppState,
    thread_id: &str,
) -> Option<&'a ipc::SupervisorTurnDecisionSummary> {
    state
        .collaboration
        .supervisor_turn_decisions
        .iter()
        .filter(|decision| decision.codex_thread_id == thread_id)
        .max_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.decision_id.cmp(&right.decision_id))
        })
}

fn thread_decision_history<'a>(
    state: &'a AppState,
    thread_id: &str,
) -> Vec<&'a ipc::SupervisorTurnDecisionSummary> {
    let mut decisions = state
        .collaboration
        .supervisor_turn_decisions
        .iter()
        .filter(|decision| decision.codex_thread_id == thread_id)
        .collect::<Vec<_>>();
    decisions.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.decision_id.cmp(&right.decision_id))
    });
    decisions.into_iter().take(6).collect()
}

fn render_decision_text_preview(text: &str, max_lines: usize, width: usize) -> Vec<String> {
    text.lines()
        .take(max_lines)
        .map(|line| format!("    text {}", abbreviate(&compact_line(line), width)))
        .collect()
}

fn render_compose_lines(
    compose: &crate::app::SteerComposeState,
    max_lines: usize,
    width: usize,
) -> Vec<String> {
    let (cursor_line, cursor_col, _) = compose_cursor_position(compose);
    let lines = if compose.buffer.is_empty() {
        vec![String::new()]
    } else {
        compose
            .buffer
            .lines()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    };
    lines
        .into_iter()
        .take(max_lines)
        .enumerate()
        .map(|(index, line)| {
            let decorated = if index == cursor_line {
                insert_cursor_marker(&line, cursor_col)
            } else {
                line
            };
            format!("  {:>2}> {}", index + 1, abbreviate(&decorated, width))
        })
        .collect()
}

fn insert_cursor_marker(line: &str, column: usize) -> String {
    let mut out = String::new();
    let mut inserted = false;
    for (idx, ch) in line.chars().enumerate() {
        if idx == column {
            out.push('|');
            inserted = true;
        }
        out.push(ch);
    }
    if !inserted {
        out.push('|');
    }
    out
}

fn compose_cursor_position(compose: &crate::app::SteerComposeState) -> (usize, usize, usize) {
    let mut line_index = 0usize;
    let mut column = 0usize;
    let mut line_start = 0usize;
    for (index, ch) in compose.buffer.char_indices() {
        if index >= compose.cursor {
            break;
        }
        if ch == '\n' {
            line_index += 1;
            column = 0;
            line_start = index + ch.len_utf8();
        } else {
            column += 1;
        }
    }
    (line_index, column, line_start)
}

fn latest_turn_state_for_thread<'a>(
    state: &'a AppState,
    thread_id: &str,
) -> Option<&'a ipc::TurnStateView> {
    state
        .turn_states
        .values()
        .filter(|turn| turn.thread_id == thread_id)
        .max_by(|left, right| left.updated_at.cmp(&right.updated_at))
}

fn turn_state_for_turn<'a>(
    state: &'a AppState,
    thread_id: &str,
    turn_id: &str,
) -> Option<&'a ipc::TurnStateView> {
    state
        .turn_states
        .values()
        .find(|turn| turn.thread_id == thread_id && turn.turn_id == turn_id)
}
