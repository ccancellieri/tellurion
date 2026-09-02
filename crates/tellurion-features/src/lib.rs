//! OGC API — Features Part 1 (Core + GeoJSON): axum handlers, driver-agnostic.
//! Every request resolves storage through `tellurion_core::Router` — this
//! crate never names a concrete backend. Landing page and `/conformance` are
//! the server crate's job; it aggregates [`CONFORMANCE_CLASSES`] into its
//! own response.

mod batch_handlers;
mod feed_handlers;
mod handlers;
mod model;
mod params;
mod problem;
mod queryables;
mod router;
mod write_handlers;

use std::sync::LazyLock;

// `#220`: the OGC/Tellurion link relation types this crate's own
// Collection resource already uses, re-exported so the wiring layer's
// cross-protocol link contributors spell each `rel` in exactly one
// place instead of keeping a second copy that could drift.
pub use handlers::{DEFAULT_TENANT, PLACES3D_REL, TILESETS_MAP_REL, TILESETS_VECTOR_REL};
pub use model::{CollectionSummary, CollectionsResponse, FeatureCollectionResponse, Link};
pub use params::{ItemsQueryParams, DEFAULT_LIMIT, MAX_LIMIT};
pub use problem::ApiError;
pub use router::router;
pub use tellurion_core::problem::{Problem, PROBLEM_JSON};

