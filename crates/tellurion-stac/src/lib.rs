//! STAC API (slice C: core + collections + items + search, `#36`): axum
//! handlers, driver-agnostic. Every request resolves storage through
//! `tellurion_core::Router` — this crate never names a concrete backend,
//! same zero-database-dependency contract every other protocol crate
//! follows. The STAC Catalog landing page and `/conformance` are the server
//! crate's job; it aggregates [`CONFORMANCE_CLASSES`] into its own
//! response, same split `tellurion-features` uses.

mod asset_handlers;
mod assets;
mod handlers;
mod iso19139;
mod mapping;
mod model;
mod params;
mod problem;
mod projection;
mod router;
mod search;

use std::sync::LazyLock;

pub use handlers::{DEFAULT_CATALOG, DEFAULT_TENANT};
pub use mapping::{to_stac_collection, STAC_VERSION};
pub use model::{
    Link, StacAsset, StacCollection, StacCollectionsResponse, StacExtent,
    StacItemCollectionResponse, StacSpatialExtent, StacTemporalExtent,
};
pub use problem::ApiError;
pub use router::router;

/// STAC API conformance classes this crate satisfies — verified against the
/// latest released `stac-api-spec` tag (`v1.0.0`, the only non-prerelease
/// release as of this writing; conformance class URIs are versioned
/// independently of the `stac_version` field, see [`STAC_VERSION`]):
///
/// - *STAC API - Core* (`https://api.stacspec.org/v1.0.0/core`): the STAC
///   Catalog landing page with an embedded `conformsTo`, `self`/`root`/
///   `service-desc` links, and the `/api` service description — all served
///   by the server crate (`landing.rs`/`openapi.rs`), not this one.
/// - *STAC API - Collections* (`https://api.stacspec.org/v1.0.0/collections`):
///   the `data` link relation on the landing page plus the `/collections`
///   and `/collections/{collectionId}` endpoints this crate implements.
/// - *STAC API - Features* (`https://api.stacspec.org/v1.0.0/ogcapi-features`,
///   `#36` slice B): `/collections/{collectionId}/items` and
///   `/collections/{collectionId}/items/{itemId}` now exist, paginated with
///   `limit`/`bbox`/`datetime` + a keyset `token` through the identical
///   `tellurion_core` query surfaces `tellurion-features` uses, each Item
///   carrying `root`/`self`/`collection`/`parent` links and each response a
///   valid GeoJSON body. Each Collection document carries the `items` link
///   into that resource as of `#245` — both this class and the OGC Part 1
///   Core class below require it by name (see `handlers::ITEMS_REL` for the
///   two quoted requirements), and until that slice this root declared them
///   while emitting only `root`/`self`/`parent`/`alternate`, which was an
///   overclaim of exactly the kind the rest of this doc records refusing.
///   The link is emitted only for a collection whose features lane actually
///   resolves, so it is never itself a dangling promise: a tiles-only
///   collection is still describable here and still has no items resource.
///   This class's own text says it "Includes OGC API -
///   Features Part 1 Core, GeoJSON, and OpenAPI 3.0 conformance classes" as
///   dependencies (verified 2026-07 against `stac-api-spec`'s
///   `ogcapi-features/README.md` at the `v1.0.0` tag) — those three are
///   declared alongside it below, genuinely met at this STAC root the same
///   way `tellurion-features` meets them at its own.
/// - *STAC API - Item Search* (`https://api.stacspec.org/v1.0.0/item-search`,
///   `#36` slice C): `GET`/`POST /search` now exist — `collections`, `ids`,
///   `bbox`, `datetime`, `intersects`, `limit`, and keyset paging, both
///   single-collection and cross-collection (see `handlers::execute_search`'s
///   own doc for the fan-out cost). Verified 2026-07 against
///   `stac-api-spec`'s `item-search/README.md` at the `v1.0.0` tag.
/// - *STAC API - Item Search: Filter Extension*
///   (`https://api.stacspec.org/v1.0.0/item-search#filter`, `#36` slice C):
///   `/search` accepts `filter`/`filter-lang`/`filter-crs`, composed with
///   `intersects` into the same `tellurion_core::filter::Filter` tree
///   `tellurion-features` already compiles for OGC API Features Part 3 — see
///   `handlers::compose_filter`. Verified 2026-07 (and re-verified 2026-08
///   for `#248`) against the `stac-api-extensions/filter` repo's `README.md`
///   at its `v1.0.0-rc.4` tag (the Filter Extension has not reached a
///   non-prerelease release).
///
///   **Not in this crate's static list any more (`#248`).** The class binds
///   *Filter and Basic CQL2* to `/search` — the extension's own words — and
///   Basic CQL2 is already folded per deployment (`#105`, below), so a
///   deployment whose drivers accept no `filter` at all published a
///   self-contradicting document: this class asserting the binding, and no
///   CQL2 class to bind. `tellurion_core::Router::
///   item_search_filter_conformance_classes` now folds it over
///   `FeatureSource::filter_capable`, and
///   `tellurion-server::landing::conformance_classes` adds the survivors, the
///   same way it already adds the CQL2 fold — see that method's own doc.
///
///   The extension's third parameter, `filter-crs`, is genuinely honoured as
///   of `#248` rather than accepted and dropped: `search::
///   resolve_search_filter_crs` accepts CRS84 — "server must only accept
///   `http://www.opengis.net/def/crs/OGC/1.3/CRS84` as a valid value, may
///   reject any others", quoted verbatim — carries it to the driver that
///   compiles the filter's spatial literals, and refuses every other value by
///   name with a 400. Omitting it is the extension's own stated default
///   ("`filter-crs` always defaults to
///   `http://www.opengis.net/def/crs/OGC/1.3/CRS84` for a STAC API") and
///   compiles byte-for-byte what this lane always produced.
///
///   The CQL2 (1.0) classes `/search` genuinely earns are never baked in
///   here either (`#105`): `handlers::compose_filter` feeds
///   `request.filter` straight into `tellurion_core::filter::parse`/
///   `validate` and each resolved collection's own driver compiler — the
///   exact same parser and per-driver compiler set `tellurion-features`
///   uses for its own `filter`/`filter-lang` query parameters, with no
///   protocol-specific carve-out of operators — so which CQL2 classes
///   `/search` can honor depends on which driver backs whichever
///   collection(s) a given search actually reaches, the same reason
///   `tellurion-features`' own `CONFORMANCE_CLASSES` stopped baking in a
///   CQL2 declaration. `tellurion-server::landing::conformance_classes`
///   folds `tellurion_core::Router::cql2_conformance_classes`'s per-
///   deployment intersection into this root's `/conformance` response the
///   same way it does for the Features root.
///
/// Deliberately *not* declared here: the OGC API Features Part 3 classes
/// (`ogcapi-features-3/1.0/conf/queryables`, `.../conf/filter`,
/// `.../conf/features-filter`). Each is withheld for its own reason, stated
/// per class rather than under one blanket sentence (`#248` found the
/// previous blanket rationale was the right reason for the wrong class;
/// re-verified 2026-08 for `#245` against OGC 19-079r2, *OGC API — Features
/// — Part 3: Filtering*, version 1.0.0):
///
/// - *Queryables* — Requirement 4/13 place the resource at
///   `/collections/{collectionId}/queryables`, and this root mounts no such
///   route at all (`router.rs`); `tellurion-features` serves it at its own
///   root and declares the class there.
/// - *Filter* — its own Requirements Class header lists exactly one
///   dependency, "Requirements Class `Queryables`", so it cannot be honest
///   here while the resource above is absent. Note this is NOT the same
///   thing as "`/search` cannot filter": `/search` genuinely can, which is
///   why the STAC Item Search *Filter Extension* class is folded in per
///   deployment instead (see above). The two are separate bindings of the
///   same CQL2 machinery to different resources.
/// - *Features Filter* — the class that binds `filter`/`filter-lang`/
///   `filter-crs` to `/collections/{cid}/items`, which takes none of them in
///   this crate (only `/search` does; see `params.rs`'s own doc). It depends
///   on *Filter*, and its Requirement 13 asks for the Queryables resource
///   too, so both of the reasons above apply to it as well.
///
/// - *Assets and object storage* (`asset_handlers.rs`, this workspace's own
///   proposal — placeholder host until it finds a standards-track home, per
///   the proposal's own "Naming" section): [`ASSET_CONFORMANCE_CLASSES`],
///   covering exactly `core` + `managed-storage` + `direct-upload` +
///   `checksum` + `object-store-profile: fs`, item- and collection-level,
///   on the primary database-backed driver — the proposal's own first-slice
///   scope, no more. [`s3_asset_conformance_classes`],
///   [`resumable_asset_conformance_classes`], and
///   [`download_redirect_asset_conformance_classes`] each declare one
///   further class conditionally, gated on this deployment's own
///   `object_stores` (see each function's own doc). Not declared: any other
///   `object-store-profile` (`gcs`/`azure`) — neither is shipped. The
///   reconcile surface (`asset_handlers.rs`'s own `get_reconcile_report`) is
///   deliberately absent here too: the proposal's own conformance-class
///   table never lists it as one — it names three drift categories under
///   its "Reconcile" step in the upload-protocol description, not a class
///   with its own URI — so there is no honest class to declare for it.
pub static CONFORMANCE_CLASSES: LazyLock<Vec<&str>> = LazyLock::new(|| {
    let mut classes = vec![
        "https://api.stacspec.org/v1.0.0/core",
        "https://api.stacspec.org/v1.0.0/collections",
        "https://api.stacspec.org/v1.0.0/ogcapi-features",
        "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core",
        "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/oas30",
        "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/geojson",
        "https://api.stacspec.org/v1.0.0/item-search",
    ];
    // Neither the CQL2 (1.0) classes (`#105`) nor the Item Search Filter
    // class (`#248`) is baked in here — see this constant's own doc above:
    // `tellurion-server::landing::conformance_classes` folds
    // `Router::cql2_conformance_classes` and
    // `Router::item_search_filter_conformance_classes` in at request time
    // instead, because both are honest only where a driver behind this
    // deployment can actually compile a filter.
    classes.extend(ASSET_CONFORMANCE_CLASSES.iter().copied());
    classes
});

