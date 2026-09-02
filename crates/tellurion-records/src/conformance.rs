//! What this crate claims, and — at greater length, because it is the more
//! consequential half — what it deliberately does not.
//!
//! # No `ogcapi-records-1` conformance class is declared
//!
//! OGC API — Records — Part 1: Core is an approved OGC Standard (OGC
//! 20-004r1, version 1.0, approval date 2025-04-07, publication date
//! 2025-05-02 — verified 2026-08 against the published document at
//! `docs.ogc.org/is/20-004r1/20-004r1.html`), so unlike OGC API — Styles and
//! OGC API — 3D GeoVolumes (see `tellurion_places::conformance`, which
//! withholds classes because none has been approved to cite) there *are*
//! real class URIs available here. This crate still declares none of them,
//! for reasons specific to each:
//!
//! - **`.../conf/records-api`** (Requirements class 4) lists Record Core
//!   Query Parameters among its prerequisites, and that class's Requirement
//!   26 (`/req/record-core-query-parameters/q-definition`) makes free-text
//!   `q` a SHALL. This lane implements no `q` at all — the issue that opens
//!   it (`#192`) defers free-text search to the search lane (`#181`) — so
//!   the class cannot be earned. `type`, `ids` and `externalIds`
//!   (Requirements 28-33) are equally unimplemented.
//!
//! - **`.../conf/json`** (Requirements class 8) requires, in Requirement 55
//!   (`/req/json/record-content`, clause B), that "the schema of records in
//!   all responses with the media type `application/geo+json` SHALL validate
//!   against the OpenAPI 3.0 schema document `recordGeoJSON.yaml`" — whose
//!   `properties` member in turn refers to `recordCommonProperties.yaml`,
//!   which types named keys such as `keywords` (array of string), `themes`,
//!   `contacts` and `formats`. A record served here is a row of the
//!   collection's own backing table, projected by whichever driver answers
//!   its features lane: its `properties` are that table's columns, with the
//!   operator's names and the backend's types. A collection with a `TEXT`
//!   column called `keywords` would emit a string where that schema demands
//!   an array. Nothing in this slice can guarantee otherwise, so claiming
//!   the class would be claiming a validity this server cannot enforce.
//!
//! - **`.../conf/record-core`** and **`.../conf/record-collection`**
//!   (Requirements classes 1 and 2) are individually within reach — this
//!   crate's records carry a non-null `id` (Requirement 1,
//!   `/req/record-core/mandatory-properties-record`) and a `collection` link
//!   (Requirement 8, `/req/record-core/links`), and its catalogs carry `id`,
//!   `type: "Collection"`, `links`, a `self` link (Requirement 22), an
//!   `items` link (Requirement 16, `/req/record-collection/links-records`)
//!   and `itemType: "record"` (Requirement 12). But each class also carries
//!   an encoding requirement — Requirement 92
//!   (`/req/record-core/default-mediatype`) and Requirement 93
//!   (`/req/record-collection/default-mediatype`) — whose clause B states
//!   that *if the JSON conformance class is not advertised*, the default
//!   media type for record content SHALL be `text/html` and for catalog
//!   content SHALL be `text/html`. This server serves no HTML
//!   representation of anything. So with the JSON class withheld for the
//!   reason above, these two cannot be claimed either: the pair is
//!   all-or-nothing.
//!
//! The result is that a `/records` root's `/conformance` lists exactly the
//! OGC API — Common classes every protocol root in this workspace lists, and
//! nothing else. That is the same anti-overclaim discipline `#192` asks for
//! in its own words ("advertise only the classes Records Part 1 actually
//! defines" — and only the ones actually honoured), and the same posture
//! `tellurion-places` already takes.
//!
//! # Shapes followed without claiming conformance
//!
//! The resource shapes below still follow the Standard, exactly the way
//! `tellurion-places` follows OGC API — 3D GeoVolumes' URL shape without
//! claiming it: a client that knows Records will find what it expects, and a
//! later slice that closes the gaps above can declare the classes without
//! reshaping a single response.

/// `itemType` for a catalog whose members are records — OGC API — Records —
/// Part 1: Core Requirement 12 (`/req/record-collection/itemType`, clause A:
/// "If a catalog homogeneously references or contains records then, its
/// `itemType` property SHALL be a string with the fixed value of `record`").
/// Every catalog this crate serves references records homogeneously, by
/// construction: the listing is filtered to `CollectionKind::Record`.
pub const ITEM_TYPE_RECORD: &str = "record";

/// Link relation for the endpoint from which a catalog's records are
/// retrieved — Requirement 16 (`/req/record-collection/links-records`): "A
/// link (relation: `items`) SHALL be included in the links section of the
/// catalog pointing to an endpoint for accessing the records of this catalog
/// via an API."
pub const REL_ITEMS: &str = "items";

/// Link relation from a record back to the catalog it belongs to —
/// Requirement 8 (`/req/record-core/links`, clause A). Clause B adds that
/// "Only a single link (relation: `collection`) SHALL be included in a
/// record", which is why this crate appends exactly one and never merges a
/// second from anywhere else.
pub const REL_COLLECTION: &str = "collection";

/// Media type of a record and of a page of records. GeoJSON, because a
/// record *is* a GeoJSON Feature — with a `geometry` member that is
/// routinely `null`, which the Standard explicitly allows (Table 9 lists
/// `geometry` as optional, "Can be null if there is no associated spatial
/// extent"; Permission 4, `/per/record-core/geometry`, leaves making it
/// mandatory to specific communities of interest).
///
/// Deliberately *not* `application/ogc-catalog+json` for catalogs: that
/// media type is Requirement 57's (`/req/json/collection-response`), and
/// serving it while withholding the JSON conformance class that defines it
/// would advertise through a header what this crate declines to advertise in
/// `conformsTo`. Catalogs are served as plain `application/json`, like every
/// other collection-metadata document in this workspace.
pub const GEOJSON_MEDIA_TYPE: &str = "application/geo+json";
pub const JSON_MEDIA_TYPE: &str = "application/json";