/// OGC API Features Part 1 (1.0) conformance classes this crate satisfies,
/// plus the one Part 3: Filtering (OGC 19-079r2, Approved 1.0) class that is
/// honest for every deployment — `queryables` (the
/// `/collections/{collectionId}/queryables` JSON Schema document, `#33`
/// follow-up, linked from every Collection resource per Requirement 14),
/// served whatever driver backs a collection.
///
/// The rest of Part 2 and Part 3 is implemented here too, but is
/// deployment-dependent and therefore folded in at request time rather than
/// named below (`#217`, the same rule CQL2 and Optimistic Locking already
/// followed):
///
/// - Part 2: CRS by Reference (OGC 18-058r1, Approved 1.0.1) — the
///   `crs`/`bbox-crs` query parameters on `/items`/`/items/{fid}`
///   (Requirements 6/8/9/10/11/12/13), the `Content-Crs` response header
///   (Requirements 15/16), and `storageCrs`/`crs` on every Collection
///   resource (Requirements 2/3/4). Every one of those is a no-op offer on a
///   driver that cannot reproject (`FeatureSource::crs_capable`, `false`
///   everywhere but PostGIS): such a collection advertises exactly one CRS —
///   the one it is genuinely served in, which is CRS84 for a 4326 storage
///   and the storage CRS itself for a projected one (`#227`) — and refuses
///   every other value with a 400. `tellurion_core::Router::
///   crs_conformance_classes` folds the class in only where every configured
///   features driver can honour a real negotiation.
/// - Part 3's `filter`/`features-filter` (the `filter`/`filter-lang` query
///   parameters on `/items`) and `queryables-query-parameters` (clause 7 of
///   19-079r2, Requirement 4, `/req/queryables-query-parameters/parameters`
///   — verified against the published text: "For every queryable of a
///   feature collection that has a simple value (string, number, integer or
///   boolean), the collection SHALL support a query parameter at path
///   `/collections/{collectionId}/items` with the same schema as the schema
///   of the queryable. If the query parameter is provided in a request, the
///   response SHALL only include resources that match the provided value for
///   the queryable" — implemented in `params::build_queryable_filter`,
///   `#52`). Both surfaces ride one capability gate
///   (`FeatureSource::filter_capable`), and FlatGeobuf, GeoParquet and the
///   memory driver answer 400 to either, so `tellurion_core::Router::
///   filtering_conformance_classes` folds these three per deployment.
/// - Part 3's `filter-crs` (Requirement 8, `/req/filter/filter-crs-param`,
///   `#217`), whose own condition is "Server supports additional coordinate
///   reference systems" — so it becomes binding exactly where Part 2's
///   `crs_capable` is `true`. `filter-crs` resolves through the same
///   `crs::resolve` seam as `crs`/`bbox-crs` (`params::resolve_items_crs`);
///   a value naming this collection's own storage CRS is honoured by a
///   driver that declares `FeatureSource::filter_crs_capable` (PostGIS
///   transforms the filter's spatial literals in SQL) and refused with a
///   400 naming `filter-crs` by every driver that does not — never accepted
///   and quietly evaluated in a different CRS. Omitting the parameter is
///   Requirement 7 (`/req/filter/filter-crs-wgs84`), the CRS84 default every
///   compiler here already implemented, and compiles byte-for-byte as it did
///   before `#217`.
///
/// Also implemented: the CQL2 (1.0, OGC 21-065r2) conformance classes for the operator/
/// encoding subset this crate actually parses, validates, and compiles.
/// Those CQL2 classes are not listed again here: this crate's `filter`
/// module re-exports `tellurion_core::filter::Filter` with no
/// protocol-specific carve-out of operators, so [`CONFORMANCE_CLASSES`]
/// composes its own CQL2 declaration per request, not at compile time
/// (`#105`): the classes a `filter`/`filter-lang` query parameter can
/// actually satisfy depend on which driver backs the collection being
/// queried, so no static list here could ever be honest about all of them
/// at once. [`CONFORMANCE_CLASSES`] below therefore never mentions CQL2 at
/// all — the two places that do are `tellurion_core::descriptor::canonical::
/// CanonicalCapabilities::cql2_conformance_classes` (this collection's own,
/// resolved from the driver backing its features lane — surfaced by
/// `handlers::collection_summary` as `CollectionSummary::
/// cql2_conformance_classes`) and `tellurion_core::Router::
/// cql2_conformance_classes` (the workspace-wide intersection across every
/// driver this deployment has configured, folded into this crate's own
/// `/conformance` response by `tellurion-server::landing::
/// conformance_classes` — see that function's own doc). Every driver
/// declares its own subset of `tellurion_core::filter::
/// CQL2_CONFORMANCE_CLASSES` (the full set the shared parser/compiler could
/// ever satisfy) through
/// `tellurion_core::storage::FeatureSource::cql2_conformance_classes`; see
/// that method's own doc, and `tellurion_core::filter`'s module doc, for the
/// requirement IDs each class cites.
///
/// Deliberately out of scope for every driver, so never declared anywhere:
/// accent-insensitivity, arrays, functions, arithmetic, and
/// property-property comparisons — see `tellurion_core::filter`'s module
/// doc for why those stay out of scope.
///
/// Also implemented, and — since `#263` — never named here at all: OGC API
/// Features — Part 4: Create, Replace, Update, Delete (OGC 20-002r1,
/// currently `1.0.0-draft.2`, "draft for Public Comment" — still a draft,
/// not yet an OGC Standard; verified 2026-08 against the published document
/// at `https://docs.ogc.org/DRAFTS/20-002r1.html`). That document's own
/// Table 2 ("Conformance class URIs") lists five classes, not the per-verb
/// split an earlier pass through this crate assumed: Create/Replace/Delete
/// are one combined requirements class, Update (PATCH) is a second,
/// Optimistic Locking is split into two (Timestamps and ETags), and
/// Features is a fifth covering feature-specific requirements (CRS handling
/// on write, the JSON/GML body id rules, feature schemas) layered on top of
/// the first. All five are now contributed at runtime; every one of them
/// depends on a fact this static, workspace-wide list cannot know.
///
/// - `http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/create-
///   replace-delete` — `write_handlers::create_item`/`put_item`/
///   `delete_item` implement every numbered Requirement in that class's own
///   clause 6: `POST`/`PUT`/`DELETE` at the resources/resource endpoints,
///   `201`+`Location` on create, `200`/`204` on replace, `200`/`204` on
///   delete, a real `OPTIONS` response naming the allowed methods
///   (`tellurion-server`'s `respond_to_plain_options_on_write_resources`),
///   and the `If-Match`-on-a-missing-resource guard (Requirement 12 clause
///   B — `write_handlers::refuse_if_match_on_missing_resource`) that keeps
///   this endpoint's upsert-by-caller-supplied-id `PUT` from silently
///   treating a guarded update as an insert.
///
///   Implementing all of that is not the same as a given deployment
///   honouring it, which is why this class left the static list in `#263`.
///   Requirement 1 clause A is "A server SHALL implement one or more of the
///   methods HTTP POST, PUT and/or DELETE for each mutable resource", and
///   whether a deployment has a mutable resource at all is a routing fact:
///   a collection is offered as mutable only by declaring `routing.write`,
///   and only if that lane resolves to a real `WriteSink`. A read-only
///   deployment — the live Italy demo is one — refuses every one of those
///   three methods on the very URIs this class is about, and `#208` already
///   narrowed its `Allow` to `GET, OPTIONS` accordingly, so a static
///   declaration here promised a requirements class the same server
///   declines in full. `tellurion_core::Router::
///   create_replace_delete_conformance_classes` earns it per deployment
///   instead, from the same `write_lane_resolves` predicate `Router::
///   resolve_write` enforces; see that method's own doc for why the
///   quantifier is "every resource offered as mutable" rather than "any" or
///   "every collection".
///
/// The `features` class is likewise not static: its CRS requirements depend
/// on the resolved write driver and collection storage CRS.
/// `Router::features_write_conformance_classes` contributes it at runtime
/// only when every writable collection in the deployment earns it — and,
/// since `#263`, only where `create-replace-delete` is itself declared,
/// which is what clause 9.1's Dependency row on Requirements Class
/// "Create/Replace/Delete" demands.
///
/// The `update` class is also implemented but deliberately not static:
/// `write_handlers::patch_item` accepts `application/merge-patch+json`,
/// applies RFC 7396, preserves the path id, validates the final Feature and
/// schema, and returns the committed representation with validators.
/// `WriteSink::update_conformance_classes` declares it per synchronous
/// driver, while `Router::update_conformance_classes` requires the actual
/// routed read/write pair to support a coherent read-modify-write cycle.
///
/// The remaining two — Optimistic Locking's `req/optimistic-locking-etags`
/// and `req/optimistic-locking-timestamps` (`#107`) — are now genuinely
/// implemented (`handlers::get_item` sets `ETag`/`Last-Modified`;
/// `write_handlers::evaluate_write_preconditions` evaluates `If-Match`/
/// `If-Unmodified-Since` on `PUT`/`DELETE`, building on — not replacing —
/// the `If-Match`-on-missing-resource guard `create-replace-delete` already
/// claims above), but, like the CQL2 classes below, neither is ever baked
/// into this static list:
///
/// - ETags depends on which driver backs a request's collection (the guard
///   needs `WriteSink::locking_conformance_classes` AND a resolving read
///   lane; since `#150` that declaration is earned by a driver being able to
///   re-verify the precondition INSIDE its own write transaction, not merely
///   by committing synchronously) —
///   `tellurion_core::Router::locking_conformance_classes`'s
///   per-deployment intersection folds it into a deployment's
///   `/conformance` response instead (`tellurion-server::landing::
///   conformance_classes`, the same seam CQL2 already uses), and a
///   collection's own true answer surfaces on
///   `CollectionSummary::locking_conformance_classes` — see that field's
///   own doc.
/// - Timestamps depends on whether a *specific collection* declared a real
///   `modified_column` (`tellurion_core::config::CollectionDecl::
///   modified_column`) — a workspace-wide static list can never be honest
///   about a per-collection config fact, so it only ever appears on
///   `CollectionSummary::locking_conformance_classes` too, never here and
///   never in the workspace-wide fold (see `tellurion_core::locking`'s own
///   module doc for why this class specifically has no driver-level
///   concept at all).
///
/// Neither locking class gained a write-capability gate in `#263`, and that
/// is a verdict rather than an omission. The two families are not symmetric
/// in 20-002r1. Create/Replace/Delete's Requirement 1 clause A is an
/// unconditional obligation to *implement* a method ("A server SHALL
/// implement one or more of the methods HTTP POST, PUT and/or DELETE for each
/// mutable resource"), which a read-only deployment fails outright. The ETags
/// class's only unconditional requirement is Requirement 29
/// (`/req/optimistic-locking-etags/get-etag-response`): "The response to a
/// HTTP GET operation used to retrieve a representation of a resource SHALL
/// include an `ETag` header representing the state of the resource as
/// determined at the conclusion of handing the request" — which
/// `handlers::get_item` honours whether or not anything is writable. Its
/// remaining requirements are about `PUT` (Requirements 30, 32, 33) or are
/// conditioned on the Update class (31, 34, 35), and Timestamps has the same
/// shape around Requirement 22. So a read-only deployment declaring them is
/// not promising an operation it refuses; it is describing validators it
/// really emits. Update, by contrast, IS gated: its Requirement 18
/// (`/req/update/update-patch-op`) clause A is unconditional — "For every
/// resource in a collection, the server SHALL support the HTTP PATCH
/// operation" — which is why `Router::update_conformance_classes` already
/// claims nothing where no collection is writable.
///
/// ## Why `conf/core` stays static even after `#255`
///
/// `#255` turned one request from a `200` into a named `400`: a `bbox` with no
/// `bbox-crs`, against a collection whose storage is not CRS84, under a driver
/// that cannot transform between the two. `bbox` is Part 1 Core (Requirement
/// 23, `/req/core/fc-bbox-definition`), so that refusal is a per-collection gap
/// in this very class — and it is deliberately not folded away here, for the
/// reason `tellurion_core::Router::filtering_conformance_classes` spells out at
/// length for Part 3's Requirement 7.
///
/// The gap is a fact about a *collection's* storage SRID, derived from the
/// backend at request time and not knowable from a `StorageDriver` — while this
/// list is workspace-wide and static. Folding on the driver capability alone
/// would strip Core from every GeoPackage deployment on earth, including the
/// overwhelming majority whose collections are all CRS84 and whose `bbox` has
/// always been exactly right. And what the `200` it replaced actually returned
/// was a result set violating Requirement 24 (`/req/core/fc-bbox-response`)
/// clause A — degrees compared against metres, with nothing in the response a
/// client could detect it by — so the alternative to the refusal is not
/// conformance, it is an undetectably wrong answer. The collection also stays
/// queryable by bounding box: `crs::advertised_crs` lists its own storage CRS,
/// and a `bbox-crs` naming it is served with no transform at all, which is what
/// the refusal's own detail points the client at.
pub static CONFORMANCE_CLASSES: LazyLock<Vec<&str>> = LazyLock::new(|| {
    vec![
        "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core",
        "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/oas30",
        "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/geojson",
        "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/queryables",
    ]
    // CQL2 (1.0) classes, all five Part 4 classes (`#107`, `#263`), and —
    // since `#217` — Part 2's `conf/crs` and Part 3's `conf/filter`/
    // `conf/features-filter`/`conf/queryables-query-parameters` are never
    // baked in here. See this constant's own doc above: all of these depend
    // on facts a static, workspace-wide list can never be honest about
    // (which driver backs a request's collection; whether a specific
    // collection declared a `modified_column`; whether this deployment
    // offers any mutable resource at all), so
    // `tellurion-server::landing::conformance_classes` folds
    // `Router::cql2_conformance_classes`/`Router::locking_conformance_classes`/
    // `Router::crs_conformance_classes`/`Router::filtering_conformance_classes`/
    // `Router::create_replace_delete_conformance_classes` in at request time
    // instead of this static list ever naming any of them.
    // `conf/queryables` above stays static on purpose: that document is
    // served for every collection whatever its driver.
});

