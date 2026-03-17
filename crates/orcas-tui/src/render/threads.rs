use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::AppState;
use crate::view_model;

use super::shared::render_panel;

pub(super) fn render_view(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    let compact = area.width < 120 || area.height < 26;
    let threads = view_model::threads_view(state);
    if compact {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Min(8),
            ])
            .split(area);
        frame.render_widget(render_thread_list(threads.list), layout[0]);
        frame.render_widget(render_panel(threads.summary, true), layout[1]);
        frame.render_widget(render_thread_detail(threads.detail), layout[2]);
    } else {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
            .split(area);
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(9), Constraint::Min(10)])
            .split(columns[1]);
        frame.render_widget(render_thread_list(threads.list), columns[0]);
        frame.render_widget(render_panel(threads.summary, true), right[0]);
        frame.render_widget(render_thread_detail(threads.detail), right[1]);
    }
}

fn render_thread_list(list: view_model::ThreadListViewModel) -> Paragraph<'static> {
    let lines = if list.rows.is_empty() {
        vec![Line::from("No threads loaded.")]
    } else {
        list.rows
            .into_iter()
            .take(14)
            .map(|row| {
                let prefix = if row.selected { ">" } else { " " };
                let badge = row
                    .turn_badge
                    .as_ref()
                    .map(|badge| format!(" turn={badge}"))
                    .unwrap_or_default();
                Line::from(format!(
                    "{prefix} {} [{}]{} {}",
                    row.id, row.status, badge, row.preview
                ))
            })
            .collect()
    };

    Paragraph::new(Text::from(lines))
        .block(Block::default().title("Threads").borders(Borders::ALL))
        .wrap(Wrap { trim: true })
}

fn render_thread_detail(detail: view_model::ThreadDetailViewModel) -> Paragraph<'static> {
    Paragraph::new(Text::from(
        detail.lines.into_iter().map(Line::from).collect::<Vec<_>>(),
    ))
    .block(Block::default().title(detail.title).borders(Borders::ALL))
    .wrap(Wrap { trim: false })
}
