//! OGC API Common: the per-`(tenant, protocol, catalog)` landing page +
//! `/conformance`, the `/{tenant}/` directory doc, and the top-level minimal
//! service descriptor (`#39`). `/api` (the OpenAPI document) lives in
//! `openapi.rs` — this module only owns `/`, `/conformance`, and the two
//! directory-shaped documents above a protocol root.
//!
//! Every href below is built from the request's own `OriginalUri` — never a
//! hardcoded prefix — so the same handler serves correctly regardless of
//! which tenant/catalog it was reached through, and never needs to know an
//! internal id: `tenant`/`catalog` path segments are already the external
//! ids the client typed, and `Router`/`Resolver` are never consulted for
//! `protocol_landing`/`protocol_conformance` at all (they carry no
//! tenant/catalog-specific state — every protocol root's landing page and
//! conformance classes are identical in shape across every tenant/catalog).
//! `tenant_directory` is the one handler here that does resolve: it needs to
//! enumerate the tenant's real catalogs.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, OriginalUri, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use tellurion_core::{AppContext, PageRequest};
use tellurion_features::Link;

use crate::protocol::{Protocol, RootAvailability};

const JSON_MEDIA_TYPE: &str = "application/json";
const OPENAPI_MEDIA_TYPE: &str = "application/vnd.oai.openapi+json;version=3.0";

/// Default/max page size for the tenant directory's catalog listing (`#42`,
/// `#59`) — same values and rationale as `tellurion_features::params::
/// COLLECTIONS_DEFAULT_LIMIT`/`COLLECTIONS_MAX_LIMIT`, kept as this crate's
/// own copy per the "duplicated on purpose" convention every protocol
/// crate's own paging params already follow (see e.g. `tellurion_stac::
/// params`'s module doc for why).
const CATALOGS_DEFAULT_LIMIT: u32 = 100;
const CATALOGS_MAX_LIMIT: u32 = 10_000;

/// `GET /{tenant}/`'s own query parameters (`#59`) — same `limit`/`token`
/// shape every other cursor-paginated listing in this workspace uses.
#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct TenantDirectoryQueryParams {
    pub limit: Option<u32>,
    pub token: Option<String>,
}

/// Builds an href for `path` echoing `params`, with `override_token`
/// substituted for the page token when present (the `next` link case) —
/// the tenant-directory counterpart of `tellurion_features::params::
/// collections_href`.
fn tenant_directory_href(
    path: &str,
    params: &TenantDirectoryQueryParams,
    override_token: Option<&str>,
) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let Some(limit) = params.limit {
        pairs.push(("limit", limit.to_string()));
    }
    let token = override_token
        .map(str::to_string)
        .or_else(|| params.token.clone());
    if let Some(token) = token {
        pairs.push(("token", token));
    }

    if pairs.is_empty() {
        return path.to_string();
    }

    let query = pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(&v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query}")
}

/// Minimal RFC 3986 percent-encoding of query values — same shape every
/// other protocol crate's own href builder in this workspace uses.
fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// OGC API Common Part 1 (Core) conformance classes every protocol root
/// satisfies. No `html` class: this server serves JSON representations
/// only.
const COMMON_CONFORMANCE_CLASSES: &[&str] = &[
    "http://www.opengis.net/spec/ogcapi-common-1/1.0/conf/core",
    "http://www.opengis.net/spec/ogcapi-common-1/1.0/conf/landing-page",
    "http://www.opengis.net/spec/ogcapi-common-1/1.0/conf/json",
    "http://www.opengis.net/spec/ogcapi-common-1/1.0/conf/oas30",
];

/// GET `/{tenant}/{protocol}/catalogs/{catalog}/` — the landing page for one
/// full OGC API root. `self_root` strips any trailing slash so every sibling
/// href below is built consistently regardless of whether the client's
/// request itself had one.
pub async fn protocol_landing(
    Extension(protocol): Extension<Protocol>,
    OriginalUri(uri): OriginalUri,
) -> impl IntoResponse {
    let self_root = uri.path().trim_end_matches('/').to_string();

    let mut links = vec![
        Link::new(self_root.clone(), "self", JSON_MEDIA_TYPE),
        Link::new(
            format!("{self_root}/conformance"),
            "conformance",
            JSON_MEDIA_TYPE,
        ),
        Link::new(
            format!("{self_root}/api"),
            "service-desc",
            OPENAPI_MEDIA_TYPE,
        ),
    ];
    match protocol {
        Protocol::Features => links.push(Link::new(
            format!("{self_root}/collections"),
            "data",
            JSON_MEDIA_TYPE,
        )),
        Protocol::Tiles => links.push(Link::new(
            format!("{self_root}/tileMatrixSets"),
            "tiles",
            JSON_MEDIA_TYPE,
        )),
        Protocol::Styles => links.push(Link::new(
            format!("{self_root}/styles"),
            "styles",
            JSON_MEDIA_TYPE,
        )),
        // 3D places has no global resource of its own to link — every
        // `3dtiles` resource lives under a specific collection, same as the
        // pre-`#39` landing page never linked one either.
        Protocol::ThreeDTiles => {}
        // `#192`: the Records root's own catalog list. `data` is the same
        // relation the Features root uses for `/collections`, which is what
        // OGC API — Common gives a landing page for the resource holding
        // this API's data; a Records catalog is a collection object of
        // exactly that shape (its `itemType` is what distinguishes it — see
        // `tellurion_records::ITEM_TYPE_RECORD`).
        Protocol::Records => links.push(Link::new(
            format!("{self_root}/collections"),
            "data",
            JSON_MEDIA_TYPE,
        )),
        // `#182`: the Processes root's own process list, with the relation
        // OGC API — Processes — Part 1: Core Requirement 2
        // (`/req/core/landingpage-success`) names for it. Spelled as the full
        // OGC relation URI rather than a short token because this arm is new
        // and so costs no other root a byte — unlike the `conformance` link
        // above, which every root shares and whose relation that same
        // requirement would also have this root change. See
        // `tellurion_processes::conformance` for why that asymmetry is
        // recorded rather than resolved.
        Protocol::Processes => links.push(Link::new(
            format!("{self_root}/processes"),
            tellurion_processes::REL_PROCESSES,
            JSON_MEDIA_TYPE,
        )),
        // Unreachable in practice: `app::stac_root` wires the STAC root's
        // `/` to `stac_landing`, never this function — a STAC Catalog
        // landing page has a different shape entirely (see `stac_landing`'s
        // own doc comment). This arm exists only so `Protocol`'s match here
        // stays exhaustive.
        Protocol::Stac => {}
    }

    Json(json!({
        "title": format!("Tellurion — {}", protocol.title()),
        "description": "OGC API serving engine — one full API root per tenant/catalog",
        "links": links,
    }))
}

