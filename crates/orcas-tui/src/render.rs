use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{AppState, BannerLevel, DaemonConnectionPhase};
use crate::view_model;

pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(42), Constraint::Min(20)])
        .split(layout[1]);

    frame.render_widget(render_status(state), layout[0]);
    frame.render_widget(render_threads(state), main[0]);
    frame.render_widget(render_thread_detail(state), main[1]);
    frame.render_widget(render_event_log(state), layout[2]);
    frame.render_widget(render_prompt(state), layout[3]);
}

fn render_status(state: &AppState) -> Paragraph<'static> {
    let status = view_model::connection_status(state);
    let mut lines = vec![
        Line::styled("Orcas TUI", Style::default().add_modifier(Modifier::BOLD)),
        Line::from(format!("socket: {}", status.socket_path)),
        Line::from(format!(
            "daemon: {}  upstream: {}  clients: {}  threads: {}",
            match status.daemon_phase {
                DaemonConnectionPhase::Connected => "connected",
                DaemonConnectionPhase::Reconnecting => "reconnecting",
                DaemonConnectionPhase::Disconnected => "disconnected",
            },
            status.upstream_status,
            status.client_count,
            status.known_threads
        )),
    ];

    if let Some(detail) = status.upstream_detail {
        lines.push(Line::from(format!("detail: {detail}")));
    } else if let Some(banner) = view_model::status_banner(state) {
        let color = match banner.level {
            BannerLevel::Info => Color::Green,
            BannerLevel::Warning => Color::Yellow,
            BannerLevel::Error => Color::Red,
        };
        lines.push(Line::styled(
            banner.message,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    } else if state.show_help {
        lines.push(Line::from(
            "keys: q quit, r refresh, j/k move, i prompt, enter send",
        ));
    } else {
        lines.push(Line::from(
            "keys: q quit, r refresh, j/k move, i prompt, ? help",
        ));
    }

    Paragraph::new(Text::from(lines)).block(Block::default().title("Daemon").borders(Borders::ALL))
}

fn render_threads(state: &AppState) -> Paragraph<'static> {
    let rows = view_model::thread_list(state).rows;
    let lines = if rows.is_empty() {
        vec![Line::from("No threads loaded.")]
    } else {
        rows.into_iter()
            .take(12)
            .map(|row| {
                let prefix = if row.selected { ">" } else { " " };
                let badge = row
                    .turn_badge
                    .as_ref()
                    .map(|badge| format!(" {{{badge}}}"))
                    .unwrap_or_default();
                Line::from(format!(
                    "{prefix} {} [{}{}] {}",
                    row.id, row.status, badge, row.preview
                ))
            })
            .collect()
    };
    Paragraph::new(Text::from(lines))
        .block(Block::default().title("Threads").borders(Borders::ALL))
        .wrap(Wrap { trim: true })
}

fn render_thread_detail(state: &AppState) -> Paragraph<'static> {
    let detail = view_model::thread_detail(state);
    Paragraph::new(Text::from(
        detail.lines.into_iter().map(Line::from).collect::<Vec<_>>(),
    ))
    .block(Block::default().title(detail.title).borders(Borders::ALL))
    .wrap(Wrap { trim: false })
}

fn render_event_log(state: &AppState) -> Paragraph<'static> {
    let lines = view_model::event_log(state).lines;
    let text = if lines.is_empty() {
        vec![Line::from("No events yet.")]
    } else {
        lines
            .into_iter()
            .rev()
            .take(8)
            .rev()
            .map(Line::from)
            .collect()
    };
    Paragraph::new(Text::from(text))
        .block(Block::default().title("Event Log").borders(Borders::ALL))
        .wrap(Wrap { trim: true })
}

fn render_prompt(state: &AppState) -> Paragraph<'static> {
    let prompt = view_model::prompt_box(state);
    let prefix = if prompt.active { "prompt>" } else { "prompt " };
    let suffix = if prompt.in_flight {
        " [waiting]"
    } else if prompt.active {
        " [editing]"
    } else {
        " [press i]"
    };
    Paragraph::new(Text::from(vec![Line::from(format!(
        "{prefix} {}{suffix}",
        prompt.text
    ))]))
    .block(Block::default().title("Prompt").borders(Borders::ALL))
}
