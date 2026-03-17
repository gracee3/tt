use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{AppState, DaemonLifecycleState, TopLevelView};

use crate::view_model::shared::{daemon_lifecycle_label, daemon_phase_label};

use super::shared::{
    daemon_phase_to_status_style, emphasis_style, focus_block_style, key_hint_style,
    key_value_line, label_style, lifecycle_style, metadata_style, status_style, value_style,
};

pub(super) fn render_view(frame: &mut Frame<'_>, state: &AppState, area: Rect) {
    let compact = area.width < 130 || area.height < 30;

    let layout = if compact {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Min(8),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(11),
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
    let has_focus = state.current_view == TopLevelView::Supervisor;
    let upstream_status = daemon
        .map(|daemon| daemon.upstream.status.as_str())
        .unwrap_or("disconnected");
    let lines = vec![
        Line::from(vec![
            Span::styled("state: ", label_style()),
            Span::styled(
                daemon_lifecycle_label(state.daemon_lifecycle),
                lifecycle_style(state.daemon_lifecycle),
            ),
        ]),
        key_value_line("daemon", daemon_phase_label(state.daemon_phase)),
        key_value_line("upstream", upstream_status),
        Line::from(vec![
            Span::styled("clients: ", label_style()),
            Span::styled(
                daemon.map_or("0".to_string(), |daemon| daemon.client_count.to_string()),
                value_style(),
            ),
            Span::styled(
                format!(
                    "  threads: {}",
                    daemon.map_or(state.threads.len(), |daemon| daemon.known_threads)
                ),
                label_style(),
            ),
            Span::styled(
                format!("  reconnect: {}", state.reconnect_attempt),
                metadata_style(),
            ),
        ]),
        key_value_line(
            "socket",
            daemon
                .map(|daemon| daemon.socket_path.clone())
                .unwrap_or_else(|| "unavailable".to_string())
                .as_str(),
        ),
    ];

    let mut expanded = Vec::new();
    expanded.extend(lines);

    if let Some(detail) = daemon.and_then(|daemon| daemon.upstream.detail.as_ref()) {
        expanded.push(Line::styled(
            format!("upstream detail: {detail}"),
            emphasis_style(),
        ));
    }
    if let Some(error) = state.daemon_lifecycle_error.as_deref() {
        expanded.push(Line::styled(
            format!("status detail: {error}"),
            status_style(
                if matches!(state.daemon_lifecycle, DaemonLifecycleState::Failed) {
                    crate::app::BannerLevel::Error
                } else {
                    crate::app::BannerLevel::Warning
                },
            ),
        ));
    }
    if let Some(runtime) = daemon.map(|daemon| &daemon.runtime) {
        expanded.push(Line::from(vec![
            Span::styled("runtime: ", label_style()),
            Span::styled(runtime.version.clone(), value_style()),
            Span::styled(format!(" {}", runtime.build_fingerprint), metadata_style()),
        ]));
        expanded.push(key_value_line("metadata path", &runtime.metadata_path));
    }

    Paragraph::new(Text::from(expanded)).block(
        Block::default()
            .title("Supervisor Daemon")
            .borders(Borders::ALL)
            .border_style(focus_block_style(has_focus)),
    )
}

fn render_models(state: &AppState) -> Paragraph<'static> {
    let mut lines = Vec::new();
    let is_inflight = matches!(
        state.daemon_lifecycle,
        DaemonLifecycleState::Starting
            | DaemonLifecycleState::Stopping
            | DaemonLifecycleState::Restarting
    );

    if is_inflight {
        lines.push(Line::styled(
            "model update deferred during daemon transition",
            emphasis_style(),
        ));
    }
    if state.models_loading {
        lines.push(Line::styled("loading models...", emphasis_style()));
    }

    if state.daemon_lifecycle == DaemonLifecycleState::Stopped {
        lines.push(Line::styled(
            "No models loaded. Start daemon to refresh model list.",
            daemon_phase_to_status_style("stopped"),
        ));
    } else if state.daemon_models.is_empty() {
        if state.models_loading {
            lines.push(Line::styled("no models loaded yet", metadata_style()));
        } else {
            lines.push(Line::styled(
                "No models loaded. Press m to refresh models.",
                metadata_style(),
            ));
        }
    } else {
        for (index, model) in state.daemon_models.iter().take(18).enumerate() {
            let status = if model.is_default {
                "default"
            } else if model.hidden {
                "hidden"
            } else {
                "available"
            };
            let row = Line::from(vec![
                Span::styled(format!("[{}] ", index + 1), metadata_style()),
                Span::styled(model.id.clone(), value_style()),
                Span::styled(
                    format!("  {}", status),
                    daemon_phase_to_status_style(status),
                ),
                Span::styled(format!("  {}", model.display_name), metadata_style()),
            ]);
            lines.push(row);
        }
        if state.daemon_models.len() > 18 {
            lines.push(Line::styled(
                format!("+ {} more models", state.daemon_models.len() - 18),
                emphasis_style(),
            ));
        }
    }

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title(format!("Available Models ({})", state.daemon_models.len()))
                .borders(Borders::ALL)
                .border_style(focus_block_style(true)),
        )
        .wrap(Wrap { trim: true })
}