/// The conformance classes cited by `protocol`'s `/conformance` endpoint
/// (and, for STAC, embedded directly into the landing page too — see
/// `stac_landing`) — factored out of `protocol_conformance` so the two never
/// silently drift apart. Unlike the pre-`#39` single aggregate
/// `/conformance`, a features root cites only Common + Features classes, a
/// tiles root only Common + Tiles, and so on — each root is its own
/// conformant API, not a slice of one shared response. Styles and 3D
/// places currently cite Common only: OGC API — Styles and 3D GeoVolumes are
/// both still drafts with no approved requirement-class URI to cite
/// honestly, the same honesty rule the pre-`#39` aggregate followed.
///
/// `config` is only consulted for the STAC root's own `s3`-profile and
/// `fs`-profile asset classes (`tellurion_stac::s3_asset_conformance_classes`/
/// `resumable_asset_conformance_classes`/
/// `download_redirect_asset_conformance_classes`'s own docs) — every other
/// non-CQL2 class here is a property of the software build, not this
/// deployment's configuration. `router` is consulted for the Features and
/// STAC roots' CQL2 (1.0) classes (`#105`): unlike every other class this
/// function assembles, which CQL2 classes are honest depends on which
/// driver actually backs a given collection, so
/// `tellurion_core::Router::cql2_conformance_classes`'s per-deployment
/// intersection is folded in here rather than either protocol crate's own
/// static list ever naming one — see that method's own doc for the full
/// reasoning and why the landing page states the conservative intersection
/// rather than the union.
fn conformance_classes(
    protocol: Protocol,
    config: &tellurion_core::AppConfig,
    router: &tellurion_core::Router,
) -> Vec<&'static str> {
    let mut classes: Vec<&str> = COMMON_CONFORMANCE_CLASSES.to_vec();
    match protocol {
        Protocol::Features => {
            classes.extend(tellurion_features::CONFORMANCE_CLASSES.iter().copied());
            // `#263`: Part 4's Create/Replace/Delete, the last family still
            // assembled statically. Requirement 1 clause A binds "for each
            // mutable resource", and whether this deployment offers a
            // mutable resource at all is a routing fact, not a build fact —
            // see `Router::create_replace_delete_conformance_classes`. Folded
            // ahead of the two write folds below because they are layered on
            // it: `conf/features`'s own Dependency row names this class.
            classes.extend(router.create_replace_delete_conformance_classes());
            classes.extend(router.features_write_conformance_classes());
            classes.extend(router.cql2_conformance_classes());
            // `#107`: Optimistic Locking's ETags class, the same per-
            // deployment intersection `cql2_conformance_classes` above
            // already folds in, for the identical reason (a static list
            // here could never be honest about which driver backs a given
            // request's collection) — see `Router::locking_conformance_
            // classes`'s own doc. STAC has no write endpoints in this
            // workspace, so this fold is Features-only, unlike CQL2 (which
            // STAC's own `filter`/search lane also needs).
            classes.extend(router.locking_conformance_classes());
            classes.extend(router.update_conformance_classes());
            // `#217`: Part 2 (CRS by Reference) and Part 3 (Filtering)'s
            // query-parameter classes, folded per deployment for exactly the
            // same reason as the two families above — a driver that cannot
            // reproject, or refuses `filter` outright, cannot honour them,
            // and `tellurion_features::CONFORMANCE_CLASSES` therefore no
            // longer names either. STAC's own list has never claimed Part 2
            // or Part 3, so, like the write folds, this stays Features-only.
            classes.extend(router.crs_conformance_classes());
            classes.extend(router.filtering_conformance_classes());
        }
        Protocol::Tiles => classes.extend([
            tellurion_tiles::CONFORMANCE_TILES_CORE,
            tellurion_tiles::CONFORMANCE_TILESET,
            tellurion_tiles::CONFORMANCE_TILESETS_LIST,
            tellurion_tiles::CONFORMANCE_MVT,
            tellurion_tiles::CONFORMANCE_PNG,
            // `#86`: OGC API — Maps Part 1's own conformance classes —
            // `/collections/{cid}/map` is mounted on this same protocol
            // root (`tellurion_tiles::handlers::router`), not a separate
            // one, so its classes join the tiles root's own list.
            //
            // `#229`: Core is only honest now that a parameterless map
            // request is answered rather than refused, and CRS joins it —
            // the `crs` parameter was always implemented and never
            // declared. Spatial Subsetting and Scaling stay UNDECLARED
            // despite this lane's `bbox`/`bbox-crs`/`width`/`height`
            // support: each class requires parameters the lane does not
            // implement (`subset`/`center` and their CRS parameters;
            // `scale-denominator`) — see `CONFORMANCE_MAPS_CRS`'s own doc.
            tellurion_tiles::CONFORMANCE_MAPS_CORE,
            tellurion_tiles::CONFORMANCE_MAPS_CRS,
            tellurion_tiles::CONFORMANCE_MAPS_PNG,
        ]),
        Protocol::Stac => {
            classes.extend(tellurion_stac::CONFORMANCE_CLASSES.iter().copied());
            classes.extend(router.cql2_conformance_classes());
            // `#248`: the STAC API Filter Extension's own class, folded per
            // deployment for the same reason the CQL2 fold above exists —
            // the extension defines Item Search Filter as binding *Filter and
            // Basic CQL2* to `/search`, so declaring it while
            // `cql2_conformance_classes` correctly withholds every CQL2 class
            // would assert a binding to something this very document does not
            // declare. Gated on `FeatureSource::filter_capable` alone; see
            // `Router::item_search_filter_conformance_classes` for why the
            // Part 3 `filter-crs` condition the Features fold carries does not
            // apply on this lane.
            classes.extend(router.item_search_filter_conformance_classes());
            let has_s3_store = config
                .object_stores
                .iter()
                .any(|decl| matches!(decl.profile, tellurion_core::ObjectStoreProfile::S3 { .. }));
            classes.extend(tellurion_stac::s3_asset_conformance_classes(has_s3_store));
            classes.extend(tellurion_stac::download_redirect_asset_conformance_classes(
                has_s3_store,
            ));
            let has_fs_store = config
                .object_stores
                .iter()
                .any(|decl| matches!(decl.profile, tellurion_core::ObjectStoreProfile::Fs { .. }));
            classes.extend(tellurion_stac::resumable_asset_conformance_classes(
                has_fs_store,
                has_s3_store,
            ));
        }
        // `#192`: the Records root cites the OGC API — Common classes above
        // and nothing else. OGC API — Records — Part 1: Core is an approved
        // standard with real class URIs to cite (OGC 20-004r1, 1.0), so
        // unlike Styles and 3D GeoVolumes this is not "no class exists yet"
        // — it is a deliberate refusal, per class, with the requirement
        // identifiers behind each one written out in
        // `tellurion_records::conformance`'s module documentation. Extended
        // from that crate's own (empty) list rather than simply omitted, so
        // the day a class is genuinely earned it lands in one place.
        Protocol::Records => classes.extend(tellurion_records::CONFORMANCE_CLASSES.iter().copied()),
        // `#182`: the Processes root cites the OGC API — Common classes above
        // and nothing else, on the same terms as the Records root right
        // before it. OGC API — Processes — Part 1: Core is an approved
        // standard (OGC 18-062r2, 1.0.0) with eight real class URIs to cite,
        // so this is a deliberate refusal rather than "no class exists yet" —
        // the requirement identifiers behind each one, per class, are in
        // `tellurion_processes::conformance`'s module documentation.
        Protocol::Processes => {
            classes.extend(tellurion_processes::CONFORMANCE_CLASSES.iter().copied())
        }
        Protocol::Styles | Protocol::ThreeDTiles => {}
    }
    classes
}

