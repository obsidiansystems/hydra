mod atomic;
mod build;
mod fod_checker;
mod inspectable_channel;
mod jobset;
mod machine;
mod metrics;
pub mod queue;
mod step_info;
mod uploader;

pub use atomic::AtomicDateTime;
pub use build::{Build, BuildOutput, BuildResultState, BuildTimings, Builds, RemoteBuild};
pub use jobset::{Jobset, JobsetID, Jobsets};
pub use machine::{Machine, Message as MachineMessage, Pressure, Stats as MachineStats};
pub use queue::Queues;
pub use step_info::DispatchEntry;

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;

use futures::TryStreamExt as _;
use hashbrown::{HashMap, HashSet};
use secrecy::ExposeSecret as _;

use db::models::{BuildID, BuildStatus};
use inspectable_channel::InspectableChannel;
use nix_utils::BaseStore as _;

use crate::config::{App, Cli};
use crate::state::build::get_mark_build_sccuess_data;
pub use crate::state::fod_checker::FodChecker;
use crate::state::machine::Machines;
use crate::utils::finish_build_step;

pub type System = String;

enum CreateStepResult {
    None,
    Valid(nix_utils::StorePath),
    PreviousFailure(nix_utils::StorePath),
}

// No longer used publicly; dispatch is done differently now.

#[allow(missing_debug_implementations)]
pub enum RemoteStoreBackend {
    S3(binary_cache::S3BinaryCacheClient),
    Nix(nix_utils::RemoteStore),
}

#[allow(missing_debug_implementations)]
pub struct State {
    pub store: nix_utils::LocalStore,
    pub remote_stores: parking_lot::RwLock<Vec<RemoteStoreBackend>>,
    pub config: App,
    pub cli: Cli,
    pub db: db::Database,

    pub machines: Machines,

    pub log_dir: std::path::PathBuf,

    pub builds: Builds,
    pub jobsets: Jobsets,
    pub queues: Queues,

    pub fod_checker: Option<Arc<FodChecker>>,

    pub started_at: jiff::Timestamp,

    pub metrics: metrics::PromMetrics,
    pub notify_dispatch: tokio::sync::Notify,
    pub uploader: uploader::Uploader,
}

impl State {
    #[tracing::instrument(skip(tracing_guard), err)]
    pub async fn new(tracing_guard: &hydra_tracing::TracingGuard) -> anyhow::Result<Arc<Self>> {
        let store = nix_utils::LocalStore::init();
        nix_utils::set_verbosity(1);
        tracing::info!("LocalStore dir={}", nix_utils::get_store_dir());

        let cli = Cli::new();
        if cli.status {
            tracing_guard.change_log_level(hydra_tracing::EnvFilter::new("error"));
        }

        let config = App::init(&cli.config_path)?;
        let log_dir = config.get_hydra_log_dir();
        let db = db::Database::new(
            config.get_db_url().expose_secret(),
            config.get_max_db_connections(),
        )
        .await?;

        match fs_err::tokio::create_dir_all(&log_dir).await {
            Ok(()) => tracing::info!("successfully created hydra log_dir={log_dir:?}"),
            Err(e) => tracing::error!("Failed to create hydra log_dir={log_dir:?} e={e}"),
        }

        let mut remote_stores = vec![];
        for uri in config.get_remote_store_addrs() {
            if let Ok(cfg) = uri.parse::<binary_cache::S3CacheConfig>() {
                remote_stores.push(RemoteStoreBackend::S3(
                    binary_cache::S3BinaryCacheClient::new(cfg).await?,
                ));
            } else {
                tracing::info!("Opening FFI store for: {uri}");
                remote_stores.push(RemoteStoreBackend::Nix(nix_utils::RemoteStore::init(&uri)));
            }
        }

        Ok(Arc::new(Self {
            store,
            remote_stores: parking_lot::RwLock::new(remote_stores),
            cli,
            db,
            machines: Machines::new(),
            log_dir,
            builds: Builds::new(),
            jobsets: Jobsets::new(),
            queues: Queues::new(),
            fod_checker: if config.get_enable_fod_checker() {
                Some(Arc::new(FodChecker::new(None)))
            } else {
                None
            },
            started_at: jiff::Timestamp::now(),
            metrics: metrics::PromMetrics::new()?,
            notify_dispatch: tokio::sync::Notify::new(),
            uploader: uploader::Uploader::new(
                config.get_hydra_data_dir().join("uploader_state.json"),
            )
            .await,
            config,
        }))
    }

    #[tracing::instrument(skip(self, new_config), err)]
    pub async fn reload_config_callback(
        &self,
        new_config: &crate::config::PreparedApp,
    ) -> anyhow::Result<()> {
        // IF this gets more complex we need a way to trap the state and revert.
        // right now it doesnt matter because only reconfigure_pool can fail and this is the first
        // thing we do.

        let curr_db_url = self.config.get_db_url();
        let curr_machine_sort_fn = self.config.get_machine_sort_fn();
        let _curr_step_sort_fn = self.config.get_step_sort_fn();
        let curr_remote_stores = self.config.get_remote_store_addrs();
        let curr_enable_fod_checker = self.config.get_enable_fod_checker();
        let mut new_remote_stores = vec![];
        if curr_remote_stores != new_config.remote_store_addr {
            for uri in &new_config.remote_store_addr {
                if let Ok(cfg) = uri.parse::<binary_cache::S3CacheConfig>() {
                    new_remote_stores.push(RemoteStoreBackend::S3(
                        binary_cache::S3BinaryCacheClient::new(cfg).await?,
                    ));
                } else {
                    tracing::info!("Opening FFI store for: {uri}");
                    new_remote_stores
                        .push(RemoteStoreBackend::Nix(nix_utils::RemoteStore::init(uri)));
                }
            }
        }

        if curr_db_url.expose_secret() != new_config.db_url.expose_secret() {
            self.db
                .reconfigure_pool(new_config.db_url.expose_secret())?;
        }
        if curr_machine_sort_fn != new_config.machine_sort_fn {
            self.machines.sort(new_config.machine_sort_fn);
        }
        // Step sort fn is now applied at dispatch time from DB query results;
        // no in-memory queue to re-sort.
        if curr_remote_stores != new_config.remote_store_addr {
            *self.remote_stores.write() = new_remote_stores;
        }

        if curr_enable_fod_checker != new_config.enable_fod_checker {
            tracing::warn!(
                "Changing the value of enable_fod_checker currently requires a restart!"
            );
        }

        self.machines
            .publish_new_config(machine::ConfigUpdate {
                max_concurrent_downloads: new_config.max_concurrent_downloads,
            })
            .await;

        Ok(())
    }

    #[tracing::instrument(skip(self, machine))]
    pub async fn insert_machine(&self, machine: Machine) -> uuid::Uuid {
        let machine_id = self
            .machines
            .insert_machine(machine, self.config.get_machine_sort_fn());
        self.trigger_dispatch();
        machine_id
    }

    #[tracing::instrument(skip(self))]
    pub async fn remove_machine(&self, machine_id: uuid::Uuid) {
        if let Some(m) = self.machines.remove_machine(machine_id) {
            let jobs = {
                let jobs = m.jobs.read();
                jobs.clone()
            };
            for job in &jobs {
                if let Err(e) = self
                    .fail_step(
                        machine_id,
                        &job.path,
                        // we fail this with preparing because we kinda want to restart all jobs if
                        // a machine is removed
                        BuildResultState::PreparingFailure,
                        BuildTimings::default(),
                    )
                    .await
                {
                    tracing::error!(
                        "Failed to fail step machine_id={machine_id} drv={} e={e}",
                        job.path
                    );
                }
            }
        }
    }

