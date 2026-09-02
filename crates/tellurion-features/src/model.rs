//! Response DTOs for OGC API Features Part 1 (Core + GeoJSON). Field order in
//! each struct is JSON key order (serde derive serializes struct fields in
//! declaration order), matching the shapes callers documented.

use serde::Serialize;
use serde_json::Value;

use tellurion_core::ContributedLink;

/// `skip_serializing_if` helper for [`Link::templated`]: a plain `false`
/// stays off the wire entirely, so every link this crate built before the
/// field existed serializes byte-for-byte as it always did.
fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, Serialize)]
pub struct Link {
    pub href: String,
    pub rel: String,
    #[serde(rename = "type")]
    pub media_type: String,
    /// Optional human-readable title (`#186`) — only ever set on links
    /// carried over from a cross-protocol [`ContributedLink`]; omitted from
    /// the wire otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// RFC 6570-style templated href (`#186`) — `true` only for contributed
    /// links whose href carries `{tileMatrix}`-shaped placeholders a client
    /// substitutes; `false` (and off the wire) for every directly
    /// dereferenceable link.
    #[serde(skip_serializing_if = "is_false")]
    pub templated: bool,
}

impl Link {
    pub fn new(
        href: impl Into<String>,
        rel: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            href: href.into(),
            rel: rel.into(),
            media_type: media_type.into(),
            title: None,
            templated: false,
        }
    }
}

/// Maps one cross-protocol contributed link (`#186`) into this crate's own
/// DTO — a straight field carry-over; the [`LinkAnchor`](tellurion_core::
/// LinkAnchor) was already consumed by the caller's filter, so it has no
/// representation here.
impl From<&ContributedLink> for Link {
    fn from(link: &ContributedLink) -> Self {
        Self {
            href: link.href.clone(),
            rel: link.rel.clone(),
            media_type: link.media_type.clone(),
            title: link.title.clone(),
            templated: link.templated,
        }
    }
}

/// OGC API Features collection-metadata `extent.spatial` object. Spatial
/// only for now — a temporal sibling can follow once a collection's
/// `datetime` column is used to derive one (see the driver-contract design
/// doc, `#19`/`#27`).
#[derive(Debug, Clone, Serialize)]
pub struct SpatialExtent {
    /// `[[minx, miny, maxx, maxy]]` — a single-entry outer array per the OGC
    /// API Features collection-metadata shape (a collection may declare
    /// several bboxes for antimeridian-crossing data; this driver never
    /// produces more than one).
    pub bbox: Vec<[f64; 4]>,
    pub crs: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Extent {
    pub spatial: SpatialExtent,
}

/// Per-feature vertex-count summary within a `geometryProfile` response
/// member (`#101`) — field-for-field mirror of
/// `tellurion_core::catalog::VertexStats`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct VertexProfile {
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
    pub max: u64,
    /// Extrapolated total vertex count across the whole collection (sample
    /// mean times the collection's own row estimate) — omitted when no row
    /// estimate was available to extrapolate against, never a guess.
    #[serde(rename = "totalEstimated", skip_serializing_if = "Option::is_none")]
    pub total_estimated: Option<u64>,
}

/// Feature-size percentile summary within a `geometryProfile` response
/// member (`#101`) — area for a polygon-typed collection, length for a
/// line-typed one; every field omitted together for a point-typed or
/// heterogeneous collection, mirroring
/// `tellurion_core::catalog::FeatureSizeStats`'s own "all together or none"
/// rule.
#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct FeatureSizeProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p95: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// This collection's sampled geometry statistics profile (`#101`, HTTP
/// exposure) — see `CollectionSummary::geometry_profile`'s own doc for when
/// this is present versus omitted, and for the schema-extensibility
/// citation that makes this an allowed member at all. `sampleSize`/
/// `computedAt` travel alongside every other stat so a consumer can judge
/// how much confidence to place in the rest — the profile's own design
/// point (see `tellurion_core::catalog::GeometryProfile`'s doc).
#[derive(Debug, Clone, Serialize)]
pub struct GeometryProfileSummary {
    #[serde(rename = "sampleSize")]
    pub sample_size: u64,
    /// RFC 3339, UTC (`Z` offset) — see `handlers::format_rfc3339`.
    #[serde(rename = "computedAt")]
    pub computed_at: String,
    pub vertices: VertexProfile,
    /// Vertices per unit area of the sampled features' own combined
    /// bounding box (native SRID units) — omitted when that bbox has zero
    /// area.
    #[serde(
        rename = "vertexDensityPerArea",
        skip_serializing_if = "Option::is_none"
    )]
    pub vertex_density_per_area: Option<f64>,
    #[serde(rename = "multiPartFraction")]
    pub multi_part_fraction: f64,
    /// Mean ring count per sampled feature, summed across every part of a
    /// multi-part feature — omitted for a collection whose geometry type
    /// has no ring concept (points, lines) or whose column is untyped/mixed
    /// `GEOMETRY`.
    #[serde(rename = "meanRingCount", skip_serializing_if = "Option::is_none")]
    pub mean_ring_count: Option<f64>,
    #[serde(rename = "featureSize")]
    pub feature_size: FeatureSizeProfile,
}

