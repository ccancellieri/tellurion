# Writing a Tellurion storage driver

Tellurion protocol handlers do not know whether data comes from PostGIS, a
flat file, an object store, or an embedded fixture. They resolve a collection
lane through `tellurion_core::Router` and receive a capability trait object.
A driver is the adapter between those traits and one storage system.

The readable reference is the `tellurion-memory` crate. It is intentionally
immutable and small enough to inspect in one sitting. It is a documentation
and contract-test fixture, not a production backend registered by the server.

## Choose capabilities honestly

Every driver implements `StorageDriver` and must return a `CatalogSource`.
Other accessors are optional claims:

| Accessor | Trait | Claim means the driver can |
|---|---|---|
| `catalog_source()` | `CatalogSource` | enumerate physical collections and report metadata used at boot |
| `feature_source()` | `FeatureSource` | page GeoJSON features and look up one feature by id |
| `tile_source()` | `TileSource` | return MVT tile bytes and, when known, vector-layer names |
| `volume_source()` | `VolumeSource` | return a triangle mesh for a 3D tile |
| `write_sink()` | `WriteSink` | atomically commit an item mutation and its outbox obligation |
| `outbox_source()` | `OutboxSource` | read committed obligations in per-collection sequence order |

Return `None` for an unsupported optional capability. Do not return a stub
that fails every request. The router can then reject an explicitly configured
lane at boot with a precise configuration error. `MemoryDriver` advertises
features and deliberately leaves tiles and volumes absent; the test
`explicit_tiles_lane_is_rejected_at_boot` pins that behavior.

Filtering is a refinement of `FeatureSource`, not a separate lane. Leave
`filter_capable()` at its default `false` unless every accepted filter can be
evaluated correctly. A non-filtering source should also reject a direct
`ItemsQuery` containing a filter, as the memory driver does, rather than
silently returning unfiltered data.

`crs_capable()` and `filter_crs_capable()` are two more refinements of the
same trait, and they are independent. The first says the driver can serve
response geometry in a CRS other than the Part 1 default; the second says it
can evaluate a `filter`'s own spatial literals in a CRS the client names
(`ItemsQuery::filter_crs`). Declaring the first makes OGC API — Features
Part 3 Requirement 8 (`/req/filter/filter-crs-param`) binding on the driver,
because that requirement's condition is "Server supports additional
coordinate reference systems" — so a driver that reprojects output but leaves
`filter_crs_capable()` at `false` costs its whole deployment the Part 3
Filtering conformance classes (`Router::filtering_conformance_classes` folds
them away). Leave both at `false` unless the transform is real: a driver that
accepted a `filter-crs` and evaluated the filter in a different CRS anyway
would return the wrong features under a `200`, which is worse than any
refusal.

`filter_crs_capable()` also decides what the STAC `/search` lane can do for a
collection whose storage is *not* CRS84. The STAC API Filter
Extension pins `filter-crs` to CRS84 — it is both the default and the only
value a STAC server must accept — so `/search` never asks a driver for a
client-named storage CRS the way `/items` does. But honouring CRS84 against a
projected collection still means a real transform of the filter's spatial
literals, so a driver that leaves this at `false` has that request refused by
name on its behalf, and one that declares it serves it. A driver whose
collections are always CRS84 is unaffected either way.

## Implement the mandatory catalog first

`CatalogSource::collections()` reports physical reality, not a copy of the
operator's configuration. For each `PhysicalCollection`, report only facts the
backend can establish:

- its physical table, layer, or object name;
- geometry column and stable primary key when features are supported;
- native SRID and broad geometry type when known;
- spatial extent transformed to CRS84;
- a cheap row estimate, or `None` rather than an expensive full count;
- non-geometry attribute names and broad backend types;
- one temporal column only when the backend can identify it unambiguously.

The router compares configured targets with this catalog and merges operator
overrides with derived facts. A `FeatureSource` needs concrete geometry and
primary-key fields after that merge. A tiles-only archive may correctly report
neither. Never expose an internal Tellurion id as if it were a physical or
public id.

The memory backend derives its catalog once from validated GeoJSON:
`geometry`, `id`, SRID 4326, an envelope calculated from coordinates, an exact
row count, and conservative attribute types. It rejects a `geometry` property
because that name is reserved for the physical geometry column. Its unit test
`derives_extent_geometry_type_and_attribute_schema` shows the expected shape.

## Add one optional source completely

Implement a source only after the catalog facts needed by that source are
available. For features, both methods are required:

