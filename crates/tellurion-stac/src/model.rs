//! Response DTOs for STAC API - Core + Collections. Field order in each
//! struct is JSON key order (serde derive serializes struct fields in
//! declaration order). The STAC Catalog landing page itself is not modeled
//! here — it lives in the server crate, same split as every other protocol
//! (`lib.rs`'s doc comment).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use tellurion_core::{ContributedLink, StacProvider};

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
    /// RFC 6570-style templated href (`#186`) — the same member
    /// [`StacAsset`] already serializes for its own placeholder-carrying
    /// hrefs; `false` (and off the wire) for every directly dereferenceable
    /// link.
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

/// STAC Collection `extent.spatial` — always exactly one bbox: this driver
/// never produces the multi-bbox antimeridian-crossing shape the spec
/// allows. No `crs` member: STAC's spatial extent is always WGS84
/// longitude/latitude (CRS84), implied rather than serialized.
#[derive(Debug, Clone, Serialize)]
pub struct StacSpatialExtent {
    pub bbox: Vec<[f64; 4]>,
}

/// STAC Collection `extent.temporal`. See `mapping::to_stac_collection`'s
/// doc for why slice A always produces a single, fully-open
/// `[null, null]` interval.
#[derive(Debug, Clone, Serialize)]
pub struct StacTemporalExtent {
    pub interval: Vec<[Option<String>; 2]>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StacExtent {
    pub spatial: StacSpatialExtent,
    pub temporal: StacTemporalExtent,
}

/// A STAC Collection (`GET /collections/{cid}`) or one entry in the
/// `collections` array of `GET /collections`. `keywords`/`providers` are
/// omitted entirely when empty — an empty array is a needless assertion
/// ("this collection has zero keywords") the STAC Collection spec never asks
/// for, since both fields are optional. `assets` (`#36` slice B, `#48`) is
/// likewise omitted when empty — a collection resolving neither the tiles
/// nor the places3d lane has nothing to materialize there.
#[derive(Debug, Clone, Serialize)]
pub struct StacCollection {
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub stac_version: &'static str,
    /// Extension schema URIs this document genuinely uses (`#36`,
    /// projection) — omitted entirely when empty, so every collection that
    /// emits no extension field serializes byte-for-byte as it did before
    /// the member existed. Declaring an extension while emitting none of
    /// its fields is the `#287` defect and is structurally impossible here:
    /// `mapping::to_stac_collection` fills this and `summaries` from the
    /// same derivation, together or not at all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stac_extensions: Vec<String>,
    pub id: String,
    pub title: String,
    pub description: String,
    pub license: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<StacProvider>,
    pub extent: StacExtent,
    /// STAC Collection `summaries` — the spec's own place for Item
    /// Properties fields at the Collection level, and the only surface a
    /// raster-backed collection (COG/Zarr — no `FeatureSource`, so no
    /// Items) has to state the `proj:*` facts its driver genuinely reads
    /// from its own georeferencing (`#36`). Each entry is the spec's
    /// array-of-unique-values form; omitted entirely when nothing was
    /// derived, so a collection with no projection knowledge serializes
    /// byte-for-byte as before.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub summaries: BTreeMap<String, Value>,
    pub links: Vec<Link>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, StacAsset>,
}

/// One entry in a STAC Collection's `assets` map: either capability-derived
/// (`#36` slice B, `#48` — a materialized link to a lane this deployment
/// already serves for the collection, never a new storage concept) or
/// operator-declared (`#36` slice 1 — `stac.assets` in config, a link this
/// deployment doesn't derive from live routing at all). `href` is the STAC
/// Asset Object spec's only required field; `type`/`title`/`roles` are
/// genuinely optional there, so `media_type`/`title` stay `Option` and
/// `roles` is skipped when empty — absent stays absent on the wire, never a
/// fabricated empty string or `[]` standing in for "not declared" (this
/// codebase's clean-omission-over-fabrication rule). A capability-derived
/// asset always populates all three anyway (see `assets::collection_assets`),
/// so this is a no-op for that path and only actually matters for a
/// declared asset that left one of them unset.
///
/// `href` may be a URI *template* (`{tileMatrix}`/`{tileRow}`/`{tileCol}`
/// placeholders) rather than a literal downloadable URL; `templated` flags
/// that case, the same convention `tellurion_tiles::handlers::tileset`'s own
/// `item` link already uses for the identical situation — the STAC Asset
/// Object spec defines no such field, but JSON objects are extensible, and
/// this documents the non-literal href honestly rather than serving a URL a
/// client would 404 against verbatim. Always `false` for a declared asset —
/// a config-declared `href` is a literal, never a template.
#[derive(Debug, Clone, Serialize)]
pub struct StacAsset {
    pub href: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// STAC's own optional Asset Object `description` (`#221`). `None` for
    /// every asset this crate could already build — a capability-derived
    /// asset has no description to give and `config::AssetDecl` carries no
    /// such field — so adding it changed no existing response byte; it is
    /// populated only by a persisted `AssetRecord`, whose registration
    /// wire contract (`asset_handlers`) has always accepted one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    pub templated: bool,
}

/// `GET /collections/{cid}/items` (`#36` slice B): a STAC ItemCollection —
/// per `stac-api-spec`'s `fragments/itemcollection` (`v1.0.0` tag), this is
/// a plain GeoJSON FeatureCollection (`type`/`features` only required); no
/// `stac_version` at this level — that belongs to each Item in `features`
/// alone. `features` stays `Value` rather than a typed `StacItem`: each
/// entry is the driver's own GeoJSON Feature, mutated in place by
/// `mapping::to_stac_item`, the same dynamic-`Value` approach
/// `tellurion_features::model::FeatureCollectionResponse` already takes for
/// the identical reason (geometry/properties are driver-shaped, not a fixed
/// schema this crate could type).
#[derive(Debug, Clone, Serialize)]
pub struct StacItemCollectionResponse {
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub features: Vec<Value>,
    #[serde(rename = "numberMatched", skip_serializing_if = "Option::is_none")]
    pub number_matched: Option<u64>,
    #[serde(rename = "numberReturned")]
    pub number_returned: u64,
    pub links: Vec<Link>,
    /// External ids of collections this page's cross-collection `/search`
    /// fan-out (`execute_search`'s own doc explains why a fan-out skips a
    /// capability mismatch rather than failing the whole request) left out
    /// because the merged `filter`/`intersects` predicate needed a
    /// capability their driver doesn't have — either compiling a `filter` at
    /// all, or (`#248`) transforming its spatial literals into the CRS84 a
    /// `filter-crs` declared, against a collection whose storage is not
    /// itself CRS84. Not a `stac-api-spec` field —
    /// same "JSON objects are extensible, document the honest gap rather
    /// than stay silent about it" reasoning `StacAsset::templated` already
    /// uses for an identical non-standard-but-honest addition. Empty (and
    /// omitted entirely, per this codebase's absent-stays-absent rule) for
    /// every other response this struct serves: single-collection `/items`
    /// never fans out, and a page that skipped nothing has nothing to
    /// report.
    #[serde(
        rename = "filterIncapableCollections",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub filter_incapable_collections: Vec<String>,
    /// `filterIncapableCollections`' `#181` free-text sibling: external ids
    /// of collections a `q`-bearing fan-out skipped because their search
    /// lane could not serve free text right now — no `routing.search` at
    /// all, an index that failed the freshness gate, or an index source
    /// that never advertised `SearchSource::text_search_capable`. Same
    /// honest-gap rationale as its sibling (a silently incomplete fan-out
    /// is the failure mode both fields exist to prevent), and the same
    /// scope rule: never a collection skipped for an unresolvable id or a
    /// policy denial — those omissions predate `q` and are deliberately not
    /// advertised. Empty and omitted for every `q`-less response.
    #[serde(
        rename = "searchIncapableCollections",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub search_incapable_collections: Vec<String>,
    /// `filterIncapableCollections`' `#255` `bbox` sibling: external ids of
    /// collections a `bbox`-bearing fan-out skipped because the request's
    /// bounding box is CRS84 (Part 1 Requirement 23 clause C — a `bbox` with
    /// no `bbox-crs` is WGS 84 longitude/latitude, and a STAC `bbox` never
    /// carries one), their storage is not, and their driver cannot transform
    /// between the two.
    ///
    /// Its own field rather than an entry in `filterIncapableCollections`,
    /// because that name would then be false: nothing about the request's
    /// `filter` was refused, and reading the two apart is exactly what tells a
    /// client whether dropping the `bbox` or dropping the `filter` would get
    /// that collection back. Same honest-gap rationale and same scope rule as
    /// both siblings — never a collection skipped for an unresolvable id or a
    /// policy denial. Empty and omitted for every `bbox`-less response, and
    /// for every response whose collections are all CRS84-stored, which is
    /// every live deployment.
    #[serde(
        rename = "bboxIncapableCollections",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub bbox_incapable_collections: Vec<String>,
}

/// `GET /collections` — the STAC API - Collections shape: an array of full
/// STAC Collection objects plus the endpoint's own `links` (`root`/`self`).
#[derive(Debug, Clone, Serialize)]
pub struct StacCollectionsResponse {
    pub collections: Vec<StacCollection>,
    pub links: Vec<Link>,
}
