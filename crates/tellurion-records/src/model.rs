//! Response DTOs for the read-only OGC API — Records surface. Field order in
//! each struct is JSON key order (serde serializes struct fields in
//! declaration order), the same convention `tellurion_features::model`
//! documents.
//!
//! Deliberately small. A catalog here carries the properties this crate can
//! source from the one `CanonicalDescriptor` every projection reads (`#50`)
//! and nothing more — no fabricated `created`/`updated`, no invented
//! `keywords`, no placeholder `license`. OGC API — Records — Part 1: Core
//! Table 11 marks every catalog property except `id`, `type` and `links` as
//! optional, and Permission 7 (`/per/record-collection/additional-properties`)
//! makes the absent ones absent rather than wrong.

use serde::Serialize;
use serde_json::Value;

/// A link in a catalog's or record's `links` array. Structurally identical
/// to `tellurion_features::Link`'s core three fields; a separate type
/// because no protocol crate in this workspace depends on another (see each
/// crate's own `Cargo.toml` — they all depend on `tellurion-core` alone),
/// the same duplication `tellurion_styles::model::Link` already carries.
#[derive(Debug, Clone, Serialize)]
pub struct Link {
    pub href: String,
    pub rel: String,
    #[serde(rename = "type")]
    pub media_type: String,
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
        }
    }
}

/// The spatial half of a catalog's `extent`, in the shape OGC API — Features
/// — Part 1: Core defines and Records inherits (Table 11's NOTE: "The
/// properties `id`, `itemType`, `title`, `description`, `extent`, `crs` and
/// `links` are inherited from OGC API — Features — Part 1: Core").
#[derive(Debug, Clone, Serialize)]
pub struct SpatialExtent {
    pub bbox: Vec<[f64; 4]>,
    pub crs: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Extent {
    pub spatial: SpatialExtent,
}

/// One catalog (an OGC API — Records "record collection"): the Records
/// projection of the same `CanonicalDescriptor` the Features and STAC
/// projections read.
#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    /// Requirement 11 via Table 11: required. This collection's external id,
    /// exactly as a client would type it into a URL — an internal id never
    /// reaches the wire (`#39`).
    pub id: String,
    /// Table 11: required, fixed value `"Collection"`.
    #[serde(rename = "type")]
    pub type_: &'static str,
    /// Requirement 12 (`/req/record-collection/itemType`): the fixed string
    /// `"record"` for a catalog that homogeneously references records.
    #[serde(rename = "itemType")]
    pub item_type: &'static str,
    /// Table 11: optional. Omitted rather than defaulted to the id — a title
    /// nobody declared is not a title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Table 11: optional. `None` when the backing store reported no spatial
    /// extent, which for a geometry-less record collection is the normal
    /// case — never a fabricated whole-Earth bbox.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extent: Option<Extent>,
    /// Table 11: optional. Carried from the collection's declared
    /// `stac.license` (the descriptor's declared-metadata block — see
    /// `StacConf`'s own doc for why that block's name is historical), absent
    /// when no level declared one. Requirement 7
    /// (`/req/record-core/license`, applied to a catalog by Requirement 14)
    /// requires an SPDX identifier here; this crate passes the operator's
    /// declared value through unchanged rather than validating it, the same
    /// treatment the STAC projection gives the same field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Table 11: optional. The collection's declared `stac.keywords`,
    /// omitted entirely when empty rather than serialized as `[]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Table 11: required. Always carries a `self` link (Requirement 22,
    /// `/req/record-collection/links`) and an `items` link (Requirement 16,
    /// `/req/record-collection/links-records`).
    pub links: Vec<Link>,
}

/// `GET /collections` — the list of catalogs this Records root serves.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogsResponse {
    pub links: Vec<Link>,
    pub collections: Vec<Catalog>,
}

/// `GET /collections/{cid}/items` — a page of records, as a GeoJSON
/// FeatureCollection. The `numberMatched`/`numberReturned` members are OGC
/// API — Features — Part 1: Core's, which Records inherits through
/// Requirement 35 (`/req/records-api/resource-name-mapping`).
#[derive(Debug, Clone, Serialize)]
pub struct RecordsResponse {
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub features: Vec<Value>,
    #[serde(rename = "numberMatched", skip_serializing_if = "Option::is_none")]
    pub number_matched: Option<u64>,
    #[serde(rename = "numberReturned")]
    pub number_returned: u64,
    pub links: Vec<Link>,
}
