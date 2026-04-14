use std::collections::BTreeMap;
use std::future::{Future, ready};
use std::path::Path;
use std::pin::Pin;

use tokio::io::AsyncBufRead;

use harmonia_protocol::daemon::{
    DaemonError as ProtocolError, DaemonResult, DaemonStore, FutureResultExt, HandshakeDaemonStore,
    ResultLog, ResultLogExt, TrustLevel,
};
use harmonia_protocol::daemon_wire::types2::{BuildMode, KeyedBuildResult};
use harmonia_protocol::types::AddToStoreItem;
use harmonia_protocol::valid_path_info::{UnkeyedValidPathInfo, ValidPathInfo};
use harmonia_store_build_result::{
    BuildResult, BuildResultFailure, BuildResultInner, BuildResultSuccess, FailureStatus,
    SuccessStatus,
};
use harmonia_store_core::derivation::{BasicDerivation, DerivationOutput};
use harmonia_store_core::derived_path::{DerivedPath, OutputName, SingleDerivedPath};
use harmonia_store_core::realisation::UnkeyedRealisation;
use harmonia_store_core::store_path::{
    ContentAddressMethodAlgorithm, StorePath, StorePathHash, StorePathSet,
};
use harmonia_store_remote::pool::{ConnectionPool, PoolConfig};

use db::StoreDir;
use db::models::BuildStatus;

use crate::waiter::BuildWaiter;

/// Daemon-store implementation that schedules build requests through Hydra.
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
        upstream_socket: &Path,
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

    /// Reject output maps with unresolved paths before synthesizing realisations.
    async fn assert_static_outputs(&self, drv_path: &StorePath) -> Result<(), ProtocolError> {
        let mut guard = self
            .upstream
            .acquire()
            .await
            .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
        let map = guard.client().query_derivation_output_map(drv_path).await?;
        let unresolved: Vec<String> = map
            .iter()
            .filter(|(_, path)| path.is_none())
            .map(|(name, _)| name.as_ref().to_owned())
            .collect();
        if !unresolved.is_empty() {
            return Err(ProtocolError::custom(format!(
                "drv-daemon BuildPathsWithResults requires static output paths; \
                 {} has unresolved (likely CA-floating) outputs: {}",
                self.store_dir.display(drv_path),
                unresolved.join(", ")
            )));
        }
        Ok(())
    }

    /// Reject BuildDerivation requests whose .drv is not present for the queue runner to read.
    async fn assert_drv_uploaded(&self, drv_path: &StorePath) -> Result<(), ProtocolError> {
        let mut guard = self
            .upstream
            .acquire()
            .await
            .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
        if !guard.client().is_valid_path(drv_path).await? {
            return Err(ProtocolError::custom(format!(
                "drv-daemon BuildDerivation: {} is not present in the \
                 upstream store; upload it via add_to_store_nar / \
                 add_multiple_to_store before requesting a build",
                self.store_dir.display(drv_path)
            )));
        }
        Ok(())
    }

    /// Register the waiter before commit so the queue runner cannot finish the row first.
    async fn run_adhoc_build(
        &self,
        drv_path: &StorePath,
        nix_name: &str,
        system: &str,
    ) -> Result<db::FinishedBuild, ProtocolError> {
        let drv_path_str = self.store_dir.display(drv_path).to_string();
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
            .insert_adhoc_build(jobset_id, nix_name, &drv_path_str, system)
            .await
            .map_err(|e| ProtocolError::custom(format!("insert adhoc build: {e}")))?;

        let rx = self
            .waiter
            .register(build_id)
            .await
            .map_err(|e| ProtocolError::custom(format!("cannot schedule ad-hoc build: {e}")))?;

        // Keep waiter cleanup centralized for every pre-completion error path.
        let result: Result<db::FinishedBuild, ProtocolError> = async {
            tx.notify_builds_added()
                .await
                .map_err(|e| ProtocolError::custom(format!("notify builds_added: {e}")))?;
            tx.commit()
                .await
                .map_err(|e| ProtocolError::custom(format!("commit: {e}")))?;

            tracing::info!(
                build_id,
                drv_path = %drv_path_str,
                "scheduled ad-hoc build, awaiting finish"
            );

            if rx.await.is_err() {
                return Err(ProtocolError::custom(
                    "build_finished listener went unhealthy mid-flight; \
                     try again once the daemon reconnects",
                ));
            }

            let mut conn = self
                .db
                .get()
                .await
                .map_err(|e| ProtocolError::custom(format!("hydra db: {e}")))?;
            conn.get_finished_build(&self.store_dir, build_id)
                .await
                .map_err(|e| ProtocolError::custom(format!("read finished build: {e}")))?
                .ok_or_else(|| {
                    ProtocolError::custom(format!("build {build_id} woke but has no finished row"))
                })
        }
        .await;

        if result.is_err() {
            self.waiter.forget(build_id).await;
        }
        result
    }
}

