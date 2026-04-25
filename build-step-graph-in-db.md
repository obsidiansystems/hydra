# Moving the BuildStep Dependency Graph to the Database

> **Note** please put one sentence per line / semantic line breaks.

## Background

In the previous commits, we have been switching build steps to be identified by `(build, step)` to instead `(drv path, attempt)`.
Right now we have both unique constraints.
We would like to finish the job, and delete the build and step columns (actually archive them, but for forward-looking purposes pretend they are gone).
However, we do still need to have some idea about which nodes are needed by what build.

This intersects with the motivation below ---
if we have the full dependency graph in the database, we don't need to store this sloppy "name me one build that needs this" information in build steps,
but if we ever want to figure that out, we can just crawl the edges ourselves.

## Motivation

The queue runner currently maintains the step-to-step dependency graph entirely in memory
via `StepState.deps: HashSet<Arc<Step>>` and `StepState.rdeps: Vec<Weak<Step>>`.
Steps are keyed by derivation path (one `Step` per `drvPath`).
Dependencies come from Nix derivation inputs and are rebuilt from scratch on each startup.

This has several problems:

- Steps are kept alive via `Weak<Step>` references, which can die unexpectedly when no `Arc` holds them
  (e.g. dependency steps that aren't a build's top-level get dropped)
- The graph is lost on restart
- The graph isn't queryable from the web UI
- The web UI shows queued builds but not queued steps or their dependency structure

## Plan 1: Move deps to DB, keep `Step` in memory

The initial approach was to keep the `Step`/`Steps` structs but move just the dependency edges to the database.

Actually, John never explicitly said to keep `Step` in memory, but he didn't say not to either.
He said to put the graph in the DB, and forgot about the other in-memory node information.

### Schema

A `Derivations` table as a normalized registry of derivation paths,
referenced by both `BuildSteps` and a new `BuildStepDeps` table:

```sql
CREATE TABLE Derivations (
    path text PRIMARY KEY NOT NULL
);

CREATE TABLE BuildStepDeps (
    drvPath    text NOT NULL REFERENCES Derivations(path) ON DELETE CASCADE,
    depDrvPath text NOT NULL REFERENCES Derivations(path) ON DELETE CASCADE,
    PRIMARY KEY (drvPath, depDrvPath)
);

CREATE INDEX IndexBuildStepDepsByDep ON BuildStepDeps(depDrvPath);

-- BuildSteps.drvPath gets a FK to Derivations
ALTER TABLE BuildSteps ADD FOREIGN KEY (drvPath) REFERENCES Derivations(path);
```

### Changes to `Step`

- Remove `deps` and `rdeps` fields from `StepState`
- `add_dep` becomes a DB INSERT into `BuildStepDeps`
- `make_rdeps_runnable` becomes a DB DELETE + check
- `get_dependents` uses a recursive CTE
- `get_all_deps_not_queued` queries `BuildStepDeps`
- Keep atomic counters `deps_len`/`rdeps_len` as caches, updated on write operations

### Problem discovered

Without `deps: HashSet<Arc<Step>>` holding strong references,
dependency steps get dropped when temporary `Arc`s go out of scope.
The `Weak<Step>` in the `Steps` HashMap can't be upgraded,
and the step vanishes from dispatch.

## Plan 2: Eliminate in-memory `Step`/`Steps` entirely

Rather than fixing the lifetime issue by switching to `Arc` (more in-memory state),
we realized that almost all `Step` state is either already in the DB,
derivable from DB queries, or can be a new DB column/table.
The `Step` struct can be eliminated entirely.

### Key insight: what does `Step` actually hold?

Going through each field:

- `drv_path` -- identity, stored in `Derivations.path`
- `drv` (parsed derivation) -- cached from Nix store, can be re-parsed on demand (local file read)
- `runnable` / `finished` / `previous_failure` -- all derivable from existing DB state (`BuildStepDeps`, `BuildSteps`)
- `tries` -- derivable from count of `BuildSteps` rows
- `highest_global_priority` / `highest_local_priority` / `lowest_build_id` -- derivable from `Builds` joined through transitive deps via recursive CTE on `BuildStepDeps`
- `after` (retry time) -- derivable from latest failed `BuildSteps` `stopTime` + backoff
- `runnable_since` -- needs storage (see `BuildStepCanCreate` below)
- `last_supported` -- per-system property, tracked in `Machines` state
- `deps_len` / `rdeps_len` -- `COUNT(*)` on `BuildStepDeps`
- `StepState.builds` -- `SELECT FROM Builds WHERE drvPath = $1`
- `StepState.jobsets` -- derivable from `Builds` join `Jobsets`

