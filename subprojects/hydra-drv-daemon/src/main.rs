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

mod config;
mod submit;

use clap::Parser;
use color_eyre::eyre;
use harmonia_store_path::StoreDir;
use secrecy::ExposeSecret as _;

use daemon_server::{BuildWaiter, DaemonServer, HydraDaemonHandler};

use crate::config::{App, BindSocket, Cli};
use crate::submit::AdhocSubmitter;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let _tracing_guard = hydra_tracing::init()?;

    let cli = Cli::parse();
    let config = App::init(&cli.config_path)?;

    let store_dir =
        StoreDir::new(&config.store_dir).map_err(|e| eyre::eyre!("invalid store dir: {e}"))?;
    let database =
        db::Database::new(config.db_url.expose_secret(), config.max_db_connections).await?;
    let waiter = BuildWaiter::start(&database).await?;
    let submitter = AdhocSubmitter::new(database.clone()).await?;
    let handler = HydraDaemonHandler::new(
        store_dir.clone(),
        database,
        &config.upstream_socket,
        waiter,
        submitter,
    );

    let server = match &cli.socket {
        BindSocket::Path(path) => DaemonServer::bind(handler, path.clone(), store_dir)?,
        BindSocket::ListenFd => {
            DaemonServer::from_listener(handler, BindSocket::inherited()?, store_dir)
        }
    };
    tracing::info!(bind = %cli.socket);

    let _notify = sd_notify::notify(&[
        sd_notify::NotifyState::Status("Running"),
        sd_notify::NotifyState::Ready,
    ]);

    server.serve().await?;
    Ok(())
}
