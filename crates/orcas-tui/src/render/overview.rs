use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::AppState;
use crate::view_model;

use super::shared::render_panel;

pub(super) fn render_view(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    let compact = area.width < 110 || area.height < 24;
    let overview = view_model::overview_view(state);
    if compact {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),
                Constraint::Length(6),
                Constraint::Min(4),
                Constraint::Min(4),
            ])
            .split(area);
        frame.render_widget(render_panel(overview.connection, true), layout[0]);
        frame.render_widget(render_panel(overview.active_work, true), layout[1]);
        frame.render_widget(render_panel(overview.warnings, true), layout[2]);
        frame.render_widget(render_panel(overview.recent_events, true), layout[3]);
    } else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
            .split(area);
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(rows[0]);
        let bottom = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(rows[1]);
        frame.render_widget(render_panel(overview.connection, true), top[0]);
        frame.render_widget(render_panel(overview.active_work, true), top[1]);
        frame.render_widget(render_panel(overview.warnings, true), bottom[0]);
        frame.render_widget(render_panel(overview.recent_events, true), bottom[1]);
    }
}