None of this needs to live in memory long-term.

### New table: `BuildStepCanCreate`

Instead of complex runnability queries, we maintain a "ready queue" table:

```sql
CREATE TABLE BuildStepCanCreate (
    drvPath   text PRIMARY KEY NOT NULL
                   REFERENCES Derivations(path) ON DELETE CASCADE,
    readyTime timestamptz NOT NULL DEFAULT NOW()
);
```

Lifecycle:

- Step created with 0 unfinished deps -> INSERT into `BuildStepCanCreate`
- Dep finishes, dependent's unfinished dep count hits 0 -> INSERT into `BuildStepCanCreate`
- Step dispatched to machine -> DELETE from `BuildStepCanCreate`, CREATE `BuildSteps` row
- Step finishes -> do NOT delete the `Derivations` row

Step lifecycle is derived from table presence:

- Row in `BuildStepCanCreate` -> ready to dispatch
- Row in `BuildSteps` with `busy != 0` -> currently building
- Row in `BuildSteps` with `status IS NOT NULL` -> finished

### Preserving dep edges (no soft-delete needed)

The original plan hard-deleted `BuildStepDeps` rows and `Derivations` rows when steps finished.
This would destroy the dependency graph structure,
making it impossible to later answer "which builds needed this derivation?"
or to display the historical build graph in the UI.

Instead, dep edges are permanent, immutable records.
Whether a dep is satisfied is derived by joining to `BuildSteps`:
if the `depDrvPath` has a successful `BuildSteps` row (`status = 0`), the dep is done.
No `finished` column is needed — the information is already in `BuildSteps`.

The `Derivations` row is also kept permanently.

This is important because once `build` and `stepnr` are retired from `BuildSteps`,
the only way to answer "which builds need this drv?" is
by walking `BuildStepDeps` edges and joining to `Builds` on `drvPath`.

### Dispatch query

The dispatch loop queries `BuildStepCanCreate` joined with `Builds` for scheduling priorities:

```sql
SELECT
  q.drvPath,
  q.readyTime,
  COALESCE(prio.max_global, 0) AS highest_global_priority,
  COALESCE(prio.max_local, 0) AS highest_local_priority,
  COALESCE(prio.min_id, 2147483647) AS lowest_build_id,
  COALESCE(rdeps.cnt, 0) AS rdeps_count
FROM BuildStepCanCreate q
LEFT JOIN LATERAL (
  WITH RECURSIVE all_rdeps AS (
    SELECT drvPath FROM BuildStepDeps WHERE depDrvPath = q.drvPath
    UNION
    SELECT dep.drvPath FROM BuildStepDeps dep
    JOIN all_rdeps r ON dep.depDrvPath = r.drvPath
  )
  SELECT MAX(b.globalPriority) AS max_global,
         MAX(b.priority) AS max_local,
         MIN(b.id) AS min_id
  FROM (SELECT drvPath FROM all_rdeps UNION ALL SELECT q.drvPath) all_paths
  JOIN Builds b ON b.drvPath = all_paths.drvPath AND b.finished = 0
) prio ON true
LEFT JOIN LATERAL (
  SELECT COUNT(*) AS cnt FROM BuildStepDeps WHERE depDrvPath = q.drvPath
) rdeps ON true
```

`system`, `required_features`, and `lowest_share_used` are computed application-side:
system/features by parsing the derivation from the Nix store,
share_used from the in-memory `Jobsets` map.

### `make_rdeps_runnable` (pure DB)

```sql
-- In a transaction, after step $1 succeeds:
-- Find rdeps of $1 whose deps are now all satisfied, and add to ready queue.
INSERT INTO BuildStepCanCreate (drvPath)
SELECT d.drvPath FROM BuildStepDeps d
WHERE d.depDrvPath = $1
  AND NOT EXISTS (
    SELECT 1 FROM BuildStepDeps d2
    WHERE d2.drvPath = d.drvPath
      AND NOT EXISTS (
        SELECT 1 FROM BuildSteps s
        WHERE s.drvPath = d2.depDrvPath AND s.status = 0
      )
  )
ON CONFLICT DO NOTHING;
```

No mutation of `BuildStepDeps` — the edges are permanent.
Runnability is derived from the join to `BuildSteps`.

### `get_dependent_builds` (replaces `Step::get_dependents`)

