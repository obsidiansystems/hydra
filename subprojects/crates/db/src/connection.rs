use std::collections::BTreeMap;

use anyhow::Context;
use sqlx::Acquire;

use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::store_path::{StoreDir, StorePath};

use super::models::{
    Build, BuildID, BuildSmall, BuildStatus, BuildSteps, InsertBuildMetric, InsertBuildProduct,
    InsertBuildStep, InsertBuildStepOutput, Jobset, UpdateBuild, UpdateBuildStep,
    UpdateBuildStepInFinish,
};

#[derive(Debug)]
pub struct Connection {
    conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
}

#[derive(Debug)]
pub struct Transaction<'a> {
    tx: sqlx::PgTransaction<'a>,
}

impl Connection {
    #[must_use]
    pub const fn new(conn: sqlx::pool::PoolConnection<sqlx::Postgres>) -> Self {
        Self { conn }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn begin_transaction(&mut self) -> sqlx::Result<Transaction<'_>> {
        let tx = self.conn.begin().await?;
        Ok(Transaction { tx })
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_not_finished_builds_fast(&mut self) -> sqlx::Result<Vec<BuildSmall>> {
        sqlx::query_as!(
            BuildSmall,
            r#"
            SELECT
              id,
              globalPriority
            FROM builds
            WHERE finished = 0;"#
        )
        .fetch_all(&mut *self.conn)
        .await
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_not_finished_builds(
        &mut self,
        store_dir: &StoreDir,
    ) -> anyhow::Result<Vec<Build>> {
        let rows = sqlx::query_as!(
            Build::<String>,
            r#"
            SELECT
              builds.id,
              builds.jobset_id,
              jobsets.project as project,
              jobsets.name as jobset,
              job,
              drvPath,
              maxsilent,
              timeout,
              timestamp,
              globalPriority,
              priority
            FROM builds
            INNER JOIN jobsets ON builds.jobset_id = jobsets.id
            WHERE finished = 0 ORDER BY globalPriority desc, schedulingshares, random();"#
        )
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter()
            .map(|r| Ok(r.parse_paths(store_dir)?))
            .collect()
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_jobsets(&mut self) -> sqlx::Result<Vec<Jobset>> {
        sqlx::query_as!(
            Jobset,
            r#"
            SELECT
              project,
              name,
              schedulingshares
            FROM jobsets"#
        )
        .fetch_all(&mut *self.conn)
        .await
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_jobset_scheduling_shares(
        &mut self,
        jobset_id: i32,
    ) -> sqlx::Result<Option<i32>> {
        Ok(sqlx::query!(
            "SELECT schedulingshares FROM jobsets WHERE id = $1",
            jobset_id,
        )
        .fetch_optional(&mut *self.conn)
        .await?
        .map(|v| v.schedulingshares))
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_jobset_build_steps(
        &mut self,
        jobset_id: i32,
        scheduling_window: i64,
    ) -> sqlx::Result<Vec<BuildSteps>> {
        #[allow(clippy::cast_precision_loss)]
        sqlx::query_as!(
            BuildSteps,
            r#"
            SELECT s.startTime, s.stopTime FROM buildsteps s
            JOIN builds b ON b.drvPath = s.drvPath
            WHERE
              s.startTime IS NOT NULL AND
              to_timestamp(s.stopTime) > (NOW() - (interval '1 second' * $1)) AND
              b.jobset_id = $2
            "#,
            Some((scheduling_window * 10) as f64),
            jobset_id,
        )
        .fetch_all(&mut *self.conn)
        .await
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn abort_build(&mut self, build_id: BuildID) -> sqlx::Result<()> {
        #[allow(clippy::cast_possible_truncation)]
        sqlx::query!(
            "UPDATE builds SET finished = 1, buildStatus = $2, startTime = $3, stopTime = $3 where id = $1 and finished = 0",
            build_id,
            BuildStatus::Aborted as i32,
            // TODO migrate to 64bit timestamp
            jiff::Timestamp::now().as_second() as i32,
        )
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, paths), err)]
    pub async fn check_if_paths_failed(
        &mut self,
        store_dir: &StoreDir,
        paths: &[StorePath],
    ) -> sqlx::Result<bool> {
        let paths: Vec<String> = paths
            .iter()
            .map(|p| store_dir.display(p).to_string())
            .collect();
        Ok(
            !sqlx::query!("SELECT path FROM failedpaths where path = ANY($1)", &paths)
                .fetch_all(&mut *self.conn)
                .await?
                .is_empty(),
        )
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn clear_busy(&mut self, stop_time: i32) -> sqlx::Result<()> {
        sqlx::query!(
            "UPDATE buildsteps SET busy = 0, status = $1, stopTime = $2 WHERE busy != 0;",
            BuildStatus::Aborted as i32,
            Some(stop_time),
        )
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, step), err)]
    pub async fn update_build_step(
        &mut self,
        store_dir: &StoreDir,
        step: UpdateBuildStep<'_>,
    ) -> sqlx::Result<()> {
        let drv_path = store_dir.display(step.drv_path).to_string();
        sqlx::query!(
            r#"
            UPDATE buildsteps SET busy = $1
            WHERE drvPath = $2
              AND attempt = $3
              AND busy != 0
              AND status IS NULL
            "#,
            step.status as i32,
            drv_path.as_str(),
            step.attempt,
        )
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn insert_debug_build(
        &mut self,
        store_dir: &StoreDir,
        jobset_id: i32,
        drv_path: &StorePath,
        system: &str,
    ) -> sqlx::Result<()> {
        let drv_path = store_dir.display(drv_path).to_string();
        sqlx::query!(
            r#"INSERT INTO builds (
              finished,
              timestamp,
              jobset_id,
              job,
              nixname,
              drvpath,
              system,
              maxsilent,
              timeout,
              ischannel,
              iscurrent,
              priority,
              globalpriority,
              keep
            ) VALUES (
              0,
              EXTRACT(EPOCH FROM NOW())::INT4,
              $1,
              'debug',
              'debug',
              $2,
              $3,
              7200,
              36000,
              0,
              0,
              100,
              0,
            0);"#,
            jobset_id,
            drv_path,
            system,
        )
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn get_build_output_for_path(
        &mut self,
        store_dir: &StoreDir,
        out_path: &StorePath,
    ) -> sqlx::Result<Option<super::models::BuildOutput>> {
        let out_path = store_dir.display(out_path).to_string();
        sqlx::query_as!(
            super::models::BuildOutput,
            r#"
            SELECT
              id, buildStatus, releaseName, closureSize, size
            FROM builds b
            JOIN buildoutputs o on b.id = o.build
            WHERE finished = 1 and (buildStatus = 0 or buildStatus = 6) and path = $1;"#,
            out_path.as_str(),
        )
        .fetch_optional(&mut *self.conn)
        .await
    }

    pub async fn get_build_products_for_build_id(
        &mut self,
        build_id: BuildID,
        store_dir: &StoreDir,
    ) -> anyhow::Result<Vec<crate::models::OwnedBuildProduct>> {
        let rows = sqlx::query_as!(
            crate::models::OwnedBuildProduct::<String>,
            r#"
            SELECT
              type,
              subtype,
              fileSize,
              sha256hash,
              path,
              name,
              defaultPath
            FROM buildproducts
            WHERE build = $1 ORDER BY productnr;"#,
            build_id
        )
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter()
            .map(|r| Ok(r.parse_paths(store_dir)?))
            .collect()
    }

    pub async fn get_build_metrics_for_build_id(
        &mut self,
        build_id: BuildID,
    ) -> sqlx::Result<Vec<crate::models::OwnedBuildMetric>> {
        sqlx::query_as!(
            crate::models::OwnedBuildMetric,
            r#"
            SELECT
              name, unit, value
            FROM buildmetrics
            WHERE build = $1;"#,
            build_id
        )
        .fetch_all(&mut *self.conn)
        .await
    }

    /// Resolve output paths for derivation chains via `buildstepoutputs`.
    ///
    /// Each entry is `(root_drv_path, &[output_name, ...])` representing a
    /// chain like `root.drv^out1^out2`. The recursive CTE walks the chain:
    /// look up `root.drv`'s `out1` output to get an intermediate drv path,
    /// then look up that drv's `out2`, etc. Returns the final resolved path
    /// for each chain (or `None` if any step fails).
    ///
    /// # Panics
    ///
    /// Panics if the SQL `ordinality` column is negative (should never happen).
    pub async fn resolve_drv_output_chains(
        &mut self,
        store_dir: &StoreDir,
        chains: &[(&StorePath, &[&OutputName])],
    ) -> sqlx::Result<Vec<Option<StorePath>>> {
        if chains.is_empty() {
            return Ok(Vec::new());
        }

        // We pack as JSON here since sqlx can't bind `text[][]` directly.
        let json_input = serde_json::Value::Array(
            chains
                .iter()
                .map(|(root, outputs)| {
                    serde_json::json!({
                        "root": store_dir.display(*root).to_string(),
                        "chain": outputs.iter().map(AsRef::as_ref).collect::<Vec<&str>>(),
                    })
                })
                .collect(),
        );

        let rows = sqlx::query_as::<_, (i32, Option<String>)>(
            "
            WITH RECURSIVE input AS (
                SELECT (ordinality)::int AS idx,
                       elem->>'root' AS drv,
                       ARRAY(SELECT jsonb_array_elements_text(elem->'chain')) AS chain
                FROM jsonb_array_elements($1::jsonb)
                    WITH ORDINALITY AS t(elem, ordinality)
            ),
            resolve(idx, drv_path, step) AS (
                SELECT idx, drv, 1 FROM input

                UNION ALL

                SELECT r.idx, sub.path, r.step + 1
                FROM resolve r
                JOIN input i ON i.idx = r.idx
                CROSS JOIN LATERAL (
                    SELECT o.path
                    FROM buildsteps s
                    JOIN buildstepoutputs o
                        ON s.drvPath = o.drvPath AND s.attempt = o.attempt
                    WHERE s.drvPath = r.drv_path
                      AND o.name = i.chain[r.step]
                      AND o.path IS NOT NULL
                      AND s.status = 0
                    ORDER BY s.attempt DESC
                    LIMIT 1
                ) sub
                WHERE r.step <= array_length(i.chain, 1)
                  AND r.drv_path IS NOT NULL
            )
            SELECT i.idx, r.drv_path
            FROM input i
            LEFT JOIN resolve r
                ON r.idx = i.idx
                AND r.step = array_length(i.chain, 1) + 1
            ORDER BY i.idx
            ",
        )
        .bind(&json_input)
        .fetch_all(&mut *self.conn)
        .await?;

        let mut results = vec![None; chains.len()];
        for (idx, path) in rows {
            let i = usize::try_from(idx - 1)
                .context("SQL ordinality is always positive")
                .map_err(|e| sqlx::Error::Decode(e.into_boxed_dyn_error()))?;
            results[i] = path
                .map(|p| {
                    store_dir
                        .parse(&p)
                        .map_err(|e| sqlx::Error::Decode(Box::new(e)))
                })
                .transpose()?;
        }
        Ok(results)
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_status(&mut self) -> sqlx::Result<Option<serde_json::Value>> {
        Ok(
            sqlx::query!("SELECT status FROM systemstatus WHERE what = 'queue-runner';",)
                .fetch_optional(&mut *self.conn)
                .await?
                .map(|v| v.status),
        )
    }

    /// Check if a derivation path already exists in the Derivations table.
    #[tracing::instrument(skip(self), err)]
    pub async fn derivation_exists(&mut self, drv_path: &str) -> sqlx::Result<bool> {
        Ok(sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM Derivations WHERE path = $1) as "exists!""#,
            drv_path,
        )
        .fetch_one(&mut *self.conn)
        .await?)
    }

    /// Count the number of build steps for a given derivation path.
    /// Used to determine retry count.
    #[tracing::instrument(skip(self), err)]
    pub async fn count_build_steps_for_drv(&mut self, drv_path: &str) -> sqlx::Result<i64> {
        Ok(sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!" FROM BuildSteps WHERE drvPath = $1"#,
            drv_path,
        )
        .fetch_one(&mut *self.conn)
        .await?)
    }

    /// Get all dispatch candidates from the ready queue, with scheduling
    /// priority data computed via joins on `Builds` through `BuildStepDeps`.
    #[tracing::instrument(skip(self), err)]
    pub async fn get_dispatch_candidates(
        &mut self,
    ) -> sqlx::Result<Vec<super::models::DispatchCandidate>> {
        Ok(sqlx::query_as!(
            super::models::DispatchCandidate,
            r#"
            SELECT
              q.drvPath as "drv_path!",
              q.readyTime as "ready_time!: i32",
              COALESCE(prio.max_global, 0) as "highest_global_priority!",
              COALESCE(prio.max_local, 0) as "highest_local_priority!",
              COALESCE(prio.min_id, 2147483647) as "lowest_build_id!",
              COALESCE(rdeps.cnt, 0) as "rdeps_count!"
            FROM BuildStepCanCreate q
            LEFT JOIN LATERAL (
              WITH RECURSIVE all_rdeps AS (
                SELECT drvPath FROM BuildStepDeps WHERE depDrvPath = q.drvPath
                UNION
                SELECT dep.drvPath FROM BuildStepDeps dep
                JOIN all_rdeps r ON dep.depDrvPath = r.drvPath
              )
              SELECT
                MAX(b.globalPriority) AS max_global,
                MAX(b.priority) AS max_local,
                MIN(b.id) AS min_id
              FROM (SELECT drvPath FROM all_rdeps UNION ALL SELECT q.drvPath) all_paths
              JOIN Builds b ON b.drvPath = all_paths.drvPath AND b.finished = 0
            ) prio ON true
            LEFT JOIN LATERAL (
              SELECT COUNT(*) AS cnt FROM BuildStepDeps WHERE depDrvPath = q.drvPath
            ) rdeps ON true
            "#,
        )
        .fetch_all(&mut *self.conn)
        .await?)
    }

    /// Get all builds that transitively depend on a step, via recursive CTE.
    #[tracing::instrument(skip(self), err)]
    pub async fn get_dependent_builds(
        &mut self,
        drv_path: &str,
    ) -> sqlx::Result<Vec<Build<String>>> {
        sqlx::query_as!(
            Build::<String>,
            r#"
            WITH RECURSIVE rdeps AS (
              SELECT drvPath FROM BuildStepDeps WHERE depDrvPath = $1
              UNION
              SELECT d.drvPath FROM BuildStepDeps d
              JOIN rdeps r ON d.depDrvPath = r.drvPath
            )
            SELECT
              builds.id,
              builds.jobset_id,
              jobsets.project as project,
              jobsets.name as jobset,
              job,
              builds.drvPath,
              maxsilent,
              timeout,
              timestamp,
              globalPriority,
              priority
            FROM Builds
            INNER JOIN Jobsets ON builds.jobset_id = jobsets.id
            WHERE builds.finished = 0
              AND builds.drvPath IN (SELECT drvPath FROM rdeps UNION ALL SELECT $1)
            "#,
            drv_path,
        )
        .fetch_all(&mut *self.conn)
        .await
    }
}

impl Transaction<'_> {
    #[tracing::instrument(skip(self), err)]
    pub async fn commit(self) -> sqlx::Result<()> {
        self.tx.commit().await
    }

    #[tracing::instrument(skip(self, v), err)]
    pub async fn update_build(
        &mut self,
        build_id: BuildID,
        v: UpdateBuild<'_>,
    ) -> sqlx::Result<()> {
        sqlx::query!(
            r#"
            UPDATE builds SET
              finished = 1,
              buildStatus = $2,
              startTime = $3,
              stopTime = $4,
              size = $5,
              closureSize = $6,
              releaseName = $7,
              isCachedBuild = $8,
              notificationPendingSince = $4
            WHERE
              id = $1"#,
            build_id,
            v.status as i32,
            v.start_time,
            v.stop_time,
            v.size,
            v.closure_size,
            v.release_name,
            i32::from(v.is_cached_build),
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, status, start_time, stop_time, is_cached_build), err)]
    pub async fn update_build_after_failure(
        &mut self,
        build_id: BuildID,
        status: BuildStatus,
        start_time: i32,
        stop_time: i32,
        is_cached_build: bool,
    ) -> sqlx::Result<()> {
        sqlx::query!(
            r#"
            UPDATE builds SET
              finished = 1,
              buildStatus = $2,
              startTime = $3,
              stopTime = $4,
              isCachedBuild = $5,
              notificationPendingSince = $4
            WHERE
              id = $1 AND finished = 0"#,
            build_id,
            status as i32,
            start_time,
            stop_time,
            i32::from(is_cached_build),
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, status), err)]
    pub async fn update_build_after_previous_failure(
        &mut self,
        build_id: BuildID,
        status: BuildStatus,
    ) -> sqlx::Result<()> {
        #[allow(clippy::cast_possible_truncation)]
        sqlx::query!(
            r#"
            UPDATE builds SET
              finished = 1,
              buildStatus = $2,
              startTime = $3,
              stopTime = $3,
              isCachedBuild = 1,
              notificationPendingSince = $3
            WHERE
              id = $1 AND finished = 0"#,
            build_id,
            status as i32,
            // TODO migrate to 64bit timestamp
            jiff::Timestamp::now().as_second() as i32,
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, name, path), err)]
    pub async fn update_build_output(
        &mut self,
        store_dir: &StoreDir,
        build_id: BuildID,
        name: &str,
        path: &StorePath,
    ) -> sqlx::Result<()> {
        let path = store_dir.display(path).to_string();
        // TODO: support inserting multiple at the same time
        sqlx::query!(
            "UPDATE buildoutputs SET path = $3 WHERE build = $1 AND name = $2",
            build_id,
            name,
            path.as_str(),
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_last_build_step_id(
        &mut self,
        store_dir: &StoreDir,
        path: &StorePath,
    ) -> sqlx::Result<Option<i32>> {
        let path = store_dir.display(path).to_string();
        Ok(sqlx::query!(
            r#"
            SELECT b.id AS build FROM buildsteps s
            JOIN builds b ON b.drvPath = s.drvPath
            WHERE s.drvPath = $1
              AND s.startTime != 0
              AND s.stopTime != 0
              AND s.status = 1
            ORDER BY s.attempt DESC LIMIT 1
            "#,
            path.as_str(),
        )
        .fetch_optional(&mut *self.tx)
        .await?
        .map(|v| v.build))
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_last_build_step_id_for_output_path(
        &mut self,
        store_dir: &StoreDir,
        path: &StorePath,
    ) -> sqlx::Result<Option<i32>> {
        let path = store_dir.display(path).to_string();
        Ok(sqlx::query!(
            r#"
            SELECT b.id AS build FROM buildsteps s
            JOIN BuildStepOutputs o
              ON s.drvPath = o.drvPath
              AND s.attempt = o.attempt
            JOIN builds b ON b.drvPath = s.drvPath
            WHERE s.startTime != 0
              AND s.stopTime != 0
              AND s.status = 1
              AND o.path = $1
            ORDER BY s.attempt DESC LIMIT 1
            "#,
            path.as_str(),
        )
        .fetch_optional(&mut *self.tx)
        .await?
        .map(|v| v.build))
    }

    #[tracing::instrument(skip(self, drv_path, name), err)]
    pub async fn get_last_build_step_id_for_output_with_drv(
        &mut self,
        store_dir: &StoreDir,
        drv_path: &StorePath,
        name: &str,
    ) -> sqlx::Result<Option<i32>> {
        let drv_path = store_dir.display(drv_path).to_string();
        Ok(sqlx::query!(
            r#"
            SELECT b.id AS build FROM buildsteps s
            JOIN BuildStepOutputs o
              ON s.drvPath = o.drvPath
              AND s.attempt = o.attempt
            JOIN builds b ON b.drvPath = s.drvPath
            WHERE s.startTime != 0
              AND s.stopTime != 0
              AND s.status = 1
              AND s.drvPath = $1
              AND o.name = $2
            ORDER BY s.attempt DESC LIMIT 1
            "#,
            drv_path,
            name,
        )
        .fetch_optional(&mut *self.tx)
        .await?
        .map(|v| v.build))
    }

    #[tracing::instrument(skip(self, step), err)]
    pub async fn insert_build_step(
        &mut self,
        store_dir: &StoreDir,
        step: InsertBuildStep<'_>,
    ) -> sqlx::Result<Option<i32>> {
        let drv_path = store_dir.display(step.drv_path).to_string();
        let success = sqlx::query!(
            r#"
              WITH ensure_drv AS (
                INSERT INTO Derivations (path) VALUES ($1)
                ON CONFLICT DO NOTHING
              ),
              new_attempt AS (SELECT COALESCE(MAX(attempt), -1) + 1 AS val FROM buildsteps WHERE drvPath = $1)
              INSERT INTO buildsteps (
                type,
                drvPath,
                attempt,
                busy,
                startTime,
                stopTime,
                system,
                status,
                propagatedFrom,
                errorMsg,
                machine
              ) VALUES (
                $2, $1, (SELECT val FROM new_attempt), $3, $4, $5, $6, $7, $8, $9, $10
              )
              ON CONFLICT DO NOTHING
              RETURNING attempt AS "attempt!"
            "#,
            drv_path.as_str(),
            step.r#type as i32,
            i32::from(step.busy),
            step.start_time,
            step.stop_time,
            step.platform,
            if step.status == BuildStatus::Busy {
                None
            } else {
                Some(step.status as i32)
            },
            step.propagated_from,
            step.error_msg,
            step.machine,
        )
        .fetch_optional(&mut *self.tx)
        .await?
        .map(|v| v.attempt);
        Ok(success)
    }

    #[tracing::instrument(skip(self, outputs), err)]
    pub(crate) async fn insert_build_step_outputs(
        &mut self,
        store_dir: &StoreDir,
        outputs: &[InsertBuildStepOutput<'_>],
    ) -> sqlx::Result<()> {
        if outputs.is_empty() {
            return Ok(());
        }

        let mut query_builder =
            sqlx::QueryBuilder::new("INSERT INTO buildstepoutputs (drvPath, attempt, name, path) ");

        query_builder.push_values(outputs, |mut b, output| {
            b.push_bind(store_dir.display(output.drv_path).to_string())
                .push_bind(output.attempt)
                .push_bind(output.name.as_ref())
                .push_bind(
                    output
                        .path
                        .as_ref()
                        .map(|p| store_dir.display(p).to_string()),
                );
        });
        let query = query_builder.build();
        query.execute(&mut *self.tx).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, name, path), err)]
    pub async fn update_build_step_output(
        &mut self,
        store_dir: &StoreDir,
        drv_path: &StorePath,
        attempt: i32,
        name: &str,
        path: &StorePath,
    ) -> sqlx::Result<()> {
        let drv_path = store_dir.display(drv_path).to_string();
        let path = store_dir.display(path).to_string();
        // TODO: support inserting multiple at the same time
        sqlx::query!(
            r#"
            UPDATE buildstepoutputs SET path = $4
            WHERE drvPath = $1
              AND attempt = $2
              AND name = $3
            "#,
            drv_path.as_str(),
            attempt,
            name,
            path.as_str(),
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, store_dir, res), err)]
    pub async fn update_build_step_in_finish(
        &mut self,
        store_dir: &StoreDir,
        res: UpdateBuildStepInFinish<'_>,
    ) -> sqlx::Result<()> {
        let drv_path = store_dir.display(res.drv_path).to_string();
        sqlx::query!(
            r#"
            UPDATE buildsteps SET
              busy = 0,
              status = $1,
              errorMsg = $4,
              startTime = $5,
              stopTime = $6,
              machine = $7,
              overhead = $8,
              timesBuilt = $9,
              isNonDeterministic = $10
            WHERE
              drvPath = $2 AND attempt = $3
            "#,
            res.status as i32,
            drv_path.as_str(),
            res.attempt,
            res.error_msg,
            res.start_time,
            res.stop_time,
            res.machine,
            res.overhead,
            res.times_built,
            res.is_non_deterministic,
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    /// Look up the build that owns a step identified by `(drvPath, attempt)`.
    #[tracing::instrument(skip(self), err)]
    pub async fn get_build_id_for_step(
        &mut self,
        store_dir: &StoreDir,
        drv_path: &StorePath,
        attempt: i32,
    ) -> sqlx::Result<Option<i32>> {
        let drv_path = store_dir.display(drv_path).to_string();
        Ok(sqlx::query!(
            r#"
            SELECT b.id AS build FROM buildsteps s
            JOIN builds b ON b.drvPath = s.drvPath
            WHERE s.drvPath = $1 AND s.attempt = $2
            "#,
            drv_path.as_str(),
            attempt,
        )
        .fetch_optional(&mut *self.tx)
        .await?
        .map(|v| v.build))
    }

    #[tracing::instrument(skip(self, build_id), err)]
    pub async fn check_if_build_is_not_finished(
        &mut self,
        build_id: BuildID,
    ) -> sqlx::Result<bool> {
        Ok(sqlx::query!(
            "SELECT id FROM builds WHERE id = $1 AND finished = 0",
            build_id,
        )
        .fetch_optional(&mut *self.tx)
        .await?
        .is_some())
    }

    #[tracing::instrument(skip(self, p), err)]
    pub async fn insert_build_product(&mut self, p: InsertBuildProduct<'_>) -> sqlx::Result<()> {
        sqlx::query!(
            r#"
              INSERT INTO buildproducts (
                build,
                productnr,
                type,
                subtype,
                fileSize,
                sha256hash,
                path,
                name,
                defaultPath
              ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9
              )
            "#,
            p.build_id,
            p.product_nr,
            p.r#type,
            p.subtype,
            p.file_size,
            p.sha256hash,
            p.path,
            p.name,
            p.default_path,
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id), err)]
    pub async fn delete_build_products_by_build_id(
        &mut self,
        build_id: BuildID,
    ) -> sqlx::Result<()> {
        sqlx::query!("DELETE FROM buildproducts WHERE build = $1", build_id)
            .execute(&mut *self.tx)
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, metric), err)]
    pub async fn insert_build_metric(&mut self, metric: InsertBuildMetric<'_>) -> sqlx::Result<()> {
        sqlx::query!(
            r#"
              INSERT INTO buildmetrics (
                build,
                name,
                unit,
                value,
                project,
                jobset,
                job,
                timestamp
              ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8
              )
            "#,
            metric.build_id,
            metric.name,
            metric.unit,
            metric.value,
            metric.project,
            metric.jobset,
            metric.job,
            metric.timestamp,
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id), err)]
    pub async fn delete_build_metrics_by_build_id(
        &mut self,
        build_id: BuildID,
    ) -> sqlx::Result<()> {
        sqlx::query!("DELETE FROM buildmetrics WHERE build = $1", build_id)
            .execute(&mut *self.tx)
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, path), err)]
    pub async fn insert_failed_paths(
        &mut self,
        store_dir: &StoreDir,
        path: &StorePath,
    ) -> sqlx::Result<()> {
        let path = store_dir.display(path).to_string();
        sqlx::query!(
            r#"
              INSERT INTO failedpaths (
                path
              ) VALUES (
                $1
              )
            "#,
            path.as_str(),
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        skip(
            self,
            start_time,
            platform,
            machine,
            status,
            error_msg,
            propagated_from
        ),
        err
    )]
    pub async fn create_build_step(
        &mut self,
        store_dir: &StoreDir,
        start_time: Option<i32>,
        drv_path: &StorePath,
        platform: Option<&str>,
        machine: String,
        status: BuildStatus,
        error_msg: Option<String>,
        propagated_from: Option<BuildID>,
        outputs: BTreeMap<OutputName, Option<StorePath>>,
    ) -> sqlx::Result<i32> {
        let attempt = loop {
            if let Some(ids) = self
                .insert_build_step(
                    store_dir,
                    InsertBuildStep {
                        r#type: crate::models::BuildType::Build,
                        drv_path,
                        status,
                        busy: status == BuildStatus::Busy,
                        start_time,
                        stop_time: if status == BuildStatus::Busy {
                            None
                        } else {
                            start_time
                        },
                        platform,
                        propagated_from,
                        error_msg: error_msg.as_deref(),
                        machine: &machine,
                    },
                )
                .await?
            {
                break ids;
            }
        };

        self.insert_build_step_outputs(
            store_dir,
            &outputs
                .into_iter()
                .map(|(name, path)| InsertBuildStepOutput {
                    drv_path,
                    attempt,
                    name,
                    path,
                })
                .collect::<Vec<_>>(),
        )
        .await?;

        if status == BuildStatus::Busy {
            self.notify_step_started(store_dir, drv_path, attempt)
                .await?;
        }

        Ok(attempt)
    }

    #[tracing::instrument(
        skip(self, start_time, stop_time, drv_path, output,),
        err,
        ret
    )]
    pub async fn create_substitution_step(
        &mut self,
        store_dir: &StoreDir,
        start_time: i32,
        stop_time: i32,
        drv_path: &StorePath,
        output: (OutputName, Option<StorePath>),
    ) -> anyhow::Result<i32> {
        let attempt = loop {
            if let Some(ids) = self
                .insert_build_step(
                    store_dir,
                    InsertBuildStep {
                        r#type: crate::models::BuildType::Substitution,
                        drv_path,
                        status: BuildStatus::Success,
                        busy: false,
                        start_time: Some(start_time),
                        stop_time: Some(stop_time),
                        platform: None,
                        propagated_from: None,
                        error_msg: None,
                        machine: "",
                    },
                )
                .await?
            {
                break ids;
            }
        };

        self.insert_build_step_outputs(
            store_dir,
            &[InsertBuildStepOutput {
                drv_path,
                attempt,
                name: output.0,
                path: output.1,
            }],
        )
        .await?;

        Ok(attempt)
    }

    #[tracing::instrument(
        skip(self, build, is_cached_build, start_time, stop_time, store_dir),
        err
    )]
    pub async fn mark_succeeded_build(
        &mut self,
        build: crate::models::MarkBuildSuccessData<'_>,
        is_cached_build: bool,
        start_time: i32,
        stop_time: i32,
        store_dir: &StoreDir,
    ) -> anyhow::Result<()> {
        if build.finished_in_db {
            return Ok(());
        }

        if !self.check_if_build_is_not_finished(build.id).await? {
            return Ok(());
        }

        self.update_build(
            build.id,
            UpdateBuild {
                status: if build.failed {
                    BuildStatus::FailedWithOutput
                } else {
                    BuildStatus::Success
                },
                start_time,
                stop_time,
                size: i64::try_from(build.size)?,
                closure_size: i64::try_from(build.closure_size)?,
                release_name: build.release_name,
                is_cached_build,
            },
        )
        .await?;

        for (name, path) in &build.outputs {
            self.update_build_output(store_dir, build.id, name.as_ref(), path)
                .await?;
        }

        self.delete_build_products_by_build_id(build.id).await?;

        for (nr, p) in build.products.iter().enumerate() {
            self.insert_build_product(InsertBuildProduct {
                build_id: build.id,
                product_nr: i32::try_from(nr + 1)?,
                r#type: p.r#type,
                subtype: p.subtype,
                file_size: p.filesize,
                sha256hash: p.sha256hash,
                path: p.path.as_deref().unwrap_or_default(),
                name: p.name,
                default_path: p.defaultpath.unwrap_or_default(),
            })
            .await?;
        }

        self.delete_build_metrics_by_build_id(build.id).await?;
        for m in &build.metrics {
            self.insert_build_metric(InsertBuildMetric {
                build_id: build.id,
                name: m.name,
                unit: m.unit,
                value: m.value,
                project: build.project_name,
                jobset: build.jobset_name,
                job: build.name,
                timestamp: i32::try_from(build.timestamp)?, // TODO
            })
            .await?;
        }
        Ok(())
    }

    #[tracing::instrument(skip(self, status), err)]
    pub async fn upsert_status(&mut self, status: &serde_json::Value) -> sqlx::Result<()> {
        sqlx::query!(
            r#"INSERT INTO systemstatus (
              what, status
            ) VALUES (
              'queue-runner', $1
            ) ON CONFLICT (what) DO UPDATE SET status = EXCLUDED.status;"#,
            Some(status),
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    /// Ensure a derivation path exists in the `Derivations` table.
    #[tracing::instrument(skip(self), err)]
    pub async fn ensure_derivation_path(&mut self, drv_path: &str) -> sqlx::Result<()> {
        sqlx::query!(
            "INSERT INTO Derivations (path) VALUES ($1) ON CONFLICT DO NOTHING",
            drv_path,
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    /// Insert a single dependency edge.
    #[tracing::instrument(skip(self), err)]
    pub async fn insert_step_dep(
        &mut self,
        drv_path: &str,
        dep_drv_path: &str,
    ) -> sqlx::Result<()> {
        sqlx::query!(
            r#"
            WITH ensure_drv AS (
                INSERT INTO Derivations (path) VALUES ($1)
                ON CONFLICT DO NOTHING
            ),
            ensure_dep AS (
                INSERT INTO Derivations (path) VALUES ($2)
                ON CONFLICT DO NOTHING
            )
            INSERT INTO BuildStepDeps (drvPath, depDrvPath) VALUES ($1, $2)
            ON CONFLICT DO NOTHING
            "#,
            drv_path,
            dep_drv_path,
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    /// Batch-insert dependency edges for a single step.
    #[tracing::instrument(skip(self, dep_drv_paths), err)]
    pub async fn insert_step_deps(
        &mut self,
        drv_path: &str,
        dep_drv_paths: &[&str],
    ) -> sqlx::Result<()> {
        if dep_drv_paths.is_empty() {
            return Ok(());
        }

        // Ensure all drvPaths exist in DerivationPaths
        let all_paths: Vec<&str> = std::iter::once(drv_path)
            .chain(dep_drv_paths.iter().copied())
            .collect();
        let mut query_builder = sqlx::QueryBuilder::new("INSERT INTO Derivations (path) ");
        query_builder.push_values(&all_paths, |mut b, path| {
            b.push_bind(*path);
        });
        query_builder.push(" ON CONFLICT DO NOTHING");
        query_builder.build().execute(&mut *self.tx).await?;

        // Insert the dependency edges
        let mut query_builder =
            sqlx::QueryBuilder::new("INSERT INTO BuildStepDeps (drvPath, depDrvPath) ");
        query_builder.push_values(dep_drv_paths, |mut b, dep| {
            b.push_bind(drv_path).push_bind(*dep);
        });
        query_builder.push(" ON CONFLICT DO NOTHING");
        query_builder.build().execute(&mut *self.tx).await?;

        Ok(())
    }

    /// Insert a step into the ready queue (`BuildStepCanCreate`).
    #[tracing::instrument(skip(self), err)]
    pub async fn mark_step_ready(&mut self, drv_path: &str, ready_time: i32) -> sqlx::Result<()> {
        sqlx::query!(
            "INSERT INTO BuildStepCanCreate (drvPath, readyTime) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            drv_path,
            ready_time,
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    /// Mark a step as ready if all its deps are satisfied (have a successful BuildSteps row).
    /// If the step has no deps, it's unconditionally ready.
    #[tracing::instrument(skip(self), err)]
    pub async fn mark_step_ready_if_deps_satisfied(
        &mut self,
        drv_path: &str,
        ready_time: i32,
    ) -> sqlx::Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO BuildStepCanCreate (drvPath, readyTime)
            SELECT $1, $2
            WHERE NOT EXISTS (
                SELECT 1 FROM BuildStepDeps d
                WHERE d.drvPath = $1
                  AND NOT EXISTS (
                    SELECT 1 FROM BuildSteps s
                    WHERE s.drvPath = d.depDrvPath AND s.status = 0
                  )
            )
            ON CONFLICT DO NOTHING
            "#,
            drv_path,
            ready_time,
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    /// Remove a step from the ready queue (when dispatched).
    #[tracing::instrument(skip(self), err)]
    pub async fn unmark_step_ready(&mut self, drv_path: &str) -> sqlx::Result<()> {
        sqlx::query!(
            "DELETE FROM BuildStepCanCreate WHERE drvPath = $1",
            drv_path,
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    /// Remove a finished step from all dependency sets and insert
    /// any newly-ready steps into `BuildStepCanCreate`.
    /// Returns the drvPaths that became ready.
    #[tracing::instrument(skip(self), err)]
    pub async fn make_rdeps_runnable(
        &mut self,
        dep_drv_path: &str,
        ready_time: i32,
    ) -> sqlx::Result<Vec<String>> {
        // 1. Remove this step from all dep edges, get affected dependents
        let affected: Vec<String> = sqlx::query!(
            "DELETE FROM BuildStepDeps WHERE depDrvPath = $1 RETURNING drvPath",
            dep_drv_path,
        )
        .fetch_all(&mut *self.tx)
        .await?
        .into_iter()
        .map(|r| r.drvpath)
        .collect();

        if affected.is_empty() {
            return Ok(Vec::new());
        }

        // 2. Insert newly-ready steps (those with zero remaining deps) into ready queue
        let newly_ready: Vec<String> = sqlx::query!(
            r#"
            INSERT INTO BuildStepCanCreate (drvPath, readyTime)
            SELECT u.path, $2 FROM unnest($1::text[]) AS u(path)
            WHERE NOT EXISTS (SELECT 1 FROM BuildStepDeps WHERE drvPath = u.path)
            ON CONFLICT DO NOTHING
            RETURNING drvPath as "drvPath!"
            "#,
            &affected,
            ready_time,
        )
        .fetch_all(&mut *self.tx)
        .await?
        .into_iter()
        .map(|r| r.drvPath)
        .collect();

        Ok(newly_ready)
    }

    // Derivations are permanent records — no delete_derivation method.
    // Whether a dep is satisfied is derived by joining to BuildSteps.
}

impl Transaction<'_> {
    #[tracing::instrument(skip(self), err)]
    async fn notify_any(&mut self, channel: &str, msg: &str) -> sqlx::Result<()> {
        sqlx::query(
            r"SELECT pg_notify(chan, payload) from (values ($1, $2)) notifies(chan, payload)",
        )
        .bind(channel)
        .bind(msg)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn notify_builds_added(&mut self) -> sqlx::Result<()> {
        self.notify_any("builds_added", "?").await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id), err)]
    pub async fn notify_build_started(&mut self, build_id: BuildID) -> sqlx::Result<()> {
        self.notify_any("build_started", &build_id.to_string())
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id, dependent_ids,), err)]
    pub async fn notify_build_finished(
        &mut self,
        build_id: BuildID,
        dependent_ids: &[BuildID],
    ) -> sqlx::Result<()> {
        let mut q = vec![build_id.to_string()];
        q.extend(dependent_ids.iter().map(ToString::to_string));

        self.notify_any("build_finished", &q.join("\t")).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, drv_path, attempt,), err)]
    pub async fn notify_step_started(
        &mut self,
        store_dir: &StoreDir,
        drv_path: &StorePath,
        attempt: i32,
    ) -> sqlx::Result<()> {
        let drv_path = store_dir.display(drv_path).to_string();
        self.notify_any("step_started", &format!("{drv_path}\t{attempt}"))
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, drv_path, attempt, log_file,), err)]
    pub async fn notify_step_finished(
        &mut self,
        store_dir: &StoreDir,
        drv_path: &StorePath,
        attempt: i32,
        log_file: &str,
    ) -> sqlx::Result<()> {
        let drv_path = store_dir.display(drv_path).to_string();
        self.notify_any(
            "step_finished",
            &format!("{drv_path}\t{attempt}\t{log_file}"),
        )
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn notify_dump_status(&mut self) -> sqlx::Result<()> {
        self.notify_any("dump_status", "").await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn notify_status_dumped(&mut self) -> sqlx::Result<()> {
        self.notify_any("status_dumped", "").await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn test_store_dir() -> StoreDir {
        StoreDir::new("/nix/store").unwrap()
    }

    fn sp(s: &str) -> StorePath {
        format!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0-{s}")
            .parse()
            .unwrap()
    }

    fn on(s: &str) -> OutputName {
        s.parse().unwrap()
    }

    async fn setup() -> (test_utils::TestPg, Connection) {
        let (pg, pool) = test_utils::TestPg::new().await;
        let mut conn = Connection::new(pool.acquire().await.unwrap());
        sqlx::raw_sql("SET session_replication_role = 'replica';")
            .execute(&mut *conn.conn)
            .await
            .unwrap();
        (pg, conn)
    }

    async fn insert_step(conn: &mut Connection, build: i32, stepnr: i32, drv_path: &StorePath) {
        let sd = test_store_dir();
        sqlx::query(
            "WITH max_attempt AS (SELECT COALESCE(MAX(attempt), -1) + 1 AS val FROM buildsteps WHERE drvPath = $3)
             INSERT INTO BuildSteps (build, stepnr, type, busy, drvPath, attempt, status) VALUES ($1, $2, 0, 0, $3, (SELECT val FROM max_attempt), 0)")
            .bind(build)
            .bind(stepnr)
            .bind(sd.display(drv_path).to_string())
            .execute(&mut *conn.conn)
            .await
            .unwrap();
    }

    async fn insert_output(
        conn: &mut Connection,
        drv_path: &StorePath,
        attempt: i32,
        name: &str,
        path: &StorePath,
    ) {
        let sd = test_store_dir();
        sqlx::query(
            "INSERT INTO BuildStepOutputs (drvPath, attempt, name, path) VALUES ($1, $2, $3, $4)",
        )
        .bind(sd.display(drv_path).to_string())
        .bind(attempt)
        .bind(name)
        .bind(sd.display(path).to_string())
        .execute(&mut *conn.conn)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn resolve_depth_1() {
        let (_pg, mut conn) = setup().await;
        insert_step(&mut conn, 1, 1, &sp("foo.drv")).await;
        insert_output(&mut conn, &sp("foo.drv"), 0, "out", &sp("result")).await;

        let results = conn
            .resolve_drv_output_chains(&test_store_dir(), &[(&sp("foo.drv"), &[&on("out")])])
            .await
            .unwrap();
        assert_eq!(results, vec![Some(sp("result"))]);
    }

    #[tokio::test]
    async fn resolve_depth_2() {
        let (_pg, mut conn) = setup().await;
        insert_step(&mut conn, 1, 1, &sp("foo.drv")).await;
        insert_output(&mut conn, &sp("foo.drv"), 0, "out", &sp("bar.drv")).await;
        insert_step(&mut conn, 2, 1, &sp("bar.drv")).await;
        insert_output(&mut conn, &sp("bar.drv"), 0, "dev", &sp("final")).await;

        let results = conn
            .resolve_drv_output_chains(
                &test_store_dir(),
                &[(&sp("foo.drv"), &[&on("out"), &on("dev")])],
            )
            .await
            .unwrap();
        assert_eq!(results, vec![Some(sp("final"))]);
    }

    #[tokio::test]
    async fn resolve_batch() {
        let (_pg, mut conn) = setup().await;
        insert_step(&mut conn, 1, 1, &sp("foo.drv")).await;
        insert_output(&mut conn, &sp("foo.drv"), 0, "out", &sp("foo-out")).await;
        insert_step(&mut conn, 2, 1, &sp("bar.drv")).await;
        insert_output(&mut conn, &sp("bar.drv"), 0, "lib", &sp("bar-lib")).await;

        let results = conn
            .resolve_drv_output_chains(
                &test_store_dir(),
                &[
                    (&sp("foo.drv"), &[&on("out")]),
                    (&sp("bar.drv"), &[&on("lib")]),
                ],
            )
            .await
            .unwrap();
        assert_eq!(results, vec![Some(sp("foo-out")), Some(sp("bar-lib")),]);
    }

    #[tokio::test]
    async fn resolve_missing() {
        let (_pg, mut conn) = setup().await;
        insert_step(&mut conn, 1, 1, &sp("foo.drv")).await;
        insert_output(&mut conn, &sp("foo.drv"), 0, "out", &sp("result")).await;

        let results = conn
            .resolve_drv_output_chains(
                &test_store_dir(),
                &[
                    (&sp("foo.drv"), &[&on("out")]),
                    (&sp("nonexistent.drv"), &[&on("out")]),
                ],
            )
            .await
            .unwrap();
        assert_eq!(results, vec![Some(sp("result")), None]);
    }

    #[tokio::test]
    async fn resolve_empty() {
        let (_pg, mut conn) = setup().await;
        let results = conn
            .resolve_drv_output_chains(&test_store_dir(), &[])
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn resolve_picks_latest_attempt() {
        let (_pg, mut conn) = setup().await;
        insert_step(&mut conn, 1, 1, &sp("foo.drv")).await;
        insert_output(
            &mut conn,
            &sp("foo.drv"),
            0,
            "out",
            &sp("aldaldaldaldaldaldaldaldaldaldal-result"),
        )
        .await;
        insert_step(&mut conn, 5, 1, &sp("foo.drv")).await;
        insert_output(
            &mut conn,
            &sp("foo.drv"),
            1,
            "out",
            &sp("nawnawnawnawnawnawnawnawnawnawna-result"),
        )
        .await;

        let results = conn
            .resolve_drv_output_chains(&test_store_dir(), &[(&sp("foo.drv"), &[&on("out")])])
            .await
            .unwrap();
        assert_eq!(
            results,
            vec![Some(sp("nawnawnawnawnawnawnawnawnawnawna-result"))]
        );
    }

    /// Batch with ragged depths: one depth-1 (Opaque), one depth-2 (Built),
    /// one depth-3 (Built(Built(...))).
    #[tokio::test]
    async fn resolve_ragged_batch() {
        let (_pg, mut conn) = setup().await;

        // Depth 1: aaa.drv ^out => result-a
        insert_step(&mut conn, 1, 1, &sp("aaa.drv")).await;
        insert_output(&mut conn, &sp("aaa.drv"), 0, "out", &sp("result-a")).await;

        // Depth 2: bbb.drv ^out => ccc.drv, ccc.drv ^lib => result-b
        insert_step(&mut conn, 2, 1, &sp("bbb.drv")).await;
        insert_output(&mut conn, &sp("bbb.drv"), 0, "out", &sp("ccc.drv")).await;
        insert_step(&mut conn, 3, 1, &sp("ccc.drv")).await;
        insert_output(&mut conn, &sp("ccc.drv"), 0, "lib", &sp("result-b")).await;

        // Depth 3: ddd.drv ^out => eee.drv, eee.drv ^dev => fff.drv, fff.drv ^bin => result-c
        insert_step(&mut conn, 4, 1, &sp("ddd.drv")).await;
        insert_output(&mut conn, &sp("ddd.drv"), 0, "out", &sp("eee.drv")).await;
        insert_step(&mut conn, 5, 1, &sp("eee.drv")).await;
        insert_output(&mut conn, &sp("eee.drv"), 0, "dev", &sp("fff.drv")).await;
        insert_step(&mut conn, 6, 1, &sp("fff.drv")).await;
        insert_output(&mut conn, &sp("fff.drv"), 0, "bin", &sp("result-c")).await;

        let results = conn
            .resolve_drv_output_chains(
                &test_store_dir(),
                &[
                    (&sp("aaa.drv"), &[&on("out")]),
                    (&sp("bbb.drv"), &[&on("out"), &on("lib")]),
                    (&sp("ddd.drv"), &[&on("out"), &on("dev"), &on("bin")]),
                ],
            )
            .await
            .unwrap();
        assert_eq!(
            results,
            vec![
                Some(sp("result-a")),
                Some(sp("result-b")),
                Some(sp("result-c")),
            ]
        );
    }
}
