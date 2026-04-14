use std::collections::BTreeMap;
use std::future::{Future, ready};
use std::pin::Pin;

use tokio::io::AsyncBufRead;

use harmonia_protocol::build_result::{
    BuildResult, BuildResultFailure, BuildResultInner, BuildResultSuccess, FailureStatus,
    SuccessStatus,
};
use harmonia_protocol::daemon::{
    DaemonError as ProtocolError, DaemonResult, DaemonStore, FutureResultExt, HandshakeDaemonStore,
    ResultLog, ResultLogExt, TrustLevel,
};
use harmonia_protocol::daemon_wire::types2::{BuildMode, KeyedBuildResult};
use harmonia_protocol::types::AddToStoreItem;
use harmonia_protocol::valid_path_info::{UnkeyedValidPathInfo, ValidPathInfo};
use harmonia_store_core::derivation::BasicDerivation;
use harmonia_store_core::derived_path::{DerivedPath, SingleDerivedPath};
use harmonia_store_core::store_path::{
    ContentAddressMethodAlgorithm, StorePath, StorePathHash, StorePathSet,
};
use harmonia_store_remote::pool::{ConnectionPool, PoolConfig};

use db::StoreDir;
use db::models::BuildStatus;

use crate::waiter::BuildWaiter;

/// Nix daemon handler that turns each `BuildDerivation` / `BuildPaths` request
/// into an ad-hoc row in the Hydra `Builds` table and waits for the queue
/// runner to mark it finished. Read-only operations are proxied to the host
/// nix daemon.
#[derive(Clone)]
pub struct DrvDaemonHandler {
    store_dir: StoreDir,
    db: db::Database,
    upstream: ConnectionPool,
    waiter: BuildWaiter,
}

impl std::fmt::Debug for DrvDaemonHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrvDaemonHandler")
            .field("store_dir", &self.store_dir)
            .finish_non_exhaustive()
    }
}

impl DrvDaemonHandler {
    pub fn new(
        store_dir: StoreDir,
        db: db::Database,
        upstream_socket: &str,
        waiter: BuildWaiter,
    ) -> Self {
        let upstream = ConnectionPool::with_store_dir(
            upstream_socket,
            store_dir.clone(),
            PoolConfig::default(),
        );
        Self {
            store_dir,
            db,
            upstream,
            waiter,
        }
    }

    /// Insert an ad-hoc Build, register a waiter for its completion under
    /// the same transaction so the row cannot become visible before the
    /// listener is ready, then block until the queue runner finishes it.
    async fn run_adhoc_build(
        &self,
        drv_path: &str,
        nix_name: &str,
        system: &str,
    ) -> Result<db::FinishedBuild, ProtocolError> {
        let mut conn = self
            .db
            .get()
            .await
            .map_err(|e| ProtocolError::custom(format!("hydra db: {e}")))?;
        let jobset_id = conn
            .ensure_adhoc_jobset()
            .await
            .map_err(|e| ProtocolError::custom(format!("ensure adhoc jobset: {e}")))?;
        let mut tx = conn
            .begin_transaction()
            .await
            .map_err(|e| ProtocolError::custom(format!("begin tx: {e}")))?;
        let build_id = tx
            .insert_adhoc_build(jobset_id, nix_name, drv_path, system)
            .await
            .map_err(|e| ProtocolError::custom(format!("insert adhoc build: {e}")))?;

        let rx = self.waiter.register(build_id).await;

        tx.notify_builds_added()
            .await
            .map_err(|e| ProtocolError::custom(format!("notify builds_added: {e}")))?;
        tx.commit()
            .await
            .map_err(|e| ProtocolError::custom(format!("commit: {e}")))?;

        tracing::info!(
            build_id,
            drv_path,
            "scheduled ad-hoc build, awaiting finish"
        );

        if rx.await.is_err() {
            self.waiter.forget(build_id).await;
            return Err(ProtocolError::custom(
                "build_finished listener gone before completion",
            ));
        }

        let mut conn = self
            .db
            .get()
            .await
            .map_err(|e| ProtocolError::custom(format!("hydra db: {e}")))?;
        conn.get_finished_build(build_id)
            .await
            .map_err(|e| ProtocolError::custom(format!("read finished build: {e}")))?
            .ok_or_else(|| {
                ProtocolError::custom(format!("build {build_id} woke but has no finished row"))
            })
    }
}

