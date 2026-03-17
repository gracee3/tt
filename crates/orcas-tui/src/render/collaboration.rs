use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{AppState, CollaborationFocus};
use crate::view_model;

use super::shared::{
    emphasis_style, focus_block_style, focus_title, key_value_line, label_style, metadata_style,
    render_panel_with_focus, row_style, selection_marker, status_label_style, status_text_style,
};

pub(super) fn render_view(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    let compact = area.width < 138 || area.height < 30;
    let collaboration = view_model::collaboration_view(state);
    let header_height = if compact { 5 } else { 4 };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_height), Constraint::Min(12)])
        .split(area);

    frame.render_widget(
        render_collaboration_status(collaboration.status.clone()),
        layout[0],
    );

    if compact {
        let body = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(7),
                Constraint::Length(5),
                Constraint::Length(8),
                Constraint::Min(10),
            ])
            .split(layout[1]);
        let detail = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(8), Constraint::Min(8)])
            .split(body[3]);
        frame.render_widget(
            render_workstreams(
                collaboration.workstreams,
                state.collaboration_focus == CollaborationFocus::Workstreams,
            ),
            body[0],
        );
        frame.render_widget(
            render_panel_with_focus(
                view_model::PanelViewModel {
                    title: collaboration.workstream_detail.title,
                    lines: collaboration.workstream_detail.lines,
                },
                true,
                true,
            ),
            body[1],
        );
        frame.render_widget(
            render_work_units(
                collaboration.work_units,
                state.collaboration_focus == CollaborationFocus::WorkUnits,
            ),
            body[2],
        );
        frame.render_widget(render_collaboration_detail(collaboration.detail), detail[0]);
        frame.render_widget(
            render_collaboration_history(collaboration.history),
            detail[1],
        );
    } else {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(28),
                Constraint::Percentage(30),
                Constraint::Percentage(42),
            ])
            .split(layout[1]);
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(9), Constraint::Min(5)])
            .split(columns[0]);
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(10), Constraint::Min(10)])
            .split(columns[2]);
        frame.render_widget(
            render_workstreams(
                collaboration.workstreams,
                state.collaboration_focus == CollaborationFocus::Workstreams,
            ),
            left[0],
        );
        frame.render_widget(
            render_panel_with_focus(
                view_model::PanelViewModel {
                    title: collaboration.workstream_detail.title,
                    lines: collaboration.workstream_detail.lines,
                },
                true,
                true,
            ),
            left[1],
        );
        frame.render_widget(
            render_work_units(
                collaboration.work_units,
                state.collaboration_focus == CollaborationFocus::WorkUnits,
            ),
            columns[1],
        );
        frame.render_widget(render_collaboration_detail(collaboration.detail), right[0]);
        frame.render_widget(
            render_collaboration_history(collaboration.history),
            right[1],
        );
    }
}

fn render_collaboration_status(
    status: view_model::CollaborationStatusViewModel,
) -> Paragraph<'static> {
    let lines = vec![
        Line::styled(
            format!(
                "focus={}  workstreams={}  work_units={}  active_assignments={}  review={}",
                view_model::collaboration_focus_label(status.focus),
                status.workstream_count,
                status.work_unit_count,
                status.active_assignment_count,
                status.review_count
            ),
            emphasis_style(),
        ),
        key_value_line(
            "selected stream",
            &status
                .selected_workstream_title
                .unwrap_or_else(|| "-".to_string()),
        ),
        key_value_line(
            "selected unit",
            &status
                .selected_work_unit_title
                .unwrap_or_else(|| "-".to_string()),
        ),
    ];

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title("Collaboration")
                .borders(Borders::ALL)
                .border_style(focus_block_style(true)),
        )
        .wrap(Wrap { trim: true })
}

fn render_workstreams(
    list: view_model::WorkstreamListViewModel,
    focused: bool,
) -> Paragraph<'static> {
    let lines = if list.rows.is_empty() {
        vec![Line::styled("No workstreams loaded.", metadata_style())]
    } else {
        list.rows
            .into_iter()
            .take(10)
            .map(|row| {
                let marker = selection_marker(row.selected, focused);
                let status_style = status_text_style(&row.status);
                let counts = format!(" {}", row.counts);
                Line::from(vec![
                    Span::styled(format!("{marker}"), row_style(row.selected, focused)),
                    Span::styled(format!(" {} ", row.title), row_style(row.selected, focused)),
                    Span::styled("[", metadata_style()),
                    Span::styled(row.status.to_string(), status_style),
                    Span::styled("]", metadata_style()),
                    Span::styled(counts, metadata_style()),
                ])
            })
            .collect()
    };

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title(focus_title("Workstreams", focused))
                .borders(Borders::ALL)
                .border_style(focus_block_style(focused)),
        )
        .wrap(Wrap { trim: true })
}

fn render_work_units(list: view_model::WorkUnitListViewModel, focused: bool) -> Paragraph<'static> {
    let lines = if list.rows.is_empty() {
        vec![Line::styled("No work units loaded.", metadata_style())]
    } else {
        let mut lines = Vec::new();
        for row in list.rows.into_iter().take(8) {
            let marker = selection_marker(row.selected, focused);
            let review = if row.needs_supervisor_review {
                " review"
            } else {
                ""
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker}"), row_style(row.selected, focused)),
                Span::styled(format!(" {} ", row.title), row_style(row.selected, focused)),
                Span::styled("[", metadata_style()),
                Span::styled(row.status.clone(), status_text_style(&row.status)),
                Span::styled("]", metadata_style()),
                Span::styled(format!(" {}", review), status_label_style()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("assignment=", label_style()),
                Span::styled(row.current_assignment.clone(), metadata_style()),
                Span::styled(
                    format!("  decision={} ", row.latest_decision),
                    metadata_style(),
                ),
                Span::styled("proposal=", label_style()),
                Span::styled(row.proposal_status.clone(), metadata_style()),
                Span::styled(
                    format!(" parse={}", row.latest_report_parse_result),
                    metadata_style(),
                ),
            ]));
        }
        lines
    };

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title(focus_title("Work Units", focused))
                .borders(Borders::ALL)
                .border_style(focus_block_style(focused)),
        )
        .wrap(Wrap { trim: true })
}

fn render_collaboration_detail(
    detail: view_model::CollaborationDetailViewModel,
) -> Paragraph<'static> {
    render_panel_with_focus(
        view_model::PanelViewModel {
            title: detail.title,
            lines: detail.lines,
        },
        true,
        true,
    )
}

fn render_collaboration_history(
    history: view_model::CollaborationHistoryViewModel,
) -> Paragraph<'static> {
    render_panel_with_focus(
        view_model::PanelViewModel {
            title: history.title,
            lines: history.lines,
        },
        false,
        false,
    )
}