fn render_controls(state: &AppState) -> Paragraph<'static> {
    let mut lines = Vec::new();
    let in_flight = matches!(
        state.daemon_lifecycle,
        DaemonLifecycleState::Starting
            | DaemonLifecycleState::Stopping
            | DaemonLifecycleState::Restarting
    );
    let lifecycle = daemon_lifecycle_label(state.daemon_lifecycle);
    lines.push(Line::styled(
        format!("status: {lifecycle}"),
        lifecycle_style(state.daemon_lifecycle),
    ));
    lines.push(Line::from(vec![
        Span::styled("actions: ", label_style()),
        Span::styled("m", key_hint_style()),
        Span::styled(" refresh models  ", metadata_style()),
        Span::styled("s", key_hint_style()),
        Span::styled(" start daemon  ", metadata_style()),
        Span::styled("x", key_hint_style()),
        Span::styled(" stop daemon  ", metadata_style()),
        Span::styled("R", key_hint_style()),
        Span::styled(" restart daemon", metadata_style()),
    ]));

    lines.push(Line::styled(
        if in_flight {
            "daemon command in progress: repeated lifecycle keys are ignored"
        } else if state.daemon_lifecycle == DaemonLifecycleState::Failed {
            "daemon failure state: use restart (R) or stop/start again once fixed"
        } else {
            "lifecycle commands can be triggered at any time"
        },
        if in_flight {
            emphasis_style()
        } else if state.daemon_lifecycle == DaemonLifecycleState::Failed {
            status_style(crate::app::BannerLevel::Error)
        } else {
            metadata_style()
        },
    ));
    if state.daemon_lifecycle == DaemonLifecycleState::Failed {
        if let Some(error) = state.daemon_lifecycle_error.as_deref() {
            lines.push(Line::styled(
                format!("last failure: {error}"),
                status_style(crate::app::BannerLevel::Error),
            ));
        }
    } else if let Some(daemon) = state.daemon.as_ref() {
        if in_flight {
            lines.push(Line::from(vec![
                Span::styled("endpoint=", metadata_style()),
                Span::styled(format!("{}  ", daemon.upstream.endpoint), value_style()),
                Span::styled("codex=", metadata_style()),
                Span::styled(daemon.codex_endpoint.clone(), value_style()),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("runtime pid=", metadata_style()),
                Span::styled(daemon.runtime.pid.to_string(), value_style()),
                Span::styled("  codex=", metadata_style()),
                Span::styled(daemon.codex_endpoint.clone(), value_style()),
            ]));
        }
    } else {
        lines.push(Line::styled(
            "daemon metadata not loaded yet.",
            metadata_style(),
        ));
    }

    Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title("Controls")
                .borders(Borders::ALL)
                .border_style(focus_block_style(true)),
        )
        .wrap(Wrap { trim: true })
}