fn require_normal_mode(mode: BuildMode) -> Result<(), ProtocolError> {
    if mode == BuildMode::Normal {
        Ok(())
    } else {
        Err(ProtocolError::custom(format!(
            "drv-daemon only supports BuildMode::Normal, got {mode:?}"
        )))
    }
}

fn drv_path_of(path: &SingleDerivedPath) -> &StorePath {
    let mut current = path;
    loop {
        match current {
            SingleDerivedPath::Opaque(sp) => return sp,
            SingleDerivedPath::Built { drv_path, .. } => current = drv_path.as_ref(),
        }
    }
}

fn finished_to_build_result(finished: &db::FinishedBuild) -> BuildResult {
    let inner = match finished.status {
        BuildStatus::Success => BuildResultInner::Success(BuildResultSuccess {
            status: SuccessStatus::Built,
            // TODO: populate per-output Realisations for CA derivations. Needs
            // the resolved drv hash which we don't currently surface from the
            // queue runner; for input-addressed builds this is unused.
            built_outputs: BTreeMap::new(),
        }),
        status => BuildResultInner::Failure(BuildResultFailure {
            status: build_status_to_failure(status),
            error_msg: build_status_message(status).into(),
            is_non_deterministic: status == BuildStatus::NotDeterministic,
        }),
    };
    BuildResult {
        inner,
        times_built: 1,
        start_time: finished.start_time.unwrap_or(0).into(),
        stop_time: finished.stop_time.unwrap_or(0).into(),
        cpu_user: None,
        cpu_system: None,
    }
}

fn build_status_to_failure(status: BuildStatus) -> FailureStatus {
    match status {
        BuildStatus::Failed | BuildStatus::FailedWithOutput => FailureStatus::PermanentFailure,
        BuildStatus::DepFailed => FailureStatus::DependencyFailed,
        BuildStatus::TimedOut => FailureStatus::TimedOut,
        BuildStatus::CachedFailure => FailureStatus::CachedFailure,
        BuildStatus::LogLimitExceeded => FailureStatus::LogLimitExceeded,
        BuildStatus::NarSizeLimitExceeded => FailureStatus::OutputRejected,
        BuildStatus::NotDeterministic => FailureStatus::NotDeterministic,
        BuildStatus::Unsupported
        | BuildStatus::Aborted
        | BuildStatus::Cancelled
        | BuildStatus::Busy
        | BuildStatus::Success => FailureStatus::MiscFailure,
    }
}

fn build_status_message(status: BuildStatus) -> &'static str {
    match status {
        BuildStatus::Success => "succeeded",
        BuildStatus::Failed => "build failed",
        BuildStatus::DepFailed => "a dependency failed to build",
        BuildStatus::Aborted => "build was aborted",
        BuildStatus::Cancelled => "build was cancelled",
        BuildStatus::FailedWithOutput => "build failed (with output)",
        BuildStatus::TimedOut => "build timed out",
        BuildStatus::CachedFailure => "build previously failed (cached)",
        BuildStatus::Unsupported => "system not supported",
        BuildStatus::LogLimitExceeded => "log size limit exceeded",
        BuildStatus::NarSizeLimitExceeded => "NAR size limit exceeded",
        BuildStatus::NotDeterministic => "build is not deterministic",
        BuildStatus::Busy => "build is still in progress",
    }
}

impl HandshakeDaemonStore for DrvDaemonHandler {
    type Store = Self;

    fn handshake(self) -> impl ResultLog<Output = DaemonResult<Self::Store>> + Send {
        ready(Ok(self)).empty_logs()
    }
}

impl DaemonStore for DrvDaemonHandler {
    fn trust_level(&self) -> TrustLevel {
        TrustLevel::Trusted
    }

