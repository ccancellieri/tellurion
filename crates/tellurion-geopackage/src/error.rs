//! Internal error type for this crate — wraps identifier validation
//! failures, GeoPackage Binary (GPB) decode failures, and every fallible
//! `rusqlite`/background-task dependency. Converted to `tellurion_core::
//! Error` at the trait-impl boundary in `driver.rs`; nothing outside this
//! crate ever sees `GeopackageError` directly.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GeopackageError {
    #[error(
        "invalid identifier '{0}': only ASCII letters, digits, and '_' are allowed, and it may not start with a digit"
    )]
    InvalidIdentifier(String),

    #[error("keyset token '{0}' is not valid for this collection (v0.1 requires an integer primary key)")]
    InvalidToken(String),

    #[error(
        "collection '{0}' has no datetime column configured but a datetime filter was supplied"
    )]
    NoDatetimeColumn(String),

    /// A `Mutation::feature_id` that doesn't parse as the v0.1 integer
    /// primary key this collection expects.
    #[error(
        "feature id '{0}' is not valid for this collection (v0.1 requires an integer primary key)"
    )]
    InvalidFeatureId(String),

    /// A feature property whose JSON value is an array or object — outside
    /// the flat scalar model the write path stores a column value as.
    #[error(
        "property '{0}' has an array/object value, which this write path cannot store in a column"
    )]
    UnsupportedPropertyValue(String),

    /// A feature property that names no real column of this collection's
    /// table — neither declared in `CollectionDecl::schema` nor reported by
    /// a live catalog lookup.
    #[error("property '{0}' does not name a column of this collection")]
    UnwritableProperty(String),

    /// The per-collection outbox table this collection's write lane needs is
    /// absent. Named and distinct from an ordinary SQLite error so it maps
    /// to a clear, actionable message rather than a raw "no such table" —
    /// the server never creates this table itself; provisioning is
    /// `tellurion-ingest geopackage create-tables`'s job.
    #[error("outbox table '{0}' does not exist; provision it with `tellurion-ingest geopackage create-tables`")]
    OutboxTableMissing(String),

    /// The outbox table exists but predates `#141`/`#142`'s `extent_crs84`
    /// column, which every write and every drain now names. Named for the
    /// same reason `OutboxTableMissing` is, and refused rather than worked
    /// around: rerunning `tellurion-ingest geopackage create-tables` adds the
    /// column in place (it is idempotent), and quietly writing obligations
    /// without it would leave every tile-invalidation decision downstream
    /// falling back to whole-collection bumps with nothing anywhere saying so.
    #[error("outbox table '{0}' has no extent_crs84 column; re-provision it with `tellurion-ingest geopackage create-tables`")]
    OutboxExtentColumnMissing(String),

    #[error("outbox returned invalid negative sequence {0}")]
    OutboxSequenceInvalid(i64),

    /// The opened file has no `gpkg_contents` table (either not a
    /// GeoPackage at all, or a GeoPackage that was never provisioned by
    /// `tellurion-ingest geopackage create-tables`) — the driver-wide
    /// "unprovisioned or non-GeoPackage file" refusal named in the crate's
    /// own top-level docs.
    #[error(
        "'{0}' is not a provisioned GeoPackage (no gpkg_contents table); provision it with `tellurion-ingest geopackage create-tables`"
    )]
    NotAGeoPackage(String),

    /// A collection's declared table has no `gpkg_geometry_columns`/
    /// `gpkg_contents` entry, or has no single-column `INTEGER PRIMARY KEY`
    /// this driver's v0.1 keyset paging can use.
    #[error("collection '{0}' is not a provisioned feature table in this GeoPackage")]
    CollectionNotProvisioned(String),

    /// A collection declares a primary-key value-space (`CollectionDecl::
    /// id_type`, `#87`) other than `Integer`. Unlike PostGIS, this is never
    /// a live, per-table question here: the GeoPackage format itself
    /// mandates an `INTEGER PRIMARY KEY` feature id column (the OGC
    /// GeoPackage spec's own requirement, not a gap this driver could ever
    /// close), so any other declared `id_type` is wrong for every
    /// GeoPackage-backed collection unconditionally. Refused by name, before
    /// any id is parsed or a query built, at the same boundary
    /// (`item_inner`/`write_apply_inner`) an id ever reaches this driver —
    /// rather than silently misreading a UUID/text id string as a failed
    /// integer parse and answering "not found".
    #[error(
        "collection '{0}' declares a non-integer id_type, which the embedded GeoPackage format does not support (its primary key is always INTEGER)"
    )]
    IdTypeUnsupported(String),

    /// A non-geometry column whose SQLite storage class is `BLOB` — outside
    /// this driver's flat, scalar-property read model (`driver.rs`'s own
    /// `value_ref_to_json`), refused by name rather than silently dropped
    /// or lossily stringified.
    #[error("column '{0}' is a BLOB, which this driver cannot represent as a JSON property")]
    UnsupportedColumnValue(String),

    /// A GeoPackage Binary (GPB) geometry BLOB that is too short, carries an
    /// unrecognized magic/version/envelope-indicator byte, or whose WKB body
    /// `geozero` cannot decode (`gpb.rs`).
    #[error("malformed GeoPackage geometry BLOB: {0}")]
    MalformedGeometry(String),

    /// The tiles lane requires a collection's stored SRID to be one this
    /// driver can put on the workspace's WebMercatorQuad tile grid: EPSG:3857
    /// itself (served as-is), or EPSG:4326 (reprojected per-vertex at
    /// tile-encode time via the standard spherical Web Mercator forward
    /// projection — `#89`, `driver.rs`'s `lonlat_to_web_mercator`). Any other
    /// SRID is refused by name rather than silently served as a
    /// geometrically distorted tile — a deliberately narrow scope (no
    /// general source-CRS matrix), not an oversight.
    #[error(
        "collection '{collection}': the tiles lane supports storage SRID 3857 (native) or 4326 (reprojected to Web Mercator); found {found:?}"
    )]
    UnsupportedTileCrs {
        collection: String,
        found: Option<i32>,
    },

    #[error(
        "collection '{collection}': CRS84 writes require storage SRID 4326 (native) or 3857 (reprojected); found {found:?}"
    )]
    UnsupportedWriteCrs {
        collection: String,
        found: Option<i32>,
    },

    /// A CQL2 construct this SQLite dialect cannot faithfully express: one of
    /// the six `S_*` binary spatial predicates beyond `S_INTERSECTS`
    /// (`Filter::Spatial`). Refused by name rather than silently dropped or
    /// approximated by the R*Tree's own coarse bounding-box test.
    #[error("filter uses spatial predicate '{0}', which this driver cannot express beyond a bbox pushdown")]
    SpatialPredicateUnsupported(&'static str),

    /// An `S_INTERSECTS` construct this driver's exact evaluator (`intersects.rs`)
    /// cannot honestly resolve: a query geometry literal outside the 2D
    /// shapes `geo_types` represents (Z/M coordinates, or a GeoJSON payload
    /// that isn't a bare geometry object), a stored row geometry carrying
    /// Z/M, or the predicate sitting beneath an `OR`/`NOT` where a sound
    /// bbox-pushdown pre-filter can't be combined safely — see `sql.rs`'s
    /// own "bbox pushdown" doc. Refused by name rather than silently
    /// answered by the coarse bbox test alone.
    #[error("S_INTERSECTS cannot be evaluated exactly: {0}")]
    IntersectsUnsupported(String),

    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    #[error("background query task failed: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Geozero(#[from] geozero::error::GeozeroError),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, GeopackageError>;

impl From<tellurion_vector_tile::TileEncodeError> for GeopackageError {
    fn from(error: tellurion_vector_tile::TileEncodeError) -> Self {
        Self::MalformedGeometry(error.to_string())
    }
}

impl GeopackageError {
    /// True only for data supplied by one item. Configuration, connection,
    /// outbox, and transaction failures must abort and roll back the chunk.
    pub(crate) fn is_deterministic_batch_refusal(&self) -> bool {
        match self {
            Self::InvalidFeatureId(_)
            | Self::UnsupportedPropertyValue(_)
            | Self::UnwritableProperty(_)
            | Self::MalformedGeometry(_) => true,
            Self::Sqlite(rusqlite::Error::SqliteFailure(error, _)) => {
                error.code == rusqlite::ErrorCode::ConstraintViolation
            }
            _ => false,
        }
    }
}

impl From<GeopackageError> for tellurion_core::Error {
    fn from(error: GeopackageError) -> Self {
        match error {
            GeopackageError::InvalidIdentifier(_)
            | GeopackageError::NotAGeoPackage(_)
            | GeopackageError::CollectionNotProvisioned(_)
            | GeopackageError::OutboxTableMissing(_)
            | GeopackageError::OutboxExtentColumnMissing(_)
            | GeopackageError::OutboxSequenceInvalid(_)
            | GeopackageError::IdTypeUnsupported(_) => {
                tellurion_core::Error::Config(error.to_string())
            }

            GeopackageError::InvalidToken(_)
            | GeopackageError::NoDatetimeColumn(_)
            | GeopackageError::InvalidFeatureId(_)
            | GeopackageError::UnsupportedPropertyValue(_)
            | GeopackageError::UnsupportedColumnValue(_)
            | GeopackageError::UnwritableProperty(_)
            | GeopackageError::UnsupportedWriteCrs { .. }
            | GeopackageError::UnsupportedTileCrs { .. }
            | GeopackageError::SpatialPredicateUnsupported(_)
            | GeopackageError::IntersectsUnsupported(_) => {
                tellurion_core::Error::Invalid(error.to_string())
            }

            GeopackageError::Sqlite(_)
            | GeopackageError::Join(_)
            | GeopackageError::Json(_)
            | GeopackageError::Geozero(_)
            | GeopackageError::Io(_)
            | GeopackageError::MalformedGeometry(_) => {
                tellurion_core::Error::Storage(Box::new(error))
            }
        }
    }
}
