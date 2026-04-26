-- Add attempt to BuildSteps so that (drvPath, attempt) is a unique key.
-- This allows identifying a build step by its derivation + attempt ordinal
-- rather than only by the build-specific stepnr.

-- In PG 11+, ADD COLUMN with a non-volatile DEFAULT is instant (no table
-- rewrite), so every existing row is logically 0 without touching the heap.
ALTER TABLE BuildSteps ADD COLUMN attempt integer NOT NULL DEFAULT 0;

-- Only update rows that are *not* the first attempt for their drvPath.
-- Most drvPaths appear exactly once, so this touches a small fraction of
-- the table — far cheaper than a full-table UPDATE.
UPDATE BuildSteps
SET attempt = sub.rn
FROM (
    SELECT build, stepnr,
           ROW_NUMBER() OVER (PARTITION BY drvPath ORDER BY build, stepnr) - 1 AS rn
    FROM BuildSteps
    WHERE drvPath IN (
        SELECT drvPath FROM BuildSteps
        GROUP BY drvPath HAVING COUNT(*) > 1
    )
) sub
WHERE BuildSteps.build = sub.build
  AND BuildSteps.stepnr = sub.stepnr
  AND sub.rn > 0;

ALTER TABLE BuildSteps ADD CONSTRAINT BuildSteps_drvPath_attempt_unique UNIQUE (drvPath, attempt);
ALTER TABLE BuildSteps ADD CONSTRAINT BuildSteps_stepnr_positive CHECK (stepnr > 0);
ALTER TABLE BuildSteps ADD CONSTRAINT BuildSteps_attempt_nonnegative CHECK (attempt >= 0);
