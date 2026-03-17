use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::TopLevelView;
use crate::view_model::PanelViewModel;

pub(super) fn render_panel(panel: PanelViewModel, trim: bool) -> Paragraph<'static> {
    Paragraph::new(Text::from(
        panel.lines.into_iter().map(Line::from).collect::<Vec<_>>(),
    ))
    .block(Block::default().title(panel.title).borders(Borders::ALL))
    .wrap(Wrap { trim })
}

pub(super) fn focus_title(base: &str, focused: bool) -> String {
    if focused {
        format!("{base} <focus>")
    } else {
        base.to_string()
    }
}

pub(super) fn title_case_view_label(view: TopLevelView) -> &'static str {
    match view {
        TopLevelView::Overview => "Overview",
        TopLevelView::Threads => "Threads",
        TopLevelView::Collaboration => "Collaboration",
    }
}
