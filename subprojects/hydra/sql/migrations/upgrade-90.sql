-- Normalize derivation-keyed data out of Builds/BuildSteps into new
-- Derivations and DerivationOutputs tables, and drop BuildOutputs,
-- which duplicated BuildStepOutputs for a build's top-level derivation.

create table Derivations (
    path          text primary key not null,
    -- TODO: make NOT NULL once the legacy rows are dealt with:
    -- historical substitution steps never recorded a system, so
    -- derivations only ever seen via substitution have NULL here.
    system        text
);

create table DerivationOutputs (
    drvPath       text not null references Derivations(path) on delete cascade,
    name          text not null,
    -- Only statically-known paths; floating content-addressed paths
    -- that are not known until a successful build do not go here; NULL
    -- is used instead to indicate the path is unknown.
    path          text,
    primary key   (drvPath, name)
);

-- Backfill Derivations from every drvPath we have ever seen. Prefer
-- any non-NULL system, with Builds as tie-breaker over steps; a NULL
-- survives only for derivations that were never seen outside
-- system-less rows.
INSERT INTO Derivations (path, system)
SELECT DISTINCT ON (drvPath) drvPath, system FROM (
    SELECT drvPath, system, 0 AS pref FROM Builds
    UNION ALL
    SELECT drvPath, system, 1 AS pref FROM BuildSteps
) sources
ORDER BY drvPath, (system IS NULL), pref;

-- Backfill DerivationOutputs as the union of the two old tables. The
-- database being migrated has no floating content-addressed paths, so
-- both sources statically know every output path and can only agree;
-- UNION dedupes the identical rows, and if they somehow disagree, the
-- primary key makes the migration fail so it gets investigated.
INSERT INTO DerivationOutputs (drvPath, name, path)
SELECT b.drvPath, o.name, o.path
FROM BuildOutputs o JOIN Builds b ON b.id = o.build
UNION
SELECT drvPath, name, path FROM BuildStepOutputs;

ALTER TABLE Builds
    ADD FOREIGN KEY (drvPath) REFERENCES Derivations(path),
    DROP COLUMN system;

ALTER TABLE BuildSteps
    ADD FOREIGN KEY (drvPath) REFERENCES Derivations(path),
    DROP COLUMN system;

ALTER TABLE BuildStepOutputs
    ADD FOREIGN KEY (drvPath) REFERENCES Derivations(path) on delete cascade,
    ADD FOREIGN KEY (drvPath, name) REFERENCES DerivationOutputs(drvPath, name) on delete cascade;

DROP TABLE BuildOutputs;

create index IndexDerivationOutputsPath on DerivationOutputs using hash(path);
