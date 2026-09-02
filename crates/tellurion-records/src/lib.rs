//! A read-only OGC API — Records surface (`#192`): geometry-less record
//! collections served as a **third projection of the canonical collection
//! descriptor**, alongside Features and STAC.
//!
//! # Why this is a projection, not a second catalog
//!
//! There is exactly one place in this workspace that answers "what is this
//! collection" — `tellurion_core::descriptor::canonical::CanonicalDescriptor`
//! (`#50`), the single read-side merge of backend physical facts, the
//! operator's declared property contract, the operator's declared metadata
//! block, and live capability advertisement. `tellurion-features` projects it
//! into an OGC API Features Collection; `tellurion-stac` projects it into a
//! STAC Collection (and, through `iso19139`, into ISO 19139 XML); this crate
//! projects it into an OGC API — Records catalog. None of the three holds
//! metadata the others cannot see, and none of them re-derives what the
//! descriptor already resolved.
//!
//! The same reuse runs all the way down. A record is read through the
//! collection's ordinary features lane (`Router::resolve_features`), so it
//! inherits that lane's drivers, keyset paging, `#34` authorization and
//! grant filters, `#184` page byte budget, and `#39` tenant/catalog
//! resolution without a line of parallel machinery. A record collection is
//! not a second storage story; it is an ordinary collection whose
//! `CollectionKind` says its rows are records.
//!
//! # What makes a collection a record collection
//!
//! One declared key: `kind: record` on the collection
//! (`tellurion_core::CollectionKind`). Owned by the data model, never by a
//! driver config — a kind stored per backend would silently misclassify the
//! moment a collection's `routing` changed. The kind is what partitions the
//! catalog across protocol roots: the Features root's `/collections` skips
//! record collections, this root's `/collections` serves only them, and
//! STAC's serves every kind.
//!
//! Because `kind` defaults to `vector`, a deployment that never declares one
//! has no record collections, this root serves nothing, and — with
//! `protocols.records` defaulting to `disabled` — the root does not answer
//! at all. Nothing about such a deployment's responses changes.
//!
//! # Conformance
//!
//! This crate declares **no** `ogcapi-records-1` conformance class, and the
//! reasoning per candidate class — with the requirement identifiers behind
//! each refusal — is in [`conformance`]'s own module documentation. Read it
//! before adding one.

/// The conformance stance — read this before adding a class. Public so the
/// per-class refusal rationale (with the OGC requirement identifiers behind
/// each one) is reachable from the rendered documentation, not just from the
/// source.
pub mod conformance;
mod handlers;
mod model;
mod problem;
mod router;

pub use conformance::{
    GEOJSON_MEDIA_TYPE, ITEM_TYPE_RECORD, JSON_MEDIA_TYPE, REL_COLLECTION, REL_ITEMS,
};
pub use handlers::{DEFAULT_CATALOG, DEFAULT_TENANT};
pub use model::{Catalog, CatalogsResponse, Extent, Link, RecordsResponse, SpatialExtent};
pub use problem::ApiError;
pub use router::router;

/// The conformance classes this root cites beyond the OGC API — Common ones
/// every protocol root in this workspace cites — deliberately empty.
///
/// Present as a named, empty constant rather than absent so the server's
/// `landing::conformance_classes` can extend from it exactly the way it
/// extends from `tellurion_features::CONFORMANCE_CLASSES` and
/// `tellurion_stac::CONFORMANCE_CLASSES`, and so that a later slice which
/// genuinely earns a class has one obvious place to add it — with
/// [`conformance`]'s refusal rationale sitting right next to it. See that
/// module's documentation for why each candidate class is withheld.
pub const CONFORMANCE_CLASSES: &[&str] = &[];
