use crate::app::{
    AppState, DeleteFooterState, FooterFieldState, MainFooterState, MainHierarchySelection,
    ProgramView, TrackedThreadFooterForm, WorkUnitFooterForm, WorkstreamFooterForm,
};
use crate::view_model::{PanelViewModel, connection_status, event_log, status_banner};
use orcas_core::{ReportParseResult, WorkUnitStatus, WorkstreamStatus, authority};

use super::shared::{abbreviate, compact_line};

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
                placeholder: false,
            },
        ],
        toast_lines,
    }
}

fn hierarchy_rows(state: &AppState) -> Vec<MainHierarchyRowViewModel> {
    let mut rows = Vec::new();
    for workstream in &state.authority_main.hierarchy.workstreams {
        let workstream_id = workstream.workstream.id.to_string();
        rows.push(MainHierarchyRowViewModel {
            kind: HierarchyRowKind::Workstream,
            selection: MainHierarchySelection::Workstream {
                workstream_id: workstream_id.clone(),
            },
            depth: 0,
            label: workstream.workstream.title.clone(),
            badges: vec![
                workstream_status_label(workstream.workstream.status),
                workstream.workstream.priority.clone(),
            ],
            secondary: Some(format!("units={}", workstream.work_units.len())),
            selected: state.main_view.selected.as_ref()
                == Some(&MainHierarchySelection::Workstream {
                    workstream_id: workstream_id.clone(),
                }),
            expanded: state
                .main_view
                .expanded_workstreams
                .contains(workstream_id.as_str()),
            collapsible: !workstream.work_units.is_empty(),
        });

        if !state
            .main_view
            .expanded_workstreams
            .contains(workstream_id.as_str())
        {
            continue;
        }

        for work_unit in &workstream.work_units {
            let work_unit_id = work_unit.work_unit.id.to_string();
            rows.push(MainHierarchyRowViewModel {
                kind: HierarchyRowKind::WorkUnit,
                selection: MainHierarchySelection::WorkUnit {
                    workstream_id: workstream_id.clone(),
                    work_unit_id: work_unit_id.clone(),
                },
                depth: 1,
                label: work_unit.work_unit.title.clone(),
                badges: vec![
                    work_unit_status_label(work_unit.work_unit.status),
                    format!("threads={}", work_unit.tracked_threads.len()),
                ],
                secondary: None,
                selected: state.main_view.selected.as_ref()
                    == Some(&MainHierarchySelection::WorkUnit {
                        workstream_id: workstream_id.clone(),
                        work_unit_id: work_unit_id.clone(),
                    }),
                expanded: state
                    .main_view
                    .expanded_work_units
                    .contains(work_unit_id.as_str()),
                collapsible: !work_unit.tracked_threads.is_empty(),
            });

            if !state
                .main_view
                .expanded_work_units
                .contains(work_unit_id.as_str())
            {
                continue;
            }

            for tracked_thread in &work_unit.tracked_threads {
                let upstream = tracked_thread
                    .upstream_thread_id
                    .as_deref()
                    .and_then(|thread_id| {
                        state.threads.iter().find(|thread| thread.id == thread_id)
                    });
                rows.push(MainHierarchyRowViewModel {
                    kind: HierarchyRowKind::Thread,
                    selection: MainHierarchySelection::Thread {
                        workstream_id: workstream_id.clone(),
                        work_unit_id: work_unit_id.clone(),
                        thread_id: tracked_thread.id.to_string(),
                    },
                    depth: 2,
                    label: tracked_thread.title.clone(),
                    badges: vec![
                        tracked_thread_binding_label(tracked_thread.binding_state),
                        tracked_thread_backend_label(tracked_thread.backend_kind),
                        tracked_thread
                            .workspace_status
                            .map(tracked_thread_workspace_status_label)
                            .unwrap_or_else(|| "workspace=none".to_string()),
                    ],
                    secondary: Some(
                        upstream
                            .map(|thread| abbreviate(&compact_line(&thread.preview), 48))
                            .unwrap_or_else(|| {
                                tracked_thread
                                    .upstream_thread_id
                                    .as_ref()
                                    .map(|thread_id| format!("upstream {thread_id}"))
                                    .unwrap_or_else(|| "local only".to_string())
                            }),
                    ),
                    selected: state.main_view.selected.as_ref()
                        == Some(&MainHierarchySelection::Thread {
                            workstream_id: workstream_id.clone(),
                            work_unit_id: work_unit_id.clone(),
                            thread_id: tracked_thread.id.to_string(),
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
    match state.main_view.selected.as_ref() {
        Some(MainHierarchySelection::Workstream { workstream_id }) => {
            if let Some(detail) = state.authority_main.workstream_details.get(workstream_id) {
                PanelViewModel {
                    title: format!("Workstream {}", detail.workstream.id),
                    lines: vec![
                        format!("title: {}", detail.workstream.title),
                        format!("root: {}", detail.workstream.objective),
                        format!(
                            "status: {}  priority: {}",
                            workstream_status_label(detail.workstream.status),
                            detail.workstream.priority
                        ),
                        format!("revision: {}", detail.workstream.revision.get()),
                        format!("work units: {}", detail.work_units.len()),
                    ],
                }
            } else {
                PanelViewModel {
                    title: "Workstream Detail".to_string(),
                    lines: vec!["Loading workstream detail…".to_string()],
                }
            }
        }
        Some(MainHierarchySelection::WorkUnit { work_unit_id, .. }) => {
            if let Some(detail) = state.authority_main.work_unit_details.get(work_unit_id) {
                PanelViewModel {
                    title: format!("Work Unit {}", detail.work_unit.id),
                    lines: vec![
                        format!("title: {}", detail.work_unit.title),
                        format!(
                            "status: {}  tracked threads: {}",
                            work_unit_status_label(detail.work_unit.status),
                            detail.tracked_threads.len()
                        ),
                        format!("task: {}", abbreviate(&detail.work_unit.task_statement, 88)),
                        format!("revision: {}", detail.work_unit.revision.get()),
                    ],
                }
            } else {
                PanelViewModel {
                    title: "Work Unit Detail".to_string(),
                    lines: vec!["Loading work unit detail…".to_string()],
                }
            }
        }
        Some(MainHierarchySelection::Thread { thread_id, .. }) => {
            if let Some(detail) = state.authority_main.tracked_thread_details.get(thread_id) {
                let tracked_thread = &detail.tracked_thread;
                let mut lines = vec![
                    format!("title: {}", tracked_thread.title),
                    format!(
                        "root: {}",
                        tracked_thread
                            .preferred_cwd
                            .clone()
                            .unwrap_or_else(|| "unset".to_string())
                    ),
                    format!(
                        "binding: {}  backend: {}",
                        tracked_thread_binding_label(tracked_thread.binding_state),
                        tracked_thread_backend_label(tracked_thread.backend_kind)
                    ),
                    format!("revision: {}", tracked_thread.revision.get()),
                ];
                if let Some(upstream_thread_id) = tracked_thread.upstream_thread_id.as_ref() {
                    lines.push(format!("upstream thread: {upstream_thread_id}"));
                } else {
                    lines.push("upstream thread: none".to_string());
                }
                if let Some(workspace) = tracked_thread.workspace.as_ref() {
                    lines.push(format!(
                        "workspace: {}  strategy: {}  status: {}",
                        workspace.worktree_path,
                        tracked_thread_workspace_strategy_label(workspace.strategy),
                        tracked_thread_workspace_status_label(workspace.status)
                    ));
                    lines.push(format!(
                        "branch: {}  base: {}  landing: {}",
                        workspace.branch_name, workspace.base_ref, workspace.landing_target
                    ));
                    lines.push(format!(
                        "last reported head: {}",
                        workspace
                            .last_reported_head_commit
                            .as_deref()
                            .unwrap_or("unset")
                    ));
                } else {
                    lines.push("workspace: none".to_string());
                }
                lines.push("delete semantics: local only".to_string());
                PanelViewModel {
                    title: format!("Tracked Thread {}", tracked_thread.id),
                    lines,
                }
            } else {
                PanelViewModel {
                    title: "Tracked Thread Detail".to_string(),
                    lines: vec!["Loading tracked thread detail…".to_string()],
                }
            }
        }
        None => PanelViewModel {
            title: "Selection Detail".to_string(),
            lines: vec!["No hierarchy row selected.".to_string()],
        },
    }
}

fn main_footer_prompt(state: &AppState) -> MainFooterPromptViewModel {
    let mut context_lines = Vec::new();
    if let Some(selection) = state.main_view.selected.as_ref() {
        context_lines.push(format!("selection: {}", selection_label(selection)));
    } else {
        context_lines.push("selection: none".to_string());
    }
    context_lines.push("backend: local authority / state.db".to_string());

    match &state.authority_main.footer {
        MainFooterState::Inspect => MainFooterPromptViewModel {
            title: "Composer".to_string(),
            prompt_lines: vec![
                "mode: Inspect".to_string(),
                format!("actions: {}", inspect_actions_label(state)),
            ],
            context_lines,
            hint_line: inspect_hint_line(state),
        },
        MainFooterState::CreateWorkstream(form) => main_workstream_footer(
            "mode: CreateWorkstream",
            form,
            context_lines,
            "tab next field  ctrl+s submit  esc cancel",
        ),
        MainFooterState::EditWorkstream(form) => main_workstream_footer(
            "mode: EditWorkstream",
            form,
            context_lines,
            "tab next field  ctrl+s submit  esc cancel",
        ),
        MainFooterState::CreateWorkUnit(form) => main_workunit_footer(
            "mode: CreateWorkUnit",
            form,
            context_lines,
            "tab next field  ctrl+s submit  esc cancel",
        ),
        MainFooterState::EditWorkUnit(form) => main_workunit_footer(
            "mode: EditWorkUnit",
            form,
            context_lines,
            "tab next field  ctrl+s submit  esc cancel",
        ),
        MainFooterState::CreateTrackedThread(form) => main_tracked_thread_footer(
            "mode: CreateTrackedThread",
            form,
            context_lines,
            "tab next field  ctrl+s submit  esc cancel",
        ),
        MainFooterState::EditTrackedThread(form) => main_tracked_thread_footer(
            "mode: EditTrackedThread",
            form,
            context_lines,
            "tab next field  ctrl+s submit  esc cancel",
        ),
        MainFooterState::ConfirmDelete(delete) => main_delete_footer(delete, context_lines),
    }
}

fn main_workstream_footer(
    mode_label: &str,
    form: &WorkstreamFooterForm,
    mut context_lines: Vec<String>,
    hint_line: &str,
) -> MainFooterPromptViewModel {
    context_lines.push("workstream schema stores root in the local objective field".to_string());
    MainFooterPromptViewModel {
        title: "Composer".to_string(),
        prompt_lines: vec![
            mode_label.to_string(),
            render_footer_field("title", &form.title, form.active_field == 0),
            render_footer_field("root", &form.root_dir, form.active_field == 1),
        ],
        context_lines,
        hint_line: hint_line.to_string(),
    }
}

fn main_workunit_footer(
    mode_label: &str,
    form: &WorkUnitFooterForm,
    mut context_lines: Vec<String>,
    hint_line: &str,
) -> MainFooterPromptViewModel {
    context_lines.push(format!("parent workstream: {}", form.workstream_id));
    MainFooterPromptViewModel {
        title: "Composer".to_string(),
        prompt_lines: vec![
            mode_label.to_string(),
            render_footer_field("title", &form.title, form.active_field == 0),
        ],
        context_lines,
        hint_line: hint_line.to_string(),
    }
}

fn main_tracked_thread_footer(
    mode_label: &str,
    form: &TrackedThreadFooterForm,
    mut context_lines: Vec<String>,
    hint_line: &str,
) -> MainFooterPromptViewModel {
    context_lines.push(format!("parent work unit: {}", form.work_unit_id));
    context_lines.push("tracked_thread is a local Orcas record".to_string());
    MainFooterPromptViewModel {
        title: "Composer".to_string(),
        prompt_lines: vec![
            mode_label.to_string(),
            render_footer_field("name", &form.title, form.active_field == 0),
            render_footer_field("root", &form.root_dir, form.active_field == 1),
        ],
        context_lines,
        hint_line: hint_line.to_string(),
    }
}

fn main_delete_footer(
    delete: &DeleteFooterState,
    mut context_lines: Vec<String>,
) -> MainFooterPromptViewModel {
    context_lines.push(format!(
        "impact: {} work units, {} tracked threads",
        delete.affected_work_units, delete.affected_tracked_threads
    ));
    if delete.has_upstream_bindings {
        context_lines.push("upstream bindings: present; delete remains local-only".to_string());
    }
    let mut prompt_lines = vec![
        "mode: ConfirmDelete".to_string(),
        format!("delete `{}`", delete.label),
    ];
    if delete.requires_typed_confirmation {
        prompt_lines.push(format!("type `{}` to confirm", delete.label));
        prompt_lines.push(render_footer_field(
            "confirm",
            &delete.typed_confirmation,
            delete.active_field == 0,
        ));
    } else {
        prompt_lines.push("press Ctrl+S to confirm delete".to_string());
    }
    MainFooterPromptViewModel {
        title: "Composer".to_string(),
        prompt_lines,
        context_lines,
        hint_line: "ctrl+s confirm delete  esc cancel".to_string(),
    }
}

fn render_footer_field(label: &str, field: &FooterFieldState, active: bool) -> String {
    let value_with_cursor = if active {
        let (head, tail) = field.value.split_at(field.cursor);
        format!("{head}|{tail}")
    } else {
        field.value.clone()
    };
    format!(
        "{} {}: {}",
        if active { ">" } else { " " },
        label,
        if value_with_cursor.is_empty() {
            "…".to_string()
        } else {
            value_with_cursor
        }
    )
}

fn inspect_actions_label(state: &AppState) -> String {
    let mut actions = vec!["n new workstream".to_string()];
    match state.main_view.selected.as_ref() {
        Some(MainHierarchySelection::Workstream { .. }) => {
            actions.push("u new work unit".to_string());
            actions.push("e edit".to_string());
            actions.push("d delete".to_string());
        }
        Some(MainHierarchySelection::WorkUnit { .. }) => {
            actions.push("t new tracked thread".to_string());
            actions.push("e edit".to_string());
            actions.push("d delete".to_string());
        }
        Some(MainHierarchySelection::Thread { .. }) => {
            actions.push("e edit".to_string());
            actions.push("d delete".to_string());
        }
        None => {}
    }
    actions.join("  ")
}

fn inspect_hint_line(state: &AppState) -> String {
    let mut hints = vec!["n new".to_string()];
    match state.main_view.selected.as_ref() {
        Some(MainHierarchySelection::Workstream { .. }) => {
            hints.push("u unit".to_string());
            hints.push("e edit".to_string());
            hints.push("d delete".to_string());
        }
        Some(MainHierarchySelection::WorkUnit { .. }) => {
            hints.push("t tracked-thread".to_string());
            hints.push("e edit".to_string());
            hints.push("d delete".to_string());
        }
        Some(MainHierarchySelection::Thread { .. }) => {
            hints.push("e edit".to_string());
            hints.push("d delete".to_string());
        }
        None => {}
    }
    hints.push("up/down move".to_string());
    hints.push("left collapse".to_string());
    hints.push("right expand".to_string());
    hints.push("tab switch tabs".to_string());
    hints.push("r refresh".to_string());
    hints.join("  ")
}

fn selection_label(selection: &MainHierarchySelection) -> String {
    match selection {
        MainHierarchySelection::Workstream { workstream_id } => {
            format!("workstream {workstream_id}")
        }
        MainHierarchySelection::WorkUnit { work_unit_id, .. } => {
            format!("work unit {work_unit_id}")
        }
        MainHierarchySelection::Thread { thread_id, .. } => {
            format!("tracked thread {thread_id}")
        }
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

fn tracked_thread_binding_label(binding: authority::TrackedThreadBindingState) -> String {
    match binding {
        authority::TrackedThreadBindingState::Unbound => "unbound".to_string(),
        authority::TrackedThreadBindingState::Bound => "bound".to_string(),
        authority::TrackedThreadBindingState::Detached => "detached".to_string(),
        authority::TrackedThreadBindingState::Missing => "missing".to_string(),
    }
}

fn tracked_thread_backend_label(backend: authority::TrackedThreadBackendKind) -> String {
    match backend {
        authority::TrackedThreadBackendKind::Codex => "codex".to_string(),
    }
}

fn tracked_thread_workspace_strategy_label(
    strategy: authority::TrackedThreadWorkspaceStrategy,
) -> String {
    match strategy {
        authority::TrackedThreadWorkspaceStrategy::Shared => "shared".to_string(),
        authority::TrackedThreadWorkspaceStrategy::DedicatedThreadWorktree => {
            "dedicated_thread_worktree".to_string()
        }
        authority::TrackedThreadWorkspaceStrategy::Ephemeral => "ephemeral".to_string(),
    }
}

fn tracked_thread_workspace_status_label(
    status: authority::TrackedThreadWorkspaceStatus,
) -> String {
    match status {
        authority::TrackedThreadWorkspaceStatus::Requested => "workspace=requested".to_string(),
        authority::TrackedThreadWorkspaceStatus::Ready => "workspace=ready".to_string(),
        authority::TrackedThreadWorkspaceStatus::Dirty => "workspace=dirty".to_string(),
        authority::TrackedThreadWorkspaceStatus::Ahead => "workspace=ahead".to_string(),
        authority::TrackedThreadWorkspaceStatus::Behind => "workspace=behind".to_string(),
        authority::TrackedThreadWorkspaceStatus::Conflicted => "workspace=conflicted".to_string(),
        authority::TrackedThreadWorkspaceStatus::Merged => "workspace=merged".to_string(),
        authority::TrackedThreadWorkspaceStatus::Abandoned => "workspace=abandoned".to_string(),
        authority::TrackedThreadWorkspaceStatus::Pruned => "workspace=pruned".to_string(),
    }
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