/// Collection summary/detail object. `extent` is `null` when the backend
/// could not derive one (an empty collection, or a derivation failure —
/// see `handlers::collection_extent`), never fabricated.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionSummary {
    pub id: String,
    pub title: String,
    /// `"feature"` exactly when this collection's features lane resolves
    /// (`#287`) — derived from the `FeatureSource` capability that would
    /// have to honour it, and omitted entirely (never `null`, never a
    /// fabricated `"feature"`) for a collection with no `FeatureSource` at
    /// all (a raster COG/Zarr, a tiles-only PMTiles archive): a client that
    /// reads `itemType: "feature"` will try `/items`, and that route
    /// refuses such a collection by name.
    #[serde(rename = "itemType", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    pub extent: Option<Extent>,
    /// This collection's native storage CRS (OGC API Features Part 2
    /// Requirement 3). Two distinct absences, both honest:
    ///
    /// - outer `None` (`#287`) — the features lane doesn't resolve at all,
    ///   so no Part 2 member belongs on this document; omitted entirely,
    ///   the same absent-not-null rule `itemType`/`crs` follow;
    /// - `Some(None)` — the features lane resolves but the storage SRID
    ///   could not be derived (same "never fabricated" rule `extent`
    ///   follows), or naming it would put a URI here that `crs` below does
    ///   not list, which Requirement 4 forbids (`#217`; see
    ///   `tellurion_core::crs::advertised_storage_crs`); serialized as the
    ///   `null` every features-capable collection has always shown.
    #[serde(rename = "storageCrs", skip_serializing_if = "Option::is_none")]
    pub storage_crs: Option<Option<String>>,
    /// Every CRS this collection's `crs`/`bbox-crs` query parameters accept
    /// (Requirement 2) — present exactly when the features lane resolves
    /// (`#287`): those parameters live on `/items`, and a collection with
    /// no `FeatureSource` has no `/items` for any CRS to be honoured on, so
    /// the member is absent entirely (never `[]`) for such a collection.
    /// When present: never empty, and never wider or narrower than what
    /// the request-time gate will actually serve, because both are the same
    /// `tellurion_core::crs::can_serve` rule read from opposite sides (see
    /// `tellurion_core::crs::advertised_crs`).
    ///
    /// Usually `[CRS84_URI]`, plus this collection's own storage CRS when a
    /// `crs_capable` driver backs it. A projected collection under a driver
    /// that cannot reproject is the one case where CRS84 is *not* in this
    /// list (`#227`): such a collection serves its storage CRS and only its
    /// storage CRS, so that is what it names — and a client needing CRS84
    /// gets a 400 rather than metres under a CRS84 `Content-Crs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crs: Option<Vec<String>>,
    /// The CQL2 (1.0) conformance classes this collection's `filter`/
    /// `filter-lang` query parameters actually satisfy (`#105`), resolved
    /// from the driver backing this collection's features lane —
    /// `tellurion_core::descriptor::canonical::CanonicalCapabilities::
    /// cql2_conformance_classes`, not the workspace-wide `/conformance`
    /// list, which only ever advertises the intersection across every
    /// driver this deployment has configured (`Router::
    /// cql2_conformance_classes`'s own doc explains why the landing page
    /// deliberately answers more conservatively than this field can). Named
    /// per collection the same way this crate already scopes `crs` above
    /// per collection rather than declaring one workspace-wide CRS list
    /// (Requirement 2, `/req/crs/fc-md-crs-list`) and the way Part 3 scopes
    /// `queryables` per collection rather than one workspace-wide schema —
    /// this field follows that same established precedent for CQL2
    /// capability rather than inventing a new media type or resource.
    /// Present exactly when the features lane resolves, mirroring
    /// `fold_conformance_classes`' two-sided contract (`#287`): a
    /// collection whose driver serves features but can't compile a CQL2
    /// filter at all (FlatGeobuf, GeoParquet, the memory driver)
    /// participates and honours nothing — `[]`, never silently absent —
    /// while a collection with no `FeatureSource` at all does not
    /// participate in filtering and carries no member here whatsoever.
    #[serde(
        rename = "cql2ConformanceClasses",
        skip_serializing_if = "Option::is_none"
    )]
    pub cql2_conformance_classes: Option<Vec<&'static str>>,
    /// OGC API Features — Part 4, Requirement 38
    /// (`/req/features/collection-endpoint`): present and `true` only when
    /// this collection's `PUT /items/{featureId}` can create a new item
    /// with a caller-supplied id rather than a server-assigned one —
    /// `Some(true)` when `Router::resolve_write` resolves for this
    /// collection, `None` (omitted from the response entirely) otherwise,
    /// the same "never fabricated" rule `extent`/`storage_crs` already
    /// follow: a collection with no write lane never claims this either
    /// way, it simply doesn't say.
    #[serde(
        rename = "supportsNonAutogeneratedResourceIds",
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_non_autogenerated_resource_ids: Option<bool>,
    /// This collection's sampled geometry statistics profile (`#101`) —
    /// present only when `Router::geometry_profile` has one to report,
    /// entirely omitted (not `null`) for a collection whose driver never
    /// overrides `CatalogSource::geometry_profile`, the same "never
    /// fabricated" rule `extent`/`storage_crs`/
    /// `supports_non_autogenerated_resource_ids` above all follow. Not part
    /// of any OGC API Features requirement class: the collection
    /// description schema
    /// (`schemas.opengis.net/ogcapi/features/part1/1.0/openapi/schemas/
    /// collection.yaml`) declares only `id`/`title`/`description`/`links`/
    /// `extent`/`itemType`/`crs` and sets no `additionalProperties: false`,
    /// so under plain JSON Schema semantics it stays open to exactly this
    /// kind of implementation-specific extension member — the same
    /// allowance `cql2ConformanceClasses` (`#105`) already leans on.
    #[serde(rename = "geometryProfile", skip_serializing_if = "Option::is_none")]
    pub geometry_profile: Option<GeometryProfileSummary>,
    /// The OGC API Features — Part 4 (20-002r1 draft) Optimistic Locking
    /// classes this collection genuinely honors right now (`#107`): the
    /// per-collection counterpart of `cql2_conformance_classes` above,
    /// resolved from `tellurion_core::descriptor::canonical::
    /// CanonicalCapabilities::locking_conformance_classes` rather than any
    /// workspace-wide `/conformance` intersection — see that field's own
    /// doc for exactly how the ETags/Timestamps classes are each decided.
    /// Present exactly when the features lane resolves (`#287`), same
    /// two-sided rule as `cql2_conformance_classes`: a features-capable
    /// collection with neither class earned reports `[]` rather than
    /// omitting the field, while a collection with no `FeatureSource` at
    /// all says nothing about locking and carries no member here.
    #[serde(
        rename = "lockingConformanceClasses",
        skip_serializing_if = "Option::is_none"
    )]
    pub locking_conformance_classes: Option<Vec<&'static str>>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CollectionsResponse {
    pub links: Vec<Link>,
    pub collections: Vec<CollectionSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureCollectionResponse {
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub features: Vec<Value>,
    #[serde(rename = "numberMatched", skip_serializing_if = "Option::is_none")]
    pub number_matched: Option<u64>,
    #[serde(rename = "numberReturned")]
    pub number_returned: u64,
    pub links: Vec<Link>,
}
