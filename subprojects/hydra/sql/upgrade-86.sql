-- drvPath should always have been NOT NULL. If this fails, investigate the
-- NULL rows before proceeding.
ALTER TABLE BuildSteps ALTER COLUMN drvPath SET NOT NULL;
