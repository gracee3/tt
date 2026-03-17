mod collaboration;
mod overview;
mod shared;
mod shell;
mod threads;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::app::{AppState, TopLevelView};

pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let compact = frame.area().width < 130 || frame.area().height < 34;
    let status_height = if compact { 5 } else { 6 };
    let footer_height = if state.show_help || frame.area().height >= 28 {
        3
    } else {
        2
    };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_height),
            Constraint::Min(12),
            Constraint::Length(footer_height),
        ])
        .split(frame.area());

    frame.render_widget(shell::render_shell_status(state), layout[0]);
    match state.current_view {
        TopLevelView::Overview => overview::render_view(frame, state, layout[1]),
        TopLevelView::Threads => threads::render_view(frame, state, layout[1]),
        TopLevelView::Collaboration => collaboration::render_view(frame, state, layout[1]),
    }
    frame.render_widget(shell::render_footer(state), layout[2]);
}
