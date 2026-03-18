/// Every SQL statement used by the db crate lives here as a named constant.
/// The `prepare_all` integration test validates each one against the real schema.

// -- Connection (non-transactional) queries --

pub const GET_NOT_FINISHED_BUILDS_FAST: &str =
    "SELECT id, globalPriority FROM builds WHERE finished = 0";

pub const GET_NOT_FINISHED_BUILDS: &str = "\
    SELECT \
      builds.id, \
      builds.jobset_id, \
      jobsets.project as project, \
      jobsets.name as jobset, \
      job, \
      drvPath, \
      maxsilent, \
      timeout, \
      timestamp, \
      globalPriority, \
      priority \
    FROM builds \
    INNER JOIN jobsets ON builds.jobset_id = jobsets.id \
    WHERE finished = 0 ORDER BY globalPriority desc, schedulingshares, random()";

pub const GET_JOBSETS: &str = "SELECT project, name, schedulingshares FROM jobsets";

pub const GET_JOBSET_SCHEDULING_SHARES: &str =
    "SELECT schedulingshares FROM jobsets WHERE id = $1";

pub const GET_JOBSET_BUILD_STEPS: &str = "\
    SELECT s.startTime, s.stopTime FROM buildsteps s join builds b on build = id \
    WHERE \
      s.startTime IS NOT NULL AND \
      to_timestamp(s.stopTime) > (NOW() - (interval '1 second' * $1)) AND \
      jobset_id = $2";

pub const ABORT_BUILD: &str = "\
    UPDATE builds SET finished = 1, buildStatus = $2, startTime = $3, stopTime = $3 \
    where id = $1 and finished = 0";

pub const CHECK_IF_PATHS_FAILED: &str = "SELECT path FROM failedpaths where path = ANY($1)";

pub const CLEAR_BUSY: &str =
    "UPDATE buildsteps SET busy = 0, status = $1, stopTime = $2 WHERE busy != 0";

pub const UPDATE_BUILD_STEP: &str = "\
    UPDATE buildsteps SET busy = $1 \
    WHERE build = $2 AND stepnr = $3 AND busy != 0 AND status IS NULL";

pub const INSERT_DEBUG_BUILD: &str = "\
    INSERT INTO builds (\
      finished, timestamp, jobset_id, job, nixname, drvpath, system, \
      maxsilent, timeout, ischannel, iscurrent, priority, globalpriority, keep\
    ) VALUES (\
      0, EXTRACT(EPOCH FROM NOW())::INT4, $1, 'debug', 'debug', $2, $3, \
      7200, 36000, 0, 0, 100, 0, 0)";

pub const GET_BUILD_OUTPUT_FOR_PATH: &str = "\
    SELECT id, buildStatus, releaseName, closureSize, size \
    FROM builds b \
    JOIN buildoutputs o on b.id = o.build \
    WHERE finished = 1 and (buildStatus = 0 or buildStatus = 6) and path = $1";

pub const GET_BUILD_PRODUCTS_FOR_BUILD_ID: &str = "\
    SELECT type, subtype, fileSize, sha256hash, path, name, defaultPath \
    FROM buildproducts \
    WHERE build = $1 ORDER BY productnr";

pub const GET_BUILD_METRICS_FOR_BUILD_ID: &str =
    "SELECT name, unit, value FROM buildmetrics WHERE build = $1";

pub const GET_STATUS: &str = "SELECT status FROM systemstatus WHERE what = 'queue-runner'";

// -- Transaction queries --

pub const UPDATE_BUILD: &str = "\
    UPDATE builds SET \
      finished = 1, \
      buildStatus = $2, \
      startTime = $3, \
      stopTime = $4, \
      size = $5, \
      closureSize = $6, \
      releaseName = $7, \
      isCachedBuild = $8, \
      notificationPendingSince = $4 \
    WHERE \
      id = $1";

pub const UPDATE_BUILD_AFTER_FAILURE: &str = "\
    UPDATE builds SET \
      finished = 1, \
      buildStatus = $2, \
      startTime = $3, \
      stopTime = $4, \
      isCachedBuild = $5, \
      notificationPendingSince = $4 \
    WHERE \
      id = $1 AND finished = 0";

pub const UPDATE_BUILD_AFTER_PREVIOUS_FAILURE: &str = "\
    UPDATE builds SET \
      finished = 1, \
      buildStatus = $2, \
      startTime = $3, \
      stopTime = $3, \
      isCachedBuild = 1, \
      notificationPendingSince = $3 \
    WHERE \
      id = $1 AND finished = 0";

pub const UPDATE_BUILD_OUTPUT: &str =
    "UPDATE buildoutputs SET path = $3 WHERE build = $1 AND name = $2";

pub const GET_LAST_BUILD_STEP_ID: &str = "\
    SELECT MAX(build) FROM buildsteps \
    WHERE drvPath = $1 and startTime != 0 and stopTime != 0 and status = 1";

pub const GET_LAST_BUILD_STEP_ID_FOR_OUTPUT_PATH: &str = "\
    SELECT MAX(s.build) FROM buildsteps s \
    JOIN BuildStepOutputs o ON s.build = o.build \
    WHERE startTime != 0 \
      AND stopTime != 0 \
      AND status = 1 \
      AND path = $1";

