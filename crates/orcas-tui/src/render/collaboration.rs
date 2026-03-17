use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{AppState, CollaborationFocus};
use crate::view_model;

use super::shared::{focus_title, render_panel};

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
            render_workstream_detail(collaboration.workstream_detail),
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
            render_workstream_detail(collaboration.workstream_detail),
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
    let mut lines = vec![Line::from(format!(
        "focus={}  workstreams={}  work_units={}  active_assignments={}  review={}",
        view_model::collaboration_focus_label(status.focus),
        status.workstream_count,
        status.work_unit_count,
        status.active_assignment_count,
        status.review_count
    ))];
    lines.push(Line::from(format!(
        "selected stream: {}",
        status
            .selected_workstream_title
            .unwrap_or_else(|| "-".to_string())
    )));
    lines.push(Line::from(format!(
        "selected unit: {}",
        status
            .selected_work_unit_title
            .unwrap_or_else(|| "-".to_string())
    )));

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title("Collaboration")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true })
}

fn render_workstreams(
    list: view_model::WorkstreamListViewModel,
    focused: bool,
) -> Paragraph<'static> {
    let lines = if list.rows.is_empty() {
        vec![Line::from("No workstreams loaded.")]
    } else {
        list.rows
            .into_iter()
            .take(10)
            .map(|row| {
                let prefix = if row.selected { ">" } else { " " };
                Line::from(format!(
                    "{prefix} {} [{}] {}",
                    row.title, row.status, row.counts
                ))
            })
            .collect()
    };

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title(focus_title("Workstreams", focused))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true })
}

fn render_workstream_detail(detail: view_model::WorkstreamDetailViewModel) -> Paragraph<'static> {
    render_panel(
        view_model::PanelViewModel {
            title: detail.title,
            lines: detail.lines,
        },
        true,
    )
}

fn render_work_units(list: view_model::WorkUnitListViewModel, focused: bool) -> Paragraph<'static> {
    let lines = if list.rows.is_empty() {
        vec![Line::from("No work units loaded.")]
    } else {
        let mut lines = Vec::new();
        for row in list.rows.into_iter().take(8) {
            let prefix = if row.selected { ">" } else { " " };
            let review = if row.needs_supervisor_review {
                " review"
            } else {
                ""
            };
            lines.push(Line::from(format!(
                "{prefix} {} [{}]",
                row.title, row.status
            )));
            lines.push(Line::from(format!(
                "  assignment={} decision={} proposal={} parse={}{}",
                row.current_assignment,
                row.latest_decision,
                row.proposal_status,
                row.latest_report_parse_result,
                review
            )));
        }
        lines
    };

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title(focus_title("Work Units", focused))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true })
}

fn render_collaboration_detail(
    detail: view_model::CollaborationDetailViewModel,
) -> Paragraph<'static> {
    render_panel(
        view_model::PanelViewModel {
            title: detail.title,
            lines: detail.lines,
        },
        true,
    )
}

fn render_collaboration_history(
    history: view_model::CollaborationHistoryViewModel,
) -> Paragraph<'static> {
    render_panel(
        view_model::PanelViewModel {
            title: history.title,
            lines: history.lines,
        },
        false,
    )
}
