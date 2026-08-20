#![forbid(unsafe_code)]
#![deny(
    clippy::all,
    future_incompatible,
    missing_debug_implementations,
    nonstandard_style,
    unreachable_pub,
    unused_qualifications
)]

//! A standalone [`daemon_server`] endpoint, for clients that have no
//! relationship to any particular Hydra evaluation: `nix-build` or
//! `nix-store --realise` pointed at this socket by hand.
//!
//! Such a request carries no context, so every build it asks for is
//! filed under one hidden `adhoc/adhoc` jobset (see [`AdhocSubmitter`]).

mod submit;

use std::path::PathBuf;

use clap::Parser;
use color_eyre::eyre;
use harmonia_store_path::StoreDir;

use daemon_server::{BuildWaiter, DaemonServer, HydraDaemonHandler};

use crate::submit::AdhocSubmitter;

#[derive(Parser, Debug)]
#[command(about = "Nix daemon proxy that turns build requests into ad-hoc Hydra builds")]
struct Args {
    /// Unix socket to listen on for nix daemon connections.
    #[arg(long, default_value = "/tmp/hydra-drv-daemon.sock")]
    socket: PathBuf,

    /// Upstream nix daemon socket to proxy read operations to.
    #[arg(long, default_value = "/nix/var/nix/daemon-socket/socket")]
    upstream_socket: String,

    /// PostgreSQL connection URL.
    #[arg(long, env = "HYDRA_DBA")]
    db_url: String,

    /// Nix store directory.
    #[arg(long, default_value = "/nix/store")]
    store_dir: String,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let store_dir =
        StoreDir::new(&args.store_dir).map_err(|e| eyre::eyre!("invalid store dir: {e}"))?;
    let database = db::Database::new(&args.db_url, 4).await?;
    let waiter = BuildWaiter::start(&database).await?;
    let submitter = AdhocSubmitter::new(database.clone()).await?;
    let handler = HydraDaemonHandler::new(
        store_dir.clone(),
        database,
        &args.upstream_socket,
        waiter,
        submitter,
    );
    let server = DaemonServer::new(handler, args.socket, store_dir);
    server.serve().await?;
    Ok(())
}