#[cfg(test)]
mod tests {
    use super::*;

    /// The one Part 3 class that is always honest: the queryables document is
    /// served for every collection regardless of driver, so unlike its three
    /// siblings (`#217`) it stays static.
    #[test]
    fn conformance_classes_declare_the_always_served_queryables_document() {
        assert!(CONFORMANCE_CLASSES
            .contains(&"http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/queryables"));
    }

    /// `#105`: this crate's own static conformance list never bakes in any
    /// CQL2 class, including `basic-spatial-functions` (declared
    /// unconditionally here before this issue) and `case-insensitive-
    /// comparison` (never declared, `#106`) — every CQL2 class this crate's
    /// `/conformance` response carries is folded in at request time by
    /// `tellurion-server::landing::conformance_classes`, from
    /// `tellurion_core::Router::cql2_conformance_classes`'s per-deployment
    /// intersection. Pinning that here guards against a future change
    /// accidentally reintroducing a hand-typed CQL2 URI into this static
    /// list, which could never be honest across every configured driver the
    /// way the dynamic computation is.
    #[test]
    fn conformance_classes_never_bakes_in_any_cql2_class_statically() {
        for class in tellurion_core::filter::CQL2_CONFORMANCE_CLASSES {
            assert!(
                !CONFORMANCE_CLASSES.contains(class),
                "statically claims a CQL2 class that must only ever be computed \
                 per deployment: {class}"
            );
        }
        assert!(!CONFORMANCE_CLASSES
            .contains(&"http://www.opengis.net/spec/cql2/1.0/conf/case-insensitive-comparison"));
    }

