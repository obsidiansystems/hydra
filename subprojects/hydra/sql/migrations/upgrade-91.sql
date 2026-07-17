-- Replace Builds' copied outcome columns (finished, buildStatus's
-- step-expressible codes, startTime, stopTime, isCachedBuild, size,
-- closureSize, releaseName) with `fulfilledBy(DrvPath, Attempt)`: a
-- reference to the build step attempt whose completion finished the
-- build. buildStatus shrinks to a residual for outcomes no step can
-- express (see hydra.sql for the codes and invariants).

-- The old triggers reference columns this migration drops (and firing
-- the row-level NrBuilds counter on the backfill updates would be
-- pointless work); they are recreated at the end.
DROP TRIGGER BuildRestarted ON Builds;
DROP TRIGGER BuildCancelled ON Builds;
DROP TRIGGER NrBuildsFinished ON Builds;

ALTER TABLE BuildSteps
    ADD COLUMN size bigint,
    ADD COLUMN closureSize bigint,
    ADD COLUMN releaseName text;

ALTER TABLE Builds
    ADD COLUMN fulfilledByDrvPath text,
    ADD COLUMN fulfilledByAttempt integer;

-- Backfill pass 1: the step recorded under the build itself (via the
-- legacy BuildSteps.build column). Prefer a step for the build's own
-- derivation; otherwise the last recorded step, which for dep-failed
-- builds is the failing dependency step (the reason the build stopped).
UPDATE Builds b
SET fulfilledByDrvPath = s.drvPath, fulfilledByAttempt = s.attempt
FROM (
    SELECT DISTINCT ON (s.build) s.build AS bid, s.drvPath, s.attempt
    FROM BuildSteps s
    JOIN Builds bb ON bb.id = s.build
    WHERE bb.finished = 1
      AND s.busy = 0 AND s.status IS NOT NULL AND s.status <> 13
      -- A step that stopped after the build was marked finished cannot
      -- be what finished it. NULL-time steps (cached-failure records)
      -- are legitimate candidates.
      AND (s.stopTime IS NULL OR s.stopTime <= bb.stopTime)
    ORDER BY s.build,
             (s.drvPath <> bb.drvPath),
             (s.stopTime IS DISTINCT FROM bb.stopTime),
             s.stepnr DESC
) s
WHERE b.id = s.bid;

-- Cancelled builds are never fulfilled: a running attempt may still
-- have finished on behalf of other builds, but it did not finish this
-- one.
UPDATE Builds SET fulfilledByDrvPath = NULL, fulfilledByAttempt = NULL
WHERE finished = 1 AND buildStatus = 4;

-- Backfill pass 2: builds with no own step (cached builds): the latest
-- attempt of the same derivation whose outcome agrees with the build's.
UPDATE Builds b
SET fulfilledByDrvPath = s.drvPath, fulfilledByAttempt = s.attempt
FROM (
    SELECT DISTINCT ON (bb.id) bb.id AS bid, s.drvPath, s.attempt
    FROM Builds bb
    JOIN BuildSteps s ON s.drvPath = bb.drvPath
    WHERE bb.finished = 1 AND bb.fulfilledByDrvPath IS NULL
      AND bb.buildStatus <> 4
      AND s.busy = 0 AND s.status IS NOT NULL AND s.status <> 13
      AND ((s.status = 0) = (bb.buildStatus IN (0, 6)))
      -- The attempt that finished this build cannot have stopped after
      -- the build was marked finished.
      AND s.stopTime <= bb.stopTime
    ORDER BY bb.id, s.attempt DESC
) s
WHERE b.id = s.bid;

