use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{AppState, TopLevelView};
use crate::view_model;
use crate::view_model::shared::{daemon_lifecycle_label, daemon_phase_label};

use super::shared::{
    daemon_phase_to_status_style, heading_style, key_hint_style, key_value_line, label_style,
    metadata_style, status_line, status_style, status_text_style, title_case_view_label,
    value_style,
};

pub(super) fn render_shell_status(state: &AppState, compact: bool) -> Paragraph<'static> {
    let connection = view_model::connection_status(state);
    let mut lines = vec![
        Line::styled(
            format!(
                "Orcas Operator Console [{}]",
                title_case_view_label(state.current_view)
            ),
            heading_style(),
        ),
        status_line(
            "daemon lifecycle",
            daemon_lifecycle_label(state.daemon_lifecycle),
            status_text_style(daemon_lifecycle_label(state.daemon_lifecycle)),
        ),
    ];
    if compact {
        let daemon_phase = daemon_phase_label(connection.daemon_phase);
        lines.push(status_line(
            "daemon phase",
            daemon_phase,
            daemon_phase_to_status_style(daemon_phase),
        ));
        lines.push(Line::from(vec![
            Span::styled("upstream: ", label_style()),
            Span::styled(
                connection.upstream_status.clone(),
                daemon_phase_to_status_style(&connection.upstream_status),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("clients: ", label_style()),
            Span::styled(connection.client_count.to_string(), value_style()),
            Span::styled(" threads: ", label_style()),
            Span::styled(connection.known_threads.to_string(), value_style()),
            Span::styled(" reconnect: ", label_style()),
            Span::styled(connection.reconnect_attempt.to_string(), metadata_style()),
        ]));
    } else {
        lines.push(key_value_line(
            "daemon",
            daemon_phase_label(connection.daemon_phase),
        ));
        lines.push(status_line(
            "upstream",
            &connection.upstream_status,
            daemon_phase_to_status_style(&connection.upstream_status),
        ));
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "clients: {}  threads: {}",
                    connection.client_count, connection.known_threads
                ),
                label_style(),
            ),
            Span::styled(
                format!("  reconnect: {}", connection.reconnect_attempt),
                metadata_style(),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("socket: ", label_style()),
            Span::styled(connection.socket_path.clone(), metadata_style()),
        ]));
    }

    if let Some(detail) = connection.upstream_detail {
        lines.push(Line::styled(
            format!("upstream detail: {detail}"),
            metadata_style(),
        ));
    } else if let Some(banner) = view_model::status_banner(state) {
        let color = status_style(banner.level);
        lines.push(Line::styled(banner.message, color));
        if let Some(lifecycle_error) = state.daemon_lifecycle_error.as_deref() {
            lines.push(Line::styled(
                format!("daemon: {lifecycle_error}"),
                metadata_style(),
            ));
        }
    } else {
        lines.push(Line::styled(selection_summary(state), metadata_style()));
        if let Some(error) = state.daemon_lifecycle_error.as_deref() {
            lines.push(Line::styled(format!("daemon: {error}"), metadata_style()));
        }
    }

    Paragraph::new(Text::from(lines)).block(Block::default().title("Shell").borders(Borders::ALL))
}

pub(super) fn render_footer(state: &AppState, compact: bool) -> Paragraph<'static> {
    let mut lines = Vec::new();
    if state.show_help {
        if compact {
            let mut help_line = String::from("views: 1/2/3/4  left/right cycle");
            if state.current_view == TopLevelView::Collaboration {
                help_line.push_str("  tab focus");
            }
            help_line.push_str("  ? help  q quit");
            lines.push(Line::styled(help_line, metadata_style()));
            lines.push(Line::styled(
                help_navigation_line_compact(state.current_view),
                metadata_style(),
            ));
            if let Some(error) = state.daemon_lifecycle_error.as_deref() {
                lines.push(Line::styled(format!("daemon: {error}"), metadata_style()));
            }
        } else {
            lines.push(Line::styled(
                "views: 1 overview  2 threads  3 collaboration  4 supervisor  left/right cycle",
                metadata_style(),
            ));
            lines.push(Line::styled(
                help_navigation_line(state.current_view),
                metadata_style(),
            ));
        }
    } else {
        let mut spans = vec![
            Span::styled("keys: ", label_style()),
            Span::styled("1/2/3/4", key_hint_style()),
            Span::styled(" views  ", metadata_style()),
            Span::styled("left/right", key_hint_style()),
            Span::styled(" cycle  ", metadata_style()),
        ];
        if state.current_view == TopLevelView::Collaboration {
            spans.push(Span::styled("tab", key_hint_style()));
            spans.push(Span::styled(" focus  ", metadata_style()));
        }
        spans.extend(key_bindings_hint(state));
        spans.push(Span::styled(" ", metadata_style()));
        spans.push(Span::styled("? help", key_hint_style()));
        spans.push(Span::styled("  ", metadata_style()));
        spans.push(Span::styled("q quit", key_hint_style()));
        lines.push(Line::from(spans));
        lines.push(Line::from(help_navigation_line(state.current_view)));
        if let Some(error) = state.daemon_lifecycle_error.as_deref() {
            lines.push(Line::styled(format!("daemon: {error}"), metadata_style()));
        }
        lines.push(Line::from(vec![
            Span::styled("focus: ", label_style()),
            Span::styled(title_case_view_label(state.current_view), value_style()),
        ]));
    }

    if compact && lines.len() > 3 {
        lines.truncate(3);
    }

    Paragraph::new(Text::from(lines))
        .block(Block::default().title("Keys").borders(Borders::ALL))
        .wrap(Wrap { trim: true })
}