/// Reject non-IA derivations because the daemon cannot return real CA realisations.
fn require_input_addressed(
    store_dir: &StoreDir,
    drv_path: &StorePath,
    outputs: &BTreeMap<OutputName, DerivationOutput>,
) -> Result<(), ProtocolError> {
    let bad: Vec<_> = outputs
        .iter()
        .filter(|(_, o)| !matches!(o, DerivationOutput::InputAddressed(_)))
        .map(|(name, o)| format!("{name}={o:?}"))
        .collect();
    if !bad.is_empty() {
        return Err(ProtocolError::custom(format!(
            "drv-daemon only supports input-addressed derivations; \
             {} has non-IA outputs: {}",
            store_dir.display(drv_path),
            bad.join(", ")
        )));
    }
    Ok(())
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

fn finished_to_build_result(
    store_dir: &StoreDir,
    drv_path: &StorePath,
    finished: &db::FinishedBuild,
) -> Result<BuildResult, ProtocolError> {
    let inner = match finished.status {
        BuildStatus::Success => BuildResultInner::Success(BuildResultSuccess {
            status: SuccessStatus::Built,
            built_outputs: synthesize_built_outputs(store_dir, drv_path, &finished.outputs)?,
        }),
        status => BuildResultInner::Failure(BuildResultFailure {
            status: build_status_to_failure(status),
            error_msg: build_status_message(status).into(),
            is_non_deterministic: status == BuildStatus::NotDeterministic,
        }),
    };
    Ok(BuildResult {
        inner,
        times_built: 1,
        start_time: finished.start_time.unwrap_or(0).into(),
        stop_time: finished.stop_time.unwrap_or(0).into(),
        cpu_user: None,
        cpu_system: None,
    })
}

/// Synthesize realisations from recorded output paths; missing paths are queue-runner bugs.
fn synthesize_built_outputs(
    store_dir: &StoreDir,
    drv_path: &StorePath,
    outputs: &BTreeMap<OutputName, Option<StorePath>>,
) -> Result<BTreeMap<OutputName, UnkeyedRealisation>, ProtocolError> {
    let mut map = BTreeMap::new();
    for (output_name, path) in outputs {
        let Some(out_path) = path else {
            return Err(ProtocolError::custom(format!(
                "ad-hoc build of {} succeeded but BuildStepOutputs.path \
                 for '{output_name}' is NULL — daemon refuses to fabricate a realisation \
                 from a missing path",
                store_dir.display(drv_path)
            )));
        };
        map.insert(
            output_name.clone(),
            UnkeyedRealisation {
                out_path: out_path.clone(),
                signatures: Default::default(),
            },
        );
    }
    Ok(map)
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
        | BuildStatus::Resolved
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
        BuildStatus::Resolved => "CA derivation resolved (transient state)",
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
        // Client options are intentionally ignored; proxied reads use upstream's settings.
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
            require_input_addressed(&this.store_dir, drv_path, &drv.outputs)?;
            this.assert_drv_uploaded(drv_path).await?;
            let nix_name: String = drv.name.to_string();
            let system = std::str::from_utf8(&drv.platform)
                .map_err(|e| ProtocolError::custom(format!("non-utf8 platform: {e}")))?;
            let finished = this.run_adhoc_build(drv_path, &nix_name, system).await?;
            finished_to_build_result(&this.store_dir, drv_path, &finished)
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
                let nix_name = drv_path.name().as_ref().to_owned();
                let finished = this.run_adhoc_build(drv_path, &nix_name, "").await?;
                if finished.status != BuildStatus::Success {
                    return Err(ProtocolError::custom(format!(
                        "ad-hoc build of {} failed: {}",
                        this.store_dir.display(drv_path),
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
                this.assert_static_outputs(drv_path).await?;
                let nix_name = drv_path.name().as_ref().to_owned();
                let finished = this.run_adhoc_build(drv_path, &nix_name, "").await?;
                let result = finished_to_build_result(&this.store_dir, drv_path, &finished)?;
                results.push(KeyedBuildResult {
                    path: path.clone(),
                    result,
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
    ) -> impl ResultLog<Output = DaemonResult<Option<UnkeyedRealisation>>> + Send + 'a {
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
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        // Temp roots attach to the upstream pooled connection, so they may outlive this client.
        let upstream = self.upstream.clone();
        async move {
            let mut guard = upstream
                .acquire()
                .await
                .map_err(|e| ProtocolError::custom(format!("upstream pool: {e}")))?;
            guard.client().add_temp_root(path).await
        }
        .empty_logs()
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
        // Upstream proxying cannot decode this op's log payload yet; return unsupported rather than pretending a root was created.
        ready(Err(ProtocolError::custom(
            "drv-daemon does not support AddIndirectRoot; \
             use `nix-store --add-root <path>` without `--indirect` \
             (the daemon proxies AddPermRoot to the upstream nix-daemon)",
        )))
        .empty_logs()
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