/// GET `/{tenant}/{protocol}/catalogs/{catalog}/conformance` — this root's
/// conformance classes ONLY (`#39`). See [`conformance_classes`] for how the
/// list is built per protocol.
pub async fn protocol_conformance(
    Extension(protocol): Extension<Protocol>,
    State(ctx): State<Arc<AppContext>>,
) -> impl IntoResponse {
    Json(
        json!({ "conformsTo": conformance_classes(protocol, &ctx.current().config, &ctx.current().router) }),
    )
}

/// GeoJSON media type — the required `type` for both `search` link entries
/// below (verified 2026-07 against `stac-api-spec`'s `item-search/README.md`
/// at the `v1.0.0` tag: "This `search` link relation must have a `type` of
/// `application/geo+json`").
const GEOJSON_MEDIA_TYPE: &str = "application/geo+json";

/// GET `/{tenant}/stac/catalogs/{catalog}/` — the STAC Catalog landing page
/// (`#36`, slice A; `search` link added in slice C). Unlike
/// `protocol_landing`, this is not a generic OGC API Common document: STAC
/// API - Core mandates a specific shape (a STAC Catalog object —
/// `type: "Catalog"`, `stac_version`, `id`, `description` — with
/// `conformsTo` embedded directly in the landing page rather than only
/// reachable via `/conformance`, verified 2026-07 against stac-api-spec's
/// `core/README.md` at the `v1.0.0` tag). `id` is this catalog's own
/// external id (`#39`) — read straight from the path, exactly as the client
/// typed it, never resolved through `Router`/`Resolver`: the same "landing
/// pages don't validate tenant/catalog existence" non-behavior
/// `protocol_landing`/`protocol_conformance` already have (only a concrete
/// resource route like `/collections/{cid}` 404s on an unknown id).
///
/// The `search` link is two separate entries, not one — verified 2026-07
/// against `item-search/README.md`'s own example landing page: "If the
/// server supports both GET and POST requests, two links should be
/// included, one with a `method` of `GET` one with a `method` of `POST`."
/// `tellurion_features::Link` (reused for every other link on this page, and
/// across every protocol root) has no `method` field — adding one there
/// would touch `tellurion-features`, a different lane's crate — so these two
/// entries are built as raw JSON objects instead, appended alongside the
/// serialized `Link`s rather than going through that type at all.
pub async fn stac_landing(
    Path(params): Path<HashMap<String, String>>,
    OriginalUri(uri): OriginalUri,
    State(ctx): State<Arc<AppContext>>,
) -> impl IntoResponse {
    let self_root = uri.path().trim_end_matches('/').to_string();
    let catalog_ext = params
        .get("catalog")
        .cloned()
        .unwrap_or_else(|| tellurion_stac::DEFAULT_CATALOG.to_string());

    let links = vec![
        Link::new(self_root.clone(), "self", JSON_MEDIA_TYPE),
        Link::new(self_root.clone(), "root", JSON_MEDIA_TYPE),
        Link::new(
            format!("{self_root}/conformance"),
            "conformance",
            JSON_MEDIA_TYPE,
        ),
        Link::new(
            format!("{self_root}/api"),
            "service-desc",
            OPENAPI_MEDIA_TYPE,
        ),
        Link::new(format!("{self_root}/collections"), "data", JSON_MEDIA_TYPE),
    ];
    let mut links: Vec<serde_json::Value> = links
        .into_iter()
        .map(|link| serde_json::to_value(link).expect("Link always serializes"))
        .collect();
    let search_href = format!("{self_root}/search");
    for method in ["GET", "POST"] {
        links.push(json!({
            "rel": "search",
            "type": GEOJSON_MEDIA_TYPE,
            "href": search_href,
            "method": method,
        }));
    }

    Json(json!({
        "type": "Catalog",
        "stac_version": tellurion_stac::STAC_VERSION,
        "id": catalog_ext,
        "description": format!("STAC Catalog root for the '{catalog_ext}' catalog."),
        "conformsTo": conformance_classes(Protocol::Stac, &ctx.current().config, &ctx.current().router),
        "links": links,
    }))
}