fn selection_summary(state: &AppState) -> String {
    match state.current_view {
        TopLevelView::Overview => format!(
            "selected thread={}  selected stream={}  selected unit={}",
            state.selected_thread_id.as_deref().unwrap_or("-"),
            state.selected_workstream_id.as_deref().unwrap_or("-"),
            state.selected_work_unit_id.as_deref().unwrap_or("-")
        ),
        TopLevelView::Threads => format!(
            "selected thread={}  recent events={}",
            state.selected_thread_id.as_deref().unwrap_or("-"),
            state.recent_events.len()
        ),
        TopLevelView::Collaboration => format!(
            "collaboration focus={}  selected stream={}  selected unit={}",
            view_model::collaboration_focus_label(state.collaboration_focus),
            state.selected_workstream_id.as_deref().unwrap_or("-"),
            state.selected_work_unit_id.as_deref().unwrap_or("-")
        ),
        TopLevelView::Supervisor => format!(
            "models={}  selected_thread={}",
            state.daemon_models.len(),
            state.selected_thread_id.as_deref().unwrap_or("-"),
        ),
    }
}

fn help_navigation_line(view: TopLevelView) -> &'static str {
    match view {
        TopLevelView::Overview => "nav: left/right views  r refresh  ? help  q quit",
        TopLevelView::Threads => {
            "nav: left/right views  up/down thread selection  s compose steer  e edit steer  i propose interrupt  a approve/send  d reject  r refresh  ? help  q quit"
        }
        TopLevelView::Collaboration => {
            "nav: left/right views  tab switch workstreams/work_units  up/down move selection  r refresh  ? help  q quit"
        }
        TopLevelView::Supervisor => {
            "nav: left/right views  m reload models  s start daemon  x request daemon stop  R restart daemon  r refresh  ? help  q quit"
        }
    }
}

fn help_navigation_line_compact(view: TopLevelView) -> &'static str {
    match view {
        TopLevelView::Overview => "nav: left/right  r",
        TopLevelView::Threads => "nav: left/right  up/down  s/e/i/a/d  r",
        TopLevelView::Collaboration => "nav: left/right  tab focus  up/down  r",
        TopLevelView::Supervisor => "nav: left/right  m/s/x/R  r",
    }
}

fn key_bindings_hint(state: &AppState) -> Vec<Span<'static>> {
    match state.current_view {
        TopLevelView::Overview => action_hint("r", "refresh"),
        TopLevelView::Threads => {
            if state.steer_compose.is_some() {
                let mut spans = action_hint("type", "edit steer text");
                spans.push(Span::styled("  ", metadata_style()));
                spans.extend(action_hint("enter", "newline"));
                spans.push(Span::styled("  ", metadata_style()));
                spans.extend(action_hint("ctrl+s", "save steer"));
                spans.push(Span::styled("  ", metadata_style()));
                spans.extend(action_hint("arrows", "move cursor"));
                spans.push(Span::styled("  ", metadata_style()));
                spans.extend(action_hint("esc", "cancel"));
                spans.push(Span::styled("  ", metadata_style()));
                spans.extend(action_hint("backspace/del", "delete"));
                return spans;
            }
            let mut spans = action_hint("up/down", "thread selection");
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("s", "compose steer"));
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("e", "edit steer"));
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("i", "propose interrupt"));
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("a", "approve/send"));
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("d", "reject"));
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("r", "refresh"));
            spans
        }
        TopLevelView::Collaboration => {
            let mut spans = action_hint("tab", "switch workstreams/work_units");
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("up/down", "selection"));
            spans
        }
        TopLevelView::Supervisor => {
            let mut spans = action_hint("m", "refresh models");
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("s", "start daemon"));
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("x", "stop daemon"));
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("R", "restart daemon"));
            spans.push(Span::styled("  ", metadata_style()));
            spans.extend(action_hint("r", "refresh"));
            spans
        }
    }
}

fn action_hint(key: &str, action: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!("{key}"), key_hint_style()),
        Span::styled(format!(" {action}"), label_style()),
    ]
}
