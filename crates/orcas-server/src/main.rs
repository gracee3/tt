use std::net::SocketAddr;

use anyhow::Result;
use clap::Parser;
use orcas_core::{AppPaths, init_file_logger};
use tracing::info;

use orcas_server::InboxMirrorServer;
use orcas_server::InboxMirrorStore;

#[derive(Debug, Parser)]
#[command(name = "orcas-server", version, about = "Orcas mirrored inbox server")]
struct ServerCli {
    #[arg(
        long,
        default_value = "127.0.0.1:9311",
        help = "Bind address for the mirrored inbox server"
    )]
    bind: SocketAddr,
    #[arg(
        long,
        env = "ORCAS_OPERATOR_API_TOKEN",
        help = "Optional bearer token required for operator-facing server APIs"
    )]
    operator_api_token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = ServerCli::parse();
    let paths = AppPaths::discover()?;
    paths.ensure().await?;
    init_file_logger("orcas-server", &paths.daemon_log_file)?;
    info!("starting orcas mirrored inbox server");

    let store = InboxMirrorStore::open(paths.data_dir.join("server_inbox.db"))?;
    InboxMirrorServer::with_operator_api_token(store, cli.operator_api_token)
        .serve(cli.bind)
        .await?;
    Ok(())
}