-- Backfill pass 3: synthesize a placeholder attempt for finished builds
-- with no surviving step at all (steps predating the BuildSteps table,
-- or since deleted). The build's times and status move onto the new
-- step row, so no history is lost. Dep-failed orphans degrade to a
-- plain failure of their own derivation: which dependency failed was
-- never recorded for them.
WITH orphans AS (
    SELECT id, drvPath, startTime, stopTime, buildStatus
    FROM Builds
    WHERE finished = 1 AND fulfilledByDrvPath IS NULL
      AND buildStatus NOT IN (3, 4, 9)
),
numbered AS (
    SELECT o.*,
           (SELECT COALESCE(MAX(attempt), -1) FROM BuildSteps s WHERE s.drvPath = o.drvPath)
           + ROW_NUMBER() OVER (PARTITION BY o.drvPath ORDER BY o.id) AS new_attempt
    FROM orphans o
),
inserted AS (
    INSERT INTO BuildSteps (build, stepnr, type, drvPath, attempt, busy, status, startTime, stopTime, machine)
    SELECT n.id,
           (SELECT COALESCE(MAX(stepnr), 0) FROM BuildSteps s WHERE s.build = n.id) + 1,
           0, n.drvPath, n.new_attempt, 0,
           CASE n.buildStatus WHEN 6 THEN 0 WHEN 2 THEN 1 ELSE n.buildStatus END,
           n.startTime, n.stopTime, ''
    FROM numbered n
    RETURNING build, drvPath, attempt
)
UPDATE Builds b
SET fulfilledByDrvPath = i.drvPath, fulfilledByAttempt = i.attempt
FROM inserted i
WHERE b.id = i.build;

-- Re-encode buildStatus as the residual: everything a fulfilling step
-- can express becomes NULL.
UPDATE Builds SET buildStatus = CASE
    WHEN finished = 0 THEN NULL
    WHEN buildStatus = 6 THEN 0                                    -- failure with output
    WHEN buildStatus = 4 THEN 1                                    -- cancelled
    WHEN buildStatus = 3 AND fulfilledByDrvPath IS NULL THEN 2     -- aborted, no attempt
    WHEN buildStatus = 9 AND fulfilledByDrvPath IS NULL THEN 3     -- unsupported
    ELSE NULL
END;