Walks ALL edges to find every build that transitively depends on a derivation:

```sql
WITH RECURSIVE rdeps AS (
  SELECT drvPath FROM BuildStepDeps WHERE depDrvPath = $1
  UNION
  SELECT d.drvPath FROM BuildStepDeps d
  JOIN rdeps r ON d.depDrvPath = r.drvPath
)
SELECT b.* FROM Builds b
WHERE b.finished = 0
  AND b.drvPath IN (SELECT drvPath FROM rdeps UNION ALL SELECT $1)
```

### What gets removed

- `step.rs` -- `Step`, `Steps`, `StepState`, `StepAtomicState` all deleted
- `Build.toplevel: ArcSwapOption<Step>` -- removed (build's `drv_path` IS the toplevel step's drvPath)
- `propagate_priorities` -- eliminated (priorities computed in dispatch query)
- `StepInfo` -- replaced by `DispatchEntry`, a short-lived struct loaded from DB during dispatch

### Transaction model for step creation

Step creation is recursive (parent creates deps, which create their deps, etc.).
Each level uses its own transaction:
inserts its `Derivations` row and dep edges atomically.
Before commit, the step doesn't exist in `Derivations`, so dispatch can't see it.
After commit, all of that step's deps are registered.
No `created` column needed.

### Implementation order

1. Schema -- `BuildStepCanCreate` table, upgrade script, update `hydra.sql`
2. DB queries -- new query functions in `connection.rs`
3. `create_step` -- rewrite to pure DB ops
4. Dispatch -- rewrite `do_dispatch_once` to use dispatch query
5. Success/failure -- rewrite to DB-only operations
6. Queue -- adapt to `DispatchEntry` instead of `Arc<StepInfo>`
7. Cleanup -- delete `step.rs`, remove `Steps` from `State`, update `Build`

## How this connects to `(drvPath, attempt)`

The `(drvPath, attempt)` migration (previous commits) and the graph-in-DB work are complementary:

- `BuildSteps` identifies step *execution attempts* by derivation
- `BuildStepDeps` / `Derivations` captures the *dependency structure* between derivations
- `BuildStepCanCreate` tracks which derivations are *ready to build*

The `build` and `stepnr` columns on `BuildSteps` are legacy —
they exist for historical data and rollback safety
but are no longer used by the queue runner or web UI for step identification.
`BuildStepOutputs` has already been migrated to `(drvPath, attempt)`.

Once the graph is in the DB, the `build` column on `BuildSteps` can be fully retired
(it's already archived in `BuildStepsHistorical`).
Today it answers "name one build that needed this step" (badly — the choice is arbitrary).
With `BuildStepDeps`, we can properly answer "which builds need this drv?"
by crawling edges and joining to `Builds` on `drvPath`.
The edges are permanent and immutable, so this works for historical builds too.

## Working with the steps of a build

The queue runner only needs to worry about things going forward,
thus it can ignore `BuildStepsHistorical` entirely.
The UI however needs to be able to render old-style and new-style historical steps alike,
so it needs to do more work.

### Old style

Old style steps will not have any of the step edges that we've discussed.
They will just be assigned builds via `BuildStepsHistorical`.
The step number indicates a possible topological sort of the original graph,
but the original graph is lost to history.

### New style

New style steps are not associated with a build, but have the explicit graph.
Given a build, we can find its root derivation, and crawl steps from there.
A build's "own" step is `BuildSteps WHERE drvPath = build.drvPath` (the `actualBuildSteps` relationship).
Dependency steps are separate derivations reachable via `BuildStepDeps`.
This means "how many steps does this build have?" is no longer a meaningful question —
instead, "how many derivations were built as part of this build's closure?" is the graph traversal.

The queue runner does NOT write to `BuildStepsHistorical` for new builds.
`historical_build_steps` returns 0 for new-style builds, which is correct.

Of course, it may be a problem that we don't know which attempt was in use at the time of a build,
if it failed and then another job built it since.
Not sure if that is a problem.

### Putting it together

UI like the steps list of a build must be bifurcated.
The step list of a build only makes sense for the old style.
Something else must be done for the new style.

For the new style, the build detail page should show the dependency graph rooted at the build's `drvPath`, with links to each step.
This can be done by walking `BuildStepDeps` edges from the root, joining to `BuildSteps` for attempt/status info.
We should not load all edges at once — instead, expand one level at a time (lazy loading) to avoid the page crawling on large graphs.