```rust
#[async_trait::async_trait]
impl tellurion_core::FeatureSource for Backend {
    async fn items(
        &self,
        collection: &tellurion_core::CollectionDecl,
        query: &tellurion_core::ItemsQuery,
    ) -> tellurion_core::Result<tellurion_core::FeaturePage> {
        // Resolve the already-validated physical target, apply every supported
        // query refinement, and return one deterministic page.
        # unimplemented!()
    }

    async fn item(
        &self,
        collection: &tellurion_core::CollectionDecl,
        id: &str,
        filter: Option<&tellurion_core::Filter>,
    ) -> tellurion_core::Result<Option<serde_json::Value>> {
        // A missing feature is Ok(None), not a backend failure. `filter` is
        // a row-level grant filter: only ever `Some` when the driver
        // advertises `filter_capable()` — a non-capable driver takes it as
        // `_filter` and the handlers deny filtered-only grants up front. A
        // capable driver must apply it so an excluded row answers Ok(None).
        # unimplemented!()
    }
}
```

Protocol concerns stay outside the driver. A source returns data, counts,
continuation tokens, or shared core errors. Protocol crates own links, media
types, authorization, RFC 9457 problem documents, and OGC/STAC response
assembly.

## Pair writes with their outbox

`WriteSink` has no data-only method. Its `apply()` operation must commit the
feature mutation and the corresponding outbox obligation in one backend
transaction, returning the committed per-collection sequence. A backend that
cannot guarantee that atomicity must leave `write_sink()` absent.

`OutboxSource` is the read side of the same contract. `read_after()` returns
obligations strictly after a sequence, in ascending order, without skipping,
and `primary_high_water()` reports the source-of-truth high-water mark. A
write-capable driver advertises both accessors; retrying an obligation must be
safe for an idempotent downstream consumer.

The reference driver advertises neither capability. The contract test
`explicit_write_lane_is_rejected_at_boot` proves it cannot be configured on a
write lane accidentally. Runtime DDL remains outside every serving driver.

## Use stable keyset paging

`ItemsQuery::token` is opaque to callers and backend-defined. It must identify
a stable position, never a numeric offset into a result that can shift between
requests. Use a deterministic ordering and reject malformed or stale tokens;
never restart silently from page one.

For each page:

1. validate `limit`, bbox, token, and optional refinements;
2. count matches before applying the page window when an exact count is
   affordable, otherwise return `number_matched: None`;
3. seek strictly after the cursor in the stable ordering;
4. fetch one item beyond `limit` to detect whether another page exists;
5. return a next token encoding the last returned key only when more matches
   remain.

The reference orders stringified GeoJSON ids in a `BTreeMap`. Its token is
`v1.` followed by lowercase hexadecimal UTF-8 bytes of the last id. This format
is intentionally inspectable, but consumers must still treat it as opaque.
`pages_all_features_once_in_stable_id_order` and `invalid_queries_are_refused`
pin the walk and rejection rules.

