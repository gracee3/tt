#![allow(unused_crate_dependencies)]

use anyhow::Result;
use clap::{Args, Parser};
use orcas_core::{AppPaths, init_file_logger};
use tracing::info;

use orcasd::{OrcasDaemonService, OrcasRuntimeOverrides};

#[derive(Debug, Parser)]
#[command(name = "orcasd", version, about = "Orcas daemon process")]
struct DaemonCli {
    #[command(flatten)]
    runtime: DaemonRuntimeArgs,
}

#[derive(Debug, Clone, Args, Default)]
struct DaemonRuntimeArgs {
    #[arg(long)]
    codex_bin: Option<std::path::PathBuf>,
    #[arg(long)]
    listen_url: Option<String>,
    #[arg(long)]
    cwd: Option<std::path::PathBuf>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, default_value_t = false)]
    connect_only: bool,
    #[arg(long, default_value_t = false)]
    force_spawn: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = DaemonCli::parse();
    let paths = AppPaths::discover()?;
    paths.ensure().await?;
    init_file_logger("orcasd", &paths.daemon_log_file)?;
    info!("starting orcas daemon process");

    let service = OrcasDaemonService::load_with_runtime_overrides(
        OrcasRuntimeOverrides::from_env().overlay(&OrcasRuntimeOverrides {
            codex_bin: cli.runtime.codex_bin,
            listen_url: cli.runtime.listen_url,
            cwd: cli.runtime.cwd,
            model: cli.runtime.model,
            connect_only: cli.runtime.connect_only,
            force_spawn: cli.runtime.force_spawn,
        }),
    )
    .await?;
    service.run().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DaemonCli, DaemonRuntimeArgs};
    use clap::{CommandFactory, Parser};

    #[test]
    fn parses_direct_daemon_runtime_flags() {
        let cli = DaemonCli::parse_from([
            "orcasd",
            "--codex-bin",
            "/tmp/codex",
            "--listen-url",
            "ws://127.0.0.1:4510",
            "--cwd",
            "/tmp/work",
            "--model",
            "gpt-5.4",
            "--connect-only",
        ]);

        assert_eq!(
            cli.runtime.codex_bin.as_deref(),
            Some(std::path::Path::new("/tmp/codex"))
        );
        assert_eq!(
            cli.runtime.listen_url.as_deref(),
            Some("ws://127.0.0.1:4510")
        );
        assert_eq!(
            cli.runtime.cwd.as_deref(),
            Some(std::path::Path::new("/tmp/work"))
        );
        assert_eq!(cli.runtime.model.as_deref(), Some("gpt-5.4"));
        assert!(cli.runtime.connect_only);
        assert!(!cli.runtime.force_spawn);
    }

    #[test]
    fn daemon_runtime_args_default_cleanly() {
        let runtime = DaemonRuntimeArgs::default();

        assert!(runtime.codex_bin.is_none());
        assert!(runtime.listen_url.is_none());
        assert!(runtime.cwd.is_none());
        assert!(runtime.model.is_none());
        assert!(!runtime.connect_only);
        assert!(!runtime.force_spawn);
    }

    #[test]
    fn daemon_help_mentions_the_daemon_process() {
        let help = DaemonCli::command().render_help().to_string();

        assert!(help.contains("Orcas daemon process"));
        assert!(help.contains("--connect-only"));
        assert!(help.contains("--force-spawn"));
    }

    #[test]
    fn daemon_version_matches_crate_version() {
        let version = DaemonCli::command().render_version().to_string();

        assert!(version.contains(env!("CARGO_PKG_VERSION")));
    }
}