/// One URI family, `.../conf/{class}`, OGC style — see [`CONFORMANCE_CLASSES`]'s
/// own doc for the placeholder-host rationale and exactly what this slice
/// ships. Kept as its own public constant (not just inlined into
/// `CONFORMANCE_CLASSES`) so a future slice can extend this one list
/// without also re-deriving which STAC/OGC-Features/CQL2 classes this crate
/// separately earns.
pub static ASSET_CONFORMANCE_CLASSES: LazyLock<Vec<&str>> = LazyLock::new(|| {
    vec![
        "https://tellurion.dev/spec/assets/1.0/conf/core",
        "https://tellurion.dev/spec/assets/1.0/conf/managed-storage",
        "https://tellurion.dev/spec/assets/1.0/conf/direct-upload",
        "https://tellurion.dev/spec/assets/1.0/conf/checksum",
        "https://tellurion.dev/spec/assets/1.0/conf/object-store-profile/fs",
    ]
});

/// `object-store-profile/s3` + `presigned-upload` (assets-and-object-storage
/// proposal, second slice: `asset_handlers.rs`'s `put_asset_presign`/
/// `get_asset_presign`/`post_asset_finalize`, `tellurion_core::sigv4`/
/// `objectstore::S3ObjectStore`) — declared conditionally, unlike every
/// class in [`ASSET_CONFORMANCE_CLASSES`] above. `object-store-profile/fs`
/// needs only a local directory to be genuinely usable, so this crate
/// already claims it unconditionally; `s3` needs a real endpoint, bucket,
/// and credentials, so claiming it regardless of whether this deployment
/// declared one would overclaim. `has_s3_store` is this crate's caller's
/// own `AppConfig.object_stores` scan (`tellurion-server`'s
/// `landing::conformance_classes`) — this crate is driver-agnostic and
/// never reads config directly (`lib.rs`'s own doc), so it cannot compute
/// this itself.
pub fn s3_asset_conformance_classes(has_s3_store: bool) -> Vec<&'static str> {
    if has_s3_store {
        vec![
            "https://tellurion.dev/spec/assets/1.0/conf/object-store-profile/s3",
            "https://tellurion.dev/spec/assets/1.0/conf/presigned-upload",
        ]
    } else {
        vec![]
    }
}

