use std::env;
use std::fs::OpenOptions;
use std::path::Path;

use tracing_subscriber::util::TryInitError;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::TTResult;

pub fn init_file_logger(component: &str, log_path: &Path) -> TTResult<()> {
    let logs_parent = log_path.parent().ok_or_else(|| {
        crate::TTError::Transport(format!(
            "log path `{}` has no parent directory",
            log_path.display()
        ))
    })?;
    std::fs::create_dir_all(logs_parent)?;

    let component_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("{component}=info,info,tokio=info")));

    let subscriber = tracing_subscriber::registry().with(
        fmt::layer()
            .with_target(false)
            .with_file(false)
            .with_line_number(false)
            .with_thread_names(false)
            .with_thread_ids(false)
            .with_writer(component_file)
            .with_ansi(false)
            .with_filter(env_filter),
    );

    subscriber
        .try_init()
        .map_err(|error: TryInitError| crate::TTError::Transport(error.to_string()))?;

    Ok(())
}

pub fn runtime_cycle_enabled() -> bool {
    parse_boolish(env::var("TT_LOG_RUNTIME_CYCLE").ok().as_deref(), false)
}

fn parse_boolish(value: Option<&str>, default: bool) -> bool {
    match value.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on") => true,
        Some(value) if matches!(value.as_str(), "0" | "false" | "no" | "off") => false,
        Some(_) => default,
        None => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boolish_flags_parse_common_values() {
        assert!(parse_boolish(Some("1"), false));
        assert!(parse_boolish(Some("true"), false));
        assert!(parse_boolish(Some("ON"), false));
        assert!(!parse_boolish(Some("0"), true));
        assert!(!parse_boolish(Some("false"), true));
        assert!(!parse_boolish(Some("off"), true));
        assert!(parse_boolish(Some("unexpected"), true));
        assert!(!parse_boolish(Some("unexpected"), false));
        assert!(parse_boolish(None, true));
        assert!(!parse_boolish(None, false));
    }
}
