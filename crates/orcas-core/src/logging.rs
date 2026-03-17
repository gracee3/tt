use std::fs::OpenOptions;
use std::path::Path;

use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use tracing_subscriber::util::TryInitError;

use crate::OrcasResult;

pub fn init_file_logger(component: &str, log_path: &Path) -> OrcasResult<()> {
    let logs_parent = log_path.parent().ok_or_else(|| {
        crate::OrcasError::Transport(format!(
            "log path `{}` has no parent directory",
            log_path.display()
        ))
    })?;
    std::fs::create_dir_all(logs_parent)?;

    let aggregate_log_path = logs_parent.join("orcas.log");

    let component_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let aggregate_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&aggregate_log_path)?;

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("{component}=debug,debug,tokio=info")));

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(false)
                .with_file(false)
                .with_line_number(false)
                .with_thread_names(false)
                .with_thread_ids(false)
                .with_writer(component_file)
                .with_ansi(false)
                .with_filter(env_filter.clone()),
        )
        .with(
            fmt::layer()
                .with_target(false)
                .with_file(false)
                .with_line_number(false)
                .with_thread_names(false)
                .with_thread_ids(false)
                .with_writer(aggregate_file)
                .with_ansi(false)
                .with_filter(env_filter),
        )
        .try_init()
        .map_err(|error: TryInitError| crate::OrcasError::Transport(error.to_string()))?;

    Ok(())
}
