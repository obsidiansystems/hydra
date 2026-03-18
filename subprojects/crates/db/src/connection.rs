use super::models::{
    Build, BuildSmall, BuildStatus, BuildSteps, InsertBuildMetric, InsertBuildProduct,
    InsertBuildStep, InsertBuildStepOutput, Jobset, UpdateBuild, UpdateBuildStep,
    UpdateBuildStepInFinish,
};
use super::queries;
use crate::Error;

#[derive(Debug)]
pub struct Connection {
    conn: deadpool_postgres::Object,
}

#[derive(Debug)]
pub struct Transaction<'a> {
    tx: deadpool_postgres::Transaction<'a>,
}

impl Connection {
    #[must_use]
    pub const fn new(conn: deadpool_postgres::Object) -> Self {
        Self { conn }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn begin_transaction(&mut self) -> Result<Transaction<'_>, Error> {
        let tx = self.conn.transaction().await?;
        Ok(Transaction { tx })
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_not_finished_builds_fast(&mut self) -> Result<Vec<BuildSmall>, Error> {
        let stmt = self
            .conn
            .prepare_cached(queries::GET_NOT_FINISHED_BUILDS_FAST)
            .await?;
        Ok(self
            .conn
            .query(&stmt, &[])
            .await?
            .iter()
            .map(|row| BuildSmall {
                id: row.get("id"),
                globalpriority: row.get("globalpriority"),
            })
            .collect())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_not_finished_builds(&mut self) -> Result<Vec<Build>, Error> {
        let stmt = self
            .conn
            .prepare_cached(queries::GET_NOT_FINISHED_BUILDS)
            .await?;
        Ok(self
            .conn
            .query(&stmt, &[])
            .await?
            .iter()
            .map(|row| Build {
                id: row.get("id"),
                jobset_id: row.get("jobset_id"),
                project: row.get("project"),
                jobset: row.get("jobset"),
                job: row.get("job"),
                drvpath: row.get("drvpath"),
                maxsilent: row.get("maxsilent"),
                timeout: row.get("timeout"),
                timestamp: i64::from(row.get::<_, i32>("timestamp")),
                globalpriority: row.get("globalpriority"),
                priority: row.get("priority"),
            })
            .collect())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_jobsets(&mut self) -> Result<Vec<Jobset>, Error> {
        let stmt = self
            .conn
            .prepare_cached(queries::GET_JOBSETS)
            .await?;
        Ok(self
            .conn
            .query(&stmt, &[])
            .await?
            .iter()
            .map(|row| Jobset {
                project: row.get("project"),
                name: row.get("name"),
                schedulingshares: row.get("schedulingshares"),
            })
            .collect())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_jobset_scheduling_shares(
        &mut self,
        jobset_id: i32,
    ) -> Result<Option<i32>, Error> {
        let stmt = self
            .conn
            .prepare_cached(queries::GET_JOBSET_SCHEDULING_SHARES)
            .await?;
        Ok(self
            .conn
            .query_opt(&stmt, &[&jobset_id])
            .await?
            .map(|row| row.get("schedulingshares")))
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_jobset_build_steps(
        &mut self,
        jobset_id: i32,
        scheduling_window: i64,
    ) -> Result<Vec<BuildSteps>, Error> {
        #[allow(clippy::cast_precision_loss)]
        let window = Some((scheduling_window * 10) as f64);
        let stmt = self
            .conn
            .prepare_cached(queries::GET_JOBSET_BUILD_STEPS)
            .await?;
        Ok(self
            .conn
            .query(&stmt, &[&window, &jobset_id])
            .await?
            .iter()
            .map(|row| BuildSteps {
                starttime: row.get("starttime"),
                stoptime: row.get("stoptime"),
            })
            .collect())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn abort_build(&mut self, build_id: i32) -> Result<(), Error> {
        #[allow(clippy::cast_possible_truncation)]
        let now = jiff::Timestamp::now().as_second() as i32;
        let status = BuildStatus::Aborted as i32;
        let stmt = self
            .conn
            .prepare_cached(queries::ABORT_BUILD)
            .await?;
        self.conn.execute(&stmt, &[&build_id, &status, &now]).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, paths), err)]
    pub async fn check_if_paths_failed(&mut self, paths: &[String]) -> Result<bool, Error> {
        let stmt = self
            .conn
            .prepare_cached(queries::CHECK_IF_PATHS_FAILED)
            .await?;
        Ok(!self
            .conn
            .query(&stmt, &[&paths])
            .await?
            .is_empty())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn clear_busy(&mut self, stop_time: i32) -> Result<(), Error> {
        let status = BuildStatus::Aborted as i32;
        let stop = Some(stop_time);
        let stmt = self
            .conn
            .prepare_cached(queries::CLEAR_BUSY)
            .await?;
        self.conn.execute(&stmt, &[&status, &stop]).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, step), err)]
    pub async fn update_build_step(&mut self, step: UpdateBuildStep) -> Result<(), Error> {
        let status = step.status as i32;
        let stmt = self
            .conn
            .prepare_cached(queries::UPDATE_BUILD_STEP)
            .await?;
        self.conn
            .execute(&stmt, &[&status, &step.build_id, &step.step_nr])
            .await?;
        Ok(())
    }

    pub async fn insert_debug_build(
        &mut self,
        jobset_id: i32,
        drv_path: &str,
        system: &str,
    ) -> Result<(), Error> {
        let stmt = self
            .conn
            .prepare_cached(queries::INSERT_DEBUG_BUILD)
            .await?;
        self.conn
            .execute(&stmt, &[&jobset_id, &drv_path, &system])
            .await?;
        Ok(())
    }

    pub async fn get_build_output_for_path(
        &mut self,
        out_path: &str,
    ) -> Result<Option<super::models::BuildOutput>, Error> {
        let stmt = self
            .conn
            .prepare_cached(queries::GET_BUILD_OUTPUT_FOR_PATH)
            .await?;
        Ok(self
            .conn
            .query_opt(&stmt, &[&out_path])
            .await?
            .map(|row| super::models::BuildOutput {
                id: row.get("id"),
                buildstatus: row.get("buildstatus"),
                releasename: row.get("releasename"),
                closuresize: row.get("closuresize"),
                size: row.get("size"),
            }))
    }

    pub async fn get_build_products_for_build_id(
        &mut self,
        build_id: i32,
    ) -> Result<Vec<crate::models::OwnedBuildProduct>, Error> {
        let stmt = self
            .conn
            .prepare_cached(queries::GET_BUILD_PRODUCTS_FOR_BUILD_ID)
            .await?;
        Ok(self
            .conn
            .query(&stmt, &[&build_id])
            .await?
            .iter()
            .map(|row| crate::models::OwnedBuildProduct {
                r#type: row.get("type"),
                subtype: row.get("subtype"),
                filesize: row.get("filesize"),
                sha256hash: row.get("sha256hash"),
                path: row.get("path"),
                name: row.get("name"),
                defaultpath: row.get("defaultpath"),
            })
            .collect())
    }

    pub async fn get_build_metrics_for_build_id(
        &mut self,
        build_id: i32,
    ) -> Result<Vec<crate::models::OwnedBuildMetric>, Error> {
        let stmt = self
            .conn
            .prepare_cached(queries::GET_BUILD_METRICS_FOR_BUILD_ID)
            .await?;
        Ok(self
            .conn
            .query(&stmt, &[&build_id])
            .await?
            .iter()
            .map(|row| crate::models::OwnedBuildMetric {
                name: row.get("name"),
                unit: row.get("unit"),
                value: row.get("value"),
            })
            .collect())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_status(&mut self) -> Result<Option<serde_json::Value>, Error> {
        let stmt = self
            .conn
            .prepare_cached(queries::GET_STATUS)
            .await?;
        Ok(self
            .conn
            .query_opt(&stmt, &[])
            .await?
            .map(|row| row.get("status")))
    }
}

impl Transaction<'_> {
    #[tracing::instrument(skip(self), err)]
    pub async fn commit(self) -> Result<(), Error> {
        self.tx.commit().await.map_err(Error::from)
    }

    #[tracing::instrument(skip(self, v), err)]
    pub async fn update_build(&mut self, build_id: i32, v: UpdateBuild<'_>) -> Result<(), Error> {
        let status = v.status as i32;
        let is_cached = i32::from(v.is_cached_build);
        let stmt = self
            .tx
            .prepare_cached(queries::UPDATE_BUILD)
            .await?;
        self.tx
            .execute(
                &stmt,
                &[
                    &build_id,
                    &status,
                    &v.start_time,
                    &v.stop_time,
                    &v.size,
                    &v.closure_size,
                    &v.release_name,
                    &is_cached,
                ],
            )
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, status, start_time, stop_time, is_cached_build), err)]
    pub async fn update_build_after_failure(
        &mut self,
        build_id: i32,
        status: BuildStatus,
        start_time: i32,
        stop_time: i32,
        is_cached_build: bool,
    ) -> Result<(), Error> {
        let status_i32 = status as i32;
        let is_cached = i32::from(is_cached_build);
        let stmt = self
            .tx
            .prepare_cached(queries::UPDATE_BUILD_AFTER_FAILURE)
            .await?;
        self.tx
            .execute(
                &stmt,
                &[&build_id, &status_i32, &start_time, &stop_time, &is_cached],
            )
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, status), err)]
    pub async fn update_build_after_previous_failure(
        &mut self,
        build_id: i32,
        status: BuildStatus,
    ) -> Result<(), Error> {
        let status_i32 = status as i32;
        #[allow(clippy::cast_possible_truncation)]
        let now = jiff::Timestamp::now().as_second() as i32;
        let stmt = self
            .tx
            .prepare_cached(queries::UPDATE_BUILD_AFTER_PREVIOUS_FAILURE)
            .await?;
        self.tx
            .execute(&stmt, &[&build_id, &status_i32, &now])
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, name, path), err)]
    pub async fn update_build_output(
        &mut self,
        build_id: i32,
        name: &str,
        path: &str,
    ) -> Result<(), Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::UPDATE_BUILD_OUTPUT)
            .await?;
        self.tx
            .execute(&stmt, &[&build_id, &name, &path])
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_last_build_step_id(&mut self, path: &str) -> Result<Option<i32>, Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::GET_LAST_BUILD_STEP_ID)
            .await?;
        Ok(self
            .tx
            .query_opt(&stmt, &[&path])
            .await?
            .and_then(|row| row.get(0)))
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_last_build_step_id_for_output_path(
        &mut self,
        path: &str,
    ) -> Result<Option<i32>, Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::GET_LAST_BUILD_STEP_ID_FOR_OUTPUT_PATH)
            .await?;
        Ok(self
            .tx
            .query_opt(&stmt, &[&path])
            .await?
            .and_then(|row| row.get(0)))
    }

    #[tracing::instrument(skip(self, drv_path, name), err)]
    pub async fn get_last_build_step_id_for_output_with_drv(
        &mut self,
        drv_path: &str,
        name: &str,
    ) -> Result<Option<i32>, Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::GET_LAST_BUILD_STEP_ID_FOR_OUTPUT_WITH_DRV)
            .await?;
        Ok(self
            .tx
            .query_opt(&stmt, &[&drv_path, &name])
            .await?
            .and_then(|row| row.get(0)))
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn alloc_build_step(&mut self, build_id: i32) -> Result<i32, Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::ALLOC_BUILD_STEP)
            .await?;
        Ok(self
            .tx
            .query_opt(&stmt, &[&build_id])
            .await?
            .and_then(|row| row.get::<_, Option<i32>>(0))
            .map_or(1, |v| v + 1))
    }

    #[tracing::instrument(skip(self, step), err)]
    pub async fn insert_build_step(&mut self, step: InsertBuildStep<'_>) -> Result<bool, Error> {
        let type_i32 = step.r#type as i32;
        let busy_i32 = i32::from(step.busy);
        let status_opt = if step.status == BuildStatus::Busy {
            None
        } else {
            Some(step.status as i32)
        };
        let stmt = self
            .tx
            .prepare_cached(queries::INSERT_BUILD_STEP)
            .await?;
        let rows = self
            .tx
            .execute(
                &stmt,
                &[
                    &step.build_id,
                    &step.step_nr,
                    &type_i32,
                    &step.drv_path,
                    &busy_i32,
                    &step.start_time,
                    &step.stop_time,
                    &step.platform,
                    &status_opt,
                    &step.propagated_from,
                    &step.error_msg,
                    &step.machine,
                ],
            )
            .await?;
        Ok(rows != 0)
    }

    #[tracing::instrument(skip(self, outputs), err)]
    pub async fn insert_build_step_outputs(
        &mut self,
        outputs: &[InsertBuildStepOutput],
    ) -> Result<(), Error> {
        if outputs.is_empty() {
            return Ok(());
        }

        let mut sql =
            String::from("INSERT INTO buildstepoutputs (build, stepnr, name, path) VALUES ");
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
        for (i, output) in outputs.iter().enumerate() {
            let base = i * 4 + 1;
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push_str(&format!(
                "(${}, ${}, ${}, ${})",
                base,
                base + 1,
                base + 2,
                base + 3
            ));
            params.push(&output.build_id);
            params.push(&output.step_nr);
            params.push(&output.name);
            params.push(&output.path);
        }
        self.tx.execute(sql.as_str(), &params).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, name, path), err)]
    pub async fn update_build_step_output(
        &mut self,
        build_id: i32,
        step_nr: i32,
        name: &str,
        path: &str,
    ) -> Result<(), Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::UPDATE_BUILD_STEP_OUTPUT)
            .await?;
        self.tx
            .execute(&stmt, &[&build_id, &step_nr, &name, &path])
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, res), err)]
    pub async fn update_build_step_in_finish(
        &mut self,
        res: UpdateBuildStepInFinish<'_>,
    ) -> Result<(), Error> {
        let status = res.status as i32;
        let stmt = self
            .tx
            .prepare_cached(queries::UPDATE_BUILD_STEP_IN_FINISH)
            .await?;
        self.tx
            .execute(
                &stmt,
                &[
                    &status,
                    &res.build_id,
                    &res.step_nr,
                    &res.error_msg,
                    &res.start_time,
                    &res.stop_time,
                    &res.machine,
                    &res.overhead,
                    &res.times_built,
                    &res.is_non_deterministic,
                ],
            )
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id, step_nr), err)]
    pub async fn get_drv_path_from_build_step(
        &mut self,
        build_id: i32,
        step_nr: i32,
    ) -> Result<Option<String>, Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::GET_DRV_PATH_FROM_BUILD_STEP)
            .await?;
        Ok(self
            .tx
            .query_opt(&stmt, &[&build_id, &step_nr])
            .await?
            .and_then(|row| row.get("drvpath")))
    }

    #[tracing::instrument(skip(self, build_id), err)]
    pub async fn check_if_build_is_not_finished(&mut self, build_id: i32) -> Result<bool, Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::CHECK_IF_BUILD_IS_NOT_FINISHED)
            .await?;
        Ok(self
            .tx
            .query_opt(&stmt, &[&build_id])
            .await?
            .is_some())
    }

    #[tracing::instrument(skip(self, p), err)]
    pub async fn insert_build_product(&mut self, p: InsertBuildProduct<'_>) -> Result<(), Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::INSERT_BUILD_PRODUCT)
            .await?;
        self.tx
            .execute(
                &stmt,
                &[
                    &p.build_id,
                    &p.product_nr,
                    &p.r#type,
                    &p.subtype,
                    &p.file_size,
                    &p.sha256hash,
                    &p.path,
                    &p.name,
                    &p.default_path,
                ],
            )
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id), err)]
    pub async fn delete_build_products_by_build_id(&mut self, build_id: i32) -> Result<(), Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::DELETE_BUILD_PRODUCTS_BY_BUILD_ID)
            .await?;
        self.tx.execute(&stmt, &[&build_id]).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, metric), err)]
    pub async fn insert_build_metric(&mut self, metric: InsertBuildMetric<'_>) -> Result<(), Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::INSERT_BUILD_METRIC)
            .await?;
        self.tx
            .execute(
                &stmt,
                &[
                    &metric.build_id,
                    &metric.name,
                    &metric.unit,
                    &metric.value,
                    &metric.project,
                    &metric.jobset,
                    &metric.job,
                    &metric.timestamp,
                ],
            )
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id), err)]
    pub async fn delete_build_metrics_by_build_id(&mut self, build_id: i32) -> Result<(), Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::DELETE_BUILD_METRICS_BY_BUILD_ID)
            .await?;
        self.tx.execute(&stmt, &[&build_id]).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, path), err)]
    pub async fn insert_failed_paths(&mut self, path: &str) -> Result<(), Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::INSERT_FAILED_PATHS)
            .await?;
        self.tx.execute(&stmt, &[&path]).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        skip(
            self,
            start_time,
            build_id,
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
        start_time: Option<i32>,
        build_id: crate::models::BuildID,
        drv_path: &str,
        platform: Option<&str>,
        machine: String,
        status: BuildStatus,
        error_msg: Option<String>,
        propagated_from: Option<crate::models::BuildID>,
        outputs: Vec<(String, Option<String>)>,
    ) -> Result<i32, Error> {
        let step_nr = loop {
            let step_nr = self.alloc_build_step(build_id).await?;
            if self
                .insert_build_step(InsertBuildStep {
                    build_id,
                    step_nr,
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
                })
                .await?
            {
                break step_nr;
            }
        };

        self.insert_build_step_outputs(
            &outputs
                .into_iter()
                .map(|(name, path)| InsertBuildStepOutput {
                    build_id,
                    step_nr,
                    name,
                    path,
                })
                .collect::<Vec<_>>(),
        )
        .await?;

        if status == BuildStatus::Busy {
            self.notify_step_started(build_id, step_nr).await?;
        }

        Ok(step_nr)
    }

    #[tracing::instrument(
        skip(self, start_time, stop_time, build_id, drv_path, output,),
        err,
        ret
    )]
    pub async fn create_substitution_step(
        &mut self,
        start_time: i32,
        stop_time: i32,
        build_id: crate::models::BuildID,
        drv_path: &str,
        output: (String, Option<String>),
    ) -> anyhow::Result<i32> {
        let step_nr = loop {
            let step_nr = self.alloc_build_step(build_id).await?;
            if self
                .insert_build_step(InsertBuildStep {
                    build_id,
                    step_nr,
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
                })
                .await?
            {
                break step_nr;
            }
        };

        self.insert_build_step_outputs(&[InsertBuildStepOutput {
            build_id,
            step_nr,
            name: output.0,
            path: output.1,
        }])
        .await?;

        Ok(step_nr)
    }

    #[tracing::instrument(skip(self, build, is_cached_build, start_time, stop_time,), err)]
    pub async fn mark_succeeded_build(
        &mut self,
        build: crate::models::MarkBuildSuccessData<'_>,
        is_cached_build: bool,
        start_time: i32,
        stop_time: i32,
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
            self.update_build_output(build.id, name, path).await?;
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
    pub async fn upsert_status(&mut self, status: &serde_json::Value) -> Result<(), Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::UPSERT_STATUS)
            .await?;
        let status_opt = Some(status);
        self.tx.execute(&stmt, &[&status_opt]).await?;
        Ok(())
    }
}

