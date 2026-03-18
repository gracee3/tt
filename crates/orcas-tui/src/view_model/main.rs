use crate::app::{AppState, MainHierarchySelection, ProgramView};
use crate::view_model::{
    PanelViewModel, collaboration_detail, connection_status, event_log, status_banner,
    thread_summary, workstream_detail,
};
use orcas_core::{
    AssignmentStatus, CodexThreadAssignmentStatus, ReportParseResult, SupervisorProposalStatus,
    WorkUnitStatus, WorkstreamStatus, ipc,
};

use super::shared::{abbreviate, compact_line, lifecycle_label};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainViewModel {
    pub header: MainHeaderViewModel,
    pub hierarchy_list: MainHierarchyListViewModel,
    pub detail_panel: PanelViewModel,
    pub footer_prompt: MainFooterPromptViewModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainHeaderViewModel {
    pub status_segments: Vec<MainStatusSegmentViewModel>,
    pub program_tabs: Vec<ProgramTabViewModel>,
    pub toast_lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainStatusSegmentViewModel {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramTabViewModel {
    pub program_view: ProgramView,
    pub label: String,
    pub selected: bool,
    pub placeholder: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HierarchyRowKind {
    Workstream,
    WorkUnit,
    Thread,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainHierarchyRowViewModel {
    pub kind: HierarchyRowKind,
    pub selection: MainHierarchySelection,
    pub depth: u16,
    pub label: String,
    pub badges: Vec<String>,
    pub secondary: Option<String>,
    pub selected: bool,
    pub expanded: bool,
    pub collapsible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainHierarchyListViewModel {
    pub rows: Vec<MainHierarchyRowViewModel>,
    pub scroll_offset: usize,
    pub selected_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainFooterPromptViewModel {
    pub title: String,
    pub prompt_lines: Vec<String>,
    pub context_lines: Vec<String>,
    pub hint_line: String,
}

pub fn main_view(state: &AppState) -> MainViewModel {
    MainViewModel {
        header: main_header(state),
        hierarchy_list: main_hierarchy_list(state),
        detail_panel: main_detail_panel(state),
        footer_prompt: main_footer_prompt(state),
    }
}

pub fn main_hierarchy_list(state: &AppState) -> MainHierarchyListViewModel {
    let rows = hierarchy_rows(state);
    let selected_index = state
        .main_view
        .selected
        .as_ref()
        .and_then(|selected| rows.iter().position(|row| &row.selection == selected));
    MainHierarchyListViewModel {
        rows,
        scroll_offset: state.main_view.scroll_offset,
        selected_index,
    }
}

fn main_header(state: &AppState) -> MainHeaderViewModel {
    let connection = connection_status(state);
    let mut toast_lines = Vec::new();
    if let Some(banner) = status_banner(state) {
        toast_lines.push(banner.message);
    } else if let Some(recent) = event_log(state).lines.last() {
        toast_lines.push(recent.clone());
    } else {
        toast_lines.push("No recent updates.".to_string());
    }
    if let Some(error) = state.daemon_lifecycle_error.as_ref() {
        toast_lines.push(format!("daemon: {error}"));
    }

    MainHeaderViewModel {
        status_segments: vec![
            MainStatusSegmentViewModel {
                label: "daemon".to_string(),
                value: format!("{:?}", connection.daemon_phase).to_ascii_lowercase(),
            },
            MainStatusSegmentViewModel {
                label: "upstream".to_string(),
                value: connection.upstream_status,
            },
            MainStatusSegmentViewModel {
                label: "reconnect".to_string(),
                value: connection.reconnect_attempt.to_string(),
            },
            MainStatusSegmentViewModel {
                label: "clients".to_string(),
                value: connection.client_count.to_string(),
            },
            MainStatusSegmentViewModel {
                label: "threads".to_string(),
                value: connection.known_threads.to_string(),
            },
            MainStatusSegmentViewModel {
                label: "turns".to_string(),
                value: if state.prompt_in_flight {
                    "in_flight".to_string()
                } else {
                    state.session.active_turns.len().to_string()
                },
            },
        ],
        program_tabs: vec![
            ProgramTabViewModel {
                program_view: ProgramView::Main,
                label: "Main".to_string(),
                selected: state.main_view.program_view == ProgramView::Main,
                placeholder: false,
            },
            ProgramTabViewModel {
                program_view: ProgramView::Review,
                label: "Review".to_string(),
                selected: state.main_view.program_view == ProgramView::Review,
                placeholder: true,
            },
        ],
        toast_lines,
    }
}

fn hierarchy_rows(state: &AppState) -> Vec<MainHierarchyRowViewModel> {
    let mut rows = Vec::new();
    for workstream in &state.collaboration.workstreams {
        let work_units = state
            .collaboration
            .work_units
            .iter()
            .filter(|work_unit| work_unit.workstream_id == workstream.id)
            .collect::<Vec<_>>();
        rows.push(MainHierarchyRowViewModel {
            kind: HierarchyRowKind::Workstream,
            selection: MainHierarchySelection::Workstream {
                workstream_id: workstream.id.clone(),
            },
            depth: 0,
            label: workstream.title.clone(),
            badges: vec![
                workstream_status_label(workstream.status),
                workstream.priority.clone(),
            ],
            secondary: Some(format!("units={}", work_units.len())),
            selected: state.main_view.selected.as_ref()
                == Some(&MainHierarchySelection::Workstream {
                    workstream_id: workstream.id.clone(),
                }),
            expanded: state
                .main_view
                .expanded_workstreams
                .contains(workstream.id.as_str()),
            collapsible: !work_units.is_empty(),
        });

        if !state
            .main_view
            .expanded_workstreams
            .contains(workstream.id.as_str())
        {
            continue;
        }

        for work_unit in work_units {
            let thread_ids = thread_ids_for_work_unit(state, &work_unit.id);
            rows.push(MainHierarchyRowViewModel {
                kind: HierarchyRowKind::WorkUnit,
                selection: MainHierarchySelection::WorkUnit {
                    workstream_id: workstream.id.clone(),
                    work_unit_id: work_unit.id.clone(),
                },
                depth: 1,
                label: work_unit.title.clone(),
                badges: vec![
                    work_unit_status_label(work_unit.status),
                    assignment_badge(state, work_unit),
                    proposal_badge(work_unit),
                ],
                secondary: Some(format!(
                    "deps={} threads={}",
                    work_unit.dependency_count,
                    thread_ids.len()
                )),
                selected: state.main_view.selected.as_ref()
                    == Some(&MainHierarchySelection::WorkUnit {
                        workstream_id: workstream.id.clone(),
                        work_unit_id: work_unit.id.clone(),
                    }),
                expanded: state
                    .main_view
                    .expanded_work_units
                    .contains(work_unit.id.as_str()),
                collapsible: !thread_ids.is_empty(),
            });

            if !state
                .main_view
                .expanded_work_units
                .contains(work_unit.id.as_str())
            {
                continue;
            }

            for thread_id in thread_ids {
                let thread = state
                    .threads
                    .iter()
                    .find(|thread| thread.id == thread_id)
                    .cloned()
                    .unwrap_or_else(|| placeholder_thread(thread_id.clone()));
                rows.push(MainHierarchyRowViewModel {
                    kind: HierarchyRowKind::Thread,
                    selection: MainHierarchySelection::Thread {
                        workstream_id: workstream.id.clone(),
                        work_unit_id: work_unit.id.clone(),
                        thread_id: thread.id.clone(),
                    },
                    depth: 2,
                    label: thread.name.clone().unwrap_or_else(|| thread.id.clone()),
                    badges: vec![
                        thread_turn_badge(state, &thread),
                        thread_loaded_badge(thread.loaded_status),
                        thread_monitor_badge(thread.monitor_state),
                    ],
                    secondary: Some(abbreviate(&compact_line(&thread.preview), 48)),
                    selected: state.main_view.selected.as_ref()
                        == Some(&MainHierarchySelection::Thread {
                            workstream_id: workstream.id.clone(),
                            work_unit_id: work_unit.id.clone(),
                            thread_id: thread.id.clone(),
                        }),
                    expanded: false,
                    collapsible: false,
                });
            }
        }
    }
    rows
}

fn main_detail_panel(state: &AppState) -> PanelViewModel {
    match state.main_view.program_view {
        ProgramView::Main => match state.main_view.selected.as_ref() {
            Some(MainHierarchySelection::Workstream { .. }) => {
                let detail = workstream_detail(state);
                PanelViewModel {
                    title: detail.title,
                    lines: detail.lines,
                }
            }
            Some(MainHierarchySelection::WorkUnit { .. }) => {
                let detail = collaboration_detail(state);
                PanelViewModel {
                    title: detail.title,
                    lines: detail.lines,
                }
            }
            Some(MainHierarchySelection::Thread { .. }) => thread_summary(state),
            None => PanelViewModel {
                title: "Selection Detail".to_string(),
                lines: vec!["No hierarchy row selected.".to_string()],
            },
        },
        ProgramView::Review => PanelViewModel {
            title: "Review".to_string(),
            lines: vec![
                "Review and decision workflows move into a separate program surface in a later pass."
                    .to_string(),
            ],
        },
    }
}

fn main_footer_prompt(state: &AppState) -> MainFooterPromptViewModel {
    let (title, prompt_lines) = if let Some(compose) = state.steer_compose.as_ref() {
        (
            "Composer".to_string(),
            compose
                .buffer
                .lines()
                .take(4)
                .enumerate()
                .map(|(index, line)| format!("{:>2}: {}", index + 1, abbreviate(line, 72)))
                .collect::<Vec<_>>(),
        )
    } else {
        (
            "Composer".to_string(),
            vec![
                "Prompt submission remains read-only in this pass.".to_string(),
                "This region is reserved for the persistent operator composer.".to_string(),
            ],
        )
    };

    let mut context_lines = Vec::new();
    if let Some(selection) = state.main_view.selected.as_ref() {
        context_lines.push(format!("selection: {}", selection_label(selection)));
    }
    if let Some(thread_id) = state.selected_thread_id.as_ref() {
        context_lines.push(format!("active thread context: {thread_id}"));
    }
    if state.prompt_in_flight {
        context_lines.push("turn status: prompt/turn activity is in flight".to_string());
    } else {
        context_lines.push(format!(
            "turn status: {} active turns",
            state.session.active_turns.len()
        ));
    }

    MainFooterPromptViewModel {
        title,
        prompt_lines,
        context_lines,
        hint_line: "up/down move  left collapse  right expand  tab switch tabs  r refresh  ? help"
            .to_string(),
    }
}

fn selection_label(selection: &MainHierarchySelection) -> String {
    match selection {
        MainHierarchySelection::Workstream { workstream_id } => {
            format!("workstream {workstream_id}")
        }
        MainHierarchySelection::WorkUnit { work_unit_id, .. } => {
            format!("work unit {work_unit_id}")
        }
        MainHierarchySelection::Thread { thread_id, .. } => format!("thread {thread_id}"),
    }
}

fn thread_ids_for_work_unit(state: &AppState, work_unit_id: &str) -> Vec<String> {
    let mut thread_ids = state
        .collaboration
        .codex_thread_assignments
        .iter()
        .filter(|assignment| assignment.work_unit_id == work_unit_id)
        .map(|assignment| assignment.codex_thread_id.clone())
        .collect::<Vec<_>>();
    thread_ids.sort_by(|left, right| {
        thread_updated_at(state, right)
            .cmp(&thread_updated_at(state, left))
            .then_with(|| left.cmp(right))
    });
    thread_ids.dedup();
    thread_ids
}

fn thread_updated_at(state: &AppState, thread_id: &str) -> i64 {
    state
        .threads
        .iter()
        .find(|thread| thread.id == thread_id)
        .map(|thread| thread.updated_at)
        .unwrap_or_default()
}

fn placeholder_thread(thread_id: String) -> ipc::ThreadSummary {
    ipc::ThreadSummary {
        id: thread_id,
        preview: "thread summary unavailable".to_string(),
        name: None,
        model_provider: String::new(),
        cwd: String::new(),
        status: "unknown".to_string(),
        created_at: 0,
        updated_at: 0,
        scope: String::new(),
        archived: false,
        loaded_status: ipc::ThreadLoadedStatus::Unknown,
        active_flags: Vec::new(),
        active_turn_id: None,
        last_seen_turn_id: None,
        recent_output: None,
        recent_event: None,
        turn_in_flight: false,
        monitor_state: ipc::ThreadMonitorState::Detached,
        last_sync_at: chrono::Utc::now(),
        source_kind: None,
        raw_summary: None,
    }
}

fn workstream_status_label(status: WorkstreamStatus) -> String {
    match status {
        WorkstreamStatus::Active => "active".to_string(),
        WorkstreamStatus::Blocked => "blocked".to_string(),
        WorkstreamStatus::Completed => "completed".to_string(),
    }
}

fn work_unit_status_label(status: WorkUnitStatus) -> String {
    match status {
        WorkUnitStatus::Ready => "ready".to_string(),
        WorkUnitStatus::Blocked => "blocked".to_string(),
        WorkUnitStatus::Running => "running".to_string(),
        WorkUnitStatus::AwaitingDecision => "awaiting_decision".to_string(),
        WorkUnitStatus::Accepted => "accepted".to_string(),
        WorkUnitStatus::NeedsHuman => "needs_human".to_string(),
        WorkUnitStatus::Completed => "completed".to_string(),
    }
}

fn assignment_badge(state: &AppState, work_unit: &ipc::WorkUnitSummary) -> String {
    let Some(assignment_id) = work_unit.current_assignment_id.as_ref() else {
        return "unassigned".to_string();
    };
    state
        .collaboration
        .assignments
        .iter()
        .find(|assignment| assignment.id == *assignment_id)
        .map(|assignment| assignment_status_label(assignment.status))
        .unwrap_or_else(|| "assigned".to_string())
}

fn assignment_status_label(status: AssignmentStatus) -> String {
    match status {
        AssignmentStatus::Created => "created".to_string(),
        AssignmentStatus::Running => "running".to_string(),
        AssignmentStatus::AwaitingDecision => "awaiting_decision".to_string(),
        AssignmentStatus::Failed => "failed".to_string(),
        AssignmentStatus::Closed => "closed".to_string(),
        AssignmentStatus::Interrupted => "interrupted".to_string(),
        AssignmentStatus::Lost => "lost".to_string(),
    }
}

fn proposal_badge(work_unit: &ipc::WorkUnitSummary) -> String {
    work_unit
        .proposal
        .as_ref()
        .map(|proposal| proposal_status_label(proposal.latest_status))
        .unwrap_or_else(|| "proposal:none".to_string())
}

fn proposal_status_label(status: SupervisorProposalStatus) -> String {
    match status {
        SupervisorProposalStatus::Open => "proposal:open".to_string(),
        SupervisorProposalStatus::Approved => "proposal:approved".to_string(),
        SupervisorProposalStatus::Rejected => "proposal:rejected".to_string(),
        SupervisorProposalStatus::Superseded => "proposal:superseded".to_string(),
        SupervisorProposalStatus::Stale => "proposal:stale".to_string(),
        SupervisorProposalStatus::GenerationFailed => "proposal:failed".to_string(),
    }
}

fn thread_turn_badge(state: &AppState, thread: &ipc::ThreadSummary) -> String {
    latest_turn_state(state, &thread.id)
        .map(|turn| lifecycle_label(&turn.lifecycle).to_string())
        .unwrap_or_else(|| thread.status.clone())
}

fn thread_loaded_badge(status: ipc::ThreadLoadedStatus) -> String {
    match status {
        ipc::ThreadLoadedStatus::NotLoaded => "not_loaded".to_string(),
        ipc::ThreadLoadedStatus::Idle => "loaded".to_string(),
        ipc::ThreadLoadedStatus::Active => "active".to_string(),
        ipc::ThreadLoadedStatus::SystemError => "system_error".to_string(),
        ipc::ThreadLoadedStatus::Unknown => "unknown".to_string(),
    }
}

fn thread_monitor_badge(status: ipc::ThreadMonitorState) -> String {
    match status {
        ipc::ThreadMonitorState::Detached => "history".to_string(),
        ipc::ThreadMonitorState::Attaching => "attaching".to_string(),
        ipc::ThreadMonitorState::Attached => "attached".to_string(),
        ipc::ThreadMonitorState::Errored => "attach_error".to_string(),
    }
}

fn latest_turn_state<'a>(state: &'a AppState, thread_id: &str) -> Option<&'a ipc::TurnStateView> {
    state
        .turn_states
        .values()
        .filter(|turn| turn.thread_id == thread_id)
        .max_by(|left, right| left.updated_at.cmp(&right.updated_at))
}

#[allow(dead_code)]
fn latest_report_parse_result(state: &AppState, work_unit_id: &str) -> Option<ReportParseResult> {
    state
        .collaboration
        .reports
        .iter()
        .filter(|report| report.work_unit_id == work_unit_id)
        .max_by(|left, right| left.created_at.cmp(&right.created_at))
        .map(|report| report.parse_result)
}

#[allow(dead_code)]
fn thread_assignment_status(
    state: &AppState,
    thread_id: &str,
) -> Option<CodexThreadAssignmentStatus> {
    state
        .collaboration
        .codex_thread_assignments
        .iter()
        .find(|assignment| assignment.codex_thread_id == thread_id)
        .map(|assignment| assignment.status)
}
