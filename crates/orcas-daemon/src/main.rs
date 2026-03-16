#![allow(unused_crate_dependencies)]

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use orcas_daemon::OrcasDaemonService;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let service = OrcasDaemonService::load_from_env().await?;
    service.run().await?;
    Ok(())
}
