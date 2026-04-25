use std::sync::Arc;

use hashbrown::HashMap;

/// An item that has been dispatched to a machine and is currently building.
#[derive(Debug, Clone)]
pub struct ScheduledItem {
    pub drv_path: nix_utils::StorePath,
    pub resolved_drv_path: Option<nix_utils::StorePath>,
    pub machine: Arc<super::Machine>,
}

/// Tracks the set of currently-scheduled (dispatched) steps.
/// The persistent queue of ready steps lives in the `BuildStepCanCreate` DB table;
/// this struct only tracks what has been dispatched.
#[derive(Debug)]
pub(super) struct InnerQueues {
    scheduled: parking_lot::RwLock<HashMap<nix_utils::StorePath, ScheduledItem>>,
}

impl Default for InnerQueues {
    fn default() -> Self {
        Self::new()
    }
}

impl InnerQueues {
    fn new() -> Self {
        Self {
            scheduled: parking_lot::RwLock::new(HashMap::with_capacity(100)),
        }
    }

    #[tracing::instrument(skip(self, item))]
    fn add_job_to_scheduled(&self, item: ScheduledItem) {
        self.scheduled.write().insert(item.drv_path.clone(), item);
    }

    #[tracing::instrument(skip(self), fields(%drv))]
    fn remove_job_from_scheduled(&self, drv: &nix_utils::StorePath) -> Option<ScheduledItem> {
        self.scheduled.write().remove(drv)
    }

    #[tracing::instrument(skip(self))]
    async fn kill_active_steps(
        &self,
        db: &db::Database,
    ) -> Vec<(nix_utils::StorePath, uuid::Uuid)> {
        tracing::info!("Kill all active steps");
        let active = {
            let scheduled = self.scheduled.read();
            scheduled.clone()
        };

        let mut cancelled_steps = vec![];
        for (drv_path, item) in &active {
            // Check if this step has any dependent builds via the DB
            let has_dependents = {
                let drv_path_str = drv_path.to_string();
                if let Ok(mut conn) = db.get().await {
                    conn.get_dependent_builds(&drv_path_str)
                        .await
                        .map(|builds| !builds.is_empty())
                        .unwrap_or(false)
                } else {
                    false
                }
            };
            if has_dependents {
                continue;
            }

            {
                tracing::info!("Cancelling step drv={drv_path}");

                if let Some(internal_build_id) =
                    item.machine.get_internal_build_id_for_drv(drv_path)
                {
                    if let Err(e) = item.machine.abort_build(internal_build_id).await {
                        tracing::error!(
                            "Failed to abort build drv_path={drv_path} build_id={internal_build_id} e={e}",
                        );
                        continue;
                    }
                } else {
                    tracing::warn!("No active build_id found for drv_path={drv_path}",);
                    continue;
                }

                cancelled_steps.push((drv_path.to_owned(), item.machine.id));
            }
        }
        cancelled_steps
    }

    fn get_scheduled(&self) -> Vec<ScheduledItem> {
        let s = self.scheduled.read();
        s.values().cloned().collect()
    }

    fn get_scheduled_count(&self) -> usize {
        self.scheduled.read().len()
    }
}

#[derive(Debug, Clone)]
pub struct Queues {
    inner: Arc<tokio::sync::RwLock<InnerQueues>>,
}

impl Default for Queues {
    fn default() -> Self {
        Self::new()
    }
}

impl Queues {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(tokio::sync::RwLock::new(InnerQueues::new())),
        }
    }

    #[tracing::instrument(skip(self, item))]
    pub async fn add_job_to_scheduled(&self, item: ScheduledItem) {
        let rq = self.inner.read().await;
        rq.add_job_to_scheduled(item);
    }

    #[tracing::instrument(skip(self), fields(%drv))]
    pub async fn remove_job_from_scheduled(
        &self,
        drv: &nix_utils::StorePath,
    ) -> Option<ScheduledItem> {
        let rq = self.inner.read().await;
        rq.remove_job_from_scheduled(drv)
    }

    #[tracing::instrument(skip(self))]
    pub async fn kill_active_steps(
        &self,
        db: &db::Database,
    ) -> Vec<(nix_utils::StorePath, uuid::Uuid)> {
        let rq = self.inner.read().await;
        rq.kill_active_steps(db).await
    }

    pub async fn get_scheduled(&self) -> Vec<ScheduledItem> {
        let rq = self.inner.read().await;
        rq.get_scheduled()
    }

    pub async fn get_scheduled_count(&self) -> usize {
        let rq = self.inner.read().await;
        rq.get_scheduled_count()
    }
}
