use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{AppState, TopLevelView};
use crate::view_model;

use super::shared::{
    emphasis_style, focus_block_style, metadata_style, render_panel_with_focus, row_style,
    selection_marker, status_text_style,
};

pub(super) fn render_view(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    let compact = area.width < 120 || area.height < 26;
    let threads = view_model::threads_view(state);
    let list_has_focus = state.current_view == TopLevelView::Threads;
    if compact {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Min(8),
            ])
            .split(area);
        frame.render_widget(render_thread_list(threads.list, list_has_focus), layout[0]);
        frame.render_widget(
            render_panel_with_focus(threads.summary, true, true),
            layout[1],
        );
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
        frame.render_widget(render_thread_list(threads.list, list_has_focus), columns[0]);
        frame.render_widget(
            render_panel_with_focus(threads.summary, true, true),
            right[0],
        );
        frame.render_widget(render_thread_detail(threads.detail), right[1]);
    }
}

fn render_thread_list(
    list: view_model::ThreadListViewModel,
    list_has_focus: bool,
) -> Paragraph<'static> {
    let lines = if list.rows.is_empty() {
        vec![Line::styled("No threads loaded.", metadata_style())]
    } else {
        list.rows
            .into_iter()
            .take(14)
            .map(|row| {
                let marker = selection_marker(row.selected, list_has_focus);
                let status_style = status_text_style(&row.status);
                let badge = row
                    .turn_badge
                    .as_ref()
                    .map(|badge| format!(" turn={badge}"))
                    .unwrap_or_default();
                Line::from(vec![
                    Span::styled(format!("{marker}"), row_style(row.selected, list_has_focus)),
                    Span::styled(row.id, row_style(row.selected, list_has_focus)),
                    Span::styled(format!(" "), row_style(row.selected, list_has_focus)),
                    Span::styled(format!("[{}]", row.status), status_style),
                    Span::styled(format!(" "), metadata_style()),
                    Span::styled(badge, metadata_style()),
                    Span::styled(format!(" {}", row.preview), metadata_style()),
                ])
            })
            .collect()
    };

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title("Threads")
                .borders(Borders::ALL)
                .border_style(focus_block_style(list_has_focus)),
        )
        .wrap(Wrap { trim: true })
}

fn render_thread_detail(detail: view_model::ThreadDetailViewModel) -> Paragraph<'static> {
    Paragraph::new(Text::from(
        std::iter::once(Line::styled(format!("{} ", detail.title), emphasis_style()))
            .chain(
                detail
                    .lines
                    .into_iter()
                    .map(|line| Line::styled(format!("  {line}"), metadata_style())),
            )
            .collect::<Vec<_>>(),
    ))
    .block(
        Block::default()
            .title(detail.title)
            .borders(Borders::ALL)
            .border_style(focus_block_style(false)),
    )
    .wrap(Wrap { trim: false })
}