    pub async fn remove_all_machines(&self) {
        for m in self.machines.get_all_machines() {
            self.remove_machine(m.id).await;
        }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn clear_busy(&self) -> anyhow::Result<()> {
        let mut db = self.db.get().await?;
        db.clear_busy(0).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, entry, machine), fields(drv=%entry.drv_path), err)]
    #[allow(clippy::too_many_lines)]
    async fn realise_drv_on_valid_machine(
        self: Arc<Self>,
        entry: &DispatchEntry,
        machine: Arc<Machine>,
    ) -> anyhow::Result<bool> {
        let drv = &entry.drv_path;
        let drv_path_str = self.store.print_store_path(drv);
        let mut build_options = nix_utils::BuildOptions::new(None);

        // Get dependent builds from DB
        let dependents = {
            let mut conn = self.db.get().await?;
            conn.get_dependent_builds(&drv_path_str).await?
        };

        if dependents.is_empty() {
            tracing::info!("maybe cancelling build step {drv} - no dependents");
            if let Ok(mut conn) = self.db.get().await {
                if let Ok(mut tx) = conn.begin_transaction().await {
                    let _ = tx.unmark_step_ready(&drv_path_str).await;
                    let _ = tx.commit().await;
                }
            }
            return Ok(false);
        }

        let build = dependents
            .iter()
            .find(|b| b.drvpath == drv_path_str)
            .or_else(|| dependents.first())
            .unwrap(); // safe: checked is_empty above

        let biggest_max_silent_time = dependents.iter().map(|x| x.maxsilent.unwrap_or(3600)).max();
        let biggest_build_timeout = dependents.iter().map(|x| x.timeout.unwrap_or(36000)).max();

        build_options.set_max_silent_time(biggest_max_silent_time.unwrap_or(3600));
        build_options.set_build_timeout(biggest_build_timeout.unwrap_or(36000));
        let mut job = machine::Job::new(drv.to_owned(), entry.resolved_drv_path.clone());
        job.result.set_start_time_now();

        // Check cached failure
        if self.check_cached_failure_by_drv(drv).await {
            job.result.step_status = BuildStatus::CachedFailure;
            self.inner_fail_job_by_drv(drv, None, job).await?;
            return Ok(false);
        }

        self.construct_log_file_path(drv)
            .await?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("failed to construct log path string."))?
            .clone_into(&mut job.result.log_file);

        // Parse the derivation to get system and output paths for the build step
        let drv_parsed = nix_utils::query_drv(&self.store, drv)
            .await?
            .ok_or_else(|| anyhow::anyhow!("derivation not found when dispatching: {drv}"))?;
        let system_str = std::str::from_utf8(&drv_parsed.platform)
            .expect("platform must be valid UTF-8")
            .to_owned();
        let output_paths = nix_utils::output_paths(&drv_parsed, self.store.store_dir());

        let mut db = self.db.get().await?;
        let attempt = {
            let mut tx = db.begin_transaction().await?;

            let attempt = tx
                .create_build_step(
                    self.store.store_dir(),
                    Some(job.result.get_start_time_as_i32()?),
                    drv,
                    Some(&system_str),
                    machine.hostname.clone(),
                    BuildStatus::Busy,
                    None,
                    None,
                    output_paths.clone(),
                )
                .await?;

            // Remove from BuildStepCanCreate now that we've dispatched
            tx.unmark_step_ready(&drv_path_str).await?;
            tx.commit().await?;
            attempt
        };
        job.attempt = Some(attempt);

        {
            let mut tx = db.begin_transaction().await?;
            tx.notify_build_started(build.id).await?;
            tx.commit().await?;
        }
        tracing::info!(
            "Submitting build drv={drv} on machine={} hostname={} attempt={attempt}",
            machine.id,
            machine.hostname
        );
        self.db
            .get()
            .await?
            .update_build_step(
                self.store.store_dir(),
                db::models::UpdateBuildStep {
                    drv_path: drv,
                    attempt,
                    status: db::models::StepStatus::Connecting,
                },
            )
            .await?;

        // Add to scheduled map
        self.queues
            .add_job_to_scheduled(queue::ScheduledItem {
                drv_path: drv.to_owned(),
                resolved_drv_path: entry.resolved_drv_path.clone(),
                machine: machine.clone(),
            })
            .await;

        machine
            .build_drv(
                job,
                &build_options,
                if self.config.use_presigned_uploads() {
                    let remote_stores = self.remote_stores.read();
                    remote_stores.iter().find_map(|s| match s {
                        RemoteStoreBackend::S3(s) => Some(machine::PresignedUrlOpts {
                            upload_debug_info: s.cfg.write_debug_info,
                        }),
                        _ => None,
                    })
                } else {
                    None
                },
            )
            .await?;
        self.metrics.nr_steps_started.inc();
        self.metrics.nr_steps_building.add(1);
        Ok(true)
    }

    #[tracing::instrument(skip(self), fields(%drv), err)]
    async fn construct_log_file_path(
        &self,
        drv: &nix_utils::StorePath,
    ) -> anyhow::Result<std::path::PathBuf> {
        let mut log_file = self.log_dir.clone();
        let base = drv.to_string();
        let (dir, file) = base.split_at(2);
        log_file.push(format!("{dir}/"));
        let _ = fs_err::tokio::create_dir_all(&log_file).await; // create dir
        log_file.push(file);
        Ok(log_file)
    }

    #[tracing::instrument(skip(self), fields(%drv), err)]
    pub async fn new_log_file(
        &self,
        drv: &nix_utils::StorePath,
    ) -> anyhow::Result<fs_err::tokio::File> {
        let log_file = self.construct_log_file_path(drv).await?;
        tracing::debug!("opening {log_file:?}");

        Ok(fs_err::tokio::File::options()
            .create(true)
            .truncate(true)
            .write(true)
            .read(false)
            .mode(0o666)
            .open(log_file)
            .await?)
    }

    #[tracing::instrument(skip(self, new_ids, new_builds_by_id, new_builds_by_path))]
    async fn process_new_builds(
        &self,
        new_ids: Vec<BuildID>,
        new_builds_by_id: Arc<parking_lot::RwLock<HashMap<BuildID, Arc<Build>>>>,
        new_builds_by_path: HashMap<nix_utils::StorePath, HashSet<BuildID>>,
    ) {
        let finished_drvs = Arc::new(parking_lot::RwLock::new(
            HashSet::<nix_utils::StorePath>::new(),
        ));

        let starttime = jiff::Timestamp::now();
        for id in new_ids {
            let Some(build) = new_builds_by_id.read().get(&id).cloned() else {
                continue;
            };

            let nr_added: Arc<AtomicI64> = Arc::new(0.into());
            let now = Instant::now();

            Box::pin(self.create_build(
                build,
                nr_added.clone(),
                new_builds_by_id.clone(),
                &new_builds_by_path,
                finished_drvs.clone(),
            ))
            .await;

            #[allow(clippy::cast_possible_truncation)]
            self.metrics
                .build_read_time_ms
                .inc_by(now.elapsed().as_millis() as u64);

            if let Ok(added_u64) = u64::try_from(nr_added.load(Ordering::Relaxed)) {
                self.metrics.nr_builds_read.inc_by(added_u64);
            }
            let stop_queue_run_after = self.config.get_stop_queue_run_after();

            if let Some(stop_queue_run_after) = stop_queue_run_after
                && jiff::Timestamp::now() > (starttime + stop_queue_run_after)
            {
                self.metrics.queue_checks_early_exits.inc();
                break;
            }
        }

        self.metrics.queue_checks_finished.inc();
        self.trigger_dispatch();
        if let Some(fod_checker) = &self.fod_checker {
            fod_checker.trigger_traverse();
        }
    }

    #[tracing::instrument(skip(self), err)]
    async fn process_queue_change(&self) -> anyhow::Result<()> {
        let mut db = self.db.get().await?;
        let curr_ids: HashMap<_, _> = db
            .get_not_finished_builds_fast()
            .await?
            .into_iter()
            .map(|b| (b.id, b.globalpriority))
            .collect();
        self.builds.update_priorities(&curr_ids);

        let cancelled_steps = self.queues.kill_active_steps(&self.db).await;
        for (drv_path, machine_id) in cancelled_steps {
            if let Err(e) = self
                .fail_step(
                    machine_id,
                    &drv_path,
                    BuildResultState::Cancelled,
                    BuildTimings::default(),
                )
                .await
            {
                tracing::error!(
                    "Failed to abort step machine_id={machine_id} drv={drv_path} e={e}",
                );
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(%drv_path))]
    pub async fn queue_one_build(
        &self,
        jobset_id: i32,
        drv_path: &nix_utils::StorePath,
    ) -> anyhow::Result<()> {
        let mut db = self.db.get().await?;
        let drv = nix_utils::query_drv(&self.store, drv_path)
            .await?
            .ok_or_else(|| anyhow::anyhow!("drv not found"))?;
        db.insert_debug_build(
            self.store.store_dir(),
            jobset_id,
            drv_path,
            std::str::from_utf8(&drv.platform).expect("platform must be valid UTF-8"),
        )
        .await?;

        let mut tx = db.begin_transaction().await?;
        tx.notify_builds_added().await?;
        tx.commit().await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub(crate) async fn manually_add_queue_build(&self, build_id: BuildID) -> anyhow::Result<()> {
        let mut new_ids = Vec::<BuildID>::new();
        let mut new_builds_by_id = HashMap::<BuildID, Arc<Build>>::new();
        let mut new_builds_by_path = HashMap::<nix_utils::StorePath, HashSet<BuildID>>::new();

        {
            let mut conn = self.db.get().await?;
            for b in conn
                .get_not_finished_builds(self.store.store_dir())
                .await?
                .into_iter()
                .filter(|b| b.id == build_id)
            {
                let jobset = self
                    .jobsets
                    .create(&mut conn, b.jobset_id, &b.project, &b.jobset)
                    .await?;
                let build = Build::new(b, jobset)?;
                new_ids.push(build.id);
                new_builds_by_id.insert(build.id, build.clone());
                new_builds_by_path
                    .entry(build.drv_path.clone())
                    .or_insert_with(HashSet::new)
                    .insert(build.id);
            }
        }
        tracing::debug!("new_ids: {new_ids:?}");
        tracing::debug!("new_builds_by_id: {new_builds_by_id:?}");
        tracing::debug!("new_builds_by_path: {new_builds_by_path:?}");

        if new_ids.is_empty() {
            return Ok(());
        }

        let new_builds_by_id = Arc::new(parking_lot::RwLock::new(new_builds_by_id));
        Box::pin(self.process_new_builds(new_ids, new_builds_by_id, new_builds_by_path)).await;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_queued_builds(&self) -> anyhow::Result<()> {
        self.metrics.queue_checks_started.inc();

        let mut new_ids = Vec::<BuildID>::with_capacity(1000);
        let mut new_builds_by_id = HashMap::<BuildID, Arc<Build>>::with_capacity(1000);
        let mut new_builds_by_path =
            HashMap::<nix_utils::StorePath, HashSet<BuildID>>::with_capacity(1000);

        {
            let mut conn = self.db.get().await?;
            for b in conn.get_not_finished_builds(self.store.store_dir()).await? {
                let jobset = self
                    .jobsets
                    .create(&mut conn, b.jobset_id, &b.project, &b.jobset)
                    .await?;
                let build = Build::new(b, jobset)?;
                new_ids.push(build.id);
                new_builds_by_id.insert(build.id, build.clone());
                new_builds_by_path
                    .entry(build.drv_path.clone())
                    .or_insert_with(HashSet::new)
                    .insert(build.id);
            }
        }
        tracing::debug!("new_ids: {new_ids:?}");
        tracing::debug!("new_builds_by_id: {new_builds_by_id:?}");
        tracing::debug!("new_builds_by_path: {new_builds_by_path:?}");

        let new_builds_by_id = Arc::new(parking_lot::RwLock::new(new_builds_by_id));
        Box::pin(self.process_new_builds(new_ids, new_builds_by_id, new_builds_by_path)).await;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub fn start_queue_monitor_loop(self: Arc<Self>) -> tokio::task::AbortHandle {
        let task = tokio::task::spawn({
            async move {
                if let Err(e) = Box::pin(self.queue_monitor_loop()).await {
                    tracing::error!("Failed to spawn queue monitor loop. e={e}");
                }
            }
        });
        task.abort_handle()
    }

    #[tracing::instrument(skip(self), err)]
    async fn queue_monitor_loop(&self) -> anyhow::Result<()> {
        let mut listener = self
            .db
            .listener(vec![
                "builds_added",
                "builds_restarted",
                "builds_cancelled",
                "builds_deleted",
                "builds_bumped",
                "jobset_shares_changed",
            ])
            .await?;

        loop {
            let before_work = Instant::now();
            self.store.clear_path_info_cache();
            if let Err(e) = self.get_queued_builds().await {
                tracing::error!("get_queue_builds failed inside queue monitor loop: {e}");
                continue;
            }

            #[allow(clippy::cast_possible_truncation)]
            self.metrics
                .queue_monitor_time_spent_running
                .inc_by(before_work.elapsed().as_micros() as u64);

            let before_sleep = Instant::now();
            let queue_trigger_timer = self.config.get_queue_trigger_timer();
            let notification = if let Some(timer) = queue_trigger_timer {
                tokio::select! {
                    () = tokio::time::sleep(timer) => {"timer_reached".into()},
                    v = listener.try_next() => match v {
                        Ok(Some(v)) => v.channel().to_owned(),
                        Ok(None) => continue,
                        Err(e) => {
                            tracing::warn!("PgListener failed with e={e}");
                            continue;
                        }
                    },
                }
            } else {
                match listener.try_next().await {
                    Ok(Some(v)) => v.channel().to_owned(),
                    Ok(None) => continue,
                    Err(e) => {
                        tracing::warn!("PgListener failed with e={e}");
                        continue;
                    }
                }
            };
            self.metrics.nr_queue_wakeups.inc();
            tracing::trace!("New notification from PgListener. notification={notification:?}");

            match notification.as_ref() {
                "builds_added" => {
                    tracing::debug!("got notification: new builds added to the queue");
                }
                "builds_restarted" => tracing::debug!("got notification: builds restarted"),
                "builds_cancelled" | "builds_deleted" | "builds_bumped" => {
                    tracing::info!("got notification: builds cancelled or bumped");
                    if let Err(e) = self.process_queue_change().await {
                        tracing::error!("Failed to process queue change. e={e}");
                    }
                }
                "jobset_shares_changed" => {
                    tracing::info!("got notification: jobset shares changed");
                    match self.db.get().await {
                        Ok(mut conn) => {
                            if let Err(e) = self.jobsets.handle_change(&mut conn).await {
                                tracing::error!("Failed to handle jobset change. e={e}");
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to get db connection for event 'jobset_shares_changed'. e={e}"
                            );
                        }
                    }
                }
                _ => (),
            }

            #[allow(clippy::cast_possible_truncation)]
            self.metrics
                .queue_monitor_time_spent_waiting
                .inc_by(before_sleep.elapsed().as_micros() as u64);
        }
    }

    #[tracing::instrument(skip(self))]
    pub fn start_dispatch_loop(self: Arc<Self>) -> tokio::task::AbortHandle {
        let task = tokio::task::spawn({
            async move {
                loop {
                    let before_sleep = Instant::now();
                    let dispatch_trigger_timer = self.config.get_dispatch_trigger_timer();
                    if let Some(timer) = dispatch_trigger_timer {
                        tokio::select! {
                            () = self.notify_dispatch.notified() => {},
                            () = tokio::time::sleep(timer) => {},
                        };
                    } else {
                        self.notify_dispatch.notified().await;
                    }
                    tracing::info!("starting dispatch");

                    #[allow(clippy::cast_possible_truncation)]
                    self.metrics
                        .dispatcher_time_spent_waiting
                        .inc_by(before_sleep.elapsed().as_micros() as u64);

                    self.metrics.nr_dispatcher_wakeups.inc();
                    let before_work = Instant::now();
                    self.clone().do_dispatch_once().await;

                    let elapsed = before_work.elapsed();

                    #[allow(clippy::cast_possible_truncation)]
                    self.metrics
                        .dispatcher_time_spent_running
                        .inc_by(elapsed.as_micros() as u64);

                    #[allow(clippy::cast_possible_truncation)]
                    self.metrics
                        .dispatch_time_ms
                        .inc_by(elapsed.as_millis() as u64);
                }
            }
        });
        task.abort_handle()
    }

    #[tracing::instrument(skip(self), err)]
    async fn dump_status_loop(self: Arc<Self>) -> anyhow::Result<()> {
        let mut listener = self.db.listener(vec!["dump_status"]).await?;

        let state = self.clone();
        loop {
            let _ = match listener.try_next().await {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!("PgListener failed with e={e}");
                    continue;
                }
            };

            let state = state.clone();
            let queue_stats = crate::io::QueueRunnerStats::new(state.clone()).await;
            let sort_fn = state.config.get_machine_sort_fn();
            let free_fn = state.config.get_machine_free_fn();
            let machines = state
                .machines
                .get_all_machines()
                .into_iter()
                .map(|m| {
                    (
                        m.hostname.clone(),
                        crate::io::Machine::from_state(&m, sort_fn, free_fn),
                    )
                })
                .collect();
            let jobsets = self.jobsets.clone_as_io();
            let s3_stores: Vec<binary_cache::S3BinaryCacheClient> = {
                let stores = state.remote_stores.read();
                stores
                    .iter()
                    .filter_map(|s| match s {
                        RemoteStoreBackend::S3(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect()
            };
            let dump_status = crate::io::DumpResponse::new(
                queue_stats,
                machines,
                jobsets,
                &state.store,
                &s3_stores,
            );
            {
                let Ok(mut db) = self.db.get().await else {
                    continue;
                };
                let Ok(mut tx) = db.begin_transaction().await else {
                    continue;
                };
                let dump_status = match serde_json::to_value(dump_status) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("Failed to update status in database: {e}");
                        continue;
                    }
                };
                if let Err(e) = tx.upsert_status(&dump_status).await {
                    tracing::error!("Failed to update status in database: {e}");
                    continue;
                }
                if let Err(e) = tx.notify_status_dumped().await {
                    tracing::error!("Failed to update status in database: {e}");
                    continue;
                }
                if let Err(e) = tx.commit().await {
                    tracing::error!("Failed to update status in database: {e}");
                }
            }
        }
    }

    #[tracing::instrument(skip(self))]
    pub fn start_dump_status_loop(self: Arc<Self>) -> tokio::task::AbortHandle {
        let task = tokio::task::spawn({
            async move {
                if let Err(e) = self.dump_status_loop().await {
                    tracing::error!("Failed to spawn queue monitor loop. e={e}");
                }
            }
        });
        task.abort_handle()
    }

    #[tracing::instrument(skip(self))]
    pub fn start_uploader_queue(self: Arc<Self>) -> tokio::task::AbortHandle {
        let task = tokio::task::spawn({
            async move {
                loop {
                    let local_store = nix_utils::LocalStore::init();
                    let s3_stores: Vec<binary_cache::S3BinaryCacheClient> = {
                        let r = self.remote_stores.read();
                        r.iter()
                            .filter_map(|s| match s {
                                RemoteStoreBackend::S3(s) => Some(s.clone()),
                                _ => None,
                            })
                            .collect()
                    };
                    let limit = self.config.get_concurrent_upload_limit();
                    if limit < 2 {
                        self.uploader.upload_once(local_store, s3_stores).await;
                    } else {
                        self.uploader
                            .upload_many(local_store, s3_stores, limit)
                            .await;
                    }
                }
            }
        });
        task.abort_handle()
    }

    #[tracing::instrument(skip(self))]
    pub async fn get_status_from_main_process(self: Arc<Self>) -> anyhow::Result<()> {
        let mut db = self.db.get().await?;

        let mut listener = self.db.listener(vec!["status_dumped"]).await?;
        {
            let mut tx = db.begin_transaction().await?;
            tx.notify_dump_status().await?;
            tx.commit().await?;
        }

        let notification =
            tokio::time::timeout(tokio::time::Duration::from_secs(5), listener.try_next()).await;

        match notification {
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) => return Ok(()),
            Ok(Err(e)) => {
                tracing::warn!("PgListener failed with e={e}");
                return Ok(());
            }
            Err(_) => {
                // No response from queue-runner daemon — print a down status.
                println!(r#"{{"status":"down"}}"#);
                return Ok(());
            }
        }

        if let Some(status) = db.get_status().await? {
            // we want a println! here so it can be consumed by other tools
            println!("{}", serde_json::to_string_pretty(&status)?);
        }

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub fn trigger_dispatch(&self) {
        self.notify_dispatch.notify_one();
    }

    #[tracing::instrument(skip(self))]
    async fn do_dispatch_once(self: Arc<Self>) {
        // Prune old historical build step info from the jobsets.
        self.jobsets.prune();

        // Query all dispatch candidates from the DB
        let candidates = match self.db.get().await {
            Ok(mut conn) => match conn.get_dispatch_candidates().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to get dispatch candidates: {e}");
                    return;
                }
            },
            Err(e) => {
                tracing::error!("Failed to get DB connection for dispatch: {e}");
                return;
            }
        };

        let free_fn = self.config.get_machine_free_fn();
        let sort_fn = self.config.get_step_sort_fn();

        // Build DispatchEntry for each candidate
        let mut entries = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            // Parse derivation to get system + required_features
            let store_dir = self.store.store_dir();
            let drv_path = match store_dir.parse(&candidate.drv_path) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse store path from dispatch candidate `{}`: {e}",
                        candidate.drv_path
                    );
                    continue;
                }
            };
            let Some(drv) = nix_utils::query_drv(&self.store, &drv_path)
                .await
                .ok()
                .flatten()
            else {
                tracing::warn!("Failed to read derivation for dispatch candidate: {drv_path}");
                continue;
            };
            let system = std::str::from_utf8(&drv.platform)
                .expect("platform must be valid UTF-8")
                .to_owned();
            let required_features = drv
                .env
                .get(b"requiredSystemFeatures".as_slice())
                .and_then(|v| std::str::from_utf8(v).ok())
                .map(|v| {
                    v.split(' ')
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default();

            // Compute lowest_share_used from jobsets
            // TODO: this is approximate since we don't track step->jobset mapping in the DB yet
            let lowest_share_used = 1e9_f64;

            // Try to resolve CA derivation
            let resolved_drv_path =
                match step_info::try_resolve(self.store.store_dir(), &self.db, &drv).await {
                    Some(ref basic_drv) => self.store.write_derivation(basic_drv).await.ok(),
                    None => None,
                };

            entries.push(DispatchEntry {
                drv_path,
                resolved_drv_path,
                system,
                required_features,
                ready_time: candidate.ready_time,
                highest_global_priority: candidate.highest_global_priority,
                highest_local_priority: candidate.highest_local_priority,
                lowest_build_id: candidate.lowest_build_id,
                rdeps_count: candidate.rdeps_count,
                lowest_share_used,
            });
        }

        // Sort entries by scheduling priority
        let cmp_fn = match sort_fn {
            crate::config::StepSortFn::Legacy => DispatchEntry::legacy_compare,
            crate::config::StepSortFn::WithRdeps => DispatchEntry::compare_with_rdeps,
        };
        entries.sort_by(|a, b| cmp_fn(a, b));

        let mut nr_steps_waiting: i64 = 0;
        for entry in &entries {
            // Try to find a matching machine
            if let Some(machine) = self.machines.get_machine_for_system(
                &entry.system,
                &entry.required_features,
                Some(free_fn),
            ) {
                match Box::pin(self.clone().realise_drv_on_valid_machine(entry, machine)).await {
                    Ok(true) => {
                        // Successfully dispatched
                    }
                    Ok(false) => {
                        // Cancelled or cached failure, already handled
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to realise drv on valid machine: drv={} e={e}",
                            entry.drv_path,
                        );
                    }
                }
            } else {
                tracing::debug!(
                    "No free machine found for system={} drv={}",
                    entry.system,
                    entry.drv_path,
                );
                nr_steps_waiting += 1;
            }
        }
        self.metrics.nr_steps_waiting.set(nr_steps_waiting);

        self.abort_unsupported().await;
    }

    #[tracing::instrument(skip(self, step_status), fields(%build_id, %machine_id), err)]
    pub async fn update_build_step(
        &self,
        build_id: uuid::Uuid,
        machine_id: uuid::Uuid,
        step_status: db::models::StepStatus,
    ) -> anyhow::Result<()> {
        let drv_and_attempt = self.machines.get_machine_by_id(machine_id).and_then(|m| {
            tracing::debug!(
                "get job from machine by build_id: build_id={build_id} m={}",
                m.id
            );
            m.get_drv_path_and_attempt_by_uuid(build_id)
        });

        let Some((drv_path, attempt)) = drv_and_attempt else {
            tracing::warn!(
                "Failed to find job for build_id={build_id:?} machine_id={machine_id:?}."
            );
            return Ok(());
        };
        self.db
            .get()
            .await?
            .update_build_step(
                self.store.store_dir(),
                db::models::UpdateBuildStep {
                    drv_path: &drv_path,
                    attempt,
                    status: step_status,
                },
            )
            .await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(self, output), fields(%machine_id, %drv_path), err)]
    pub async fn succeed_step(
        &self,
        machine_id: uuid::Uuid,
        drv_path: &nix_utils::StorePath,
        output: BuildOutput,
    ) -> anyhow::Result<()> {
        tracing::info!("marking job as done: drv_path={drv_path}");
        let item = self
            .queues
            .remove_job_from_scheduled(drv_path)
            .await
            .ok_or_else(|| anyhow::anyhow!("Step is missing in queues.scheduled"))?;

        // Parse derivation to verify output paths
        let drv_path_str = self.store.print_store_path(drv_path);
        if let Some(drv) = nix_utils::query_drv(&self.store, drv_path)
            .await
            .ok()
            .flatten()
        {
            let expected = nix_utils::output_paths(&drv, self.store.store_dir());
            for (name, expected_path) in &expected {
                let Some(expected_path) = expected_path else {
                    continue;
                };
                if let Some(actual_path) = output.outputs.get(name) {
                    anyhow::ensure!(
                        expected_path == actual_path,
                        "output path mismatch for output `{name}` of {drv_path}: \
                         expected {}, got {}",
                        self.store.print_store_path(expected_path),
                        self.store.print_store_path(actual_path),
                    );
                }
            }
        }

        tracing::debug!(
            "removing job from machine: drv_path={drv_path} m={}",
            item.machine.id
        );
        let mut job = item
            .machine
            .remove_job(drv_path)
            .ok_or_else(|| anyhow::anyhow!("Job is missing in machine.jobs m={}", item.machine,))?;

        job.result.step_status = BuildStatus::Success;
        job.result.set_stop_time_now();
        job.result.set_overhead(output.timings.get_overhead())?;

        let total_step_time = job.result.get_total_step_time_ms();
        item.machine
            .stats
            .track_build_success(output.timings, total_step_time);
        self.metrics
            .track_build_success(output.timings, total_step_time);

        finish_build_step(
            &self.db,
            &self.store,
            &job.path,
            job.attempt.expect("attempt set after create_build_step"),
            &job.result,
            Some(&item.machine.hostname),
            Some(&output.outputs),
        )
        .await?;

        // Copy outputs to non-S3 (FFI) stores
        {
            let ffi_base_stores: Vec<(String, nix_utils::BaseStoreImpl)> = {
                let stores = self.remote_stores.read();
                stores
                    .iter()
                    .filter_map(|s| match s {
                        RemoteStoreBackend::Nix(s) => {
                            Some((s.uri.clone(), s.as_base_store().clone()))
                        }
                        _ => None,
                    })
                    .collect()
            };
            let outputs_to_copy = output.outputs.values().cloned().collect::<Vec<_>>();
            for (uri, base_store) in &ffi_base_stores {
                if let Err(e) = nix_utils::copy_paths(
                    self.store.as_base_store(),
                    base_store,
                    &outputs_to_copy,
                    false,
                    false,
                    false,
                )
                .await
                {
                    tracing::error!("Failed to copy outputs to store {uri}: {e}");
                }
            }
        }

        let has_s3_stores = {
            let r = self.remote_stores.read();
            r.iter().any(|s| matches!(s, RemoteStoreBackend::S3(_)))
        };
        if has_s3_stores {
            if !self.config.use_presigned_uploads() {
                let outputs_to_upload = output
                    .outputs
                    .values()
                    .map(Clone::clone)
                    .collect::<Vec<_>>();

                self.uploader
                    .schedule_upload(
                        outputs_to_upload,
                        format!("log/{}", job.path),
                        job.result.log_file.clone(),
                    )
                    .await;
            }
        }

        // Query direct builds from the DB
        let direct_builds: Vec<Arc<Build>> = {
            let builds = self.builds.clone_matching_drv(drv_path);
            builds
        };

        {
            let mut db = self.db.get().await?;
            let mut tx = db.begin_transaction().await?;
            let attempt = job.attempt.expect("attempt set after create_build_step");
            let owning_build_id = tx
                .get_build_id_for_step(self.store.store_dir(), &job.path, attempt)
                .await?;
            let start_time = job.result.get_start_time_as_i32()?;
            let stop_time = job.result.get_stop_time_as_i32()?;
            for b in &direct_builds {
                let is_cached = owning_build_id != Some(b.id) || job.result.is_cached;
                tx.mark_succeeded_build(
                    get_mark_build_sccuess_data(&self.store, b, &output),
                    is_cached,
                    start_time,
                    stop_time,
                    self.store.store_dir(),
                )
                .await?;
                self.metrics.nr_builds_done.inc();
            }

            tx.commit().await?;
        }

        for b in &direct_builds {
            b.set_finished_in_db(true);
            self.builds.remove_by_id(b.id);
        }

        {
            let mut db = self.db.get().await?;
            let mut tx = db.begin_transaction().await?;
            for b in &direct_builds {
                tx.notify_build_finished(b.id, &[]).await?;
            }

            tx.commit().await?;
        }

        // Make reverse deps runnable in the DB
        {
            #[allow(clippy::cast_possible_truncation)]
            let ready_time = jiff::Timestamp::now().as_second() as i32;
            if let Ok(mut conn) = self.db.get().await {
                if let Ok(mut tx) = conn.begin_transaction().await {
                    let _ = tx.make_rdeps_runnable(&drv_path_str, ready_time).await;
                    let _ = tx.commit().await;
                }
            }
        }

        // always trigger dispatch, as we now might have a free machine again
        self.trigger_dispatch();

        Ok(())
    }

    #[tracing::instrument(skip(self), fields(%machine_id, %drv_path), err)]
    pub async fn fail_step(
        &self,
        machine_id: uuid::Uuid,
        drv_path: &nix_utils::StorePath,
        state: BuildResultState,
        timings: BuildTimings,
    ) -> anyhow::Result<()> {
        tracing::info!("removing job from running in system queue: drv_path={drv_path}");
        let item = self
            .queues
            .remove_job_from_scheduled(drv_path)
            .await
            .ok_or_else(|| anyhow::anyhow!("Step is missing in queues.scheduled"))?;

        tracing::debug!(
            "removing job from machine: drv_path={drv_path} m={}",
            item.machine.id
        );
        let mut job = item
            .machine
            .remove_job(drv_path)
            .ok_or_else(|| anyhow::anyhow!("Job is missing in machine.jobs m={}", item.machine))?;

        job.result.step_status = BuildStatus::Failed;
        job.result.update_with_result_state(&state);
        job.result.set_stop_time_now();
        job.result.set_overhead(timings.get_overhead())?;

        let total_step_time = job.result.get_total_step_time_ms();
        item.machine
            .stats
            .track_build_failure(timings, total_step_time);
        self.metrics.track_build_failure(timings, total_step_time);

        let (max_retries, retry_interval, retry_backoff) = self.config.get_retry();

        if job.result.can_retry {
            // Count previous attempts from BuildSteps for this drv_path
            let drv_path_str = self.store.print_store_path(drv_path);
            let tries = {
                let mut conn = self.db.get().await?;
                conn.count_build_steps_for_drv(&drv_path_str)
                    .await
                    .unwrap_or(0) as u32
            };
            if tries < max_retries {
                self.metrics.nr_retries.inc();
                #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
                let delta =
                    (retry_interval * retry_backoff.powf(tries.saturating_sub(1) as f32)) as i64;
                tracing::info!("will retry '{drv_path}' after {delta}s");

                // Re-add to BuildStepCanCreate with a future readyTime
                #[allow(clippy::cast_possible_truncation)]
                let future_ready_time = (jiff::Timestamp::now().as_second() + delta) as i32;
                if let Ok(mut conn) = self.db.get().await {
                    if let Ok(mut tx) = conn.begin_transaction().await {
                        let _ = tx.mark_step_ready(&drv_path_str, future_ready_time).await;
                        let _ = tx.commit().await;
                    }
                }

                if i64::from(tries) > self.metrics.max_nr_retries.get() {
                    self.metrics.max_nr_retries.set(i64::from(tries));
                }

                finish_build_step(
                    &self.db,
                    &self.store,
                    &job.path,
                    job.attempt.expect("attempt set after create_build_step"),
                    &job.result,
                    Some(&item.machine.hostname),
                    None,
                )
                .await?;
                self.trigger_dispatch();
                return Ok(());
            }
        }

        self.inner_fail_job_by_drv(drv_path, Some(item.machine), job)
            .await
    }

    #[tracing::instrument(skip(self, output), fields(%machine_id, build_id=%build_id), err)]
    pub async fn succeed_step_by_uuid(
        &self,
        build_id: uuid::Uuid,
        machine_id: uuid::Uuid,
        output: BuildOutput,
    ) -> anyhow::Result<()> {
        let machine = self
            .machines
            .get_machine_by_id(machine_id)
            .ok_or_else(|| anyhow::anyhow!("Machine with machine_id not found"))?;
        let drv_path = machine
            .get_job_drv_for_build_id(build_id)
            .ok_or_else(|| anyhow::anyhow!("Job with build_id not found"))?;

        self.succeed_step(machine_id, &drv_path, output).await
    }

    #[tracing::instrument(skip(self), fields(%machine_id, build_id=%build_id), err)]
    pub async fn fail_step_by_uuid(
        &self,
        build_id: uuid::Uuid,
        machine_id: uuid::Uuid,
        state: BuildResultState,
        timings: BuildTimings,
    ) -> anyhow::Result<()> {
        let machine = self
            .machines
            .get_machine_by_id(machine_id)
            .ok_or_else(|| anyhow::anyhow!("Machine with machine_id not found"))?;
        let drv_path = machine
            .get_job_drv_for_build_id(build_id)
            .ok_or_else(|| anyhow::anyhow!("Job with build_id not found"))?;

        self.fail_step(machine_id, &drv_path, state, timings).await
    }

    /// Fail a job using only the drv_path (no in-memory Step required).
    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(self, machine, job), fields(%drv_path), err)]
    async fn inner_fail_job_by_drv(
        &self,
        drv_path: &nix_utils::StorePath,
        machine: Option<Arc<Machine>>,
        mut job: machine::Job,
    ) -> anyhow::Result<()> {
        if !job.result.has_stop_time() {
            job.result.set_stop_time_now();
        }

        if let Some(attempt) = job.attempt {
            finish_build_step(
                &self.db,
                &self.store,
                &job.path,
                attempt,
                &job.result,
                machine.as_ref().map(|m| m.hostname.as_str()),
                None,
            )
            .await?;
        }

        // Parse derivation to get output paths for caching failures
        let output_paths = nix_utils::query_drv(&self.store, drv_path)
            .await
            .ok()
            .flatten()
            .map(|d| nix_utils::output_paths(&d, self.store.store_dir()));

        // Get all builds that depend transitively on this derivation
        let indirect: Vec<Arc<Build>> = self.get_all_indirect_builds_by_drv(drv_path).await;

        let mut dependent_ids = Vec::new();
        if !indirect.is_empty() {
            let mut db = self.db.get().await?;
            let mut tx = db.begin_transaction().await?;
            for b in &indirect {
                if b.get_finished_in_db() {
                    continue;
                }

                tracing::info!("marking build {} as failed", b.id);
                let start_time = job.result.get_start_time_as_i32()?;
                let stop_time = job.result.get_stop_time_as_i32()?;
                tx.update_build_after_failure(
                    b.id,
                    if b.drv_path != *drv_path && job.result.step_status == BuildStatus::Failed {
                        BuildStatus::DepFailed
                    } else {
                        job.result.step_status
                    },
                    start_time,
                    stop_time,
                    job.result.step_status == BuildStatus::CachedFailure,
                )
                .await?;
                self.metrics.nr_builds_done.inc();
            }

            // Remember failed paths
            if job.result.step_status != BuildStatus::CachedFailure && job.result.can_cache {
                if let Some(ref paths) = output_paths {
                    for (_, path) in paths {
                        if let Some(path) = path {
                            tx.insert_failed_paths(self.store.store_dir(), path).await?;
                        }
                    }
                }
            }

            tx.commit().await?;
        }

        for b in &indirect {
            b.set_finished_in_db(true);
            self.builds.remove_by_id(b.id);
            dependent_ids.push(b.id);
        }

        {
            let mut db = self.db.get().await?;
            let mut tx = db.begin_transaction().await?;
            tx.notify_build_finished(dependent_ids.first().copied().unwrap_or(0), &dependent_ids)
                .await?;
            tx.commit().await?;
        }

        self.trigger_dispatch();
        Ok(())
    }

    /// Get all builds that transitively depend on a derivation, using DB queries.
    #[tracing::instrument(skip(self), fields(%drv_path))]
    async fn get_all_indirect_builds_by_drv(
        &self,
        drv_path: &nix_utils::StorePath,
    ) -> Vec<Arc<Build>> {
        let drv_path_str = self.store.print_store_path(drv_path);
        let db_builds = {
            let Ok(mut conn) = self.db.get().await else {
                return Vec::new();
            };
            conn.get_dependent_builds(&drv_path_str)
                .await
                .unwrap_or_default()
        };

        // Match DB build IDs to our in-memory Build objects
        let store_dir = self.store.store_dir();
        let mut result = Vec::new();
        for db_build in db_builds {
            // Also include direct builds for this drv_path
            if let Ok(drv) = store_dir.parse(&db_build.drvpath) {
                result.extend(self.builds.clone_matching_drv(&drv));
            }
        }
        // Also include direct builds
        result.extend(self.builds.clone_matching_drv(drv_path));
        // Deduplicate
        let mut seen = HashSet::new();
        result.retain(|b| seen.insert(b.id));
        result
    }

    #[tracing::instrument(skip(self, build), err)]
    async fn handle_previous_failure(
        &self,
        build: Arc<Build>,
        drv_path: &nix_utils::StorePath,
    ) -> anyhow::Result<()> {
        tracing::warn!(
            "marking build {} as cached failure due to ‘{}’",
            build.id,
            drv_path
        );
        if build.get_finished_in_db() {
            return Ok(());
        }

        // Parse derivation for system and output paths
        let drv_parsed = nix_utils::query_drv(&self.store, drv_path)
            .await
            .ok()
            .flatten();
        let system_str = drv_parsed.as_ref().map(|d| {
            std::str::from_utf8(&d.platform)
                .expect("platform must be valid UTF-8")
                .to_owned()
        });
        let output_paths = drv_parsed
            .as_ref()
            .map(|d| nix_utils::output_paths(d, self.store.store_dir()))
            .unwrap_or_default();

        let mut conn = self.db.get().await?;
        let mut tx = conn.begin_transaction().await?;

        let mut propagated_from = tx
            .get_last_build_step_id(self.store.store_dir(), drv_path)
            .await?
            .unwrap_or_default();

        if propagated_from == 0 {
            for (name, path) in &output_paths {
                let res = if let Some(path) = path {
                    tx.get_last_build_step_id_for_output_path(self.store.store_dir(), path)
                        .await
                } else {
                    tx.get_last_build_step_id_for_output_with_drv(
                        self.store.store_dir(),
                        drv_path,
                        name.as_ref(),
                    )
                    .await
                };
                if let Ok(Some(res)) = res {
                    propagated_from = res;
                    break;
                }
            }
        }

        tx.create_build_step(
            self.store.store_dir(),
            None,
            drv_path,
            system_str.as_deref(),
            String::new(),
            BuildStatus::CachedFailure,
            None,
            Some(propagated_from),
            output_paths.into_iter().collect(),
        )
        .await?;
        tx.update_build_after_previous_failure(
            build.id,
            if drv_path == &build.drv_path {
                BuildStatus::Failed
            } else {
                BuildStatus::DepFailed
            },
        )
        .await?;

        let _ = tx.notify_build_finished(build.id, &[]).await;
        tx.commit().await?;

        build.set_finished_in_db(true);
        self.metrics.nr_builds_done.inc();
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(
        self,
        build,
        nr_added,
        new_builds_by_id,
        new_builds_by_path,
        finished_drvs,
    ), fields(build_id=build.id))]
    async fn create_build(
        &self,
        build: Arc<Build>,
        nr_added: Arc<AtomicI64>,
        new_builds_by_id: Arc<parking_lot::RwLock<HashMap<BuildID, Arc<Build>>>>,
        new_builds_by_path: &HashMap<nix_utils::StorePath, HashSet<BuildID>>,
        finished_drvs: Arc<parking_lot::RwLock<HashSet<nix_utils::StorePath>>>,
    ) {
        self.metrics.queue_build_loads.inc();
        tracing::info!("loading build {} ({})", build.id, build.full_job_name());
        nr_added.fetch_add(1, Ordering::Relaxed);
        {
            let mut new_builds_by_id = new_builds_by_id.write();
            new_builds_by_id.remove(&build.id);
        }

        if !self.store.is_valid_path(&build.drv_path).await {
            tracing::error!(
                "aborting GC'ed build id={} path={}",
                build.id,
                self.store.print_store_path(&build.drv_path)
            );
            if !build.get_finished_in_db() {
                match self.db.get().await {
                    Ok(mut conn) => {
                        if let Err(e) = conn.abort_build(build.id).await {
                            tracing::error!("Failed to abort the build={} e={}", build.id, e);
                        }
                    }
                    Err(e) => tracing::error!(
                        "Failed to get database connection so we can abort the build={} e={}",
                        build.id,
                        e
                    ),
                }
            }

            build.set_finished_in_db(true);
            self.metrics.nr_builds_done.inc();
            return;
        }

        // Create steps for this derivation and its dependencies.
        let new_step_paths = Arc::new(parking_lot::RwLock::new(
            HashSet::<nix_utils::StorePath>::new(),
        ));
        let step = match self
            .create_step(
                build.clone(),
                build.drv_path.clone(),
                finished_drvs.clone(),
                new_step_paths.clone(),
            )
            .await
        {
            CreateStepResult::None => None,
            CreateStepResult::Valid(drv_path) => Some(drv_path),
            CreateStepResult::PreviousFailure(drv_path) => {
                if let Err(e) = self.handle_previous_failure(build, &drv_path).await {
                    tracing::error!("Failed to handle previous failure: {e}");
                }
                return;
            }
        };

        {
            use futures::stream::StreamExt as _;

            let builds = {
                let new_step_paths = new_step_paths.read();
                new_step_paths
                    .iter()
                    .filter_map(|r| Some(new_builds_by_path.get(r)?.clone()))
                    .flatten()
                    .collect::<Vec<_>>()
            };
            let mut stream = futures::StreamExt::map(tokio_stream::iter(builds), |b| {
                let nr_added = nr_added.clone();
                let new_builds_by_id = new_builds_by_id.clone();
                let finished_drvs = finished_drvs.clone();
                async move {
                    let j = {
                        if let Some(j) = new_builds_by_id.read().get(&b) {
                            j.clone()
                        } else {
                            return;
                        }
                    };

                    Box::pin(self.create_build(
                        j,
                        nr_added,
                        new_builds_by_id,
                        new_builds_by_path,
                        finished_drvs,
                    ))
                    .await;
                }
            })
            .buffered(10);
            while tokio_stream::StreamExt::next(&mut stream).await.is_some() {}
        }

        if let Some(drv_path) = step {
            if !build.get_finished_in_db() {
                self.builds.insert_new_build(build.clone());
            }

            tracing::info!("added build {} (top-level step {})", build.id, drv_path,);
        } else {
            // If we didn't get a step, it means the step's outputs are
            // all valid. So we mark this as a finished, cached build.
            if let Err(e) = self.handle_cached_build(build).await {
                tracing::error!("failed to handle cached build: {e}");
            }
        }
    }

    /// Create a step in the DB for the given derivation and recursively for its deps.
    ///
    /// Returns `Valid(drv_path)` if the step was created or already exists,
    /// `None` if the outputs are already built, or
    /// `PreviousFailure(drv_path)` if there's a cached failure.
    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(
        self,
        finished_drvs,
        new_step_paths,
    ), fields(%drv_path))]
    async fn create_step(
        &self,
        build: Arc<Build>,
        drv_path: nix_utils::StorePath,
        finished_drvs: Arc<parking_lot::RwLock<HashSet<nix_utils::StorePath>>>,
        new_step_paths: Arc<parking_lot::RwLock<HashSet<nix_utils::StorePath>>>,
    ) -> CreateStepResult {
        use futures::stream::StreamExt as _;

        {
            let finished_drvs = finished_drvs.read();
            if finished_drvs.contains(&drv_path) {
                return CreateStepResult::None;
            }
        }

        let drv_path_str = self.store.print_store_path(&drv_path);

        // Check if this derivation already exists in the DB
        {
            if let Ok(mut conn) = self.db.get().await {
                if let Ok(exists) = conn.derivation_exists(&drv_path_str).await {
                    if exists {
                        return CreateStepResult::Valid(drv_path);
                    }
                }
            }
        }

        self.metrics.queue_steps_created.inc();
        tracing::debug!("considering derivation '{drv_path}'");

        let Some(drv) = nix_utils::query_drv(&self.store, &drv_path)
            .await
            .ok()
            .flatten()
        else {
            return CreateStepResult::None;
        };
        if let Some(fod_checker) = &self.fod_checker {
            fod_checker.add_ca_drv_parsed(&drv_path, &drv);
        }

        let system_type = std::str::from_utf8(&drv.platform).expect("platform must be valid UTF-8");
        #[allow(clippy::cast_precision_loss)]
        self.metrics.observe_build_input_drvs(
            harmonia_store_core::derivation::DerivationInputs::from(&drv.inputs)
                .drvs
                .len() as f64,
            system_type,
        );

        let use_substitutes = self.config.get_use_substitutes();
        let remote_store: Option<binary_cache::S3BinaryCacheClient> = {
            let r = self.remote_stores.read();
            r.iter().find_map(|s| match s {
                RemoteStoreBackend::S3(s) => Some(s.clone()),
                _ => None,
            })
        };
        let output_paths = nix_utils::output_paths(&drv, self.store.store_dir());
        let missing_outputs = if let Some(ref remote_store) = remote_store {
            let mut missing = remote_store
                .query_missing_remote_outputs(output_paths.clone())
                .await;
            if !missing.is_empty()
                && self
                    .store
                    .query_missing_outputs(output_paths.clone())
                    .await
                    .is_empty()
            {
                if let Ok(log_file) = self.construct_log_file_path(&drv_path).await {
                    let missing_paths: Vec<nix_utils::StorePath> =
                        missing.values().filter_map(Clone::clone).collect();
                    self.uploader
                        .schedule_upload(
                            missing_paths,
                            format!("log/{drv_path}"),
                            log_file.to_string_lossy().to_string(),
                        )
                        .await;
                    missing.clear();
                }
            }
            missing
        } else {
            self.store.query_missing_outputs(output_paths.clone()).await
        };

        // Check cached failure using output paths
        if self.check_cached_failure_by_outputs(&output_paths).await {
            return CreateStepResult::PreviousFailure(drv_path);
        }

        tracing::debug!("missing outputs: {missing_outputs:?}");
        let finished = if !missing_outputs.is_empty() && use_substitutes {
            use futures::stream::StreamExt as _;

            let mut substituted = 0;
            let missing_outputs_len = missing_outputs.len();
            let mut stream = futures::StreamExt::map(tokio_stream::iter(missing_outputs), |o| {
                self.metrics.nr_substitutes_started.inc();
                crate::utils::substitute_output(
                    self.db.clone(),
                    nix_utils::LocalStore::init(),
                    o,
                    &drv_path,
                    remote_store.as_ref(),
                )
            })
            .buffer_unordered(10);
            while let Some(v) = tokio_stream::StreamExt::next(&mut stream).await {
                match v {
                    Ok(v) if v => {
                        self.metrics.nr_substitutes_succeeded.inc();
                        substituted += 1;
                    }
                    Ok(_) => {
                        self.metrics.nr_substitutes_failed.inc();
                    }
                    Err(e) => {
                        self.metrics.nr_substitutes_failed.inc();
                        tracing::warn!("Failed to substitute path: {e}");
                    }
                }
            }
            substituted == missing_outputs_len
        } else {
            missing_outputs.is_empty()
        };

        if finished {
            if let Some(fod_checker) = &self.fod_checker {
                fod_checker.to_traverse(&drv_path);
            }

            finished_drvs.write().insert(drv_path.clone());
            return CreateStepResult::None;
        }

        tracing::debug!("creating build step '{drv_path}");
        let input_drvs: Vec<nix_utils::StorePath> =
            harmonia_store_core::derivation::DerivationInputs::from(&drv.inputs)
                .drvs
                .into_keys()
                .collect();

        let mut dep_drv_paths: Vec<String> = Vec::new();
        let mut stream = futures::StreamExt::map(tokio_stream::iter(input_drvs), |i| {
            let build = build.clone();
            let finished_drvs = finished_drvs.clone();
            let new_step_paths = new_step_paths.clone();
            async move { Box::pin(self.create_step(build, i, finished_drvs, new_step_paths)).await }
        })
        .buffered(25);
        while let Some(v) = tokio_stream::StreamExt::next(&mut stream).await {
            match v {
                CreateStepResult::None => (),
                CreateStepResult::Valid(dep_drv) => {
                    dep_drv_paths.push(self.store.print_store_path(&dep_drv));
                }
                CreateStepResult::PreviousFailure(step) => {
                    return CreateStepResult::PreviousFailure(step);
                }
            }
        }

        // Insert this derivation and its dep edges into the DB
        if let Ok(mut conn) = self.db.get().await {
            if let Ok(mut tx) = conn.begin_transaction().await {
                let _ = tx.ensure_derivation_path(&drv_path_str).await;
                if !dep_drv_paths.is_empty() {
                    let dep_refs: Vec<&str> = dep_drv_paths.iter().map(String::as_str).collect();
                    let _ = tx.insert_step_deps(&drv_path_str, &dep_refs).await;
                }
                // Mark as ready if all deps are already satisfied
                // (either zero deps, or all deps have a successful BuildSteps row).
                #[allow(clippy::cast_possible_truncation)]
                let ready_time = jiff::Timestamp::now().as_second() as i32;
                let _ = tx.mark_step_ready_if_deps_satisfied(&drv_path_str, ready_time).await;
                let _ = tx.commit().await;
            }
        }

        {
            let mut new_step_paths = new_step_paths.write();
            new_step_paths.insert(drv_path.clone());
        }
        CreateStepResult::Valid(drv_path)
    }

    /// Check if a derivation's outputs have been previously marked as failed.
    #[tracing::instrument(skip(self, drv_path), ret, level = "debug")]
    async fn check_cached_failure_by_drv(&self, drv_path: &nix_utils::StorePath) -> bool {
        let Some(drv) = nix_utils::query_drv(&self.store, drv_path)
            .await
            .ok()
            .flatten()
        else {
            return false;
        };
        let output_paths = nix_utils::output_paths(&drv, self.store.store_dir());
        self.check_cached_failure_by_outputs(&output_paths).await
    }

    /// Check if the given output paths have been previously marked as failed.
    async fn check_cached_failure_by_outputs(
        &self,
        output_paths: &std::collections::BTreeMap<
            nix_utils::OutputName,
            Option<nix_utils::StorePath>,
        >,
    ) -> bool {
        let Ok(mut conn) = self.db.get().await else {
            return false;
        };

        conn.check_if_paths_failed(
            self.store.store_dir(),
            &output_paths
                .iter()
                .filter_map(|(_, path)| path.clone())
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap_or_default()
    }

    #[tracing::instrument(skip(self, build), fields(build_id=build.id), err)]
    async fn handle_cached_build(&self, build: Arc<Build>) -> anyhow::Result<()> {
        let res = self.get_build_output_cached(&build.drv_path).await?;

        {
            let mut db = self.db.get().await?;
            let mut tx = db.begin_transaction().await?;

            tracing::info!("marking build {} as succeeded (cached)", build.id);
            let now = jiff::Timestamp::now().as_second();
            tx.mark_succeeded_build(
                get_mark_build_sccuess_data(&self.store, &build, &res),
                true,
                i32::try_from(now)?, // TODO
                i32::try_from(now)?, // TODO
                self.store.store_dir(),
            )
            .await?;
            self.metrics.nr_builds_done.inc();

            tx.notify_build_finished(build.id, &[]).await?;
            tx.commit().await?;
        }
        build.set_finished_in_db(true);

        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    async fn get_build_output_cached(
        &self,
        drv_path: &nix_utils::StorePath,
    ) -> anyhow::Result<BuildOutput> {
        let drv = nix_utils::query_drv(&self.store, drv_path)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Derivation not found"))?;

        let output_paths = nix_utils::output_paths(&drv, self.store.store_dir());
        {
            let mut db = self.db.get().await?;
            for out_path in output_paths.values() {
                let Some(out_path) = out_path else {
                    continue;
                };
                let Some(db_build_output) = db
                    .get_build_output_for_path(self.store.store_dir(), out_path)
                    .await?
                else {
                    continue;
                };
                let build_id = db_build_output.id;
                let Ok(mut res): anyhow::Result<BuildOutput> = db_build_output.try_into() else {
                    continue;
                };

                res.products = db
                    .get_build_products_for_build_id(build_id, self.store.store_dir())
                    .await?
                    .into_iter()
                    .map(build::BuildProduct::from_db)
                    .collect();
                res.metrics = db
                    .get_build_metrics_for_build_id(build_id)
                    .await?
                    .into_iter()
                    .map(Into::into)
                    .collect();

                return Ok(res);
            }
        }

        let build_output = BuildOutput::new(&self.store, output_paths).await?;

        #[allow(clippy::cast_precision_loss)]
        self.metrics.observe_build_closure_size(
            build_output.closure_size as f64,
            std::str::from_utf8(&drv.platform).expect("platform must be valid UTF-8"),
        );

        Ok(build_output)
    }

    #[allow(unused)]
    fn add_root(&self, store_path: &nix_utils::StorePath) {
        let roots_dir = self.config.get_roots_dir();
        nix_utils::add_root(&self.store, &roots_dir, store_path);
    }

    /// Check all ready steps in `BuildStepCanCreate` and abort those that have been
    /// unsupported for too long.
    async fn abort_unsupported(&self) {
        // Query ready steps from the DB
        let candidates = match self.db.get().await {
            Ok(mut conn) => conn.get_dispatch_candidates().await.unwrap_or_default(),
            Err(_) => return,
        };

        let now = jiff::Timestamp::now();
        let max_unsupported_time = self.config.get_max_unsupported_time();
        let mut count: i64 = 0;
        let mut aborted: u64 = 0;

        for candidate in &candidates {
            let store_dir = self.store.store_dir();
            let drv_path = match store_dir.parse(&candidate.drv_path) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let Some(drv) = nix_utils::query_drv(&self.store, &drv_path)
                .await
                .ok()
                .flatten()
            else {
                continue;
            };
            let system = std::str::from_utf8(&drv.platform)
                .expect("platform must be valid UTF-8")
                .to_owned();
            let required_features: Vec<String> = drv
                .env
                .get(b"requiredSystemFeatures".as_slice())
                .and_then(|v| std::str::from_utf8(v).ok())
                .map(|v| {
                    v.split(' ')
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default();

            let supported = self.machines.support_system(&system, &required_features);
            if supported {
                continue;
            }

            count += 1;

            // Check how long this step has been ready (using ready_time)
            let ready_timestamp = jiff::Timestamp::from_second(i64::from(candidate.ready_time))
                .unwrap_or(jiff::Timestamp::UNIX_EPOCH);
            let unsupported_duration = (now - ready_timestamp)
                .total(jiff::Unit::Second)
                .unwrap_or_default();
            if unsupported_duration < max_unsupported_time.as_secs_f64() {
                continue;
            }

            tracing::error!("aborting unsupported build step '{drv_path}' (type '{system}')",);

            // Get dependent builds from DB
            let drv_path_str = self.store.print_store_path(&drv_path);
            let dependents = self.get_all_indirect_builds_by_drv(&drv_path).await;
            if dependents.is_empty() {
                continue;
            }

            let mut job = machine::Job::new(drv_path.clone(), None);
            job.result.set_start_and_stop(now);
            job.result.step_status = BuildStatus::Unsupported;
            job.result.error_msg = Some(format!("unsupported system type '{system}'"));
            if let Err(e) = self.inner_fail_job_by_drv(&drv_path, None, job).await {
                tracing::error!("Failed to fail step drv={drv_path} e={e}");
            }

            // Remove from BuildStepCanCreate
            if let Ok(mut conn) = self.db.get().await {
                if let Ok(mut tx) = conn.begin_transaction().await {
                    let _ = tx.unmark_step_ready(&drv_path_str).await;
                    let _ = tx.commit().await;
                }
            }

            aborted += 1;
        }

        self.metrics.nr_unsupported_steps.set(count);
        self.metrics.nr_unsupported_steps_aborted.inc_by(aborted);
    }
}
