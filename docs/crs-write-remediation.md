# Repairing rows written through the pre-fix default CRS write path

Applies only to a collection whose PostGIS storage column's SRID is **not**
4326, on a Tellurion build that predates the fix for the default write path
tagging geometries 4326 instead of transforming them (`write_sql::
input_geom_expr`, both the `create`/`POST` and `apply`/`PUT` paths). A
collection stored in 4326 was never affected — the pre-fix SQL and the
fixed SQL are byte-for-byte identical for that case, so there is nothing to
repair.

## What went wrong

Every write that arrived without a `Content-Crs` header — the ordinary
case, since that header is optional and its absence means "interpret the
body as CRS84" — had its geometry tagged `ST_SetSRID(..., 4326)`
unconditionally, regardless of the collection's real storage SRID.
`ST_SetSRID` only labels a geometry's metadata; it never touches the
coordinate values. So for a collection whose storage SRID is, say, 3857,
the raw CRS84 (longitude, latitude) values from the request body were
stored as-is, mislabelled `4326`, next to correctly-tagged `3857` rows
written through `Content-Crs: <this collection's own EPSG URI>` (the
`RequestedCrs::Storage` path, unaffected by this bug). Nothing about the
write failed or warned — the row is simply in the wrong place, and every
later read, tile, or spatial predicate against it inherits the mistake.

Because the coordinate values themselves were never altered, they are still
the genuine, correct CRS84 numbers — only the SRID tag is wrong. That is
what makes an in-place repair possible: reproject from the (wrongly)
tagged 4326 to the collection's real storage SRID, exactly what the write
path itself should have done at insert time.

## Identifying affected rows

For a collection whose storage SRID is `<storage_srid>` (not 4326), a row
whose stored geometry reports `ST_SRID(geom) = 4326` is a candidate:

```sql
SELECT count(*) FROM <table> WHERE ST_SRID(<geometry_column>) = 4326;
```

**Applicability caveat — read before running anything below.** A nonzero
count is *consistent with* this bug, not proof of it. Only the operator
running a given deployment can know whether every 4326-tagged row in a
non-4326 collection's table really is a product of this bug, as opposed to
genuinely-4326 data intentionally stored alongside other SRIDs in an
untyped/mixed `geometry` column (PostGIS permits this when the column
carries no `geometry(Type, SRID)` typmod constraint). Confirm the
provenance of the affected rows — for example, by cross-referencing
`committed_at` against the deployment's own upgrade history, or a
`before`/`after` snapshot — before running the repair below against
production data. When in doubt, back up the table first.

## The repair

Once the candidate rows are confirmed, reprojecting them in place is a
single statement per affected table:

```sql
BEGIN;

UPDATE <table>
SET <geometry_column> = ST_Transform(<geometry_column>, <storage_srid>)
WHERE ST_SRID(<geometry_column>) = 4326;

-- Spot-check before committing: every previously-4326-tagged row should
-- now report the collection's real storage SRID, and its coordinates
-- should look like plausible values in that SRID's own unit (e.g. meters
-- for a projected CRS like 3857, not degrees).
SELECT ST_SRID(<geometry_column>), ST_AsText(<geometry_column>)
FROM <table>
LIMIT 20;

COMMIT; -- or ROLLBACK if the spot-check looks wrong
```

This is exactly `write_sql::input_geom_expr`'s own fixed default-path SQL,
applied retroactively: `ST_Transform` reprojects from the geometry's
current (wrongly-tagged, but numerically correct CRS84) representation into
`<storage_srid>`.

### Axis order

If `<storage_srid>` is itself authority-ordered latitude-before-longitude
(`tellurion_core::crs::is_lat_lon_order` — narrowly SRID 4326 today, see
that function's own doc), wrap the transform in `ST_FlipCoordinates` the
same way the write path's own fixed SQL does:

```sql
UPDATE <table>
SET <geometry_column> = ST_FlipCoordinates(ST_Transform(<geometry_column>, <storage_srid>))
WHERE ST_SRID(<geometry_column>) = 4326;
```

In practice this branch is unreachable for any storage SRID this repair
ever applies to: `is_lat_lon_order` only recognizes SRID 4326, and a
collection stored in 4326 was never affected by this bug in the first
place (see the byte-for-byte note above). It is documented here only so the
repair stays correct if that recognition is ever widened to another
lat-lon-ordered authority.

### Outbox and any downstream index

Running the repair above does not append a new outbox obligation for the
rows it touches — it is a direct data correction, not a `WriteSink::apply`
call. A collection with a derived index (`IndexSink`) built off the outbox
will not see this correction reflected until its own reconciliation or a
fresh full rebuild; plan for that separately if the collection has one.