    /// `#217`, the direct mirror of the CQL2 test above for OGC API Features
    /// Part 2: `conf/crs` is only honest where a driver can actually
    /// reproject, so it may only ever reach a `/conformance` response through
    /// `tellurion_core::Router::crs_conformance_classes`'s per-deployment
    /// fold, never from this static list. A deployment whose drivers each
    /// advertise a single CRS per collection would otherwise claim a
    /// negotiation every one of its collections refuses with a 400.
    #[test]
    fn conformance_classes_never_bakes_in_the_part_2_crs_class_statically() {
        for class in tellurion_core::crs::CRS_CONFORMANCE_CLASSES {
            assert!(
                !CONFORMANCE_CLASSES.contains(class),
                "statically claims a Part 2 CRS class that must only ever be computed \
                 per deployment: {class}"
            );
        }
    }

    /// `#217`, the same mirror for OGC API Features Part 3's query-parameter
    /// classes: FlatGeobuf, GeoParquet and the memory driver answer 400 to
    /// any `filter` or queryable query parameter, so these three may only
    /// reach a `/conformance` response through `tellurion_core::Router::
    /// filtering_conformance_classes`'s per-deployment fold.
    /// `conf/queryables` is deliberately not in that set — see
    /// `conformance_classes_declare_the_always_served_queryables_document`.
    #[test]
    fn conformance_classes_never_bakes_in_any_part_3_filtering_class_statically() {
        for class in tellurion_core::filter::FILTERING_CONFORMANCE_CLASSES {
            assert!(
                !CONFORMANCE_CLASSES.contains(class),
                "statically claims a Part 3 filtering class that must only ever be computed \
                 per deployment: {class}"
            );
        }
    }

