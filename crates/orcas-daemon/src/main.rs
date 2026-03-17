#![allow(unused_crate_dependencies)]

use anyhow::Result;
use orcas_core::{AppPaths, init_file_logger};
use tracing::info;

use orcas_daemon::OrcasDaemonService;

#[tokio::main]
async fn main() -> Result<()> {
    let paths = AppPaths::discover()?;
    paths.ensure().await?;
    init_file_logger("orcasd", &paths.daemon_log_file)?;
    info!("starting orcas daemon process");

    let service = OrcasDaemonService::load_from_env().await?;
    service.run().await?;
    Ok(())
}