/// GET `/{tenant}/` — the tenant directory: every catalog this tenant owns,
/// crossed with every protocol this server mounts, as links into that
/// protocol root (`#39`). NOT an OGC API root itself (no `/conformance` or
/// `/api` here) — purely a directory of the real roots underneath it. A
/// tenant external id that doesn't resolve is a 404, the same as any other
/// unresolvable path segment.
///
/// Cursor-paginated (`#42`, `#59`), the same registry-seam mechanism the
/// `/collections` listings already use: reads the tenant's catalogs through
/// `AppContext`'s registry seam (`RegistryReader::list_catalogs`) instead of
/// the resolver's own in-memory `catalogs_for_tenant` index, which has no
/// paging concept and — under the relational backend — never reflects a
/// catalog published after the last full registry walk anyway (`Resolver`
/// is a snapshot index, `RegistryReader` reads live). A tenant with fewer
/// catalogs than `CATALOGS_DEFAULT_LIMIT` still gets exactly today's
/// single-page response back — a `next` link only ever appears once the
/// registry actually has more to serve.
pub async fn tenant_directory(
    State(ctx): State<Arc<AppContext>>,
    Extension(availability): Extension<RootAvailability>,
    Path(params): Path<HashMap<String, String>>,
    Query(raw_query): Query<TenantDirectoryQueryParams>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let Some(tenant_ext) = params.get("tenant") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let state = ctx.current();
    let Ok(tenant_id) = state.resolver.resolve_tenant(tenant_ext).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let limit = match raw_query.limit {
        None => CATALOGS_DEFAULT_LIMIT,
        Some(0) => return StatusCode::BAD_REQUEST.into_response(),
        // Spec behavior: values above the maximum are clamped, not
        // rejected — same rule every other cursor-paged listing follows.
        Some(value) => value.min(CATALOGS_MAX_LIMIT),
    };
    let page = match state
        .registry
        .list_catalogs(
            &tenant_id,
            PageRequest {
                limit,
                after: raw_query.token.clone(),
            },
        )
        .await
    {
        Ok(page) => page,
        Err(error) => {
            tracing::error!(
                %error,
                tenant = tenant_ext.as_str(),
                "failed to list catalogs for the tenant directory"
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let self_root = uri.path().trim_end_matches('/').to_string();
    let mut links = vec![Link::new(
        tenant_directory_href(&self_root, &raw_query, None),
        "self",
        JSON_MEDIA_TYPE,
    )];
    for catalog in &page.items {
        // `#185`: a protocol this catalog's exposure matrix turns off has no
        // root left to link — `app::enforce_protocol_exposure` answers 404
        // at that prefix, and a directory advertising it anyway would be
        // publishing links it already knows are dead. A catalog the current
        // routing snapshot never indexed has no matrix at all and is listed
        // in full, exactly as before.
        let protocols = state.router.catalog_protocols(&catalog.id);
        for protocol in Protocol::ALL {
            if protocols.is_some_and(|matrix| !protocol.exposure(&matrix).is_enabled()) {
                continue;
            }
            // `#182`: and a root whose *capability* this deployment lacks has
            // no prefix left to link either, for exactly the reason above —
            // `app::processes_root` answers `404` there regardless of what the
            // exposure matrix says, so linking it would publish a dead link.
            // Every root without a capability precondition is unaffected: see
            // `RootAvailability::serves`.
            if !availability.serves(protocol) {
                continue;
            }
            links.push(Link::new(
                format!(
                    "{self_root}/{}/catalogs/{}",
                    protocol.segment(),
                    catalog.external_id()
                ),
                protocol.segment(),
                JSON_MEDIA_TYPE,
            ));
        }
    }
    if let Some(next_token) = page.next.as_deref() {
        links.push(Link::new(
            tenant_directory_href(&self_root, &raw_query, Some(next_token)),
            "next",
            JSON_MEDIA_TYPE,
        ));
    }

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static(JSON_MEDIA_TYPE),
        )],
        Json(json!({
            "tenant": tenant_ext,
            "links": links,
        })),
    )
        .into_response()
}

