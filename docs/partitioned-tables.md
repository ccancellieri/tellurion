# Pointing a collection at a partitioned table

The PostGIS driver reads whatever table a collection names — it has no
partitioning-aware code anywhere, and needs none. A declaratively
partitioned parent table (`PARTITION BY RANGE`/`LIST`/`HASH`) works exactly
like a plain table: point `table` (or let it derive from the collection id)
at the parent's name, and every read lane — items, bbox
filtering, `datetime` filtering, MVT tiles — goes through PostgreSQL's own
partition routing with no special configuration. This is proven end to end
in `crates/tellurion-postgis/tests/partitioning.rs`.

## Example: a time-partitioned table

```sql
CREATE TABLE observations (
    id bigserial,
    geom geometry(Point, 4326) NOT NULL,
    observed_at timestamptz NOT NULL,
    name text,
    -- Declarative partitioning requires the partition key to be part of
    -- any primary key on the parent — `id` alone can't be a PK here.
    PRIMARY KEY (id, observed_at)
) PARTITION BY RANGE (observed_at);

CREATE TABLE observations_2024 PARTITION OF observations
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
CREATE TABLE observations_2025 PARTITION OF observations
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');

-- One CREATE INDEX on the parent; PostgreSQL creates a matching index on
-- every current partition, and on every future one created with
-- `PARTITION OF ... FOR VALUES ...` afterward.
CREATE INDEX ON observations USING GIST (geom);

ANALYZE observations;
```

```yaml
collections:
  - id: observations
    catalog: default
    storage: main
    table: observations   # the parent's name — never a partition's own name
    geometry: geom
    pk: id                # still a single column; see "Primary keys" below
    datetime: observed_at
```

Nothing here needs a driver flag or a config option that says "this table is
partitioned" — the collection declaration is identical to one pointed at a
plain table. `table`/`geometry`/`pk`/`datetime` can also all be omitted and
derived from the backend's catalog, the same as for an unpartitioned
table — see `crates/tellurion-postgis/tests/partitioning.rs`'s
routing-only-collection test.

## Pruning: `datetime` filters skip non-matching partitions

A request with a `datetime` filter compiles to an ordinary
`observed_at >= $1 AND observed_at <= $2` predicate against the parent
(`sql::build_items_plan`) — no different from an unpartitioned table's
query. PostgreSQL's own planner does the rest: because the predicate is on
the partition key, it statically prunes every partition whose range can't
satisfy it before ever touching them, visible in `EXPLAIN` as `Subplans
Removed: N` and the pruned partitions' names simply absent from the plan.

Reads that carry no `datetime` filter — a plain `items` call, `bbox`-only
filtering, an `item`-by-id lookup, or an MVT tile request (the tiles lane
has no time dimension in v0.1) — get no pruning benefit: PostgreSQL scans
every partition, same as it would scan a plain table's whole row set absent
an index that narrows it. The `GiST` index on `geom` still applies within
each partition scanned, exactly as it would on an unpartitioned table; it
just doesn't get to skip partitions the way a time predicate does.

## Caveats where derivation meets partitioning

These only matter for collections that omit `table`/`geometry`/`pk`/
`datetime` and rely on catalog derivation; a collection with an
explicit `table` pointed at the parent is unaffected by all three.

- **Geometry-column/type detection on the parent.** PostGIS's
  `geometry_columns` view reports the partitioned parent as its own entry —
  same geometry column, srid, and type a plain table would report — so
  derivation works unchanged. It *also* reports every partition
  individually, each as its own physical collection in `CatalogSource::
  collections()`'s result. That's harmless today: nothing in this codebase
  auto-registers every catalog entry as a served collection, a collection is
  only ever served if config names its `table` explicitly or derives one
  from the collection id. It does mean an operator should never give a
  collection the same id as one of its own partitions' physical table names
  (e.g. don't declare a collection `id: observations_2024` alongside a
  parent `observations` collection) — the two would otherwise both resolve
  to real, independently queryable tables, which is confusing even though
  it isn't broken.

- **Extent and stats derivation.** `ST_EstimatedExtent` — the fast,
  statistics-only path `extent()` prefers — reads a physical relation's
  `pg_statistic` row, and a partitioned parent has no storage of its own to
  read; PostGIS raises a real error calling it on one. The existing `Err`
  fallback in `extent_inner` (already there for the "table was never
  `ANALYZE`d" case) covers this automatically: it falls through to the
  `ST_Extent` real-scan plan, an ordinary `SELECT ST_Extent(geom) FROM
  parent` that PostgreSQL executes as an `Append` across every partition and
  returns the correct combined bbox. Row-count estimation takes the
  opposite path: `pg_class.reltuples` on the parent *is* populated (current
  PostgreSQL aggregates each partition's statistics up to the parent on
  `ANALYZE`), so `row_estimate` answers directly, no fallback needed. Either
  way, per-partition statistics and parent-level statistics are computed
  independently — an `ANALYZE` on one partition alone does not update the
  parent's own row estimate or extent; run `ANALYZE` on the parent (which
  recurses into every partition) after a bulk load.

- **The paging cursor's primary-key expectations across partitions.**
  Keyset paging orders by the pk column ascending (`ORDER BY "id"::bigint
  ASC LIMIT n`) and compares the next token against it — this is unaffected
  by partitioning as far as *correctness* goes: PostgreSQL guarantees the
  global ordering across every partition regardless of which one each row
  physically lives in. It does affect the *plan*: unless the pk happens to
  be correlated with the partition key (it isn't in the example above — ids
  increase independently of which time range a row falls into), sorting
  requires a real `Sort` over an `Append` of every partition the query
  touches, not a cheap per-partition-index merge. A `datetime` filter that
  prunes to one partition sidesteps this too (nothing left to merge-sort);
  an unfiltered or bbox-only page over a large partitioned table pays for
  the full cross-partition sort like it would against an unpartitioned
  table of the same total size.

## Scaling stance: partitioning is a table-depth lever, not the platform lever

Partitioning is a tool a collection's *storage* can use internally — same
category as an index or a materialized view. It scales one table deeper
within a single PostgreSQL/PostGIS instance and Tellurion never needs to
know it's there. It is not, and does not replace, the platform's own answer
to scale, which stays routing: many storages, driver heterogeneity — a
tenant or catalog can point different collections at different databases,
different drivers entirely, or a partitioned table where that fits — rather
than one ever-deeper database everything funnels through. The read path
that serves a partitioned collection takes no DDL and does no runtime
schema management, ever — partitions are created and maintained
operator-side, outside Tellurion entirely, the same way an index is.
Separation between catalogs stays logical (routing to the storage/table a
catalog's collections are configured to use), never physical
schema-per-catalog.
