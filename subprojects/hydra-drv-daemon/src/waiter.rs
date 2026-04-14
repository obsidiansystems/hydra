//! Async wakeup for ad-hoc Builds finished by the Hydra queue runner.
//!
//! The daemon listens on the `build_finished` postgres channel and
//! dispatches each notification to the waiting handler that registered
//! the build id.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt as _;
use tokio::sync::{Mutex, oneshot};

use db::models::BuildID;

type WaiterMap = Arc<Mutex<HashMap<BuildID, oneshot::Sender<()>>>>;

/// Registry of in-flight ad-hoc builds. Cloning shares the same backing
/// state, so all daemon connections wake from the same listener task.
#[derive(Clone)]
pub struct BuildWaiter {
    waiters: WaiterMap,
}

impl std::fmt::Debug for BuildWaiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildWaiter").finish_non_exhaustive()
    }
}

impl BuildWaiter {
    /// Spawn the listener task on the tokio runtime and return a handle
    /// that other components can clone. The task runs for the lifetime of
    /// the daemon process, ending only if the listener stream errors out.
    pub async fn start(db: &db::Database) -> Result<Self, db::Error> {
        // Make sure we can build a listener now so startup fails fast on
        // bad credentials / missing channels, then own a clone in the task.
        drop(db.listener(vec!["build_finished"]).await?);

        let waiters: WaiterMap = Arc::new(Mutex::new(HashMap::new()));
        let task_db = db.clone();
        let task_waiters = waiters.clone();
        tokio::spawn(async move {
            match task_db.listener(vec!["build_finished"]).await {
                Ok(stream) => run_listener(stream, task_waiters).await,
                Err(e) => tracing::error!("failed to start build_finished listener: {e}"),
            }
        });

        Ok(Self { waiters })
    }

    /// Register interest in `build_id`. The returned receiver fires once
    /// the queue runner emits a `build_finished` notification covering it.
    /// If a registration already exists for the same id it is overwritten.
    pub async fn register(&self, build_id: BuildID) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().await.insert(build_id, tx);
        rx
    }

    /// Drop a pending registration without waking it. Used to clean up
    /// when a caller bails out before the build finishes.
    pub async fn forget(&self, build_id: BuildID) {
        self.waiters.lock().await.remove(&build_id);
    }
}

async fn run_listener<S>(mut stream: S, waiters: WaiterMap)
where
    S: futures::Stream<Item = Result<sqlx::postgres::PgNotification, db::Error>> + Unpin,
{
    while let Some(item) = stream.next().await {
        let notif = match item {
            Ok(n) => n,
            Err(e) => {
                tracing::error!("build_finished listener error: {e}");
                break;
            }
        };
        let payload = notif.payload();
        let mut map = waiters.lock().await;
        for id_str in payload.split('\t') {
            let Ok(id) = id_str.parse::<BuildID>() else {
                continue;
            };
            if let Some(tx) = map.remove(&id) {
                let _ = tx.send(());
            }
        }
    }
    tracing::warn!("build_finished listener task exiting");
}
