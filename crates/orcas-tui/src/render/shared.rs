use crate::app::{BannerLevel, DaemonLifecycleState, TopLevelView};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::view_model::PanelViewModel;

pub(super) fn render_panel(panel: PanelViewModel, trim: bool) -> Paragraph<'static> {
    render_panel_with_focus(panel, trim, true)
}

pub(super) fn render_panel_with_focus(
    panel: PanelViewModel,
    trim: bool,
    focused: bool,
) -> Paragraph<'static> {
    Paragraph::new(Text::from(
        panel.lines.into_iter().map(panel_line).collect::<Vec<_>>(),
    ))
    .block(
        Block::default()
            .title(Line::styled(panel.title, panel_title_style(focused)))
            .borders(Borders::ALL)
            .border_style(focus_block_style(focused)),
    )
    .wrap(Wrap { trim })
}

fn panel_line(raw_line: String) -> Line<'static> {
    if let Some((label, value)) = raw_line.split_once(": ") {
        let mut spans = Vec::new();
        spans.push(Span::styled(format!("{label}: "), label_style()));
        spans.push(Span::styled(value.to_string(), value_style()));
        Line::from(spans)
    } else {
        Line::styled(raw_line, muted_style())
    }
}

pub(super) fn focus_title(base: &str, focused: bool) -> String {
    if focused {
        format!("{base} <focus>")
    } else {
        base.to_string()
    }
}

pub(super) fn focus_block_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

pub(super) fn panel_title_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    }
}

pub(super) fn heading_style() -> Style {
    Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn status_label_style() -> Style {
    Style::default().fg(Color::Gray)
}

pub(super) fn selection_marker(selected: bool, _list_focused: bool) -> &'static str {
    if selected {
        ">"
    } else {
        " "
    }
}

pub(super) fn row_style(selected: bool, list_has_focus: bool) -> Style {
    if selected {
        if list_has_focus {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM)
        }
    } else if list_has_focus {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

pub(super) fn metadata_style() -> Style {
    muted_style()
}

pub(super) fn muted_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub(super) fn label_style() -> Style {
    Style::default().fg(Color::Gray)
}

pub(super) fn value_style() -> Style {
    Style::default().fg(Color::White)
}

pub(super) fn emphasis_style() -> Style {
    Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn key_hint_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn status_style(level: BannerLevel) -> Style {
    match level {
        BannerLevel::Info => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        BannerLevel::Warning => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        BannerLevel::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

pub(super) fn lifecycle_style(state: DaemonLifecycleState) -> Style {
    match state {
        DaemonLifecycleState::Running => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        DaemonLifecycleState::Starting | DaemonLifecycleState::Restarting => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        DaemonLifecycleState::Stopping => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        DaemonLifecycleState::Failed => {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        }
        DaemonLifecycleState::Stopped | DaemonLifecycleState::Unknown => {
            Style::default().fg(Color::DarkGray)
        }
    }
}

pub(super) fn status_text_style(status: &str) -> Style {
    let status = status.to_ascii_lowercase();
    match status.as_str() {
        "running" | "active" | "completed" | "success" => Style::default().fg(Color::Green),
        "starting" | "stopping" | "restarting" | "working" | "submitted" | "pending"
        | "reconnecting" => Style::default().fg(Color::Yellow),
        "failed" | "error" | "interrupted" | "lost" | "cancelled" => {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        }
        "stopped" | "idle" | "closed" | "disconnected" => Style::default().fg(Color::DarkGray),
        "connected" => Style::default().fg(Color::Green),
        _ => Style::default().fg(Color::Gray),
    }
}

pub(super) fn status_line(label: &str, status: &str, status_color: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), label_style()),
        Span::styled(format!("{status}"), status_color),
    ])
}

pub(super) fn key_value_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), label_style()),
        Span::styled(value.to_string(), value_style()),
    ])
}

pub(super) fn title_case_view_label(view: TopLevelView) -> &'static str {
    match view {
        TopLevelView::Overview => "Overview",
        TopLevelView::Threads => "Threads",
        TopLevelView::Collaboration => "Collaboration",
        TopLevelView::Supervisor => "Supervisor",
    }
}

pub(super) fn daemon_phase_to_status_style(status: &str) -> Style {
    status_text_style(status)
}
