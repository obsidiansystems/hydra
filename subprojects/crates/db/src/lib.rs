#![forbid(unsafe_code)]
#![deny(
    clippy::all,
    clippy::pedantic,
    clippy::expect_used,
    clippy::unwrap_used,
    future_incompatible,
    missing_debug_implementations,
    nonstandard_style,
    unreachable_pub,
    missing_copy_implementations,
    unused_qualifications
)]
#![allow(clippy::missing_errors_doc)]

mod connection;
pub mod models;
pub mod queries;

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::StreamExt as _;

pub use connection::{Connection, Transaction};
pub use tokio_postgres::Notification;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Postgres(#[from] tokio_postgres::Error),
    #[error("{0}")]
    Pool(#[from] deadpool_postgres::PoolError),
    #[error("{0}")]
    Build(#[from] deadpool_postgres::BuildError),
    #[error("{0}")]
    Config(String),
}

#[derive(Debug)]
struct PoolState {
    pool: deadpool_postgres::Pool,
    url: String,
}

#[derive(Debug)]
pub struct Database {
    state: arc_swap::ArcSwap<PoolState>,
}

impl Clone for Database {
    fn clone(&self) -> Self {
        Self {
            state: arc_swap::ArcSwap::new(Arc::clone(&self.state.load())),
        }
    }
}

fn build_pool(url: &str, max_size: usize) -> Result<PoolState, String> {
    let pg_config = url
        .parse::<tokio_postgres::Config>()
        .map_err(|e| e.to_string())?;
    let mgr_config = deadpool_postgres::ManagerConfig {
        recycling_method: deadpool_postgres::RecyclingMethod::Fast,
    };
    let mgr =
        deadpool_postgres::Manager::from_config(pg_config, tokio_postgres::NoTls, mgr_config);
    let pool = deadpool_postgres::Pool::builder(mgr)
        .max_size(max_size)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(PoolState {
        pool,
        url: url.to_owned(),
    })
}

impl Database {
    pub async fn new(url: &str, max_connections: u32) -> Result<Self, Error> {
        let max_size = max_connections.try_into().unwrap_or(usize::MAX);
        let ps = build_pool(url, max_size).map_err(Error::Config)?;
        // Verify connectivity
        let _ = ps.pool.get().await?;
        Ok(Self {
            state: arc_swap::ArcSwap::from_pointee(ps),
        })
    }

    pub async fn get(&self) -> Result<Connection, Error> {
        let state = self.state.load();
        let conn = state.pool.get().await?;
        Ok(Connection::new(conn))
    }

    #[tracing::instrument(skip(self, url), err)]
    pub fn reconfigure_pool(&self, url: &str) -> anyhow::Result<()> {
        let old = self.state.load();
        let max_size = old.pool.status().max_size;
        let ps = build_pool(url, max_size)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.state.store(Arc::new(ps));
        Ok(())
    }

    pub async fn listener(
        &self,
        channels: Vec<&str>,
    ) -> Result<
        impl futures::Stream<Item = Result<Notification, Error>> + Unpin,
        Error,
    > {
        let url = self.state.load().url.clone();
        let (client, mut connection) =
            tokio_postgres::connect(&url, tokio_postgres::NoTls).await?;

        // Spawn the connection driver FIRST — Client operations only complete
        // when the Connection future is being polled.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = tokio::spawn(async move {
            use std::future::poll_fn;
            loop {
                let msg = poll_fn(|cx| connection.poll_message(cx)).await;
                match msg {
                    Some(Ok(tokio_postgres::AsyncMessage::Notification(n))) => {
                        if tx.send(n).is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::error!("PgListener connection error: {e}");
                        break;
                    }
                    None => break,
                }
            }
        });

        for ch in &channels {
            client
                .batch_execute(&format!("LISTEN {ch}"))
                .await?;
        }

        Ok(ListenerStream {
            inner: tokio_stream::wrappers::UnboundedReceiverStream::new(rx).map(Ok),
            _handle: handle,
            _client: client,
        })
    }
}

/// Wraps the notification stream and aborts the background connection task on drop.
/// Holds the `Client` to keep the underlying postgres connection alive.
struct ListenerStream<S> {
    inner: S,
    _handle: tokio::task::JoinHandle<()>,
    _client: tokio_postgres::Client,
}

impl<S: std::fmt::Debug> std::fmt::Debug for ListenerStream<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListenerStream")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<S> Drop for ListenerStream<S> {
    fn drop(&mut self) {
        self._handle.abort();
    }
}

impl<S: futures::Stream + Unpin> futures::Stream for ListenerStream<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}
