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
    #[arg(
        long,
        env = "ORCAS_PUSH_VAPID_PRIVATE_KEY_BASE64",
        help = "Optional base64url VAPID private key used for browser push delivery"
    )]
    push_vapid_private_key_base64: Option<String>,
    #[arg(
        long,
        env = "ORCAS_PUSH_VAPID_SUBJECT",
        help = "Optional VAPID subject URI used for browser push delivery"
    )]
    push_vapid_subject: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = ServerCli::parse();
    let paths = AppPaths::discover()?;
    paths.ensure().await?;
    init_file_logger("orcas-server", &paths.daemon_log_file)?;
    info!("starting orcas mirrored inbox server");

    let store = InboxMirrorStore::open(paths.data_dir.join("server_inbox.db"))?;
    let config = orcas_server::InboxMirrorServerConfig {
        bind_addr: cli.bind,
        data_dir: paths.data_dir.clone(),
        daemon_socket_file: Some(paths.socket_file.clone()),
        operator_api_token: cli.operator_api_token,
        push_vapid_private_key_base64: cli.push_vapid_private_key_base64,
        push_vapid_subject: cli.push_vapid_subject,
    };
    let bind_addr = config.bind_addr;
    InboxMirrorServer::from_config(store, config)
        .serve(bind_addr)
        .await?;
    Ok(())
}