impl Transaction<'_> {
    #[tracing::instrument(skip(self), err)]
    async fn notify_any(&mut self, channel: &str, msg: &str) -> Result<(), Error> {
        let stmt = self
            .tx
            .prepare_cached(queries::PG_NOTIFY)
            .await?;
        self.tx.execute(&stmt, &[&channel, &msg]).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn notify_builds_added(&mut self) -> Result<(), Error> {
        self.notify_any("builds_added", "?").await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id), err)]
    pub async fn notify_build_started(&mut self, build_id: i32) -> Result<(), Error> {
        self.notify_any("build_started", &build_id.to_string())
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id, dependent_ids,), err)]
    pub async fn notify_build_finished(
        &mut self,
        build_id: i32,
        dependent_ids: &[i32],
    ) -> Result<(), Error> {
        let mut q = vec![build_id.to_string()];
        q.extend(dependent_ids.iter().map(ToString::to_string));

        self.notify_any("build_finished", &q.join("\t")).await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id, step_nr,), err)]
    pub async fn notify_step_started(&mut self, build_id: i32, step_nr: i32) -> Result<(), Error> {
        self.notify_any("step_started", &format!("{build_id}\t{step_nr}"))
            .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, build_id, step_nr, log_file,), err)]
    pub async fn notify_step_finished(
        &mut self,
        build_id: i32,
        step_nr: i32,
        log_file: &str,
    ) -> Result<(), Error> {
        self.notify_any(
            "step_finished",
            &format!("{build_id}\t{step_nr}\t{log_file}"),
        )
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn notify_dump_status(&mut self) -> Result<(), Error> {
        self.notify_any("dump_status", "").await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn notify_status_dumped(&mut self) -> Result<(), Error> {
        self.notify_any("status_dumped", "").await?;
        Ok(())
    }
}
