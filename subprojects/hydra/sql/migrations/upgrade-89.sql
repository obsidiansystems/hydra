-- Migrate BuildStepOutputs from `(build, stepnr)` to `(drvPath, attempt)`.

-- Add new columns.
ALTER TABLE BuildStepOutputs ADD COLUMN drvPath text;
ALTER TABLE BuildStepOutputs ADD COLUMN attempt integer;

-- Populate from the parent BuildSteps row.
UPDATE BuildStepOutputs o
SET drvPath = s.drvPath, attempt = s.attempt
FROM BuildSteps s
WHERE o.build = s.build AND o.stepnr = s.stepnr;

-- Now make them NOT NULL.
ALTER TABLE BuildStepOutputs ALTER COLUMN drvPath SET NOT NULL;
ALTER TABLE BuildStepOutputs ALTER COLUMN attempt SET NOT NULL;

-- Add new unique constraint first (validates data before we drop the old PK).
-- Technically, since this is all happening in a single transaction, it
-- shouldn't matter that we do this first, but it just feels right to do it
-- this way :)
ALTER TABLE BuildStepOutputs ADD CONSTRAINT buildstepoutputs_drvpath_attempt_name_key
    UNIQUE (drvPath, attempt, name);

-- Add new FK (likewise, validates first).
ALTER TABLE BuildStepOutputs ADD CONSTRAINT buildstepoutputs_drvpath_attempt_fkey
    FOREIGN KEY (drvPath, attempt) REFERENCES BuildSteps(drvPath, attempt) ON DELETE CASCADE;

-- Now that we have all our new indices, drop old constraints and columns.
-- Drop old PK.
ALTER TABLE BuildStepOutputs DROP CONSTRAINT buildstepoutputs_pkey;
-- Drop all old FKs on build/stepnr columns by finding them dynamically.
DO $$
DECLARE r RECORD;
BEGIN
    FOR r IN
        SELECT con.conname
        FROM pg_constraint con
        JOIN pg_attribute att ON att.attrelid = con.conrelid AND att.attnum = ANY(con.conkey)
        WHERE con.conrelid = 'buildstepoutputs'::regclass
          AND con.contype = 'f'
          AND att.attname IN ('build', 'stepnr')
        GROUP BY con.conname
    LOOP
        EXECUTE format('ALTER TABLE BuildStepOutputs DROP CONSTRAINT %I', r.conname);
    END LOOP;
END $$;
ALTER TABLE BuildStepOutputs DROP COLUMN build;
ALTER TABLE BuildStepOutputs DROP COLUMN stepnr;

-- Promote the unique constraint to PK.
ALTER TABLE BuildStepOutputs DROP CONSTRAINT buildstepoutputs_drvpath_attempt_name_key;
ALTER TABLE BuildStepOutputs ADD PRIMARY KEY (drvPath, attempt, name);
