use daemon_server::{BuildRequest, SubmitBuild};
use db::models::BuildID;

/// Files every request under one hidden `adhoc/adhoc` jobset.
///
/// A client connecting to the standalone daemon tells us nothing about
/// why it wants the derivation built, so there is no evaluation or job
/// to attribute the build to. The rows still need *a* jobset to hang
/// off, hence the shared hidden one.
#[derive(Debug, Clone)]
pub(crate) struct AdhocSubmitter {
    jobset_id: i32,
}

impl AdhocSubmitter {
    /// Create the `adhoc/adhoc` jobset if it does not exist yet, and
    /// remember its id.
    ///
    /// Resolved once at startup rather than per build: the jobset is
    /// created on demand and never removed, so re-checking on every
    /// request would be a round trip to learn the same answer.
    pub(crate) async fn new(db: db::Database) -> Result<Self, db::Error> {
        let jobset_id = db.get().await?.ensure_adhoc_jobset().await?;
        Ok(Self { jobset_id })
    }
}

impl SubmitBuild for AdhocSubmitter {
    async fn submit(
        &self,
        tx: &mut db::Transaction<'_>,
        request: BuildRequest<'_>,
    ) -> Result<BuildID, db::Error> {
        Ok(tx
            .insert_daemon_build(
                self.jobset_id,
                request.nix_name,
                request.drv_path,
                request.system,
            )
            .await?)
    }
}
