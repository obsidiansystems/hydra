use std::path::PathBuf;

use hashbrown::HashMap;
use harmonia_store_remote::pool::{ConnectionPool, PoolConfig};
use harmonia_store_remote::DaemonStore as _;
use harmonia_utils_hash::fmt::CommonHash as _;

use crate::{Error, PathInfo, StoreDir, StorePath};

fn convert_path_info(
    info: harmonia_store_remote::UnkeyedValidPathInfo,
) -> PathInfo {
    PathInfo {
        deriver: info.deriver,
        nar_hash: format!("{}", info.nar_hash.as_base32()),
        registration_time: info.registration_time.map_or(0, |t| t.get()),
        nar_size: info.nar_size,
        refs: info.references.into_iter().collect(),
        sigs: info.signatures.iter().map(ToString::to_string).collect(),
        ca: info.ca.map(|ca| ca.to_string()),
    }
}

/// A pure-Rust store implementation that talks to the Nix daemon over Unix socket.
///
/// This replaces the C++ FFI `BaseStoreImpl` for read operations, using harmonia's
/// daemon protocol client with connection pooling.
#[allow(missing_debug_implementations)]
pub struct DaemonLocalStore {
    pool: ConnectionPool,
    store_dir: StoreDir,
}

impl DaemonLocalStore {
    /// Connect to the default Nix daemon socket at `/nix/var/nix/daemon-socket/socket`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_socket(PathBuf::from("/nix/var/nix/daemon-socket/socket"))
    }

    /// Connect to a Nix daemon at the given socket path.
    #[must_use]
    pub fn with_socket(socket_path: PathBuf) -> Self {
        let store_dir = StoreDir::default();
        let pool = ConnectionPool::new(socket_path, PoolConfig::default());
        Self { pool, store_dir }
    }

    #[must_use]
    pub fn store_dir(&self) -> &StoreDir {
        &self.store_dir
    }

    pub async fn is_valid_path(&self, path: &StorePath) -> bool {
        let Ok(mut guard) = self.pool.acquire().await else {
            return false;
        };
        guard.client().is_valid_path(path).await.unwrap_or(false)
    }

    pub async fn query_path_info(&self, path: &StorePath) -> Option<PathInfo> {
        let mut guard = self.pool.acquire().await.ok()?;
        let info = guard.client().query_path_info(path).await.ok()??;
        Some(convert_path_info(info))
    }

    pub async fn query_path_infos(
        &self,
        paths: &[&StorePath],
    ) -> HashMap<StorePath, PathInfo> {
        let mut result = HashMap::with_capacity(paths.len());
        // Query in parallel using separate pool connections
        let futs: Vec<_> = paths
            .iter()
            .map(|&path| {
                let path = path.clone();
                async move {
                    let info = self.query_path_info(&path).await?;
                    Some((path, info))
                }
            })
            .collect();
        for fut in futs {
            if let Some((path, info)) = fut.await {
                result.insert(path, info);
            }
        }
        result
    }

    pub async fn ensure_path(&self, path: &StorePath) -> Result<(), Error> {
        let mut guard = self
            .pool
            .acquire()
            .await
            .map_err(|e| anyhow::anyhow!("daemon pool: {e}"))?;
        guard
            .client()
            .ensure_path(path)
            .await
            .map_err(|e| anyhow::anyhow!("daemon ensure_path: {e}"))?;
        Ok(())
    }

    pub async fn query_valid_paths(&self, paths: &[&StorePath]) -> Vec<StorePath> {
        let set: std::collections::BTreeSet<StorePath> =
            paths.iter().map(|&p| p.clone()).collect();
        let Ok(mut guard) = self.pool.acquire().await else {
            return vec![];
        };
        guard
            .client()
            .query_valid_paths(&set, false)
            .await
            .map(|s| s.into_iter().collect())
            .unwrap_or_default()
    }

    pub async fn query_realisation(
        &self,
        output_id: &crate::DrvOutput,
    ) -> Option<crate::Realisation> {
        let mut guard = self.pool.acquire().await.ok()?;
        let realisations = guard
            .client()
            .query_realisation(output_id)
            .await
            .ok()?;
        realisations.into_iter().next()
    }
}

impl Default for DaemonLocalStore {
    fn default() -> Self {
        Self::new()
    }
}