-- Builds.stopTime survives with its meaning intact ("when this build
-- reached its terminal state" — for cached builds the marking time,
-- while the fulfilling step records when the work ran). But it becomes
-- the finished predicate (NULL iff unfinished), so an unfinished build
-- carrying a stale stopTime (restarts historically didn't clear it)
-- would be misread as finished. Hydra is down during the migration and
-- this population is static: refuse to proceed so the rows can be
-- investigated and resolved deliberately (the stale values describe
-- superseded attempts, whose own step rows retain their times; if that
-- is all they are, clear them manually and re-run).
DO $$
DECLARE n bigint;
BEGIN
    SELECT COUNT(*) INTO n FROM Builds
    WHERE finished = 0 AND stopTime IS NOT NULL;
    IF n > 0 THEN
        RAISE EXCEPTION 'upgrade-91: % unfinished (restarted) builds carry a stale stopTime; investigate and clear before migrating', n;
    END IF;
END $$;

-- Every previously-finished build must still read as finished, and the
-- fulfilling step's timestamps must be coherent with the build's: an
-- attempt that stopped *after* the build was marked finished cannot be
-- the attempt that finished it. Inequality the other way is expected
-- (cached builds are marked later than the attempt that fulfils them),
-- so it is only reported.
DO $$
DECLARE n bigint;
DECLARE m bigint;
BEGIN
    SELECT COUNT(*) INTO n FROM Builds
    WHERE finished = 1 AND fulfilledByDrvPath IS NULL AND buildStatus IS NULL;
    IF n > 0 THEN
        RAISE EXCEPTION 'upgrade-91: % finished builds have neither a fulfilling step nor a residual status', n;
    END IF;

    SELECT COUNT(*) INTO n
    FROM Builds b
    JOIN BuildSteps fs ON fs.drvPath = b.fulfilledByDrvPath
                      AND fs.attempt = b.fulfilledByAttempt
    WHERE fs.stopTime > b.stopTime;
    IF n > 0 THEN
        RAISE EXCEPTION 'upgrade-91: % builds are fulfilled by a step that stopped after the build did', n;
    END IF;

    SELECT COUNT(*) INTO m
    FROM Builds b
    JOIN BuildSteps fs ON fs.drvPath = b.fulfilledByDrvPath
                      AND fs.attempt = b.fulfilledByAttempt
    WHERE fs.stopTime IS DISTINCT FROM b.stopTime;
    RAISE NOTICE 'upgrade-91: % fulfilled builds have a stopTime differing from their fulfilling step (expected for cached builds)', m;
END $$;

-- Move the per-outcome data onto the fulfilling steps, preferring the
-- values recorded by a non-cached (original) build.
UPDATE BuildSteps s
SET size = src.size, closureSize = src.closureSize, releaseName = src.releaseName
FROM (
    SELECT DISTINCT ON (fulfilledByDrvPath, fulfilledByAttempt)
        fulfilledByDrvPath AS d, fulfilledByAttempt AS a,
        size, closureSize, releaseName
    FROM Builds
    WHERE fulfilledByDrvPath IS NOT NULL
      AND (size IS NOT NULL OR closureSize IS NOT NULL OR releaseName IS NOT NULL)
    ORDER BY fulfilledByDrvPath, fulfilledByAttempt, (isCachedBuild = 1), id
) src
WHERE s.drvPath = src.d AND s.attempt = src.a;

-- CASCADE drops the old check constraints and partial indexes that
-- referenced these columns.
ALTER TABLE Builds
    ADD FOREIGN KEY (fulfilledByDrvPath, fulfilledByAttempt)
        REFERENCES BuildSteps(drvPath, attempt),
    ADD CONSTRAINT builds_fulfilledby_paired_check
        CHECK ((fulfilledByDrvPath IS NULL) = (fulfilledByAttempt IS NULL)),
    ADD CONSTRAINT builds_residual_status_check
        CHECK (buildStatus IS NULL OR fulfilledByDrvPath IS NULL OR buildStatus = 0),
    ADD CONSTRAINT builds_stoptime_check
        CHECK ((stopTime IS NULL) = (fulfilledByDrvPath IS NULL AND buildStatus IS NULL));

ALTER TABLE Builds
    DROP COLUMN finished CASCADE,
    DROP COLUMN startTime CASCADE,
    DROP COLUMN isCachedBuild,
    DROP COLUMN size,
    DROP COLUMN closureSize,
    DROP COLUMN releaseName;

DROP INDEX IF EXISTS IndexFinishedSuccessfulBuilds;
DROP INDEX IF EXISTS IndexBuildsJobsetIdCurrentFinishedStatus;

create index IndexBuildsUnfinished on Builds(id) where stopTime is null;
create index IndexBuildsJobsetIdCurrentUnfinished on Builds(jobset_id) where isCurrent = 1 and stopTime is null;
create index IndexBuildsOnJobsetIdJobId on Builds(jobset_id, job, id DESC);
create index IndexBuildsOnFulfilledBy on Builds(fulfilledByDrvPath, fulfilledByAttempt) where fulfilledByDrvPath is not null;

-- Recreate the triggers against the new finished predicate.
create or replace function modifyNrBuildsFinished() returns trigger as $$
  declare
    old_finished boolean := tg_op <> 'INSERT' and old.stopTime is not null;
    new_finished boolean := tg_op <> 'DELETE' and new.stopTime is not null;
  begin
    if (new_finished and not old_finished) then
      update NrBuilds set count = count + 1 where what = 'finished';
    elsif (old_finished and not new_finished) then
      update NrBuilds set count = count - 1 where what = 'finished';
    end if;
    return null;
  end;
$$ language plpgsql;

create trigger NrBuildsFinished after insert or update or delete on Builds
  for each row
  execute procedure modifyNrBuildsFinished();

create trigger BuildRestarted after update on Builds for each row
  when (old.stopTime is not null and new.stopTime is null)
  execute procedure notifyBuildRestarted();

create trigger BuildCancelled after update on Builds for each row
  when (old.stopTime is null and new.buildStatus = 1)
  execute procedure notifyBuildCancelled();

update NrBuilds set count =
    (select count(*) from Builds
     where stopTime is not null)
    where what = 'finished';
