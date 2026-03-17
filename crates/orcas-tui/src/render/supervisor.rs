use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::AppState;

use crate::view_model::shared::daemon_phase_label;

use super::shared::render_panel;

pub(super) fn render_view(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    let compact = area.width < 130 || area.height < 30;

    let layout = if compact {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Min(8),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Length(12),
                Constraint::Min(10),
            ])
            .split(area)
    };

    frame.render_widget(render_daemon_status(state), layout[0]);
    frame.render_widget(render_models(state), layout[1]);
    frame.render_widget(render_controls(state), layout[2]);
}

fn render_daemon_status(state: &AppState) -> Paragraph<'static> {
    let daemon = state.daemon.as_ref();
    let mut lines = vec![Line::from(format!(
        "daemon: {}  upstream: {}  clients: {}  threads: {}  reconnect: {}",
        daemon_phase_label(state.daemon_phase),
        daemon
            .map(|daemon| daemon.upstream.status.clone())
            .unwrap_or_else(|| "disconnected".to_string()),
        daemon.map_or(0, |daemon| daemon.client_count),
        daemon.map_or(state.threads.len(), |daemon| daemon.known_threads),
        state.reconnect_attempt
    ))];
    lines.push(Line::from(format!(
        "socket: {}",
        daemon
            .map(|daemon| daemon.socket_path.clone())
            .unwrap_or_else(|| "unavailable".to_string())
    )));
    lines.push(Line::from(format!(
        "codex: {}",
        daemon
            .map(|daemon| daemon.codex_endpoint.clone())
            .unwrap_or_else(|| "unknown".to_string())
    )));
    if let Some(detail) = daemon.and_then(|daemon| daemon.upstream.detail.as_ref()) {
        lines.push(Line::from(format!("detail: {detail}")));
    }

    Paragraph::new(Text::from(lines)).block(
        Block::default()
            .title("Supervisor Daemon")
            .borders(Borders::ALL),
    )
}

fn render_models(state: &AppState) -> Paragraph<'static> {
    let mut lines = Vec::new();
    if state.daemon_models.is_empty() {
        lines.push(Line::from(
            "No models loaded. Press m to refresh models.".to_string(),
        ));
    } else {
        for model in state.daemon_models.iter().take(18) {
            let mut prefix = if model.is_default { "* " } else { "  " }.to_string();
            if model.hidden {
                prefix.push_str("h ");
            }
            lines.push(Line::from(format!(
                "{prefix}{} [{}]{}",
                model.id,
                model.display_name,
                if model.is_default { " (default)" } else { "" }
            )));
        }
        if state.daemon_models.len() > 18 {
            lines.push(Line::from(format!(
                "+ {} more models",
                state.daemon_models.len() - 18
            )));
        }
    }

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title("Available Models")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true })
}

fn render_controls(state: &AppState) -> Paragraph<'static> {
    let mut lines = vec![Line::from("status: ready")];
    if let Some(daemon) = state.daemon.as_ref() {
        let stop_hint = if daemon.upstream.status == "connected" {
            "x stop daemon"
        } else {
            "daemon not connected"
        };
        lines.push(Line::from(format!(
            "actions: {}  m refresh models",
            stop_hint
        )));
        lines.push(Line::from(format!(
            "runtime: {} {} {}",
            daemon.runtime.version, daemon.runtime.build_fingerprint, daemon.runtime.binary_path
        )));
        lines.push(Line::from(format!(
            "metadata: {}",
            daemon.runtime.metadata_path
        )));
    } else {
        lines.push(Line::from("daemon status not loaded yet."));
    }

    render_panel(
        crate::view_model::PanelViewModel {
            title: "Controls".to_string(),
            lines: lines.into_iter().map(|line| line.to_string()).collect(),
        },
        true,
    )
}