    /// `#107`: same "computed per deployment/collection, never baked in
    /// statically" rule as CQL2's own test above, for Optimistic Locking's
    /// two classes. Both ARE satisfied by this crate now — see
    /// `conformance_classes_never_bakes_in_any_part_4_class_statically`'s
    /// own doc — just never through this constant.
    #[test]
    fn conformance_classes_never_bakes_in_either_locking_class_statically() {
        assert!(
            !CONFORMANCE_CLASSES.contains(&tellurion_core::locking::OPTIMISTIC_LOCKING_ETAGS_CLASS)
        );
        assert!(!CONFORMANCE_CLASSES
            .contains(&tellurion_core::locking::OPTIMISTIC_LOCKING_TIMESTAMPS_CLASS));
    }

    /// Asserts the declared set exactly — the "conformance document test"
    /// this lane's spec asks for: no class beyond what's cited above is
    /// claimed.
    #[test]
    fn conformance_classes_declares_exactly_the_expected_set_no_more_no_less() {
        let expected = [
            "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core",
            "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/oas30",
            "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/geojson",
            "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/queryables",
        ];
        assert_eq!(CONFORMANCE_CLASSES.len(), expected.len());
        for class in expected {
            assert!(
                CONFORMANCE_CLASSES.contains(&class),
                "missing conformance class: {class}"
            );
        }
    }

    /// Never claim the classes this crate's own module doc says are out of
    /// scope (accent-insensitivity, arrays, functions, arithmetic,
    /// property-property comparisons).
    #[test]
    fn conformance_classes_never_claim_out_of_scope_classes() {
        let out_of_scope = [
            "http://www.opengis.net/spec/cql2/1.0/conf/accent-insensitive-comparison",
            "http://www.opengis.net/spec/cql2/1.0/conf/array-functions",
            "http://www.opengis.net/spec/cql2/1.0/conf/property-property",
            "http://www.opengis.net/spec/cql2/1.0/conf/functions",
            "http://www.opengis.net/spec/cql2/1.0/conf/arithmetic",
        ];
        for class in out_of_scope {
            assert!(
                !CONFORMANCE_CLASSES.contains(&class),
                "wrongly claims: {class}"
            );
        }
    }

    /// `#263`, the direct mirror of the CQL2/Part 2/Part 3 tests above for
    /// OGC API Features — Part 4, and the test that replaces the one which
    /// used to pin `create-replace-delete` INTO this list.
    ///
    /// The five entries below are OGC 20-002r1's Table 2 in full, so the
    /// assertion is "no Part 4 class, whichever one" rather than a hand-kept
    /// subset that a sixth class could slip past. Every one of them is
    /// implemented by this crate's `write_handlers` and every one of them is
    /// earned per deployment or per collection: `create-replace-delete` from
    /// `Router::create_replace_delete_conformance_classes` (a deployment
    /// with no mutable resource honours none of Requirement 1 clause A),
    /// `features` from `Router::features_write_conformance_classes`,
    /// `update` from `Router::update_conformance_classes`, and the two
    /// Optimistic Locking classes as
    /// `conformance_classes_never_bakes_in_either_locking_class_statically`
    /// above already pins.
    #[test]
    fn conformance_classes_never_bakes_in_any_part_4_class_statically() {
        for class in [
            tellurion_core::outbox::CREATE_REPLACE_DELETE_CONFORMANCE_CLASS,
            tellurion_core::outbox::UPDATE_CONFORMANCE_CLASS,
            tellurion_core::outbox::FEATURES_PART4_FEATURES_CLASS,
            tellurion_core::locking::OPTIMISTIC_LOCKING_ETAGS_CLASS,
            tellurion_core::locking::OPTIMISTIC_LOCKING_TIMESTAMPS_CLASS,
        ] {
            assert!(
                !CONFORMANCE_CLASSES.contains(&class),
                "statically claims a Part 4 class that must only ever be computed \
                 per deployment: {class}"
            );
        }
    }
}