/// `resumable-upload` (assets-and-object-storage proposal, third slice:
/// `asset_handlers.rs`'s `post_create_upload`/`get_upload_offset`/
/// `patch_append_upload`/`delete_upload`/`post_complete_upload`,
/// `tellurion_core::objectstore::ResumableUploadStore`) — declared
/// conditionally, the same "only claim it when this deployment could
/// actually serve it" rule [`s3_asset_conformance_classes`] follows: the
/// class is true when EITHER profile that could serve it is declared.
/// `ResumableUploadStore` was `fs`-only through the third slice; this slice
/// adds a real S3 multipart-upload implementation
/// (`tellurion_core::objectstore::S3ObjectStore`'s own `impl
/// ResumableUploadStore`), so an `s3`-only deployment now earns this class
/// too, unlike `object-store-profile/fs`/`managed-storage`/`direct-upload`/
/// `checksum` themselves (unconditional in [`ASSET_CONFORMANCE_CLASSES`] —
/// this is deliberately the stricter, config-gated sibling those already
/// were, not a second copy of that looser rule). `has_fs_store`/
/// `has_s3_store` are this crate's caller's own `AppConfig.object_stores`
/// scan (`tellurion-server`'s `landing::conformance_classes`), the same two
/// booleans [`s3_asset_conformance_classes`]/
/// [`download_redirect_asset_conformance_classes`] already compute.
pub fn resumable_asset_conformance_classes(
    has_fs_store: bool,
    has_s3_store: bool,
) -> Vec<&'static str> {
    if has_fs_store || has_s3_store {
        vec!["https://tellurion.dev/spec/assets/1.0/conf/resumable-upload"]
    } else {
        vec![]
    }
}