pub const GET_LAST_BUILD_STEP_ID_FOR_OUTPUT_WITH_DRV: &str = "\
    SELECT MAX(s.build) FROM buildsteps s \
    JOIN BuildStepOutputs o ON s.build = o.build \
    WHERE startTime != 0 \
      AND stopTime != 0 \
      AND status = 1 \
      AND drvPath = $1 \
      AND name = $2";

pub const ALLOC_BUILD_STEP: &str = "SELECT MAX(stepnr) FROM buildsteps WHERE build = $1";

pub const INSERT_BUILD_STEP: &str = "\
    INSERT INTO buildsteps (\
      build, stepnr, type, drvPath, busy, startTime, stopTime, \
      system, status, propagatedFrom, errorMsg, machine\
    ) VALUES (\
      $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12\
    ) \
    ON CONFLICT DO NOTHING";

pub const UPDATE_BUILD_STEP_OUTPUT: &str =
    "UPDATE buildstepoutputs SET path = $4 WHERE build = $1 AND stepnr = $2 AND name = $3";

pub const UPDATE_BUILD_STEP_IN_FINISH: &str = "\
    UPDATE buildsteps SET \
      busy = 0, \
      status = $1, \
      errorMsg = $4, \
      startTime = $5, \
      stopTime = $6, \
      machine = $7, \
      overhead = $8, \
      timesBuilt = $9, \
      isNonDeterministic = $10 \
    WHERE \
      build = $2 AND stepnr = $3";

pub const GET_DRV_PATH_FROM_BUILD_STEP: &str =
    "SELECT drvPath FROM BuildSteps WHERE build = $1 AND stepnr = $2";

pub const CHECK_IF_BUILD_IS_NOT_FINISHED: &str =
    "SELECT id FROM builds WHERE id = $1 AND finished = 0";

pub const INSERT_BUILD_PRODUCT: &str = "\
    INSERT INTO buildproducts (\
      build, productnr, type, subtype, fileSize, sha256hash, path, name, defaultPath\
    ) VALUES (\
      $1, $2, $3, $4, $5, $6, $7, $8, $9\
    )";

pub const DELETE_BUILD_PRODUCTS_BY_BUILD_ID: &str = "DELETE FROM buildproducts WHERE build = $1";

pub const INSERT_BUILD_METRIC: &str = "\
    INSERT INTO buildmetrics (\
      build, name, unit, value, project, jobset, job, timestamp\
    ) VALUES (\
      $1, $2, $3, $4, $5, $6, $7, $8\
    )";

pub const DELETE_BUILD_METRICS_BY_BUILD_ID: &str = "DELETE FROM buildmetrics WHERE build = $1";

pub const INSERT_FAILED_PATHS: &str = "INSERT INTO failedpaths (path) VALUES ($1)";

pub const UPSERT_STATUS: &str = "\
    INSERT INTO systemstatus (what, status) VALUES ('queue-runner', $1) \
    ON CONFLICT (what) DO UPDATE SET status = EXCLUDED.status";

pub const PG_NOTIFY: &str =
    "SELECT pg_notify(chan, payload) from (values ($1, $2)) notifies(chan, payload)";

/// Returns every query constant for use in the prepare-all integration test.
/// `insert_build_step_outputs` builds SQL dynamically and is not included here
/// (it is exercised by the existing integration tests instead).
pub const ALL: &[&str] = &[
    GET_NOT_FINISHED_BUILDS_FAST,
    GET_NOT_FINISHED_BUILDS,
    GET_JOBSETS,
    GET_JOBSET_SCHEDULING_SHARES,
    GET_JOBSET_BUILD_STEPS,
    ABORT_BUILD,
    CHECK_IF_PATHS_FAILED,
    CLEAR_BUSY,
    UPDATE_BUILD_STEP,
    INSERT_DEBUG_BUILD,
    GET_BUILD_OUTPUT_FOR_PATH,
    GET_BUILD_PRODUCTS_FOR_BUILD_ID,
    GET_BUILD_METRICS_FOR_BUILD_ID,
    GET_STATUS,
    UPDATE_BUILD,
    UPDATE_BUILD_AFTER_FAILURE,
    UPDATE_BUILD_AFTER_PREVIOUS_FAILURE,
    UPDATE_BUILD_OUTPUT,
    GET_LAST_BUILD_STEP_ID,
    GET_LAST_BUILD_STEP_ID_FOR_OUTPUT_PATH,
    GET_LAST_BUILD_STEP_ID_FOR_OUTPUT_WITH_DRV,
    ALLOC_BUILD_STEP,
    INSERT_BUILD_STEP,
    UPDATE_BUILD_STEP_OUTPUT,
    UPDATE_BUILD_STEP_IN_FINISH,
    GET_DRV_PATH_FROM_BUILD_STEP,
    CHECK_IF_BUILD_IS_NOT_FINISHED,
    INSERT_BUILD_PRODUCT,
    DELETE_BUILD_PRODUCTS_BY_BUILD_ID,
    INSERT_BUILD_METRIC,
    DELETE_BUILD_METRICS_BY_BUILD_ID,
    INSERT_FAILED_PATHS,
    UPSERT_STATUS,
    PG_NOTIFY,
];
