-- Create Derivations table as a normalized registry of derivation paths.
CREATE TABLE Derivations (
    path          text PRIMARY KEY NOT NULL
);

-- Populate from existing BuildSteps.
INSERT INTO Derivations (path)
    SELECT DISTINCT drvPath FROM BuildSteps
    ON CONFLICT DO NOTHING;

-- BuildSteps.drvPath now references Derivations.
ALTER TABLE BuildSteps
    ADD FOREIGN KEY (drvPath) REFERENCES Derivations(path);

-- Dependency edges between derivations. Edges are permanent — whether
-- a dep is satisfied is derived by joining to BuildSteps.
CREATE TABLE BuildStepDeps (
    drvPath       text NOT NULL REFERENCES Derivations(path) ON DELETE CASCADE,
    depDrvPath    text NOT NULL REFERENCES Derivations(path) ON DELETE CASCADE,
    PRIMARY KEY (drvPath, depDrvPath)
);

CREATE INDEX IndexBuildStepDepsByDep ON BuildStepDeps(depDrvPath);

-- Ready queue: steps whose unfinished deps are all satisfied.
CREATE TABLE BuildStepCanCreate (
    drvPath       text PRIMARY KEY NOT NULL REFERENCES Derivations(path) ON DELETE CASCADE,
    readyTime     integer NOT NULL
);

-- Archive table for legacy (build, stepnr) data.
-- Preserves historical mapping without cluttering BuildSteps.
CREATE TABLE BuildStepsHistorical (
    drvPath       text NOT NULL,
    attempt       integer NOT NULL,
    build         integer NOT NULL,
    stepnr        integer NOT NULL,
    CHECK         (stepnr > 0),
    PRIMARY KEY   (drvPath, attempt),
    UNIQUE        (build, stepnr),
    FOREIGN KEY   (drvPath, attempt) REFERENCES BuildSteps(drvPath, attempt) ON DELETE CASCADE,
    FOREIGN KEY   (build) REFERENCES Builds(id) ON DELETE CASCADE
);

-- Migrate existing (build, stepnr) data to the archive table.
INSERT INTO BuildStepsHistorical (drvPath, attempt, build, stepnr)
    SELECT drvPath, attempt, build, stepnr FROM BuildSteps;

-- Move propagatedFrom to its own nullable column (it references Builds,
-- which is build-level info, but we keep it on BuildSteps for now).

-- Drop legacy columns from BuildSteps.
ALTER TABLE BuildSteps DROP COLUMN build;
ALTER TABLE BuildSteps DROP COLUMN stepnr;
