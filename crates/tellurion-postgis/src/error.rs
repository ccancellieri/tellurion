//! Internal error type for this crate — wraps identifier validation failures
//! and every fallible dependency (`deadpool_postgres`, `tokio_postgres`,
//! background-task join errors). Converted to `tellurion_core::Error` at the
//! trait-impl boundary in `driver.rs`; nothing outside this crate ever sees
//! `PostgisError` directly.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PostgisError {
    #[error(
        "invalid identifier '{0}': only ASCII letters, digits, and '_' are allowed, and it may not start with a digit"
    )]
    InvalidIdentifier(String),

    /// A keyset token that doesn't parse as this collection's declared
    /// `id_type` (`#87`: `i64` for `Integer`, `uuid::Uuid` for `Uuid`).
    #[error("keyset token '{0}' is not valid for this collection's declared id type")]
    InvalidToken(String),

    #[error(
        "collection '{0}' has no datetime column configured but a datetime filter was supplied"
    )]
    NoDatetimeColumn(String),

    #[error(
        "collection '{collection}' exact item response crosses its {limit}-vertex budget at feature '{feature_id}' ({cumulative_vertices} cumulative vertices)"
    )]
    ItemsVertexBudgetExceeded {
        collection: String,
        feature_id: String,
        cumulative_vertices: u64,
        limit: u64,
    },

    #[error("PostGIS returned an invalid negative vertex count: {0}")]
    InvalidVertexCount(i64),

    #[error("tile coordinate {0} does not fit PostGIS's int4 ST_TileEnvelope arguments")]
    TileCoordOutOfRange(u32),

    /// A `Mutation::feature_id` that doesn't parse as this collection's
    /// declared `id_type` (`#87`: `i64` for `Integer`, `uuid::Uuid` for
    /// `Uuid`; `write.rs`, `#25`).
    #[error("feature id '{0}' is not valid for this collection's declared id type")]
    InvalidFeatureId(String),

    /// A feature property whose JSON value is an array or object — outside
    /// the flat scalar model the write path stores a column value as
    /// (`write.rs`, `#25`).
    #[error(
        "property '{0}' has an array/object value, which this write path cannot store in a column"
    )]
    UnsupportedPropertyValue(String),

    /// A feature property that names no real column of this collection's
    /// table — neither declared in `CollectionDecl::schema` nor reported by
    /// a live catalog lookup (`write.rs`, `#25`).
    #[error("property '{0}' does not name a column of this collection")]
    UnwritableProperty(String),

    /// The per-collection outbox table this collection's write lane needs is
    /// absent (`#25`). Named and distinct from an ordinary `Postgres`
    /// error so it maps to a clear, actionable message rather than a raw SQL
    /// error — the server never creates this table itself; provisioning is
    /// `tellurion-ingest outbox create-tables`'s job.
    #[error("outbox table '{0}' does not exist; provision it with `tellurion-ingest outbox create-tables`")]
    OutboxTableMissing(String),

    /// The per-collection outbox table exists but predates `#141`/`#142`'s
    /// `extent_crs84` column, which every write and every drain now names.
    /// Named and distinct from an ordinary `Postgres` error for exactly the
    /// reason `SearchColumnMissing` (`#181`) is: the server never does DDL
    /// itself, rerunning `tellurion-ingest outbox create-tables` adds the
    /// column in place (its `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` is
    /// idempotent), and an operator deserves to be told that rather than to
    /// have every write silently record an invalidation extent nobody can
    /// read back.
    #[error("outbox table '{0}' has no extent_crs84 column; re-provision it with `tellurion-ingest outbox create-tables`")]
    OutboxExtentColumnMissing(String),

    #[error("outbox returned invalid negative sequence {0}")]
    OutboxSequenceInvalid(i64),

    /// A server-assigned create's `INSERT` (`write_sql::build_insert_plan`,
    /// `#88`) hit a `NOT NULL` violation on the pk column it deliberately
    /// omitted — the collection's pk column has no `DEFAULT` (no
    /// `bigserial`/identity backing it) to mint a value from. Named and
    /// distinct from an ordinary `Postgres` error, the create-lane
    /// counterpart of `OutboxTableMissing`: an unprovisioned create target,
    /// not a caller mistake.
    #[error("collection '{0}' has no bigserial/identity default backing its primary key column; a server-assigned create cannot mint an id for it")]
    PkNotServerAssignable(String),

    /// A collection declares `id_type: uuid`/`text` but its physical pk
    /// column's own real type doesn't match (`#87` for `uuid`, `#94` for
    /// `text` — `driver.rs`'s `validate_id_type_for_create`) — checked live,
    /// before a server-assigned create's `INSERT` is ever built, so this
    /// surfaces as a named refusal rather than an opaque client-side
    /// type-mismatch error when the `RETURNING` row is read back, or a
    /// read/write lane that silently never matches any id.
    #[error("collection '{collection}': id_type is declared '{declared}' but pk column '{pk}' is '{actual}'")]
    IdTypeMismatch {
        collection: String,
        pk: String,
        declared: &'static str,
        actual: String,
    },

    /// A server-assigned create's feature body carried no top-level `id`
    /// member for a `Text` id-type collection (`#94`) — unlike
    /// `Integer`/`Uuid`, a `Text` pk has no server-side generator, so the
    /// caller must supply one. Named and refused before any SQL is built —
    /// a caller mistake, never a table-provisioning problem the way
    /// `PkNotServerAssignable` is.
    #[error("collection '{0}': id_type is declared 'text'; POST create requires a top-level 'id' in the feature body")]
    TextIdRequired(String),

    /// A server-assigned create for a `Text` id-type collection (`#94`, a
    /// caller-supplied pk) hit its own `UNIQUE`/primary-key violation — the
    /// id named in the request body is already claimed by another row in
    /// this collection. Named and mapped to a `409`, the create-lane
    /// counterpart of `AssetKeyConflict`, never a raw SQL error or an opaque
    /// `500`. Never reachable for `Integer`/`Uuid` (the pk column is always
    /// omitted from their `INSERT`, so no `UNIQUE` violation on it can ever
    /// occur there).
    #[error("collection '{table}': primary key '{id}' already exists")]
    PkConflict { table: String, id: String },

    /// `#150`: this collection's `table:` names a relation with no `xmin`
    /// system column — a VIEW, in practice. A view carries no per-row
    /// version, so there is no witness this driver could compare inside the
    /// write transaction, and the atomic optimistic-locking guard genuinely
    /// cannot be offered for that collection. Named, and mapped to the same
    /// `CapabilityUnsupported` the `WriteSink` trait's own defaults refuse
    /// with, so an operator reads one consistent "this write lane cannot do
    /// optimistic locking" whether the driver declined wholesale or only for
    /// this relation — never a raw SQL error, and never a silent fall back
    /// to the racy pre-transaction check.
    #[error("relation '{0}' has no xmin system column, so an optimistic-locking precondition cannot be evaluated inside this collection's write transaction")]
    OptimisticLockingUnsupported(String),

    /// The per-collection derived-index table this collection's index lane
    /// needs is absent (`#67`). Named and distinct from an ordinary
    /// `Postgres` error for the same reason `OutboxTableMissing` is: the
    /// server never creates this table itself; provisioning is
    /// `tellurion-ingest index create-tables`'s job. Surfaces as a clean
    /// refusal (`Error::Config`) rather than a raw SQL error whenever a
    /// collection is configured with `routing.index` but the table was
    /// never provisioned.
    #[error("index table '{0}' does not exist; provision it with `tellurion-ingest index create-tables`")]
    IndexTableMissing(String),

    /// `#181`: the derived-index table exists but predates the free-text
    /// slice — its generated `search_text` `tsvector` column is absent, so
    /// a `q`-bearing search has nothing to compile its predicate against.
    /// Named and distinct from an ordinary `Postgres` error for the same
    /// reason `IndexTableMissing` is: the server never does DDL itself;
    /// rerunning `tellurion-ingest index create-tables` upgrades the table
    /// in place (its `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` is
    /// idempotent). Only ever produced on the `q` path — a `q`-less search
    /// never touches the column.
    #[error("index table '{0}' has no search_text column; re-provision it with `tellurion-ingest index create-tables`")]
    SearchColumnMissing(String),

    /// The per-collection asset-records table this collection's
    /// `AssetRecordStore` capability needs is absent (assets-and-object-
    /// storage proposal, first slice). Same treatment as
    /// `OutboxTableMissing`/`IndexTableMissing`: the server never creates
    /// this table itself; provisioning is `tellurion-ingest assets
    /// create-tables`'s job.
    #[error("asset records table '{0}' does not exist; provision it with `tellurion-ingest assets create-tables`")]
    AssetsTableMissing(String),

    /// The per-collection STAC metadata sidecar this collection's
    /// `stac_metadata: true` opt-in needs is absent (`#202`). Same
    /// treatment as `OutboxTableMissing`/`IndexTableMissing`/
    /// `AssetsTableMissing`: the server never creates this table itself;
    /// provisioning is `tellurion-ingest stac create-tables`'s job. Named
    /// rather than answered as an empty sidecar, because an empty answer is
    /// exactly what a correctly provisioned sidecar with no row for these
    /// items looks like — an operator could never tell the two apart.
    #[error("STAC metadata sidecar table '{0}' does not exist; provision it with `tellurion-ingest stac create-tables`")]
    StacTableMissing(String),

    /// The deployment-wide durable job ledger (`#182`) is absent. Same
    /// treatment as `OutboxTableMissing`/`IndexTableMissing`/
    /// `AssetsTableMissing`/`StacTableMissing`: the server never creates this
    /// table itself; provisioning is `tellurion-ingest processes
    /// create-tables`'s job. Named rather than answered as an empty ledger —
    /// "no such job" is exactly what a correctly provisioned but empty ledger
    /// looks like, and a submission answered `201` against a table that does
    /// not exist would be a job the server promised to run and could not even
    /// record.
    #[error("job ledger table '{0}' does not exist; provision it with `tellurion-ingest processes create-tables`")]
    JobsTableMissing(String),

    /// A ledger row this driver read back does not match the shape this
    /// driver itself writes — a `status` outside the closed vocabulary, say.
    /// A storage-layer anomaly (a hand-edited row, a schema drift), never a
    /// caller mistake, and named for the same reason `MalformedAssetRow` is
    /// rather than silently coerced onto a neighbouring state.
    #[error("malformed job ledger row: {0}")]
    MalformedJobRow(String),

    /// A `"<table>_stac"` row whose `doc` is not a JSON object, so there is
    /// no member set to merge into an Item at all — a storage-layer anomaly
    /// (a hand-edited row, an out-of-band populator writing a scalar),
    /// never a caller mistake, and named for the same reason
    /// `MalformedAssetRow` is rather than silently skipped.
    #[error("malformed STAC sidecar row for feature '{0}': doc is not a JSON object")]
    MalformedStacRow(String),

    /// `AssetRecordStore::register` refused a `UNIQUE (item_id, asset_key)`
    /// violation — a key already claimed. Named and distinct from an
    /// ordinary `Postgres` error so `tellurion_core::Error::Conflict` (409)
    /// is what a caller sees, never a raw constraint-violation message.
    #[error("asset key '{0}' is already registered")]
    AssetKeyConflict(String),

    /// A row `asset_sql::row_to_asset_record` read back doesn't match the
    /// shape this driver itself writes — a storage-layer anomaly (a
    /// hand-edited row, a schema drift), never a caller mistake.
    #[error("malformed asset row: {0}")]
    MalformedAssetRow(String),

    /// A relational registry row whose indexed identity columns disagree
    /// with the JSON declaration they are supposed to index.
    #[error("malformed registry row: {0}")]
    MalformedRegistryRow(String),

    /// `AssetRecordStore::finalize` named no row to update — `(item_id,
    /// key)` doesn't exist. `tellurion_core::asset::complete_upload`
    /// already checks existence via `get` before ever calling `finalize`,
    /// so this is a defensive backstop, not a path a normal caller reaches.
    #[error("asset not found")]
    AssetNotFound,

    /// `#41`: the collection's geometry column, as reported by
    /// `geometry_columns`, is not one of the 3D solid types `VolumeSource`
    /// can decode (`PolyhedralSurface Z`, `TIN Z`, `MultiPolygon Z`) — a
    /// named, request-time refusal instead of a confusing per-row EWKB
    /// decode failure. `found` names whatever the catalog actually reported
    /// (e.g. `"POLYGON (coord_dimension 2)"`), or explains that the column
    /// isn't registered in `geometry_columns` at all.
    #[error(
        "collection '{collection}': geometry column is not a supported 3D solid type for volume serving (found {found}); expected an XYZ PolyhedralSurface, TIN, or MultiPolygon column"
    )]
    UnsupportedVolumeGeometryType { collection: String, found: String },

    /// `#41`: the EWKB reader (`ewkb.rs`) ran out of bytes, or hit a byte
    /// sequence it cannot make sense of, decoding a row this driver's own
    /// `ST_AsEWKB` query produced — a backend anomaly, not a caller mistake
    /// (the geometry-type check already ran before this query, so a healthy
    /// backend should never actually trigger this).
    #[error("malformed EWKB while decoding volume geometry for collection '{0}'")]
    MalformedEwkb(String),

    /// `#41`: a nested sub-geometry inside a PolyhedralSurface/TIN/
    /// MultiPolygon carried a WKB type code this reader doesn't recognize
    /// (only `Polygon`/`Triangle` members are ever valid there). Distinct
    /// from `MalformedEwkb` so a caller can tell "ran out of bytes" apart
    /// from "the bytes parsed but named something unexpected".
    #[error("collection '{0}': unsupported EWKB geometry type code {1} inside a volume geometry")]
    UnsupportedEwkbGeometryType(String, u32),

    /// `#193`: opening the dedicated session an advisory-lock lease holds
    /// for the duration of leadership did not complete within the
    /// configured ceiling. Named and distinct from an ordinary `Postgres`
    /// error because the caller must be able to tell "the coordinator did
    /// not answer" from "the coordinator said somebody else leads" — the
    /// second is an ordinary `Ok(None)`, and conflating them would let an
    /// unreachable database read as permission to lead
    /// (`tellurion_core::lease::Lease::try_acquire`'s own contract).
    #[error("lease coordinator did not answer within {0}ms")]
    LeaseCoordinatorTimeout(u64),

    #[error(transparent)]
    PoolConfig(#[from] deadpool_postgres::ConfigError),

    #[error(transparent)]
    CreatePool(#[from] deadpool_postgres::CreatePoolError),

    #[error(transparent)]
    BuildPool(#[from] deadpool_postgres::BuildError),

    #[error(transparent)]
    Pool(#[from] deadpool_postgres::PoolError),

    #[error(transparent)]
    Postgres(#[from] tokio_postgres::Error),

    #[error("background query task failed: {0}")]
    Join(#[from] tokio::task::JoinError),

    /// A stored `decl` column's JSON didn't deserialize into the expected
    /// `CatalogDecl`/`CollectionDecl` shape (`registry.rs`) — a storage
    /// fault, not a caller mistake: the operator-facing validation for that
    /// shape already happened at `tellurion-ingest registry publish` time.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, PostgisError>;

impl PostgisError {
    /// True only when retrying this exact item would deterministically fail.
    /// Infrastructure, configuration, and transaction failures abort the
    /// whole chunk so the outer transaction rolls back.
    pub(crate) fn is_deterministic_batch_refusal(&self) -> bool {
        match self {
            Self::InvalidFeatureId(_)
            | Self::UnsupportedPropertyValue(_)
            | Self::UnwritableProperty(_) => true,
            Self::Postgres(error) => error
                .code()
                .is_some_and(|code| code.code().starts_with("22") || code.code().starts_with("23")),
            _ => false,
        }
    }
}

impl From<PostgisError> for tellurion_core::Error {
    fn from(error: PostgisError) -> Self {
        match error {
            PostgisError::InvalidIdentifier(_)
            | PostgisError::PoolConfig(_)
            | PostgisError::CreatePool(_)
            | PostgisError::BuildPool(_) => tellurion_core::Error::Config(error.to_string()),

            // A named, actionable error distinct from an ordinary storage
            // fault — never created by the server, provisioned by
            // `tellurion-ingest outbox create-tables` instead (`#25`).
            // `OutboxExtentColumnMissing` (`#141`/`#142`) joins them: the
            // same command, rerun, adds the column in place.
            PostgisError::OutboxTableMissing(_)
            | PostgisError::OutboxExtentColumnMissing(_)
            | PostgisError::OutboxSequenceInvalid(_) => {
                tellurion_core::Error::Config(error.to_string())
            }

            // Same treatment as `OutboxTableMissing`, for the derived-index
            // table (`#67`, provisioned by `tellurion-ingest index
            // create-tables`) and its `#181` free-text column (same command,
            // rerun).
            PostgisError::IndexTableMissing(_) | PostgisError::SearchColumnMissing(_) => {
                tellurion_core::Error::Config(error.to_string())
            }

            // Same treatment as `OutboxTableMissing`/`IndexTableMissing`: an
            // unprovisioned target for the operation, not a caller mistake
            // (`#88`).
            PostgisError::PkNotServerAssignable(_) => {
                tellurion_core::Error::Config(error.to_string())
            }

            // Same treatment again, for the asset-records table (assets-
            // and-object-storage proposal, first slice, provisioned by
            // `tellurion-ingest assets create-tables`).
            PostgisError::AssetsTableMissing(_) => tellurion_core::Error::Config(error.to_string()),

            // Same treatment once more, for the per-item STAC metadata
            // sidecar (`#202`, provisioned by `tellurion-ingest stac
            // create-tables`): a collection that declared `stac_metadata:
            // true` without provisioning the table is misconfigured, not
            // faulting.
            PostgisError::StacTableMissing(_) => tellurion_core::Error::Config(error.to_string()),

            // Same treatment once more, for the durable job ledger (`#182`,
            // provisioned by `tellurion-ingest processes create-tables`): a
            // deployment that declared `server.processes` without
            // provisioning the table is misconfigured, not faulting.
            PostgisError::JobsTableMissing(_) => tellurion_core::Error::Config(error.to_string()),

            // A named `409` — a key already claimed, not an internal fault.
            PostgisError::AssetKeyConflict(_) => tellurion_core::Error::Conflict(error.to_string()),

            // Same treatment, for a `Text` id-type collection's
            // caller-supplied pk conflicting with an existing row (`#94`).
            PostgisError::PkConflict { .. } => tellurion_core::Error::Conflict(error.to_string()),

            // A storage-layer decode anomaly — never a caller mistake, so
            // this surfaces as `Error::Storage` (logged, generic `500`)
            // rather than naming internal column shapes to a client.
            PostgisError::MalformedAssetRow(_)
            | PostgisError::MalformedRegistryRow(_)
            | PostgisError::MalformedJobRow(_)
            | PostgisError::MalformedStacRow(_) => tellurion_core::Error::Storage(Box::new(error)),

            PostgisError::AssetNotFound => tellurion_core::Error::NotFound,

            // `#150`: the same named `CapabilityUnsupported` refusal the
            // `WriteSink` trait's own `row_version`/`apply_conditional`
            // defaults produce — one capability name for one honest fact
            // about this write lane, whatever the reason underneath.
            PostgisError::OptimisticLockingUnsupported(table) => {
                tellurion_core::Error::CapabilityUnsupported {
                    collection: table,
                    capability: "optimistic-locking".to_string(),
                }
            }

            PostgisError::ItemsVertexBudgetExceeded {
                collection,
                feature_id,
                cumulative_vertices,
                limit,
            } => tellurion_core::Error::ItemsVertexBudgetExceeded {
                collection,
                feature_id,
                cumulative_vertices,
                limit,
            },

            // A declared `id_type` that doesn't match reality — a config
            // mistake, refused the same `Error::Config` way as
            // `PkNotServerAssignable` (`#87`).
            PostgisError::IdTypeMismatch { .. } => tellurion_core::Error::Config(error.to_string()),

            PostgisError::InvalidToken(_)
            | PostgisError::NoDatetimeColumn(_)
            | PostgisError::TileCoordOutOfRange(_)
            | PostgisError::InvalidFeatureId(_)
            | PostgisError::UnsupportedPropertyValue(_)
            | PostgisError::UnwritableProperty(_)
            | PostgisError::TextIdRequired(_)
            | PostgisError::UnsupportedVolumeGeometryType { .. } => {
                tellurion_core::Error::Invalid(error.to_string())
            }

            // The same honest "did not answer in time" the pool's own
            // bounded wait maps to — never an `Ok(None)`, which is what
            // makes an unreachable coordinator refuse leadership instead of
            // granting it (`#193`).
            PostgisError::LeaseCoordinatorTimeout(_) => tellurion_core::Error::Timeout,

            // A bounded pool checkout wait that expired: an honest, fast
            // "no capacity right now", not an internal storage fault.
            PostgisError::Pool(deadpool_postgres::PoolError::Timeout(_)) => {
                tellurion_core::Error::Timeout
            }

            PostgisError::Pool(_)
            | PostgisError::Postgres(_)
            | PostgisError::Join(_)
            | PostgisError::Json(_)
            | PostgisError::InvalidVertexCount(_)
            | PostgisError::MalformedEwkb(_)
            | PostgisError::UnsupportedEwkbGeometryType(_, _) => {
                tellurion_core::Error::Storage(Box::new(error))
            }
        }
    }
}

#[cfg(test)]
mod items_vertex_budget_tests {
    use super::*;

    #[test]
    fn postgis_budget_refusal_preserves_every_structured_field_at_the_core_boundary() {
        let error: tellurion_core::Error = PostgisError::ItemsVertexBudgetExceeded {
            collection: "places".to_string(),
            feature_id: "large".to_string(),
            cumulative_vertices: 60_000,
            limit: 50_000,
        }
        .into();

        assert!(matches!(
            error,
            tellurion_core::Error::ItemsVertexBudgetExceeded {
                collection,
                feature_id,
                cumulative_vertices: 60_000,
                limit: 50_000,
            } if collection == "places" && feature_id == "large"
        ));
    }
}