/// GET `/` — the top-level minimal service descriptor (`#39`). Deliberately
/// bare: it must NEVER enumerate tenants (tenant existence is not public,
/// and the tenant set is unbounded — see the design doc's privacy/scale
/// rationale), so there is no `Router`/`Resolver` consultation here at all,
/// and no `data`/`tiles`/`styles` link the way a protocol root's landing
/// page has — those only make sense once a tenant and catalog are known.
pub async fn service_descriptor() -> impl IntoResponse {
    Json(json!({
        "title": "Tellurion",
        "description": "OGC API serving engine. Every tenant serves its own set of full OGC API roots at /{tenant}/{protocol}/catalogs/{catalog}/ — this endpoint does not list them.",
        "links": [
            Link::new("/", "self", JSON_MEDIA_TYPE),
        ],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellurion_core::{AppConfig, ObjectStoreDecl, ObjectStoreProfile, Registry, Router};

    /// A driver-less `Router` (`#105`): every test in this module exercises
    /// config-gated classes (S3/FS object-store profiles) that have nothing
    /// to do with `config.storages`, so an empty one built from a
    /// `Registry` with no factories registered at all is enough — no test
    /// here declares a `storages:` entry. The mixed-driver/PostGIS-only CQL2
    /// intersection itself is pinned at the `Router::cql2_conformance_classes`
    /// unit-test level (`tellurion-core::router`'s own test module), not
    /// re-derived here; this helper only proves `conformance_classes` wires
    /// that method's result into the Features/STAC responses at all (see
    /// `features_conformance_folds_in_the_routers_cql2_intersection` below).
    fn empty_router() -> Router {
        Router::build(&AppConfig::default(), &Registry::new()).unwrap()
    }

    const S3_ASSET_CLASSES: [&str; 2] = [
        "https://tellurion.dev/spec/assets/1.0/conf/object-store-profile/s3",
        "https://tellurion.dev/spec/assets/1.0/conf/presigned-upload",
    ];

    fn s3_object_store() -> ObjectStoreDecl {
        ObjectStoreDecl {
            id: "blobs".to_string(),
            profile: ObjectStoreProfile::S3 {
                endpoint: "https://minio.example.test:9000".to_string(),
                bucket: "photos".to_string(),
                region: "us-east-1".to_string(),
                key_prefix: String::new(),
                access_key_env: "TEST_ACCESS_KEY".to_string(),
                secret_key_env: "TEST_SECRET_KEY".to_string(),
                presign_expiry_s: 900,
            },
        }
    }

    /// A deployment with no `object_stores` at all (the common case: a
    /// remote-assets-only STAC root, or one that only ever declared the
    /// `fs` profile) never claims `object-store-profile/s3` or
    /// `presigned-upload` — claiming them without a real s3-compatible
    /// endpoint behind them would be a lie the same way every other class
    /// here is refused honestly (this function's own doc).
    #[test]
    fn stac_conformance_omits_s3_asset_classes_without_an_s3_object_store() {
        let config = AppConfig::default();
        let classes = conformance_classes(Protocol::Stac, &config, &empty_router());
        for class in S3_ASSET_CLASSES {
            assert!(
                !classes.contains(&class),
                "'{class}' must not be claimed without a declared s3 object_store"
            );
        }
    }

    /// The `fs` profile alone still doesn't earn the `s3` classes — only an
    /// actual `s3`-profile declaration does (`tellurion_stac::
    /// s3_asset_conformance_classes`'s own doc).
    #[test]
    fn stac_conformance_omits_s3_asset_classes_with_only_an_fs_object_store() {
        let mut config = AppConfig::default();
        config.object_stores.push(ObjectStoreDecl {
            id: "local".to_string(),
            profile: ObjectStoreProfile::Fs {
                root: "/var/lib/assets".to_string(),
            },
        });
        let classes = conformance_classes(Protocol::Stac, &config, &empty_router());
        for class in S3_ASSET_CLASSES {
            assert!(!classes.contains(&class));
        }
    }

    #[test]
    fn stac_conformance_declares_s3_asset_classes_when_an_s3_store_is_declared() {
        let mut config = AppConfig::default();
        config.object_stores.push(s3_object_store());
        let classes = conformance_classes(Protocol::Stac, &config, &empty_router());
        for class in S3_ASSET_CLASSES {
            assert!(classes.contains(&class), "missing '{class}'");
        }
    }

    /// The `s3`-store declaration only widens the STAC root's own
    /// conformance response — a features or tiles root shares nothing with
    /// the assets proposal at all (this function's own doc: "each root is
    /// its own conformant API, not a slice of one shared response").
    #[test]
    fn an_s3_object_store_never_leaks_asset_classes_into_the_features_root() {
        let mut config = AppConfig::default();
        config.object_stores.push(s3_object_store());
        let classes = conformance_classes(Protocol::Features, &config, &empty_router());
        for class in S3_ASSET_CLASSES {
            assert!(!classes.contains(&class));
        }
    }

    const DOWNLOAD_REDIRECT_CLASS: &str =
        "https://tellurion.dev/spec/assets/1.0/conf/download-redirect";

    /// Same gating as the two `S3_ASSET_CLASSES` — `download-redirect`
    /// needs a real `s3`-profile store to redirect to, same as
    /// `presigned-upload` needs one to negotiate against.
    #[test]
    fn stac_conformance_omits_download_redirect_without_an_s3_object_store() {
        let config = AppConfig::default();
        let classes = conformance_classes(Protocol::Stac, &config, &empty_router());
        assert!(!classes.contains(&DOWNLOAD_REDIRECT_CLASS));
    }

    #[test]
    fn stac_conformance_omits_download_redirect_with_only_an_fs_object_store() {
        let mut config = AppConfig::default();
        config.object_stores.push(ObjectStoreDecl {
            id: "local".to_string(),
            profile: ObjectStoreProfile::Fs {
                root: "/var/lib/assets".to_string(),
            },
        });
        let classes = conformance_classes(Protocol::Stac, &config, &empty_router());
        assert!(!classes.contains(&DOWNLOAD_REDIRECT_CLASS));
    }

    #[test]
    fn stac_conformance_declares_download_redirect_when_an_s3_store_is_declared() {
        let mut config = AppConfig::default();
        config.object_stores.push(s3_object_store());
        let classes = conformance_classes(Protocol::Stac, &config, &empty_router());
        assert!(classes.contains(&DOWNLOAD_REDIRECT_CLASS));
    }

    #[test]
    fn an_s3_object_store_never_leaks_download_redirect_into_the_features_root() {
        let mut config = AppConfig::default();
        config.object_stores.push(s3_object_store());
        let classes = conformance_classes(Protocol::Features, &config, &empty_router());
        assert!(!classes.contains(&DOWNLOAD_REDIRECT_CLASS));
    }

    const RESUMABLE_UPLOAD_CLASS: &str =
        "https://tellurion.dev/spec/assets/1.0/conf/resumable-upload";

    fn fs_object_store() -> ObjectStoreDecl {
        ObjectStoreDecl {
            id: "local".to_string(),
            profile: ObjectStoreProfile::Fs {
                root: "/var/lib/assets".to_string(),
            },
        }
    }

    /// A deployment with no `object_stores` at all never claims
    /// `resumable-upload` — the class only means something once this
    /// deployment could actually serve it (`resumable_asset_conformance_
    /// classes`'s own doc).
    #[test]
    fn stac_conformance_omits_resumable_upload_without_an_fs_object_store() {
        let config = AppConfig::default();
        let classes = conformance_classes(Protocol::Stac, &config, &empty_router());
        assert!(!classes.contains(&RESUMABLE_UPLOAD_CLASS));
    }

    /// An `s3`-only deployment now earns `resumable-upload` too — this
    /// slice adds a real S3 multipart-upload `ResumableUploadStore`
    /// implementation (`tellurion_core::objectstore::S3ObjectStore`), so
    /// the class is honest for either profile, not `fs` alone.
    #[test]
    fn stac_conformance_declares_resumable_upload_with_only_an_s3_object_store() {
        let mut config = AppConfig::default();
        config.object_stores.push(s3_object_store());
        let classes = conformance_classes(Protocol::Stac, &config, &empty_router());
        assert!(classes.contains(&RESUMABLE_UPLOAD_CLASS));
    }

    #[test]
    fn stac_conformance_declares_resumable_upload_when_an_fs_store_is_declared() {
        let mut config = AppConfig::default();
        config.object_stores.push(fs_object_store());
        let classes = conformance_classes(Protocol::Stac, &config, &empty_router());
        assert!(classes.contains(&RESUMABLE_UPLOAD_CLASS));
    }

    /// The `fs`-store declaration only widens the STAC root's own
    /// conformance response, the identical rule
    /// `an_s3_object_store_never_leaks_asset_classes_into_the_features_root`
    /// already proves for the `s3` classes.
    #[test]
    fn an_fs_object_store_never_leaks_resumable_upload_into_the_features_root() {
        let mut config = AppConfig::default();
        config.object_stores.push(fs_object_store());
        let classes = conformance_classes(Protocol::Features, &config, &empty_router());
        assert!(!classes.contains(&RESUMABLE_UPLOAD_CLASS));
    }

    // -- CQL2 classes fold in `Router::cql2_conformance_classes` (`#105`) ----
    //
    // The exhaustive intersection algebra (mixed drivers, PostGIS-only
    // re-earning, the empty no-driver case, CASEI never surviving) is
    // pinned once at the source, `tellurion_core::router`'s own test module
    // — these two tests only prove `conformance_classes` actually wires that
    // method's result into the Features/STAC responses, through a real
    // `Router` built the same way `AppContext` builds one at boot.

    struct WeakFeatureSource;

    #[async_trait::async_trait]
    impl tellurion_core::FeatureSource for WeakFeatureSource {
        async fn items(
            &self,
            _collection: &tellurion_core::CollectionDecl,
            _query: &tellurion_core::ItemsQuery,
        ) -> tellurion_core::Result<tellurion_core::FeaturePage> {
            unreachable!("not exercised by the conformance-wiring test")
        }

        async fn item(
            &self,
            _collection: &tellurion_core::CollectionDecl,
            _id: &str,
            _filter: Option<&tellurion_core::Filter>,
        ) -> tellurion_core::Result<Option<serde_json::Value>> {
            unreachable!("not exercised by the conformance-wiring test")
        }

        fn cql2_conformance_classes(&self) -> Vec<&'static str> {
            vec![
                "http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2",
                "http://www.opengis.net/spec/cql2/1.0/conf/cql2-text",
                "http://www.opengis.net/spec/cql2/1.0/conf/cql2-json",
            ]
        }
    }

    struct WeakCatalog;

    #[async_trait::async_trait]
    impl tellurion_core::CatalogSource for WeakCatalog {
        async fn collections(
            &self,
        ) -> tellurion_core::Result<Vec<tellurion_core::PhysicalCollection>> {
            Ok(vec![])
        }
    }

    struct WeakDriver;

    impl tellurion_core::StorageDriver for WeakDriver {
        fn catalog_source(&self) -> Arc<dyn tellurion_core::CatalogSource> {
            Arc::new(WeakCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn tellurion_core::FeatureSource>> {
            Some(Arc::new(WeakFeatureSource))
        }
    }

    struct WeakFactory;

    impl tellurion_core::DriverFactory for WeakFactory {
        fn name(&self) -> &str {
            "weak-fake"
        }

        fn build(
            &self,
            _decl: &tellurion_core::config::StorageDecl,
        ) -> tellurion_core::Result<Arc<dyn tellurion_core::StorageDriver>> {
            Ok(Arc::new(WeakDriver))
        }
    }

    fn router_with_one_weak_driver() -> Router {
        let config: AppConfig = serde_yaml::from_str(
            "storages: [ { id: main, driver: weak-fake, url_env: DATABASE_URL } ]\n",
        )
        .unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(WeakFactory));
        Router::build(&config, &registry).unwrap()
    }

    /// `#107`: a driver with a real, resolving write lane whose
    /// `WriteSink::locking_conformance_classes` stays at the trait default
    /// (declares nothing) — the fixture `locking_class_narrows_to_empty_
    /// through_the_features_root` below needs to distinguish "the fold
    /// genuinely narrowed to empty" from "no driver participated," which
    /// `WeakDriver` above can't do (it has no `write_sink` at all).
    struct NonLockingWriteSink;

    #[async_trait::async_trait]
    impl tellurion_core::WriteSink for NonLockingWriteSink {
        async fn apply(
            &self,
            _collection: &tellurion_core::CollectionDecl,
            _mutation: tellurion_core::Mutation,
        ) -> tellurion_core::Result<tellurion_core::Sequence> {
            unreachable!("not exercised by the conformance-wiring test")
        }

        fn update_conformance_classes(&self) -> Vec<&'static str> {
            vec![tellurion_core::outbox::UPDATE_CONFORMANCE_CLASS]
        }

        fn features_conformance_classes(
            &self,
            _collection: &tellurion_core::CollectionDecl,
        ) -> Vec<&'static str> {
            vec![tellurion_core::FEATURES_PART4_FEATURES_CLASS]
        }
    }

    struct NonLockingWriteDriver;

    impl tellurion_core::StorageDriver for NonLockingWriteDriver {
        fn catalog_source(&self) -> Arc<dyn tellurion_core::CatalogSource> {
            Arc::new(WeakCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn tellurion_core::FeatureSource>> {
            Some(Arc::new(WeakFeatureSource))
        }

        fn write_sink(&self) -> Option<Arc<dyn tellurion_core::WriteSink>> {
            Some(Arc::new(NonLockingWriteSink))
        }
    }

    struct NonLockingWriteFactory;

    impl tellurion_core::DriverFactory for NonLockingWriteFactory {
        fn name(&self) -> &str {
            "non-locking-write-fake"
        }

        fn build(
            &self,
            _decl: &tellurion_core::config::StorageDecl,
        ) -> tellurion_core::Result<Arc<dyn tellurion_core::StorageDriver>> {
            Ok(Arc::new(NonLockingWriteDriver))
        }
    }

    fn router_with_one_non_locking_write_driver() -> Router {
        let config: AppConfig = serde_yaml::from_str(
            "storages: [ { id: main, driver: non-locking-write-fake, url_env: DATABASE_URL } ]\n\
             tenants: [ { id: public } ]\n\
             catalogs: [ { id: default, tenant: public } ]\n\
             collections:\n\
               - { id: demo, catalog: default, storage: main, table: demo, geometry: geom, pk: id, routing: { write: main } }\n",
        )
        .unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(NonLockingWriteFactory));
        Router::build(&config, &registry).unwrap()
    }

    #[test]
    fn features_conformance_folds_in_the_routers_cql2_intersection() {
        let router = router_with_one_weak_driver();
        let classes = conformance_classes(Protocol::Features, &AppConfig::default(), &router);
        assert!(classes.contains(&"http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2"));
        assert!(
            !classes.contains(&"http://www.opengis.net/spec/cql2/1.0/conf/temporal-functions"),
            "a weak-only deployment must not claim a class its one driver doesn't earn"
        );
    }

    #[test]
    fn stac_conformance_folds_in_the_routers_cql2_intersection() {
        let router = router_with_one_weak_driver();
        let classes = conformance_classes(Protocol::Stac, &AppConfig::default(), &router);
        assert!(classes.contains(&"http://www.opengis.net/spec/cql2/1.0/conf/cql2-json"));
        assert!(
            !classes.contains(&"http://www.opengis.net/spec/cql2/1.0/conf/spatial-functions"),
            "a weak-only deployment must not claim a class its one driver doesn't earn"
        );
    }

    /// A pure-tiles deployment has no CQL2 evaluator, so none of the
    /// driver-honoured CQL2 seed reaches the Features root's response.
    #[test]
    fn features_conformance_omits_cql2_with_no_features_capable_driver() {
        let router = empty_router();
        let classes = conformance_classes(Protocol::Features, &AppConfig::default(), &router);
        assert!(!classes.contains(&"http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2"));
    }

    // -- Optimistic Locking classes fold in `Router::locking_conformance_
    // classes` (`#107`) -------------------------------------------------
    //
    // The fold algebra itself (mixed drivers, the write-only structural
    // gap, the empty no-driver case) is pinned once at the source,
    // `tellurion_core::router`'s own test module — these tests only prove
    // `conformance_classes` wires that method's result into the Features
    // root's response, and that STAC never sees it (no write endpoints
    // there in this workspace).

    #[test]
    fn features_conformance_folds_in_the_routers_locking_intersection() {
        let absent =
            conformance_classes(Protocol::Features, &AppConfig::default(), &empty_router());
        assert!(
            !absent.contains(&tellurion_core::locking::OPTIMISTIC_LOCKING_ETAGS_CLASS),
            "no write-capable driver means no optimistic-locking claim"
        );

        let narrowed = conformance_classes(
            Protocol::Features,
            &AppConfig::default(),
            &router_with_one_non_locking_write_driver(),
        );
        assert!(
            !narrowed.contains(&tellurion_core::locking::OPTIMISTIC_LOCKING_ETAGS_CLASS),
            "a real write-capable driver that declares nothing must narrow the fold to \
             empty, proving `conformance_classes` genuinely wires in the router's answer \
             rather than ignoring the router's answer"
        );
    }

    #[test]
    fn features_body_conformance_is_runtime_gated_by_the_write_router() {
        let absent =
            conformance_classes(Protocol::Features, &AppConfig::default(), &empty_router());
        assert!(!absent.contains(&tellurion_core::FEATURES_PART4_FEATURES_CLASS));

        let present = conformance_classes(
            Protocol::Features,
            &AppConfig::default(),
            &router_with_one_non_locking_write_driver(),
        );
        assert!(present.contains(&tellurion_core::FEATURES_PART4_FEATURES_CLASS));
    }

    #[test]
    fn locking_class_never_leaks_into_the_stac_root() {
        let router = empty_router();
        let features_classes =
            conformance_classes(Protocol::Features, &AppConfig::default(), &router);
        let stac_classes = conformance_classes(Protocol::Stac, &AppConfig::default(), &router);
        assert!(
            !features_classes.contains(&tellurion_core::locking::OPTIMISTIC_LOCKING_ETAGS_CLASS)
        );
        assert!(
            !stac_classes.contains(&tellurion_core::locking::OPTIMISTIC_LOCKING_ETAGS_CLASS),
            "STAC has no write endpoints in this workspace; it must never claim this class"
        );
    }

    // -- Part 2 (CRS) and Part 3 (Filtering) folds in `Router::
    // crs_conformance_classes`/`filtering_conformance_classes` (`#217`) ----
    //
    // Same division of labour as the two families above: the fold algebra is
    // pinned once at the source, in `tellurion_core::router`'s own test
    // module; these tests only prove `conformance_classes` wires both
    // results into the Features root and that neither ever reaches STAC.

    /// A PostGIS-shaped driver for the wiring tests below: it reprojects and
    /// it filters, so both `#217` folds survive it. `WeakFeatureSource` above
    /// leaves both capabilities at the trait default (`false`), which is what
    /// makes it the negative fixture.
    struct StrongFeatureSource;

    #[async_trait::async_trait]
    impl tellurion_core::FeatureSource for StrongFeatureSource {
        async fn items(
            &self,
            _collection: &tellurion_core::CollectionDecl,
            _query: &tellurion_core::ItemsQuery,
        ) -> tellurion_core::Result<tellurion_core::FeaturePage> {
            unreachable!("not exercised by the conformance-wiring test")
        }

        async fn item(
            &self,
            _collection: &tellurion_core::CollectionDecl,
            _id: &str,
            _filter: Option<&tellurion_core::Filter>,
        ) -> tellurion_core::Result<Option<serde_json::Value>> {
            unreachable!("not exercised by the conformance-wiring test")
        }

        fn crs_capable(&self) -> bool {
            true
        }

        fn filter_capable(&self) -> bool {
            true
        }

        /// `#217`: being `crs_capable` above is what makes Part 3
        /// Requirement 8 (`/req/filter/filter-crs-param`) binding on this
        /// driver, so a "strong" (PostGIS-shaped) fixture has to honour
        /// `filter-crs` too — without this the fold correctly withholds the
        /// Filtering classes and the wiring test below stops proving
        /// anything about wiring.
        fn filter_crs_capable(&self) -> bool {
            true
        }
    }

    struct StrongDriver;

    impl tellurion_core::StorageDriver for StrongDriver {
        fn catalog_source(&self) -> Arc<dyn tellurion_core::CatalogSource> {
            Arc::new(WeakCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn tellurion_core::FeatureSource>> {
            Some(Arc::new(StrongFeatureSource))
        }
    }

    struct StrongFactory;

    impl tellurion_core::DriverFactory for StrongFactory {
        fn name(&self) -> &str {
            "strong-fake"
        }

        fn build(
            &self,
            _decl: &tellurion_core::config::StorageDecl,
        ) -> tellurion_core::Result<Arc<dyn tellurion_core::StorageDriver>> {
            Ok(Arc::new(StrongDriver))
        }
    }

    fn router_with_one_strong_driver() -> Router {
        let config: AppConfig = serde_yaml::from_str(
            "storages: [ { id: main, driver: strong-fake, url_env: DATABASE_URL } ]\n",
        )
        .unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(StrongFactory));
        Router::build(&config, &registry).unwrap()
    }

    /// `#217`'s acceptance criterion, both directions: a deployment whose one
    /// features driver can neither reproject nor filter (the
    /// FlatGeobuf/GeoParquet/memory shape) claims neither family, while a
    /// PostGIS-shaped one claims both.
    #[test]
    fn features_conformance_folds_in_the_routers_crs_and_filtering_classes() {
        let incapable = conformance_classes(
            Protocol::Features,
            &AppConfig::default(),
            &router_with_one_weak_driver(),
        );
        assert!(
            !incapable.contains(&tellurion_core::crs::CRS_CONFORMANCE_CLASS),
            "a driver that cannot reproject must not carry Part 2 onto the landing page"
        );
        for class in tellurion_core::filter::FILTERING_CONFORMANCE_CLASSES {
            assert!(
                !incapable.contains(class),
                "a driver that refuses `filter` must not carry {class} onto the landing page"
            );
        }

        let capable = conformance_classes(
            Protocol::Features,
            &AppConfig::default(),
            &router_with_one_strong_driver(),
        );
        assert!(capable.contains(&tellurion_core::crs::CRS_CONFORMANCE_CLASS));
        for class in tellurion_core::filter::FILTERING_CONFORMANCE_CLASSES {
            assert!(capable.contains(class), "missing {class}");
        }
    }

    /// The queryables document is served whatever the driver, so its class
    /// stays static in `tellurion_features::CONFORMANCE_CLASSES` and survives
    /// a deployment where the three folded Part 3 classes do not.
    #[test]
    fn features_conformance_keeps_the_queryables_class_when_filtering_folds_away() {
        let classes = conformance_classes(
            Protocol::Features,
            &AppConfig::default(),
            &router_with_one_weak_driver(),
        );
        assert!(
            classes.contains(&"http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/queryables")
        );
    }

    #[test]
    fn crs_and_filtering_classes_never_leak_into_the_stac_root() {
        let router = router_with_one_strong_driver();
        let stac = conformance_classes(Protocol::Stac, &AppConfig::default(), &router);
        assert!(!stac.contains(&tellurion_core::crs::CRS_CONFORMANCE_CLASS));
        for class in tellurion_core::filter::FILTERING_CONFORMANCE_CLASSES {
            assert!(
                !stac.contains(class),
                "STAC's own list has never claimed OGC API Features Part 3: {class}"
            );
        }
    }

    /// `#248`: the STAC root's own Item Search Filter class is folded here,
    /// both directions. The class binds Filter *and Basic CQL2* to `/search`,
    /// so a deployment whose one driver refuses every `filter` — and whose
    /// `/conformance` therefore correctly declares no CQL2 class at all —
    /// must not claim it either; a PostGIS-shaped one keeps it.
    #[test]
    fn stac_conformance_folds_in_the_routers_item_search_filter_class() {
        let incapable = conformance_classes(
            Protocol::Stac,
            &AppConfig::default(),
            &router_with_one_weak_driver(),
        );
        assert!(
            !incapable.contains(&tellurion_core::filter::ITEM_SEARCH_FILTER_CLASS),
            "a driver that refuses `filter` must not carry the Item Search Filter class onto \
             the STAC landing page"
        );
        assert!(
            incapable.contains(&"https://api.stacspec.org/v1.0.0/item-search"),
            "plain Item Search stays unconditional: `/search` still answers bbox/datetime/ids"
        );

        let capable = conformance_classes(
            Protocol::Stac,
            &AppConfig::default(),
            &router_with_one_strong_driver(),
        );
        assert!(capable.contains(&tellurion_core::filter::ITEM_SEARCH_FILTER_CLASS));
    }

    /// ...and it stays STAC-only: the Features root has its own Part 3 fold
    /// and never cites the STAC extension's class.
    #[test]
    fn the_item_search_filter_class_never_leaks_into_the_features_root() {
        let features = conformance_classes(
            Protocol::Features,
            &AppConfig::default(),
            &router_with_one_strong_driver(),
        );
        assert!(!features.contains(&tellurion_core::filter::ITEM_SEARCH_FILTER_CLASS));
    }

    #[test]
    fn update_class_is_driver_gated_and_features_only() {
        let empty = empty_router();
        let empty_classes = conformance_classes(Protocol::Features, &AppConfig::default(), &empty);
        assert!(!empty_classes.contains(&tellurion_core::outbox::UPDATE_CONFORMANCE_CLASS));

        let router = router_with_one_non_locking_write_driver();
        let features = conformance_classes(Protocol::Features, &AppConfig::default(), &router);
        let stac = conformance_classes(Protocol::Stac, &AppConfig::default(), &router);
        assert!(features.contains(&tellurion_core::outbox::UPDATE_CONFORMANCE_CLASS));
        assert!(!stac.contains(&tellurion_core::outbox::UPDATE_CONFORMANCE_CLASS));
    }
}