/// `download-redirect` (assets-and-object-storage proposal, fourth slice:
/// `asset_handlers.rs`'s `get_asset_data`, which answers a `307` to a
/// presigned `GET` URL instead of proxying bytes exactly when the resolved
/// object store has the presigned-URL capability) — declared conditionally,
/// gated on an `s3`-profile store the same way
/// [`s3_asset_conformance_classes`]'s own two classes are: `fs` has no URL
/// space to redirect to (`tellurion_core::ObjectStore::as_presigned`'s own
/// doc), so this class is only true exactly when an `s3` store is declared.
/// `has_s3_store` mirrors that function's own parameter (this crate's
/// caller, `tellurion-server`'s `landing::conformance_classes`, already
/// computes it once and passes it to both).
pub fn download_redirect_asset_conformance_classes(has_s3_store: bool) -> Vec<&'static str> {
    if has_s3_store {
        vec!["https://tellurion.dev/spec/assets/1.0/conf/download-redirect"]
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conformance_classes_declare_stac_api_core_and_collections() {
        let expected = [
            "https://api.stacspec.org/v1.0.0/core",
            "https://api.stacspec.org/v1.0.0/collections",
        ];
        for class in expected {
            assert!(
                CONFORMANCE_CLASSES.contains(&class),
                "missing conformance class: {class}"
            );
        }
    }

    /// `#36` slice B: items now exist, so this crate genuinely earns the
    /// STAC API - Features class plus the OGC API Features Part 1 classes
    /// it depends on.
    #[test]
    fn conformance_classes_declare_stac_api_features_and_its_ogc_dependencies() {
        let expected = [
            "https://api.stacspec.org/v1.0.0/ogcapi-features",
            "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core",
            "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/oas30",
            "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/geojson",
        ];
        for class in expected {
            assert!(
                CONFORMANCE_CLASSES.contains(&class),
                "missing conformance class: {class}"
            );
        }
    }

    /// `#36` slice C: `/search` now exists, so this crate genuinely earns
    /// the STAC API - Item Search class unconditionally — every deployment
    /// that mounts this crate answers `/search` with `collections`/`ids`/
    /// `bbox`/`datetime`/`intersects`/`limit`/paging, none of which depends
    /// on a driver capability.
    #[test]
    fn conformance_classes_declare_stac_api_item_search() {
        assert!(CONFORMANCE_CLASSES.contains(&"https://api.stacspec.org/v1.0.0/item-search"));
    }

    /// `#248`: the Filter Extension's own class is NOT static here. It binds
    /// Filter *and Basic CQL2* to `/search`, and Basic CQL2 has been folded
    /// per deployment since `#105` — so a deployment whose drivers refuse
    /// every `filter` would have published this class next to no CQL2 class
    /// at all. `tellurion_core::Router::item_search_filter_conformance_classes`
    /// folds it instead, and `tellurion-server::landing::conformance_classes`
    /// adds the survivors. Same shape as the CQL2 test below.
    #[test]
    fn conformance_classes_never_bake_in_the_item_search_filter_class() {
        assert!(
            !CONFORMANCE_CLASSES.contains(&tellurion_core::filter::ITEM_SEARCH_FILTER_CLASS),
            "the Item Search Filter class is honest only where a driver can compile a filter, \
             so it must be folded per deployment, never declared statically"
        );
    }

    /// All three OGC API Features Part 3 classes stay undeclared here even
    /// though `tellurion-features` declares them at its own root — this
    /// crate declares the STAC Item Search flavor instead (the previous
    /// test). See `CONFORMANCE_CLASSES`' own doc for the per-class reason:
    /// this root serves no `/collections/{cid}/queryables` route at all
    /// (which `Queryables` requires and `Filter` depends on), and `/items`
    /// takes no `filter` parameter (which `Features Filter` binds).
    #[test]
    fn conformance_classes_do_not_declare_features_part_3_filtering() {
        assert!(!CONFORMANCE_CLASSES
            .contains(&"http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/filter"));
        assert!(!CONFORMANCE_CLASSES
            .contains(&"http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/features-filter"));
        assert!(!CONFORMANCE_CLASSES
            .contains(&"http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/queryables"));
    }

    /// `#105`: neither this crate's nor `tellurion-features`' static
    /// `CONFORMANCE_CLASSES` bakes in any CQL2 class any more — both fold
    /// `tellurion_core::Router::cql2_conformance_classes`'s per-deployment
    /// intersection in at request time (`tellurion-server::landing::
    /// conformance_classes`) instead, since which CQL2 classes `/search`
    /// (this crate) or `/items` (`tellurion-features`) can honor depends on
    /// which driver backs whichever collection a request actually reaches.
    /// Supersedes the pre-`#105` sync-pinning test that once asserted the
    /// two crates declared an *identical* static CQL2 set — there is no
    /// longer a static CQL2 set on either side to keep in sync.
    #[test]
    fn features_and_stac_never_bake_in_any_cql2_class_statically() {
        for class in tellurion_core::filter::CQL2_CONFORMANCE_CLASSES {
            assert!(
                !tellurion_features::CONFORMANCE_CLASSES.contains(class),
                "tellurion-features statically claims a CQL2 class: {class}"
            );
            assert!(
                !CONFORMANCE_CLASSES.contains(class),
                "tellurion-stac statically claims a CQL2 class: {class}"
            );
        }
    }

    #[test]
    fn s3_asset_conformance_classes_is_empty_without_an_s3_store() {
        assert!(s3_asset_conformance_classes(false).is_empty());
    }

    #[test]
    fn s3_asset_conformance_classes_declares_object_store_profile_s3_and_presigned_upload() {
        let classes = s3_asset_conformance_classes(true);
        assert!(
            classes.contains(&"https://tellurion.dev/spec/assets/1.0/conf/object-store-profile/s3")
        );
        assert!(classes.contains(&"https://tellurion.dev/spec/assets/1.0/conf/presigned-upload"));
        // Never bleeds into the always-declared static list — this class
        // stays config-conditional the way `ASSET_CONFORMANCE_CLASSES`'s
        // own classes (declared unconditionally) do not.
        assert!(!CONFORMANCE_CLASSES
            .contains(&"https://tellurion.dev/spec/assets/1.0/conf/object-store-profile/s3"));
    }

    #[test]
    fn resumable_asset_conformance_classes_is_empty_with_neither_profile_declared() {
        assert!(resumable_asset_conformance_classes(false, false).is_empty());
    }

    #[test]
    fn resumable_asset_conformance_classes_declares_resumable_upload_with_an_fs_store() {
        let classes = resumable_asset_conformance_classes(true, false);
        assert!(classes.contains(&"https://tellurion.dev/spec/assets/1.0/conf/resumable-upload"));
        // Config-gated, not unconditional — never bleeds into the always-
        // declared static list.
        assert!(!CONFORMANCE_CLASSES
            .contains(&"https://tellurion.dev/spec/assets/1.0/conf/resumable-upload"));
    }

    /// This slice's own new behavior: `s3` now has a real multipart-upload
    /// `ResumableUploadStore` implementation
    /// (`tellurion_core::objectstore::S3ObjectStore`), so an `s3`-only
    /// deployment (no `fs` store declared at all) earns this class too.
    #[test]
    fn resumable_asset_conformance_classes_declares_resumable_upload_with_only_an_s3_store() {
        let classes = resumable_asset_conformance_classes(false, true);
        assert!(classes.contains(&"https://tellurion.dev/spec/assets/1.0/conf/resumable-upload"));
    }

    #[test]
    fn resumable_asset_conformance_classes_declares_resumable_upload_with_both_stores() {
        let classes = resumable_asset_conformance_classes(true, true);
        assert_eq!(
            classes,
            vec!["https://tellurion.dev/spec/assets/1.0/conf/resumable-upload"],
            "declaring both profiles must not duplicate the class"
        );
    }

    #[test]
    fn download_redirect_asset_conformance_classes_is_empty_without_an_s3_store() {
        assert!(download_redirect_asset_conformance_classes(false).is_empty());
    }

    #[test]
    fn download_redirect_asset_conformance_classes_declares_the_class_with_an_s3_store() {
        let classes = download_redirect_asset_conformance_classes(true);
        assert!(classes.contains(&"https://tellurion.dev/spec/assets/1.0/conf/download-redirect"));
        // Config-gated, not unconditional — never bleeds into the always-
        // declared static list, the same check every other conditional
        // class's own test already makes.
        assert!(!CONFORMANCE_CLASSES
            .contains(&"https://tellurion.dev/spec/assets/1.0/conf/download-redirect"));
    }
}