    fn set_options<'a>(
        &'a mut self,
        _options: &'a harmonia_protocol::types::ClientOptions,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        ready(Ok(())).empty_logs()
    }

    fn build_derivation<'a>(
        &'a mut self,
        drv_path: &'a StorePath,
        drv: &'a BasicDerivation,
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<BuildResult>> + Send + 'a {
        let this = self.clone();
        async move {
            require_normal_mode(mode)?;
            let drv_path_str = format!("{}/{}", this.store_dir, drv_path);
            let nix_name: String = drv.name.to_string();
            let system = std::str::from_utf8(&drv.platform)
                .map_err(|e| ProtocolError::custom(format!("non-utf8 platform: {e}")))?;
            let finished = this
                .run_adhoc_build(&drv_path_str, &nix_name, system)
                .await?;
            Ok(finished_to_build_result(&finished))
        }
        .empty_logs()
    }

    fn build_paths<'a>(
        &'a mut self,
        drvs: &'a [DerivedPath],
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        let this = self.clone();
        async move {
            require_normal_mode(mode)?;
            for path in drvs {
                let drv_path = match path {
                    DerivedPath::Built { drv_path, .. } => drv_path_of(drv_path),
                    DerivedPath::Opaque(_) => {
                        return Err(ProtocolError::custom(
                            "drv-daemon BuildPaths does not support opaque (already-built) paths",
                        ));
                    }
                };
                let drv_path_str = format!("{}/{}", this.store_dir, drv_path);
                let nix_name = drv_path.name().as_ref().to_owned();
                let finished = this.run_adhoc_build(&drv_path_str, &nix_name, "").await?;
                if finished.status != BuildStatus::Success {
                    return Err(ProtocolError::custom(format!(
                        "ad-hoc build of {drv_path_str} failed: {}",
                        build_status_message(finished.status)
                    )));
                }
            }
            Ok(())
        }
        .empty_logs()
    }

    fn build_paths_with_results<'a>(
        &'a mut self,
        drvs: &'a [DerivedPath],
        mode: BuildMode,
    ) -> impl ResultLog<Output = DaemonResult<Vec<KeyedBuildResult>>> + Send + 'a {
        let this = self.clone();
        async move {
            require_normal_mode(mode)?;
            let mut results = Vec::with_capacity(drvs.len());
            for path in drvs {
                let drv_path = match path {
                    DerivedPath::Built { drv_path, .. } => drv_path_of(drv_path),
                    DerivedPath::Opaque(_) => {
                        return Err(ProtocolError::custom(
                            "drv-daemon BuildPathsWithResults does not support opaque paths",
                        ));
                    }
                };
                let drv_path_str = format!("{}/{}", this.store_dir, drv_path);
                let nix_name = drv_path.name().as_ref().to_owned();
                let finished = this.run_adhoc_build(&drv_path_str, &nix_name, "").await?;
                results.push(KeyedBuildResult {
                    path: path.clone(),
                    result: finished_to_build_result(&finished),
                });
            }
            Ok(results)
        }
        .empty_logs()
    }

    fn is_valid_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + Send + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().is_valid_path(path).await
        }
        .empty_logs()
    }

    fn query_valid_paths<'a>(
        &'a mut self,
        paths: &'a StorePathSet,
        substitute: bool,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().query_valid_paths(paths, substitute).await
        }
        .empty_logs()
    }

    fn query_path_info<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<Option<UnkeyedValidPathInfo>>> + Send + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().query_path_info(path).await
        }
        .empty_logs()
    }

    fn query_path_from_hash_part<'a>(
        &'a mut self,
        hash: &'a StorePathHash,
    ) -> impl ResultLog<Output = DaemonResult<Option<StorePath>>> + Send + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().query_path_from_hash_part(hash).await
        }
        .empty_logs()
    }

    fn query_derivation_output_map<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<
        Output = DaemonResult<
            std::collections::BTreeMap<
                harmonia_store_core::derived_path::OutputName,
                Option<StorePath>,
            >,
        >,
    > + Send
    + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().query_derivation_output_map(path).await
        }
        .empty_logs()
    }

    fn query_missing<'a>(
        &'a mut self,
        paths: &'a [DerivedPath],
    ) -> impl ResultLog<
        Output = DaemonResult<harmonia_protocol::daemon_wire::types2::QueryMissingResult>,
    > + Send
    + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().query_missing(paths).await
        }
        .empty_logs()
    }

    fn query_referrers<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().query_referrers(path).await
        }
        .empty_logs()
    }

    fn query_substitutable_paths<'a>(
        &'a mut self,
        paths: &'a StorePathSet,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().query_substitutable_paths(paths).await
        }
        .empty_logs()
    }

    fn query_valid_derivers<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<StorePathSet>> + Send + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().query_valid_derivers(path).await
        }
        .empty_logs()
    }

    fn query_realisation<'a>(
        &'a mut self,
        output_id: &'a harmonia_store_core::realisation::DrvOutput,
    ) -> impl ResultLog<
        Output = DaemonResult<
            std::collections::BTreeSet<harmonia_store_core::realisation::Realisation>,
        >,
    > + Send
    + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().query_realisation(output_id).await
        }
        .empty_logs()
    }

    fn add_temp_root<'a>(
        &'a mut self,
        _path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        ready(Ok(())).empty_logs()
    }

    fn ensure_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().ensure_path(path).await
        }
        .empty_logs()
    }

    fn add_indirect_root<'a>(
        &'a mut self,
        _path: &'a harmonia_protocol::types::DaemonPath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        // No-op: gc-root bookkeeping for ad-hoc builds is the queue
        // runner's job (it sets one in `hydra-roots` when the build
        // starts), and harmonia 1.37 doesn't decode the log payload nix
        // 2.34 emits for AddIndirectRoot anyway.
        ready(Ok(())).empty_logs()
    }

    fn add_perm_root<'a>(
        &'a mut self,
        store_path: &'a StorePath,
        gc_root: &'a harmonia_protocol::types::DaemonPath,
    ) -> impl ResultLog<Output = DaemonResult<harmonia_protocol::types::DaemonPath>> + Send + 'a
    {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().add_perm_root(store_path, gc_root).await
        }
        .empty_logs()
    }

    fn add_ca_to_store<'a, 'r, R>(
        &'a mut self,
        name: &'a str,
        cam: ContentAddressMethodAlgorithm,
        refs: &'a StorePathSet,
        repair: bool,
        source: R,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<ValidPathInfo>> + Send + 'r>>
    where
        R: AsyncBufRead + Send + Unpin + 'r,
        'a: 'r,
    {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard
                .client()
                .add_ca_to_store(name, cam, refs, repair, source)
                .await
        }
        .empty_logs()
        .boxed_result()
    }

    fn add_multiple_to_store<'s, 'i, 'r, S, R>(
        &'s mut self,
        repair: bool,
        dont_check_sigs: bool,
        stream: S,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        S: futures::Stream<Item = Result<AddToStoreItem<R>, ProtocolError>> + Send + 'i,
        R: AsyncBufRead + Send + Unpin + 'i,
        's: 'r,
        'i: 'r,
    {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard
                .client()
                .add_multiple_to_store(repair, dont_check_sigs, stream)
                .await
        }
        .empty_logs()
        .boxed_result()
    }

    fn add_to_store_nar<'s, 'r, 'i, R>(
        &'s mut self,
        info: &'i ValidPathInfo,
        source: R,
        repair: bool,
        dont_check_sigs: bool,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        R: AsyncBufRead + Send + Unpin + 'r,
        's: 'r,
        'i: 'r,
    {
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard
                .client()
                .add_to_store_nar(info, source, repair, dont_check_sigs)
                .await
        }
        .empty_logs()
        .boxed_result()
    }

    fn shutdown(&mut self) -> impl Future<Output = DaemonResult<()>> + Send + '_ {
        ready(Ok(()))
    }
}
