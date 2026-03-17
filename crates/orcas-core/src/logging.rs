use std::fs::OpenOptions;
use std::path::Path;

use tracing_subscriber::{EnvFilter, fmt};

use crate::OrcasResult;

pub fn init_file_logger(component: &str, log_path: &Path) -> OrcasResult<()> {
    let logs_parent = log_path.parent().ok_or_else(|| {
        crate::OrcasError::Transport(format!(
            "log path `{}` has no parent directory",
            log_path.display()
        ))
    })?;
    std::fs::create_dir_all(logs_parent)?;

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("{component}=debug,debug,tokio=info")));

    fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .with_thread_names(false)
        .with_thread_ids(false)
        .with_writer(log_file)
        .with_ansi(false)
        .try_init()
        .map_err(|error| crate::OrcasError::Transport(error.to_string()))?;

    Ok(())
}