OGC API Features Part 1 defines feature-collection paging through link
relations such as `next` and defines `numberMatched` independently of the page
size. The protocol handler turns the driver's continuation token into that
link; the driver does not construct HTTP URLs. See the normative
[OGC API Features Part 1 standard](https://docs.ogc.org/is/17-069r4/17-069r4.html).

## Map errors at the driver boundary

Return the shared `tellurion_core::Error` categories so handlers can construct
one consistent problem response:

- `Error::Config` for invalid preload/configuration, missing registrations,
  and a physical target the driver cannot resolve;
- `Error::Invalid` for bad tokens, invalid bboxes, and unsupported query
  refinements supplied directly to the source;
- `Error::Storage` for backend or I/O failures, retaining the source error;
- `Ok(None)` for a valid item lookup whose id is absent.

Do not create driver-specific HTTP errors. Do not turn an unavailable backend
into `NotFound`, and do not ignore an unsupported refinement.

## Register through a factory

`DriverFactory::name()` matches `StorageDecl.driver`. `build()` receives one
validated storage declaration and returns a fully built `StorageDriver`.
Backend behavior belongs in the configuration tree; a named environment
variable may carry a secret connection URL, but it must not become a second
behavior-configuration surface.

The reference factory is preloaded by storage id because it is a fixture:

```rust
use std::sync::Arc;
use serde_json::json;
use tellurion_core::Registry;
use tellurion_memory::{MemoryDataset, MemoryDriver, MemoryDriverFactory};

let roads = MemoryDataset::from_feature_collection(
    "roads",
    json!({"type": "FeatureCollection", "features": []}),
)?;
let driver = MemoryDriver::new([roads])?;
let mut factory = MemoryDriverFactory::new();
factory.insert("memory-main", driver)?;

let mut registry = Registry::new();
registry.register(Arc::new(factory));
# Ok::<(), Box<dyn std::error::Error>>(())
```

It neither reads an environment variable nor registers itself with the server.
`factory_build_is_keyed_by_storage_id` proves that factory lookup uses the
storage id and that a missing preload is a configuration error.

## Expect boot validation to exercise the contract

After building a router, eager startup calls `Router::validate_catalog()`.
Plan for it to:

- enumerate every registered storage catalog;
- verify configured physical collection names;
- require every driver in an explicitly routed lane to advertise that lane's
  capability;
- require feature-capable collections to resolve geometry and primary-key
  fields;
- derive and cache collection descriptors.

Keep catalog calls safe to perform at boot and return actionable errors naming
the bad storage or collection. Do not create schemas or other runtime DDL from
the serving process.

`reference_driver_passes_boot_and_exposes_derived_metadata`,
`absent_physical_collection_is_rejected_at_boot`, and
`explicit_tiles_lane_is_rejected_at_boot` are executable examples using the
ordinary registry and router. `explicit_write_lane_is_rejected_at_boot` pins
the same rule for the transactional write capability.

## Filtering and OGC conformance

The protocol layer checks `filter_capable()` before passing a parsed CQL2
expression to a source. A driver that returns `true` must evaluate the complete
supported AST with safe parameter binding and the same semantics advertised by
the API. Partial or best-effort evaluation is not acceptable. OGC API Features
Part 3 defines the filtering and queryables conformance classes; see the
normative [Part 3: Filtering standard](https://docs.ogc.org/is/19-079r2/19-079r2.html).

`cql2_conformance_classes()` is where a driver names the CQL2 (OGC 21-065r2)
requirements classes it satisfies, and a class is satisfied only when the
*whole* grammar it describes is compiled — operators, operand shapes, literal
types, **and every position the expression grammar admits the predicate in**.
Compiling an operator in a restricted position is not the class: CQL2 states
each class's permitted narrowings as explicit permissions, so a narrowing no
permission covers is not one the class allows. Read the class's own Abstract
Test Suite (normative Annex A), not only its requirement statements — the tests
name the compositions a conforming server must evaluate. The GeoPackage driver is the
worked example: `tellurion-geopackage` compiles `S_INTERSECTS` only once per filter
and only in AND-position, so it withholds `basic-spatial-functions` while still
declaring `basic-cql2` (whose Requirement 1 excepts `spatialPredicate` by
name). Withholding a class never means removing the capability — the restricted
form keeps working, and the unsupported form keeps being refused by name.

Implementing these Rust traits does not by itself make a deployment OGC- or
STAC-conformant. Conformance belongs to the assembled HTTP surface: declared
conformance classes, response documents, links, encodings, query behavior, and
error responses together.

## Driver conformance checklist

Adapt the reference tests to the new driver and verify every claimed item:

- [ ] `CatalogSource::collections()` reports backend facts rather than copied
  configuration.
- [ ] Extent is CRS84, row estimate is cheap, schema types are honest, and
  unknown facts are `None`.
- [ ] Duplicate or incomplete driver configuration fails as `Error::Config`.
- [ ] Every advertised optional capability works through `Router`, not only by
  calling the backend type directly.
- [ ] Every unadvertised explicit lane fails boot validation precisely.
- [ ] A write-capable driver commits data and its outbox obligation atomically,
  advertises the matching `OutboxSource`, and is tested for ordered replay.
- [ ] Feature pages use stable keyset order and a complete walk returns every
  match exactly once.
- [ ] `number_matched` describes all matches before the page window when it is
  returned.
- [ ] Malformed, unknown-version, non-canonical, and stale tokens fail as
  `Error::Invalid`.
- [ ] Bbox boundary semantics and null geometries are tested.
- [ ] Missing feature ids return `Ok(None)`.
- [ ] `filter_capable()` is true only when the complete accepted filter surface
  is implemented; direct unsupported refinements are rejected.
- [ ] `filter_crs_capable()` is true only when a filter's spatial literals are
  genuinely transformed from the declared CRS into the storage CRS — and it is
  true whenever `crs_capable()` is, or the deployment loses Part 3.
- [ ] Backend failures retain their source through `Error::Storage`.
- [ ] Default builds and tests do not require optional external services.

Run the reference contract itself with:

```text
cargo fmt --all --check
cargo clippy -p tellurion-memory --all-targets -- -D warnings
cargo test -p tellurion-memory
cargo test -p tellurion-core
```

For another workspace crate, replace the package name and keep an equivalent
router-level contract test. A trait-signature change should fail compilation;
a semantic change should fail a named test.
