//! STAC API - Core + Collections + Items handlers (`#36`): `GET
//! /collections`, `GET /collections/{cid}`, `GET /collections/{cid}/items`,
//! `GET /collections/{cid}/items/{fid}`. The STAC Catalog landing page
//! and `/conformance` are the server crate's job, same split
//! `tellurion-features`' own handlers doc describes — this crate owns the
//! Collections and Items endpoints plus the descriptor/feature -> STAC
//! mapping (`mapping.rs`) and the asset materialization (`assets.rs`).
//! Every request runs under a `/{tenant}/stac/catalogs/{catalog}` mount
//! (`#39`-style); `tenant`/`catalog` path parameters carry EXTERNAL ids
//! exactly as the client typed them — `resolve_tenant_catalog` turns them
//! (plus a collection's own external id) into the internal ids `Router`
//! expects, through `AppContext::current().resolver`. Response bodies echo
//! external ids straight back from the path — an internal id is never
//! serialized. A handler that runs with no mount at all (this crate's own
//! unit tests) falls back to [`DEFAULT_TENANT`]/[`DEFAULT_CATALOG`], the
//! same convention every sibling protocol crate uses.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use tellurion_core::policy::{self, PolicyDecision, ResourceContext};
use tellurion_core::{
    AppContext, AssetRecordEntry, CanonicalDescriptor, CollectionDecl, Credential,
    Error as CoreError, FeatureSource, Filter, GeometryLiteral, ItemsQuery, LinkAnchor, PolicyLane,
    RateCharge, RateCounter, RateVerdict, RequestedCrs, ResourceRef,
    SearchQuery as CoreSearchQuery, SearchResolution, ServerConfig,
};

use crate::assets::{collection_assets, AssetCapabilities, PageItemAssets};
use crate::iso19139::{to_iso19139, ISO19139_MEDIA_TYPE};
use crate::mapping::{to_stac_collection, to_stac_item};
use crate::model::{Link, StacCollectionsResponse, StacItemCollectionResponse};
use crate::params::{
    collections_href, items_href, parse_collections_query, parse_items_query,
    CollectionsQueryParams, GetCollectionQueryParams, ItemsQueryParams,
};
use crate::problem::ApiError;
use crate::projection::{derive_projection, DerivedProjection};
use crate::search::{
    parse_get, parse_post, search_href, SearchBody, SearchHrefParams, SearchQueryParams,
    SearchRequest, SearchToken,
};

pub const DEFAULT_TENANT: &str = "public";
pub const DEFAULT_CATALOG: &str = "default";
const JSON_MEDIA_TYPE: &str = "application/json";
/// `?f=` value that selects the ISO 19139 XML alternate representation on
/// `GET /collections/{cid}` (`#50`) — same `f`-parameter convention
/// `tellurion-tiles::handlers::negotiate_format` already uses for its own
/// MVT/PNG negotiation.
const ISO19139_QUERY_FORMAT: &str = "xml";
const GEOJSON_MEDIA_TYPE: &str = "application/geo+json";

/// `#245`: the link from a Collection document to the resource that serves
/// that collection's items — the one link the two conformance classes this
/// root declares both require by name, and the one that was missing.
///
/// - *OGC API — Features — Part 1: Core* (OGC 17-069r4, version 1.0.1),
///   Requirement 15 `/req/core/fc-md-items-links`: "For each feature
///   collection included in the response, the links property of the
///   collection SHALL include an item for each supported encoding with a
///   link to the features resource (relation: `items`)... All links SHALL
///   include the `rel` and `type` properties." Requirement 19
///   `/req/core/sfc-md-success` then carries it onto the single-collection
///   resource: `/collections/{collectionId}`'s "links SHALL include all
///   links included for this feature collection in the `/collections`
///   response", which is why both [`list_collections`] and
///   [`get_collection`] emit it rather than only the listing.
/// - *STAC API — Features* (`stac-api-spec`, `v1.0.0` tag,
///   `ogcapi-features/README.md`): "This endpoint must be exposed via a link
///   in the individual collection's endpoint with `rel=items`... the
///   collection resource linking to a paginated endpoint returning items
///   through a link relation `items`, e.g., `/collections/{collectionId}`
///   has a link with relation `items` linking to
///   `/collections/{collectionId}/items`." That README's own example
///   Collection carries it as `"rel": "items", "type":
///   "application/geo+json"` — which is exactly the one encoding
///   [`list_items`] serves (it sets that media type on its own response),
///   so "an item for each supported encoding" is one link here, not several.
///
/// Emitted where the document is built, deliberately NOT through the
/// `LinkContributor` seam (`#186`/`#220`): a contributor cannot know which
/// root is serializing, and `items` means the STAC ItemCollection under this
/// root while it means the OGC API Features FeatureCollection under the
/// Features root — see `tellurion-server::link_contributors`' own module doc
/// for that refusal.
const ITEMS_REL: &str = "items";

fn tenant_of(params: &HashMap<String, String>) -> String {
    params
        .get("tenant")
        .cloned()
        .unwrap_or_else(|| DEFAULT_TENANT.to_string())
}

fn catalog_of(params: &HashMap<String, String>) -> String {
    params
        .get("catalog")
        .cloned()
        .unwrap_or_else(|| DEFAULT_CATALOG.to_string())
}

fn require_param(params: &HashMap<String, String>, name: &str) -> Result<String, ApiError> {
    params
        .get(name)
        .cloned()
        .ok_or(CoreError::NotFound)
        .map_err(ApiError::from)
}

/// Resolves this request's `(tenant, catalog)` path segments — external ids
/// — to internal ones, the one seam every handler in this crate calls
/// before touching `Router`. Same pattern as
/// `tellurion_features::handlers::resolve_tenant_catalog`.
async fn resolve_tenant_catalog(
    ctx: &AppContext,
    params: &HashMap<String, String>,
) -> Result<(String, String), ApiError> {
    let state = ctx.current();
    let tenant_ext = tenant_of(params);
    let catalog_ext = catalog_of(params);
    let tenant_id = state.resolver.resolve_tenant(&tenant_ext).await?;
    let catalog_id = state
        .resolver
        .resolve_catalog(&tenant_id, &catalog_ext)
        .await?;
    Ok((tenant_id, catalog_id))
}

/// Extracts a [`Credential`] from `Authorization: Bearer <token>` — mirrors
/// `tellurion-server::app`'s own `extract_credential` exactly (duplicated
/// per protocol crate, not shared — `tellurion-core` stays framework-free,
/// see `auth.rs`'s own module doc). Any other or malformed `Authorization`
/// header is `Credential::None`, same as no header at all.
fn extract_credential(headers: &HeaderMap) -> Credential {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Credential::None;
    };
    let Ok(value) = value.to_str() else {
        return Credential::None;
    };
    match value.strip_prefix("Bearer ") {
        Some(token) if !token.is_empty() => Credential::Bearer(token.to_string()),
        _ => Credential::None,
    }
}

/// The `#34` policy checkpoint every single-collection handler in this
/// crate calls right after resolving `(tenant_id, catalog_id,
/// collection_id)` — same shape as `tellurion_features::handlers`'s own
/// `authorize_lane`. `/search`'s own checkpoint
/// (`authorize_search_collection`) is separate, since a fan-out search
/// resolves many collections per request and needs to skip a denied one
/// rather than fail the whole search — see that function's own doc.
///
/// `#188`: an allowed request then charges whatever rate ceilings its
/// matching grants declare. `charge` distinguishes a served request from a
/// listing's per-collection visibility probe — see
/// `policy::enforce_rate_limits`'s own doc for why exactly one checkpoint
/// per request may charge. The counter is `AppContext`'s, not the
/// reloadable `ContextState`'s, so a config reload never resets a window.
#[allow(clippy::too_many_arguments)]
async fn authorize_lane(
    state: &tellurion_core::ContextState,
    rate_counter: &dyn RateCounter,
    headers: &HeaderMap,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
    lane_supports_filter: bool,
    charge: RateCharge,
) -> Result<Option<Filter>, ApiError> {
    let Some(authorizer) = state.authorizer.as_ref() else {
        return Ok(None);
    };
    let credential = extract_credential(headers);
    let subject = authorizer.subject(&credential).await;
    let visibility = state
        .router
        .effective_visibility(collection_id)
        .cloned()
        .unwrap_or_default();
    let resource = ResourceContext {
        tenant_id,
        catalog_id,
        collection_id,
        lane: PolicyLane::Stac,
        visibility: &visibility,
    };
    let filter =
        match policy::authorize_resource(&state.config, &resource, &subject, lane_supports_filter)?
        {
            PolicyDecision::Allow { filter } => filter,
            PolicyDecision::Deny => return Err(crate::problem::policy_denied(&credential)),
        };
    if let RateVerdict::Refused(refusal) = policy::enforce_rate_limits(
        &state.config,
        &resource,
        &subject,
        Some(rate_counter),
        charge,
    )
    .await
    {
        return Err(crate::problem::policy_rate_limited(&refusal));
    }
    Ok(filter)
}

fn set_content_type(response: &mut Response, media_type: &'static str) {
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
}

/// Whether `GET /collections/{cid}` should serve the ISO 19139 XML alternate
/// representation instead of its default STAC Collection JSON (`#50`): the
/// `?f=xml` query parameter wins outright (same "explicit parameter beats
/// `Accept`" rule `tellurion-tiles::handlers::negotiate_format` follows for
/// its own suffix/`f`/`Accept` chain), otherwise an `Accept` header that
/// names `ISO19139_MEDIA_TYPE` exactly.
fn wants_iso19139(query_format: Option<&str>, headers: &HeaderMap) -> bool {
    if query_format == Some(ISO19139_QUERY_FORMAT) {
        return true;
    }
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains(ISO19139_MEDIA_TYPE))
}

/// Resolves `decl`'s `CanonicalDescriptor` for the response (`#50`),
/// tolerating an unresolvable `(tenant, catalog, collection)` triple by
/// logging and returning `None` — `Router::canonical_descriptor` already
/// absorbs a mere descriptor-derivation failure internally (physical facts
/// simply come back absent), so an `Err` reaching here means the collection
/// itself couldn't be looked up, not a lesser derivation gap.
/// `mapping::to_stac_collection` already knows how to serve a STAC
/// Collection with no canonical descriptor at all (whole-Earth bbox, an
/// always-open temporal interval, default license) — this metadata is not
/// worth a 500 over, same never-fail-the-request philosophy
/// `tellurion_features::handlers::collection_extent` applies to spatial
/// extent (`#27`).
async fn resolved_canonical(
    ctx: &AppContext,
    tenant: &str,
    catalog: &str,
    decl: &CollectionDecl,
) -> Option<CanonicalDescriptor> {
    let state = ctx.current();
    match state
        .router
        .canonical_descriptor(tenant, catalog, &decl.id)
        .await
    {
        Ok(canonical) => Some(canonical),
        Err(error) => {
            tracing::warn!(
                %error,
                tenant,
                catalog,
                collection = %decl.external_id(),
                "failed to resolve collection; serving STAC defaults"
            );
            None
        }
    }
}

/// `href`'s trailing `/collections` suffix stripped off — the STAC Catalog
/// root (`/`) every `root`/`parent` link in this crate points back to.
fn stac_root_of(collections_href: &str) -> String {
    collections_href
        .strip_suffix("/collections")
        .unwrap_or(collections_href)
        .to_string()
}

/// The canonical deployment prefix contributors concatenate with their
/// server-owned paths. [`ServerConfig::public_href`] performs the same
/// trailing-slash normalization for links built directly by this crate.
fn public_base_url(server: &ServerConfig) -> &str {
    server
        .public_base_url
        .as_deref()
        .unwrap_or_default()
        .trim_end_matches('/')
}

/// This collection's servable-lane capabilities (`#36` slice B, `#48`):
/// gathered through the exact same `Router::resolve_tiles` probe
/// `list_collections`/`get_collection` already make for the tiles-or-features
/// tolerance rule, plus `ctx.style_store`'s global registry. A `resolve_tiles`
/// failure (no tiles capability at all) is the ordinary "this collection
/// can't do MVT/PNG/glb" answer, not an error to propagate — same
/// never-fail-the-request-over-metadata philosophy `resolved_descriptor`
/// already applies to extent. A `style_store.list()` failure is logged and
/// treated as "no styles known" rather than failing the whole response,
/// for the same reason.
///
/// `#220`: also carries this collection's resolved `stac.service_assets`
/// mode, read off the same `EffectiveSettings` the rest of the `stac:`
/// block already resolves through (nearest level in the platform -> tenant
/// -> catalog -> collection chain wins). A collection whose chain never
/// mentions the key — every collection of every deployment written before
/// `#220` — resolves to `ServiceAssetsMode::default()`, the templated map,
/// so nothing here invents a default of its own. Deliberately kept
/// *beside* `has_tiles` rather than folded into it: `has_tiles` is also the
/// listing-visibility predicate (`list_collections`' tiles-or-features
/// tolerance rule), and zeroing it to suppress assets would silently drop a
/// tiles-only collection out of `/collections` entirely.
async fn asset_capabilities(
    ctx: &AppContext,
    tenant: &str,
    catalog: &str,
    collection: &str,
) -> AssetCapabilities {
    let state = ctx.current();
    let tiles = state
        .router
        .resolve_tiles(tenant, catalog, collection)
        .await
        .ok();
    let has_tiles = tiles.is_some();
    let places3d = tiles
        .map(|(decl, _source)| decl.places3d.is_some())
        .unwrap_or(false);
    let style_ids = ctx.style_store.list().unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to list styles; serving assets with no styled variants");
        Vec::new()
    });
    let service_assets = state
        .router
        .effective_settings(collection)
        .and_then(|settings| settings.stac.as_ref())
        .map(|stac| stac.service_assets)
        .unwrap_or_default();
    AssetCapabilities {
        has_tiles,
        places3d,
        style_ids,
        service_assets,
    }
}

/// One batched read of this collection's per-item STAC metadata sidecar
/// (`#202`) for a whole page of items, keyed by feature id.
///
/// Empty for every collection that never opted in (`Router::
/// resolve_stac_metadata` answers `Ok(None)` without probing a driver), so
/// a pre-`#202` collection pays exactly one cheap router lookup and its
/// Items stay byte-identical. When a sidecar IS configured this is the one
/// extra round trip the whole page shares — never one per item, which is
/// why the ids are collected up front rather than looked up inside the
/// per-feature loop.
///
/// Errors propagate: a collection that declares `stac_metadata: true`
/// against an incapable driver (`CapabilityUnsupported`) or an
/// unprovisioned sidecar table (the driver's own named `StacTableMissing`)
/// is misconfigured, and serving Items silently stripped of the metadata
/// the operator asked for would look exactly like a collection that never
/// opted in at all.
async fn stac_sidecar_docs(
    ctx: &AppContext,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
    decl: &CollectionDecl,
    feature_ids: &[String],
) -> Result<HashMap<String, Value>, ApiError> {
    let state = ctx.current();
    let Some(source) = state
        .router
        .resolve_stac_metadata(tenant_id, catalog_id, collection_id)
        .await?
    else {
        return Ok(HashMap::new());
    };
    source
        .stac_metadata(decl, feature_ids)
        .await
        .map_err(ApiError::from)
}

/// One batched read of this collection's item-scoped asset records
/// (`#221`) for a whole page of items — the assets counterpart of
/// [`stac_sidecar_docs`], deliberately the same shape, resolved through the
/// same anchor driver, and paying the same one-round-trip-per-page cost.
///
/// Empty for every collection that never opted in (`Router::
/// resolve_item_assets` answers `Ok(None)` without probing a driver), so a
/// pre-`#221` collection pays exactly one cheap router lookup and its Items
/// keep the capability-derived asset map byte for byte. When the projection
/// IS configured this is the one extra round trip the whole page shares —
/// never one per item, which is why the ids are collected up front rather
/// than looked up inside the per-feature loop, and why the capability
/// exposes a batched `item_assets` rather than the per-key `get` the assets
/// API's own handlers use.
///
/// Errors propagate: a collection that declares `stac_item_assets: true`
/// against an incapable driver (`CapabilityUnsupported("assets")`) or an
/// unprovisioned `"<table>_assets"` table (the driver's own named
/// `AssetsTableMissing`) is misconfigured, and serving Items silently
/// stripped of the assets the operator asked for would look exactly like a
/// collection that never opted in at all.
async fn item_asset_records(
    ctx: &AppContext,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
    decl: &CollectionDecl,
    item_ids: &[String],
) -> Result<Vec<AssetRecordEntry>, ApiError> {
    let state = ctx.current();
    let Some(store) = state
        .router
        .resolve_item_assets(tenant_id, catalog_id, collection_id)
        .await?
    else {
        return Ok(Vec::new());
    };
    store
        .item_assets(decl, item_ids)
        .await
        .map_err(ApiError::from)
}

/// Every feature id on a page, in page order — the input
/// [`stac_sidecar_docs`] and [`item_asset_records`] batch. Ids are read
/// exactly the way each item loop already reads them (a missing/non-string
/// `id` degrades to `""`, which matches neither a sidecar row nor an asset
/// record — the assets lookup excludes the `""` collection-level scope in
/// the query itself, see `AssetRecordStore::item_assets`).
fn page_feature_ids(features: &[Value]) -> Vec<String> {
    features
        .iter()
        .map(|feature| {
            feature
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Cross-protocol links contributed for `resource` (`#186`), narrowed to
/// `anchor` and converted into this crate's own `Link` DTO. An `AppContext`
/// with nothing registered — every deployment or test that never called
/// `AppContext::with_link_contributors`, including all of this crate's own
/// tests — returns an empty vec without touching the router at all, so
/// those responses stay byte-for-byte what they were before the seam
/// existed.
///
/// `#220`: these links are what a collection's tiles/maps/3D surfaces are
/// *properly* expressed as — rel-tagged navigation a generic STAC browser
/// follows — while `crate::assets` carries the Asset Objects. An operator
/// who has switched `stac.service_assets` to `links` is served by these
/// alone; one who has not gets both, exactly as before.
async fn contributed_links(
    ctx: &AppContext,
    resource: &ResourceRef<'_>,
    anchor: LinkAnchor,
) -> Vec<Link> {
    if ctx.link_contributors.is_empty() {
        return Vec::new();
    }
    let state = ctx.current();
    ctx.link_contributors
        .contribute(&state.router, resource)
        .await
        .iter()
        .filter(|link| link.anchor == anchor)
        .map(Link::from)
        .collect()
}

/// Appends `contributed` to `links`, dropping any contribution the document
/// already carries under the same `rel` for the same `href` (`#220`).
///
/// The contribution seam is protocol-neutral by design: a contributor names
/// a sibling root's resource without knowing which root is serializing the
/// answer. Merging blindly would let one `links` array state the same
/// `(rel, href)` claim twice — legal per RFC 8288, but a claim stated twice
/// is not a claim stated better. The document's own link wins, because it
/// was built first and its shape is this crate's own contract. Identical in
/// intent and wording to `tellurion_features::handlers`' own merge; the two
/// `Link` DTOs are separate types, so the three lines are not shareable
/// without a trait nothing else would use.
fn extend_with_contributed(links: &mut Vec<Link>, contributed: Vec<Link>) {
    for link in contributed {
        if links
            .iter()
            .any(|existing| existing.rel == link.rel && existing.href == link.href)
        {
            continue;
        }
        links.push(link);
    }
}

/// GET /collections — a collection is listed under the same tolerant,
/// capability-agnostic rule `tellurion_features::handlers::list_collections`
/// uses: either the features or the tiles lane resolving is enough, since a
/// STAC Collection describes a servable dataset regardless of which
/// protocol actually serves its data.
///
/// Cursor-paginated (`#42`, `#59`), the same registry-seam mechanism
/// `tellurion_features::handlers::list_collections` already uses instead of
/// this crate's former direct scan of `AppConfig.collections` — reads
/// through `AppContext`'s registry seam (`RegistryReader::list_collections`)
/// so a catalog under the relational backend paginates here exactly as it
/// does on the features side, `limit`/`token` and all. A small registry
/// (fewer collections than `params::COLLECTIONS_DEFAULT_LIMIT`) still gets
/// exactly today's single-page response back — a `next` link only ever
/// appears once the registry actually has more to serve. A collection
/// filtered out below (neither lane resolves) can leave a page shorter than
/// the requested `limit` even when the registry has more beyond it; the
/// `next` link, not the page's length, is what a client must follow to know
/// whether more remain — same caveat `tellurion_features`' own doc already
/// states for the identical situation.
pub async fn list_collections(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(raw_query): Query<CollectionsQueryParams>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let tenant_ext = tenant_of(&params);
    let catalog_ext = catalog_of(&params);
    let state = ctx.current();
    let self_path = state.config.server.public_href(uri.path());
    let root_path = stac_root_of(&self_path);
    let page_request = parse_collections_query(&raw_query)?;

    let page = state
        .registry
        .list_collections(&catalog_id, page_request)
        .await?;

    let mut collections = Vec::with_capacity(page.items.len());
    for decl in &page.items {
        let has_features = state
            .router
            .resolve_features(&tenant_id, &catalog_id, &decl.id)
            .await
            .is_ok();
        let caps = asset_capabilities(&ctx, &tenant_id, &catalog_id, &decl.id).await;
        // `#36` projection slice: the raster lane joins the tolerance rule.
        // A COG/Zarr-backed collection resolves neither `FeatureSource` nor
        // `TileSource`, yet is exactly as servable a dataset as an MVT one —
        // the canonical descriptor's own `#37` capability probe
        // (`Router::canonical_descriptor`'s `has_tiles`) and the Features
        // root's `/collections` listing both already treat raster as tiles
        // for this purpose, and the STAC root omitting what its sibling
        // root advertises was an inconsistency, not a decision. Probed only
        // when the cheaper answers already said no.
        let has_raster = !has_features
            && !caps.has_tiles
            && state
                .router
                .resolve_raster(&tenant_id, &catalog_id, &decl.id)
                .await
                .is_ok();
        if !has_features && !caps.has_tiles && !has_raster {
            continue;
        }
        // `#34`: a collection the subject isn't authorized to see is
        // omitted from the listing entirely, not merely refused on direct
        // access (`get_collection`, below) — a private collection should
        // not be advertised.
        // `#188`: a probe, not a served request — see `RateCharge`.
        if authorize_lane(
            &state,
            ctx.rate_counter.as_ref(),
            &headers,
            &tenant_id,
            &catalog_id,
            &decl.id,
            true,
            RateCharge::Skip,
        )
        .await
        .is_err()
        {
            continue;
        }

        let canonical = resolved_canonical(&ctx, &tenant_id, &catalog_id, decl).await;
        let href = format!("{}/{}", self_path.trim_end_matches('/'), decl.external_id());
        let mut links = vec![
            Link::new(root_path.clone(), "root", JSON_MEDIA_TYPE),
            Link::new(href.clone(), "self", JSON_MEDIA_TYPE),
        ];
        // `#245`: the items link, gated on the SAME `has_features` probe
        // that decided this collection is listed at all — never on
        // `has_tiles`. `list_items` resolves the features lane strictly (a
        // tiles-only collection has no rows to page), so advertising it for
        // a tiles-only collection would put a link in this document that
        // this same server answers `404` to. See [`ITEMS_REL`] for the two
        // requirements this satisfies; `tellurion_features::handlers::
        // collection_summary` gates its own `items` link on the identical
        // predicate at the Features root.
        if has_features {
            links.push(Link::new(
                format!("{href}/items"),
                ITEMS_REL,
                GEOJSON_MEDIA_TYPE,
            ));
        }
        // `#186`: capability-derived cross-protocol links, appended after
        // this crate's own so existing consumers' link order is untouched.
        let contributed = contributed_links(
            &ctx,
            &ResourceRef {
                tenant: &tenant_ext,
                catalog: &catalog_ext,
                collection: decl.external_id(),
                item_id: None,
                base_url: public_base_url(&state.config.server),
                tenant_id: &tenant_id,
                catalog_id: &catalog_id,
                collection_id: &decl.id,
            },
            LinkAnchor::Collection,
        )
        .await;
        extend_with_contributed(&mut links, contributed);
        let assets = collection_assets(
            &state.config.server,
            &tenant_ext,
            &catalog_ext,
            decl.external_id(),
            &caps,
        );
        collections.push(to_stac_collection(
            canonical.as_ref(),
            decl.external_id(),
            links,
            assets,
        ));
    }

    let mut links = vec![
        Link::new(root_path, "root", JSON_MEDIA_TYPE),
        Link::new(
            collections_href(&self_path, &raw_query, None),
            "self",
            JSON_MEDIA_TYPE,
        ),
    ];
    if let Some(next_token) = page.next.as_deref() {
        links.push(Link::new(
            collections_href(&self_path, &raw_query, Some(next_token)),
            "next",
            JSON_MEDIA_TYPE,
        ));
    }

    let body = StacCollectionsResponse { links, collections };

    let mut response = (StatusCode::OK, Json(body)).into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    Ok(response)
}

/// GET /collections/{cid} — same features-or-tiles tolerance as
/// `list_collections`: tries the features lane first, falls through to the
/// tiles lane, whose error (if that lane is unrouted too) is what a
/// genuinely unknown collection id surfaces as.
///
/// Also serves an ISO 19115 (19139 XML) alternate representation of this
/// same resource (`#50`) — `?f=xml` or an `Accept: application/vnd.iso.
/// 19139+xml` header selects it (see `wants_iso19139`); the default STAC
/// Collection JSON always carries an `alternate` link to it, so a client
/// need not already know the query parameter to discover it. No new route,
/// no new protocol crate: this is the same resource, a second
/// representation of the one `CanonicalDescriptor` already resolved for the
/// JSON body — see `crate::iso19139`'s own module doc for the mapping
/// itself and its documented omissions.
pub async fn get_collection(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(query): Query<GetCollectionQueryParams>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    authorize_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        true,
        RateCharge::Charge,
    )
    .await?;

    let features = state
        .router
        .resolve_features(&tenant_id, &catalog_id, &collection_id)
        .await;
    // `#245`: whether `/items` under this collection would actually answer —
    // the same probe `list_items` itself makes, kept rather than discarded by
    // the fallback below, because a tiles-only collection is describable here
    // but has no items resource to link to.
    let has_features = features.is_ok();
    let decl = match features {
        Ok((decl, _source)) => decl,
        Err(_) => {
            match state
                .router
                .resolve_tiles(&tenant_id, &catalog_id, &collection_id)
                .await
            {
                Ok((decl, _source)) => decl,
                // `#36` projection slice: same raster tolerance
                // `list_collections` applies (see its own comment). The
                // tiles lane's error — the shape every pre-raster caller
                // already received for a genuinely unknown or
                // capability-less collection — is preserved when the raster
                // lane cannot serve either.
                Err(tiles_error) => match state
                    .router
                    .resolve_raster(&tenant_id, &catalog_id, &collection_id)
                    .await
                {
                    Ok((decl, _source)) => decl,
                    Err(_) => return Err(tiles_error.into()),
                },
            }
        }
    };

    let canonical = resolved_canonical(&ctx, &tenant_id, &catalog_id, &decl).await;

    if wants_iso19139(query.f.as_deref(), &headers) {
        let xml = to_iso19139(canonical.as_ref(), &cid);
        let mut response = (StatusCode::OK, xml).into_response();
        set_content_type(&mut response, ISO19139_MEDIA_TYPE);
        return Ok(response);
    }

    let self_path = state.config.server.public_href(uri.path());
    let collections_path = self_path
        .strip_suffix(&format!("/{cid}"))
        .unwrap_or(&self_path)
        .to_string();
    let root_path = stac_root_of(&collections_path);
    let mut links = vec![
        Link::new(root_path.clone(), "root", JSON_MEDIA_TYPE),
        Link::new(self_path.clone(), "self", JSON_MEDIA_TYPE),
        Link::new(root_path, "parent", JSON_MEDIA_TYPE),
        Link::new(
            format!("{self_path}?f={ISO19139_QUERY_FORMAT}"),
            "alternate",
            ISO19139_MEDIA_TYPE,
        ),
    ];
    // `#245`: Requirement 19 `/req/core/sfc-md-success` — this response's
    // links must include every link the `/collections` entry for the same
    // collection carries, so the items link is emitted here under exactly
    // the gate `list_collections` applies (see [`ITEMS_REL`]).
    if has_features {
        links.push(Link::new(
            format!("{self_path}/items"),
            ITEMS_REL,
            GEOJSON_MEDIA_TYPE,
        ));
    }
    // `#186`: same capability-derived cross-protocol links the listing
    // appends per collection — see `contributed_links`'s own doc.
    let contributed = contributed_links(
        &ctx,
        &ResourceRef {
            tenant: &tenant_of(&params),
            catalog: &catalog_of(&params),
            collection: &cid,
            item_id: None,
            base_url: public_base_url(&state.config.server),
            tenant_id: &tenant_id,
            catalog_id: &catalog_id,
            collection_id: &collection_id,
        },
        LinkAnchor::Collection,
    )
    .await;
    extend_with_contributed(&mut links, contributed);

    let caps = asset_capabilities(&ctx, &tenant_id, &catalog_id, &collection_id).await;
    let assets = collection_assets(
        &state.config.server,
        &tenant_of(&params),
        &catalog_of(&params),
        &cid,
        &caps,
    );

    let body = to_stac_collection(canonical.as_ref(), &cid, links, assets);
    let mut response = (StatusCode::OK, Json(body)).into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    Ok(response)
}

/// GET /collections/{cid}/items (`#36` slice B): STAC Items, paginated the
/// same bbox/datetime/limit + keyset-token way
/// `tellurion_features::handlers::list_items` already does — `parse_items_query`
/// (this crate's own, see `params.rs`'s doc) builds the identical
/// `tellurion_core::ItemsQuery` type, `Router::resolve_features` resolves
/// the same features lane, and `source.items` is the exact same
/// `FeatureSource` call `tellurion-features` makes; only the response
/// shape differs. Items strictly require the features capability — unlike
/// `list_collections`/`get_collection`, there is no tiles-only fallback,
/// since an item IS a row and a tiles-only driver has none to offer; a
/// `CapabilityUnsupported` here surfaces as the same 404
/// `resolve_collection` gives a genuinely unknown collection id.
pub async fn list_items(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(raw_query): Query<ItemsQueryParams>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;

    let (decl, source) = state
        .router
        .resolve_features(&tenant_id, &catalog_id, &collection_id)
        .await?;
    // `#34`: same AND-merge treatment `tellurion_features::handlers::
    // list_items` gives its own items-list lane — this crate's `/items`
    // rides the identical `ItemsQuery`/`FeatureSource::items` call.
    let policy_filter = authorize_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        source.filter_capable(),
        RateCharge::Charge,
    )
    .await?;
    let mut query = parse_items_query(&raw_query)?;
    // `#255`: this lane's `bbox` is CRS84 and has no `bbox-crs` to say
    // otherwise, so a collection whose storage is not CRS84 under a driver
    // that cannot transform is refused by name here rather than served rows
    // selected by comparing degrees against metres. One named target
    // collection, so this is a 400 outright — the fan-out's skip-and-report
    // tolerance (`run_cursor_search`) exists only because a `/search` has
    // other collections to answer from.
    if let Some(reason) = unservable_bbox_reason(&cid, &decl, source.as_ref(), query.bbox.is_some())
    {
        return Err(ApiError::from(CoreError::Invalid(reason)));
    }
    if let Some(grant_filter) = policy_filter {
        query.filter = Some(match query.filter.take() {
            None => grant_filter,
            Some(existing) => Filter::And(vec![existing, grant_filter]),
        });
    }
    let page = source.items(&decl, &query).await?;

    let path = state.config.server.public_href(uri.path());
    let collection_href = path.strip_suffix("/items").unwrap_or(&path).to_string();
    let root_path = stac_root_of(&collection_href);

    let caps = asset_capabilities(&ctx, &tenant_id, &catalog_id, &collection_id).await;
    let assets = collection_assets(
        &state.config.server,
        &tenant_of(&params),
        &catalog_of(&params),
        &cid,
        &caps,
    );
    // `#186`: item-anchored cross-protocol links are a per-collection fact
    // (tiles/stylesheets don't vary per row — see `ResourceRef::item_id`'s
    // own doc), so one contribution serves every item on this page instead
    // of one router probe per row.
    let item_contributed = contributed_links(
        &ctx,
        &ResourceRef {
            tenant: &tenant_of(&params),
            catalog: &catalog_of(&params),
            collection: &cid,
            item_id: None,
            base_url: public_base_url(&state.config.server),
            tenant_id: &tenant_id,
            catalog_id: &catalog_id,
            collection_id: &collection_id,
        },
        LinkAnchor::Item,
    )
    .await;

    // `#202`/`#221`: one sidecar lookup and one asset-record lookup for the
    // whole page, before the per-item loop — see `stac_sidecar_docs` and
    // `item_asset_records`.
    let page_ids = page_feature_ids(&page.features_geojson);
    let sidecar = stac_sidecar_docs(
        &ctx,
        &tenant_id,
        &catalog_id,
        &collection_id,
        &decl,
        &page_ids,
    )
    .await?;
    let assets = PageItemAssets::new(
        &state.config.server,
        assets,
        &tenant_of(&params),
        &catalog_of(&params),
        &cid,
        &item_asset_records(
            &ctx,
            &tenant_id,
            &catalog_id,
            &collection_id,
            &decl,
            &page_ids,
        )
        .await?,
    );

    // `#36`: this collection's derived `proj:*` fields, built once for the
    // whole page — a per-collection fact (the effective decl's own
    // `projection`/`srid` carriers), never a per-item probe.
    let projection = derive_projection(decl.projection.as_ref(), decl.srid);
    let features: Vec<Value> = page
        .features_geojson
        .into_iter()
        .map(|feature| {
            let item_id = feature
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut item_links = vec![
                Link::new(root_path.clone(), "root", JSON_MEDIA_TYPE),
                Link::new(
                    format!("{collection_href}/items/{item_id}"),
                    "self",
                    GEOJSON_MEDIA_TYPE,
                ),
                Link::new(collection_href.clone(), "collection", JSON_MEDIA_TYPE),
                Link::new(collection_href.clone(), "parent", JSON_MEDIA_TYPE),
            ];
            extend_with_contributed(&mut item_links, item_contributed.clone());
            to_stac_item(
                feature,
                &cid,
                decl.datetime.as_deref(),
                assets.for_item(&item_id),
                item_links,
                sidecar.get(&item_id),
                projection.as_ref(),
            )
        })
        .collect();

    let mut links = vec![
        Link::new(root_path, "root", JSON_MEDIA_TYPE),
        Link::new(
            items_href(&path, &raw_query, None),
            "self",
            GEOJSON_MEDIA_TYPE,
        ),
        Link::new(collection_href, "collection", JSON_MEDIA_TYPE),
    ];
    if let Some(next_token) = page.next_token.as_deref() {
        links.push(Link::new(
            items_href(&path, &raw_query, Some(next_token)),
            "next",
            GEOJSON_MEDIA_TYPE,
        ));
    }

    let body = StacItemCollectionResponse {
        type_: "FeatureCollection",
        number_returned: features.len() as u64,
        number_matched: page.number_matched,
        features,
        links,
        // Single-collection `/items`, not a `/search` fan-out — nothing was
        // ever a candidate to skip. `#255`'s own bbox refusal on this lane is
        // a 400 above, for the same reason.
        filter_incapable_collections: Vec::new(),
        search_incapable_collections: Vec::new(),
        bbox_incapable_collections: Vec::new(),
    };

    let mut response = (StatusCode::OK, Json(body)).into_response();
    set_content_type(&mut response, GEOJSON_MEDIA_TYPE);
    Ok(response)
}

/// GET /collections/{cid}/items/{fid} (`#36` slice B): a single STAC Item,
/// same features-lane resolution and `FeatureSource::item` call
/// `tellurion_features::handlers::get_item` makes.
pub async fn get_item(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let fid = require_param(&params, "fid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    let (decl, source) = state
        .router
        .resolve_features(&tenant_id, &catalog_id, &collection_id)
        .await?;
    // `#34`: single-item GET pushes the grant filter into the same
    // single-row query `FeatureSource::item` now compiles it into, when the
    // resolved driver can compile one — same `source.filter_capable()`
    // check `tellurion_features::handlers::get_item` uses. An item the
    // filter excludes comes back `Ok(None)`, indistinguishable from a
    // genuinely absent id.
    let policy_filter = authorize_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        source.filter_capable(),
        RateCharge::Charge,
    )
    .await?;

    let feature = source
        .item(&decl, &fid, policy_filter.as_ref())
        .await?
        .ok_or(CoreError::NotFound)?;

    let path = state.config.server.public_href(uri.path());
    let collection_href = path
        .rsplit_once("/items/")
        .map(|(base, _)| base.to_string())
        .unwrap_or_else(|| path.clone());
    let root_path = stac_root_of(&collection_href);

    let caps = asset_capabilities(&ctx, &tenant_id, &catalog_id, &collection_id).await;
    let assets = collection_assets(
        &state.config.server,
        &tenant_of(&params),
        &catalog_of(&params),
        &cid,
        &caps,
    );

    let mut links = vec![
        Link::new(root_path, "root", JSON_MEDIA_TYPE),
        Link::new(path, "self", GEOJSON_MEDIA_TYPE),
        Link::new(collection_href.clone(), "collection", JSON_MEDIA_TYPE),
        Link::new(collection_href, "parent", JSON_MEDIA_TYPE),
    ];
    // `#186`: item-anchored cross-protocol links — see `contributed_links`'s
    // own doc.
    let contributed = contributed_links(
        &ctx,
        &ResourceRef {
            tenant: &tenant_of(&params),
            catalog: &catalog_of(&params),
            collection: &cid,
            item_id: Some(&fid),
            base_url: public_base_url(&state.config.server),
            tenant_id: &tenant_id,
            catalog_id: &catalog_id,
            collection_id: &collection_id,
        },
        LinkAnchor::Item,
    )
    .await;
    extend_with_contributed(&mut links, contributed);

    // `#202`/`#221`: the single-item lane batches a one-element page — the
    // same one round trip each `list_items` pays, never a second code path.
    let sidecar = stac_sidecar_docs(
        &ctx,
        &tenant_id,
        &catalog_id,
        &collection_id,
        &decl,
        std::slice::from_ref(&fid),
    )
    .await?;
    let assets = PageItemAssets::new(
        &state.config.server,
        assets,
        &tenant_of(&params),
        &catalog_of(&params),
        &cid,
        &item_asset_records(
            &ctx,
            &tenant_id,
            &catalog_id,
            &collection_id,
            &decl,
            std::slice::from_ref(&fid),
        )
        .await?,
    );

    let body = to_stac_item(
        feature,
        &cid,
        decl.datetime.as_deref(),
        assets.for_item(&fid),
        links,
        sidecar.get(&fid),
        derive_projection(decl.projection.as_ref(), decl.srid).as_ref(),
    );
    let mut response = (StatusCode::OK, Json(body)).into_response();
    set_content_type(&mut response, GEOJSON_MEDIA_TYPE);
    Ok(response)
}

// -- /search (`#36` slice C, STAC API - Item Search) ------------------------

/// Everything `run_ids_search`/`run_cursor_search`/`resolve_collection_features`
/// need about the current request besides the collection being resolved
/// right now — bundled so those functions stay under clippy's
/// too-many-arguments threshold instead of threading six same-lifetime
/// parameters through each call individually.
struct SearchContext<'a> {
    ctx: &'a AppContext,
    tenant_id: &'a str,
    catalog_id: &'a str,
    tenant_ext: &'a str,
    catalog_ext: &'a str,
    root_path: &'a str,
    /// The same server configuration snapshot used to build the search
    /// response's root/self/next links. Holding it here keeps assets and
    /// contributed links on that snapshot if configuration reloads while
    /// this request is in flight.
    server: &'a ServerConfig,
    /// `#34`: the request's credential, re-derived into a `Subject` by
    /// [`authorize_search_collection`] for each candidate collection —
    /// unlike every other handler in this crate, a fan-out search may touch
    /// many collections per request, so this is deliberately just the raw
    /// credential (cheap to hold), not a pre-derived `Subject`.
    credential: &'a Credential,
    /// `#188`: whether this request has already charged its rate ceilings.
    /// A fan-out authorizes many collections but serves ONE response, and a
    /// ceiling counts requests — so [`authorize_search_collection`] charges
    /// at the first collection it actually authorizes and probes for every
    /// one after. An atomic rather than a `Cell`: the whole
    /// `SearchContext` is held by `&` across `.await` points inside an axum
    /// handler, whose future must be `Send` — a `Cell` would quietly make
    /// it not, and the fan-out loop is sequential either way.
    rate_charged: std::sync::atomic::AtomicBool,
}

/// `GET /search` (`#36` slice C). See `crate::search`'s module doc for the
/// per-parameter GET encoding this parses, and [`execute_search`]'s doc for
/// how a parsed request is served.
pub async fn search_get(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(raw_query): Query<SearchQueryParams>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let request = parse_get(&raw_query).map_err(ApiError::from)?;
    let href_params = SearchHrefParams::from(&raw_query);
    let credential = extract_credential(&headers);
    execute_search(
        &ctx,
        &params,
        request,
        &href_params,
        uri.path(),
        &credential,
    )
    .await
}

/// `POST /search` (`#36` slice C). Same execution path as [`search_get`] —
/// only the request-shape parsing differs (`crate::search::parse_post`); the
/// response's `self`/`next` links are still built as followable `GET` hrefs
/// (`SearchHrefParams::from(&SearchBody)`) — see that type's own doc for
/// why.
pub async fn search_post(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Json(body): Json<SearchBody>,
) -> Result<Response, ApiError> {
    let request = parse_post(&body).map_err(ApiError::from)?;
    let href_params = SearchHrefParams::from(&body);
    let credential = extract_credential(&headers);
    execute_search(
        &ctx,
        &params,
        request,
        &href_params,
        uri.path(),
        &credential,
    )
    .await
}

/// Executes a parsed `/search` request (`#36` slice C, STAC API - Item
/// Search) — the single implementation `search_get`/`search_post` share.
///
/// A `q`-bearing request (`#181`) takes its own third path before either
/// mode below: [`run_q_search`], dispatched to the freshness-gated search
/// lane (`Router::resolve_search`) rather than the features lane both
/// modes below ride — see that function's doc for the agreement gates.
/// `crate::search::validate_q` has already refused every parameter
/// combination that path cannot express, so the two worlds never mix
/// inside one request.
///
/// Otherwise, two independent narrowing modes, chosen by whether
/// `request.ids` is non-empty:
///
/// - **`ids` present**: direct per-id lookups (`FeatureSource::item`), never
///   through `items()`/`ItemsQuery` at all — see [`run_ids_search`]'s own
///   doc for why `ids` can't ride the same `Filter`/SQL path
///   `bbox`/`datetime`/`filter`/`intersects` share (the primary key `ids`
///   narrows on is not a filterable *property* in this system's model:
///   `properties_expr` strips the pk column out of a row's GeoJSON
///   `properties` entirely, and `tellurion_core::filter::validate` only ever
///   accepts the geometry/datetime columns or a declared attribute as a
///   filterable property). `bbox`/`datetime`/`filter`/`intersects`, if also
///   supplied, are NOT combined with `ids` in this slice — a documented
///   simplification, not a spec requirement either way (the item-search
///   spec itself leaves `ids`'s interaction with other parameters
///   unspecified).
/// - **Otherwise**: the ordinary keyset-paged query path
///   ([`run_cursor_search`]) — a direct, single `FeatureSource::items` call
///   when `request.collections` narrows to exactly one collection (as cheap
///   as `/items`'s own `list_items`), or a fan-out across every collection
///   this catalog owns (or every collection named in `request.collections`,
///   when more than one is named) otherwise. The fan-out is a real cost this
///   module is honest about: each page may issue one `resolve_features` +
///   (only when filtering) one `collection_descriptor` + one `items()` call
///   *per collection touched to fill the page* — bounded by how many
///   collections it takes to reach `limit` items, not by the catalog's total
///   collection count, but still `O(collections touched)` round trips per
///   page where the single-collection path is always exactly one.
///   Collections are walked in a fixed, alphabetically-sorted order and
///   exhausted one at a time (concatenation, not a true interleaved k-way
///   merge): there is no cross-collection sort key to interleave on in this
///   slice (`sortby` is explicitly out of scope, per the issue's own
///   non-goals list), so a stable per-collection-then-per-item order is the
///   honest, simplest answer.
///
/// Capability/validation mismatches (a collection whose `FeatureSource`
/// can't compile the composed `filter`/`intersects` predicate, or whose
/// descriptor rejects one of the filter's properties) are handled
/// differently by collection count: exactly one target collection means the
/// request unambiguously names an unfilterable collection, so it 400s (same
/// message shape `tellurion_features`'s own `/items` handler already uses
/// for the identical situation); more than one target collection means a
/// fan-out where some collections may legitimately lack the capability while
/// others don't — those are skipped rather than failing the whole
/// cross-collection search. Deliberate, documented judgment call: a global
/// search across heterogeneous collections should not degrade to "the whole
/// search 400s because one collection out of many can't filter." The skip is
/// not silent, though: `run_cursor_search` collects the external ids of
/// every collection dropped for exactly this reason (never for an
/// unresolvable id or a policy denial — those are a different, pre-existing
/// kind of omission this field does not cover) into the response body's
/// `filterIncapableCollections` (`StacItemCollectionResponse`), so a caller
/// that cares can tell a filtered fan-out was incomplete without having to
/// separately enumerate the catalog and diff it against what came back.
async fn execute_search(
    ctx: &AppContext,
    params: &HashMap<String, String>,
    request: SearchRequest,
    href_params: &SearchHrefParams,
    request_path: &str,
    credential: &Credential,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(ctx, params).await?;
    let tenant_ext = tenant_of(params);
    let catalog_ext = catalog_of(params);
    let state = ctx.current();
    let self_path = state.config.server.public_href(request_path);
    let root_path = self_path
        .strip_suffix("/search")
        .unwrap_or(&self_path)
        .to_string();

    let sc = SearchContext {
        ctx,
        tenant_id: &tenant_id,
        catalog_id: &catalog_id,
        tenant_ext: &tenant_ext,
        catalog_ext: &catalog_ext,
        root_path: &root_path,
        server: &state.config.server,
        credential,
        rate_charged: std::sync::atomic::AtomicBool::new(false),
    };

    // `#181`: a `q`-bearing request rides the search lane, never the
    // features lane below. It is always token-less (`validate_q` refused
    // `q` + `token` at parse time) and always single-page: the derived-
    // index query has no cursor to encode, so no `next` link is ever
    // built — a deliberate, documented limit of this slice, not an
    // oversight (`SearchQuery`'s own doc carries the widening rule).
    if let Some(q) = request.q.as_deref() {
        let collections = candidate_collection_ids(ctx, &catalog_id, &request.collections).await;
        let (features, search_incapable_collections) =
            run_q_search(&sc, &collections, q, request.limit).await?;
        let links = vec![
            Link::new(root_path.clone(), "root", JSON_MEDIA_TYPE),
            Link::new(
                search_href(&self_path, href_params, None),
                "self",
                GEOJSON_MEDIA_TYPE,
            ),
        ];
        let body = StacItemCollectionResponse {
            type_: "FeatureCollection",
            number_returned: features.len() as u64,
            // The derived-index query reports no total; an invented one
            // would be exactly the count/features disagreement `#181`'s
            // gates exist to prevent.
            number_matched: None,
            features,
            links,
            // `q` never composes a `filter`/`intersects` predicate
            // (`validate_q`), so the filter-incapable list has nothing to
            // ever report in this mode. The same `validate_q` refuses `q`
            // alongside `bbox` outright, so neither has `#255`'s.
            filter_incapable_collections: Vec::new(),
            search_incapable_collections,
            bbox_incapable_collections: Vec::new(),
        };
        let mut response = (StatusCode::OK, Json(body)).into_response();
        set_content_type(&mut response, GEOJSON_MEDIA_TYPE);
        return Ok(response);
    }

    // Once present, `token` is authoritative for both the paging *mode*
    // (`ids` vs the ordinary cursor walk) and the stable, alphabetically-
    // sorted collection list a multi-page search is walking — re-deriving
    // that list from `request.collections`/live config on every page would
    // let a concurrent config reload (a collection added/removed between
    // page 1 and page 2) silently reshuffle or duplicate results mid-walk.
    // A fresh, token-less request always re-derives it from current config.
    let plan = match &request.token {
        Some(token) => SearchToken::decode(token).map_err(ApiError::from)?,
        None if !request.ids.is_empty() => SearchToken::Ids {
            collections: candidate_collection_ids(ctx, &catalog_id, &request.collections).await,
            ids: request.ids.clone(),
            start: 0,
        },
        None => SearchToken::Cursor {
            collections: candidate_collection_ids(ctx, &catalog_id, &request.collections).await,
            idx: 0,
            cursor: None,
        },
    };

    let page = match plan {
        SearchToken::Ids {
            collections,
            ids,
            start,
        } => run_ids_search(&sc, &collections, &ids, start, request.limit).await?,
        SearchToken::Cursor {
            collections,
            idx,
            cursor,
        } => run_cursor_search(&sc, &collections, idx, cursor, &request).await?,
    };
    let SearchPage {
        features,
        next_token,
        number_matched,
        filter_incapable_collections,
        bbox_incapable_collections,
    } = page;

    let mut links = vec![
        Link::new(root_path.clone(), "root", JSON_MEDIA_TYPE),
        Link::new(
            search_href(&self_path, href_params, None),
            "self",
            GEOJSON_MEDIA_TYPE,
        ),
    ];
    if let Some(next_token) = next_token.as_deref() {
        links.push(Link::new(
            search_href(&self_path, href_params, Some(next_token)),
            "next",
            GEOJSON_MEDIA_TYPE,
        ));
    }

    let body = StacItemCollectionResponse {
        type_: "FeatureCollection",
        number_returned: features.len() as u64,
        number_matched,
        features,
        links,
        filter_incapable_collections,
        // Only a `q`-bearing request (handled above) ever dispatches to the
        // search lane, so the `q`-less modes have nothing to report here.
        search_incapable_collections: Vec::new(),
        bbox_incapable_collections,
    };
    let mut response = (StatusCode::OK, Json(body)).into_response();
    set_content_type(&mut response, GEOJSON_MEDIA_TYPE);
    Ok(response)
}

/// One page of `/search` results, plus everything the response must state
/// honestly about what the fan-out left out — the shared return of
/// [`run_ids_search`] and [`run_cursor_search`]. A named struct rather than
/// the four-wide tuple it was through `#248`: `#255` gave the pair a third
/// `Vec<String>` list, and at that width a positional tuple stops saying which
/// list is which at either end.
struct SearchPage {
    features: Vec<Value>,
    next_token: Option<String>,
    number_matched: Option<u64>,
    filter_incapable_collections: Vec<String>,
    bbox_incapable_collections: Vec<String>,
}

/// Every collection this catalog owns, as external ids, alphabetically
/// sorted — the fixed cross-collection walk order `run_cursor_search`/
/// `run_ids_search` rely on for a stable paging sequence — narrowed to
/// `requested` when non-empty. Unlike `list_collections`, this does NOT
/// filter out collections that fail to resolve any capability at all: an
/// unresolvable collection is caught later, per-collection, by
/// `resolve_collection_features` (and skipped there in fan-out mode) —
/// checking it here would cost an extra `Router` round trip per candidate
/// before the search even starts.
async fn candidate_collection_ids(
    ctx: &AppContext,
    catalog_id: &str,
    requested: &[String],
) -> Vec<String> {
    let state = ctx.current();
    let mut ids: Vec<String> = state
        .config
        .collections
        .iter()
        .filter(|decl| decl.catalog == catalog_id)
        .map(|decl| decl.external_id().to_string())
        .filter(|ext| requested.is_empty() || requested.contains(ext))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// A resolved collection plus its materialized STAC assets, cached by
/// [`run_ids_search`] across the `(collections, ids)` cross product's
/// consecutive same-collection entries. Its own named type, rather than an
/// inline tuple, purely to stay under clippy's type-complexity threshold.
struct ResolvedCollection {
    decl: CollectionDecl,
    source: Arc<dyn FeatureSource>,
    /// `#221`: the capability-derived map plus this collection's own
    /// item-scoped asset records for EVERY id the request named, folded
    /// once when the entry is built — the same per-collection-not-per-id
    /// batching `sidecar` below documents, and the reason `ids` mode's
    /// `(collections, ids)` walk still costs one asset read per collection
    /// touched rather than one per pair.
    assets: PageItemAssets,
    /// `#34`: this collection's resolved grant filter for the requesting
    /// subject (`None` for unrestricted access) — cached alongside
    /// `decl`/`source` so every `(collection, id)` pair sharing this entry
    /// reuses the one policy evaluation instead of re-authorizing per id.
    policy_filter: Option<Filter>,
    /// `#186`: this collection's item-anchored cross-protocol links, cached
    /// for the same per-collection-not-per-id reason as `assets` — the
    /// contributed set is a per-collection fact (see `ResourceRef::
    /// item_id`'s own doc), so one contribution serves every id resolved
    /// against this entry.
    contributed: Vec<Link>,
    /// `#202`: this collection's STAC metadata sidecar rows for EVERY id
    /// the request named, fetched once when the entry is built. `ids` mode
    /// walks the `(collections, ids)` cross product one pair at a time, so
    /// batching per resolved collection — not per pair — is what keeps the
    /// sidecar at one round trip per collection touched instead of one per
    /// item, the same per-collection-not-per-id caching `assets` and
    /// `contributed` above already do. Empty for a collection with no
    /// sidecar configured.
    sidecar: HashMap<String, Value>,
    /// `#36`: this collection's derived `proj:*` fields, computed once when
    /// the entry is built (a per-collection fact off the effective decl's
    /// `projection`/`srid` carriers) — same per-collection-not-per-id
    /// caching as everything above. `None` for a collection whose driver
    /// knows nothing.
    projection: Option<DerivedProjection>,
}

/// Resolves `collection_ext` (an external id) to its internal id, effective
/// decl, and `FeatureSource` — the same `resolve_collection`-then-
/// `resolve_features` sequence `list_items`/`get_item` already run for a
/// single collection, factored out here since `run_cursor_search`/
/// `run_ids_search` call it once per candidate collection touched.
async fn resolve_collection_features(
    sc: &SearchContext<'_>,
    collection_ext: &str,
) -> tellurion_core::Result<(String, CollectionDecl, Arc<dyn FeatureSource>)> {
    let state = sc.ctx.current();
    let collection_id = state
        .resolver
        .resolve_collection(sc.catalog_id, collection_ext)
        .await?;
    let (decl, source) = state
        .router
        .resolve_features(sc.tenant_id, sc.catalog_id, &collection_id)
        .await?;
    Ok((collection_id, decl, source))
}

/// The `#34` policy checkpoint for one candidate collection touched during
/// `/search`'s fan-out. Deliberately separate from
/// `handlers::authorize_lane` (used by every single-collection endpoint in
/// this crate): a `Deny` here is not necessarily fatal to the whole
/// request — see `run_cursor_search`/`run_ids_search`'s own call sites for
/// how a denial is handled differently for a single explicitly-named
/// target collection (a real 401/403) versus a fan-out across many (the
/// collection is silently skipped, the same tolerant treatment those
/// functions already give a collection that fails to resolve at all or
/// lacks a needed capability).
///
/// `#188`: a rate refusal, unlike a denial, is NOT skipped in a fan-out.
/// The two mean different things — a denial is about this collection (skip
/// it, serve the rest, and don't advertise its existence), while a ceiling
/// is about the caller (it has spent its budget, and quietly returning a
/// short page would answer a different question than the one asked). So a
/// refusal fails the whole search, in every mode, single-collection or not.
/// The charge itself happens once per request — at the first collection
/// this actually authorizes — via `SearchContext::rate_charged`.
async fn authorize_search_collection(
    sc: &SearchContext<'_>,
    collection_id: &str,
    lane_supports_filter: bool,
) -> tellurion_core::Result<SearchDecision> {
    let state = sc.ctx.current();
    let Some(authorizer) = state.authorizer.as_ref() else {
        return Ok(SearchDecision::Allow { filter: None });
    };
    let subject = authorizer.subject(sc.credential).await;
    let visibility = state
        .router
        .effective_visibility(collection_id)
        .cloned()
        .unwrap_or_default();
    let resource = ResourceContext {
        tenant_id: sc.tenant_id,
        catalog_id: sc.catalog_id,
        collection_id,
        lane: PolicyLane::Stac,
        visibility: &visibility,
    };
    let filter =
        match policy::authorize_resource(&state.config, &resource, &subject, lane_supports_filter)?
        {
            PolicyDecision::Allow { filter } => filter,
            PolicyDecision::Deny => return Ok(SearchDecision::Deny),
        };
    let charge = if sc
        .rate_charged
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        RateCharge::Skip
    } else {
        RateCharge::Charge
    };
    let verdict = policy::enforce_rate_limits(
        &state.config,
        &resource,
        &subject,
        Some(sc.ctx.rate_counter.as_ref()),
        charge,
    )
    .await;
    match verdict {
        RateVerdict::Permitted => Ok(SearchDecision::Allow { filter }),
        RateVerdict::Refused(refusal) => Ok(SearchDecision::RateLimited(refusal)),
    }
}

/// [`authorize_search_collection`]'s verdict — `PolicyDecision` plus the
/// rate-ceiling refusal (`#188`) a fan-out must not treat as a skippable
/// denial. Local to this crate: `PolicyDecision` is `tellurion-core`'s
/// answer to "may this subject see this resource," which is deliberately
/// still a two-answer question there (see `policy.rs`'s own doc for why the
/// ceiling is charged by a separate call).
enum SearchDecision {
    Allow { filter: Option<Filter> },
    Deny,
    RateLimited(tellurion_core::RateRefusal),
}

/// Composes `request.filter`/`request.intersects` into one `Filter` this
/// collection's `FeatureSource` can compile, or `None` when neither was
/// supplied. `intersects` becomes `Filter::Intersects` against `decl`'s own
/// resolved geometry column (`resolved_geometry`, guaranteed `Some` for any
/// `FeatureSource`-backed collection — see `Router::resolve_features`'s own
/// contract) — the same core AST node OGC API Features Part 3's
/// `S_INTERSECTS(geom, ...)` filter already carries, composed here rather
/// than re-derived, per the issue's own instruction to reuse "the same query
/// path the driver already compiles."
fn compose_filter(decl: &CollectionDecl, request: &SearchRequest) -> Option<Filter> {
    let intersects_filter = request
        .intersects
        .as_ref()
        .map(|geometry| Filter::Intersects {
            property: decl.resolved_geometry().to_string(),
            geometry: GeometryLiteral::GeoJson(geometry.clone()),
        });
    match (request.filter.clone(), intersects_filter) {
        (Some(f), Some(i)) => Some(Filter::And(vec![f, i])),
        (Some(f), None) => Some(f),
        (None, Some(i)) => Some(i),
        (None, None) => None,
    }
}

/// Why this collection cannot serve the filter this request composed, as the
/// message a 400 would carry — or `None` when it can. Called only when there
/// *is* a filter to serve (`compose_filter` produced one, or a `#34` policy
/// grant contributed one), since neither reason below can bite a request whose
/// query has no filter tree at all.
///
/// Three independent reasons, all refusals **by name**:
///
/// - The driver refuses a `filter` outright (`FeatureSource::filter_capable`)
///   — FlatGeobuf, GeoParquet, and the memory driver.
/// - `#248`: the request declared `filter-crs=CRS84` explicitly and honouring
///   it against *this* collection means a real coordinate transform of the
///   filter's spatial literals (`crs::crs84_literals_need_transform`: the
///   collection's storage is not itself CRS84), which only a driver declaring
///   `FeatureSource::filter_crs_capable` can perform. PostGIS can; every other
///   driver in this workspace leaves that capability at its `false` default,
///   and accepting the parameter there would evaluate the filter's geometries
///   in the storage CRS while answering `200` — the exact defect `#248` was
///   opened for, moved one collection to the left. A CRS84 `filter-crs`
///   against a CRS84-stored collection needs no transform at all, so every
///   driver honours it for free — the common case, and the only one the live
///   GeoPackage demos can produce.
/// - `#247`: no `filter-crs` on the wire (`RequestedCrs::Omitted`), a spatial
///   literal somewhere in the composed filter, and the same
///   projected-storage-plus-incapable-driver pair. The Filter Extension is
///   explicit that "the parameter `filter-crs` always defaults to
///   `http://www.opengis.net/def/crs/OGC/1.3/CRS84` for a STAC API", so an
///   omitted parameter is a CRS84 one — the same conclusion the Features lane
///   reaches from Part 3 Requirement 7 (`/req/filter/filter-crs-wgs84`). Before
///   `#247` this case was served regardless of storage SRID: a mixed-SRID `500`
///   from PostGIS on a projected collection, and rows selected in the storage
///   CRS under a `200` from a driver comparing raw coordinates. Neither is
///   "processing the geometries in CRS84".
///
///   Narrowed to a filter that actually carries a geometry
///   (`Filter::has_spatial_literal`), unlike the `#248` branch above. The two
///   are asking different questions and deserve different widths: `#248`
///   refuses a parameter the client *named* and this lane cannot honour for
///   this collection, which is a fact about the request whatever it filters on;
///   `#247` refuses a *default* the client never mentioned, so it must not
///   reach past what that default can affect. An attribute-only `name='x'` has
///   no geometry to process in any CRS, and refusing it here would break
///   ordinary attribute filtering on every projected collection over a rule
///   about coordinates.
///
/// A `filter-crs` naming any *other* CRS never reaches here: `search::
/// resolve_search_filter_crs` already refused it at parse time, for the whole
/// request rather than per collection, because a cross-collection `/search`
/// has no single storage CRS a URI could name.
fn unservable_filter_reason(
    coll_ext: &str,
    decl: &CollectionDecl,
    source: &dyn FeatureSource,
    request: &SearchRequest,
    filter: Option<&Filter>,
) -> Option<String> {
    if !source.filter_capable() {
        return Some(format!(
            "collection '{coll_ext}' does not support the 'filter'/'intersects' parameters"
        ));
    }
    let needs_transform_this_driver_cannot_do =
        tellurion_core::crs::crs84_literals_need_transform(decl.srid)
            && !source.filter_crs_capable();
    if request.filter_crs == RequestedCrs::Crs84 && needs_transform_this_driver_cannot_do {
        return Some(format!(
            "collection '{coll_ext}' does not support the 'filter-crs' parameter: its storage \
             is not CRS84, and this driver cannot transform a filter's spatial literals into it"
        ));
    }
    if request.filter_crs == RequestedCrs::Omitted
        && filter.is_some_and(Filter::has_spatial_literal)
        && needs_transform_this_driver_cannot_do
    {
        return Some(format!(
            "collection '{coll_ext}' cannot evaluate a spatial filter in CRS84: its storage is \
             not CRS84, and this driver cannot transform a filter's spatial literals into it"
        ));
    }
    None
}

/// Why this collection cannot serve this request's `bbox`, as the message a
/// 400 would carry — or `None` when it can, which includes every request that
/// carries no `bbox` at all. The `bbox` counterpart of
/// [`unservable_filter_reason`], shared by both STAC lanes that accept one:
/// `/collections/{cid}/items` and `/search`.
///
/// Neither STAC lane has a `bbox-crs` parameter to read — the STAC API item
/// search's `bbox` is defined in WGS 84 longitude/latitude and nothing else,
/// the same fixed reading OGC API - Features - Part 1 Requirement 23
/// (`/req/core/fc-bbox-definition`) clause C gives a four-number `bbox` that
/// arrives with no `bbox-crs`. So every `bbox` reaching here is CRS84, and
/// honouring it against a collection whose storage is not CRS84 is a real
/// coordinate transform of the box's four numbers.
///
/// `crs::can_serve` (`#227`) is asked that question, about
/// [`RequestedCrs::Crs84`] rather than about a parameter nobody sent — the
/// same single predicate the Features lane's own `bbox` gate asks, so the two
/// protocols cannot reach different conclusions about one collection. A
/// driver that answers `false` is one that would otherwise compare degrees
/// against metres and answer `200`: PostGIS's `&&` operator does not raise on
/// mixed SRIDs, and a driver evaluating the box in memory against raw storage
/// coordinates (GeoPackage) has no database to raise at all. Both return rows
/// that violate Part 1 Requirement 24 (`/req/core/fc-bbox-response`) clause A
/// — "Only features that have a spatial geometry that intersects the bounding
/// box SHALL be part of the result set" — with nothing in the response a
/// client could detect it by. Refused by name instead.
///
/// Unconditional on `filter`, unlike its sibling, and therefore called
/// unconditionally: a `bbox` is a predicate in its own right, and `#247`'s
/// `has_spatial_literal` narrowing has no counterpart here because a `bbox`
/// always carries coordinates.
fn unservable_bbox_reason(
    coll_ext: &str,
    decl: &CollectionDecl,
    source: &dyn FeatureSource,
    has_bbox: bool,
) -> Option<String> {
    if !has_bbox
        || tellurion_core::crs::can_serve(RequestedCrs::Crs84, decl.srid, source.crs_capable())
    {
        return None;
    }
    Some(format!(
        "collection '{coll_ext}' cannot evaluate a 'bbox': a STAC bounding box is CRS84, its \
         storage is not, and this driver cannot transform the bbox into it"
    ))
}

/// Builds one STAC Item's `links` for a `/search` response — same four
/// relations `list_items` builds per item, generalized to name whichever
/// collection this particular item actually came from (`coll_ext`) rather
/// than a single path-derived collection id.
fn search_item_links(root_path: &str, coll_ext: &str, item_id: &str) -> Vec<Link> {
    let collection_href = format!("{root_path}/collections/{coll_ext}");
    vec![
        Link::new(root_path.to_string(), "root", JSON_MEDIA_TYPE),
        Link::new(
            format!("{collection_href}/items/{item_id}"),
            "self",
            GEOJSON_MEDIA_TYPE,
        ),
        Link::new(collection_href.clone(), "collection", JSON_MEDIA_TYPE),
        Link::new(collection_href, "parent", JSON_MEDIA_TYPE),
    ]
}

/// `ids`-mode `/search` (see [`execute_search`]'s doc for why this never
/// rides `ItemsQuery`/`Filter`): walks the `(collections, ids)` cross
/// product collection-major, starting at flat index `start`, issuing one
/// `FeatureSource::item` call per `(collection, id)` pair until either
/// `limit` items have been found or the cross product is exhausted. An
/// unresolvable collection contributes no items (skipped, not an error —
/// same tolerant fan-out rule `run_cursor_search` documents) and a missing
/// id within a resolvable collection is simply absent from the result, per
/// `FeatureSource::item`'s own `Ok(None)` contract.
///
/// `number_matched` is always `None`: the true total is not cheaply known
/// without exhausting the entire cross product (which the `limit`-bounded
/// walk deliberately avoids), and even a full scan would cost one extra
/// point lookup per `(collection, id)` pair for a number with no further use
/// once the page is built.
async fn run_ids_search(
    sc: &SearchContext<'_>,
    collections: &[String],
    ids: &[String],
    start: usize,
    limit: u32,
) -> Result<SearchPage, ApiError> {
    let total = collections.len().saturating_mul(ids.len());
    let mut features = Vec::new();
    let mut i = start;
    // Caches the last collection resolved (plus its materialized assets) so
    // consecutive `i` values sharing a `coll_idx` — the common case, since
    // the cross product is collection-major — don't re-resolve or
    // re-probe capabilities per id.
    let mut resolved: Option<(usize, ResolvedCollection)> = None;

    while i < total && features.len() < limit as usize {
        let coll_idx = i / ids.len();
        let item_id = &ids[i % ids.len()];
        let coll_ext = &collections[coll_idx];

        if resolved.as_ref().map(|(idx, _)| *idx) != Some(coll_idx) {
            resolved = match resolve_collection_features(sc, coll_ext).await {
                Ok((collection_id, decl, source)) => {
                    // `#34`: same pushdown `tellurion_features::handlers::
                    // get_item`/this crate's own single-item `get_item` use —
                    // `source.filter_capable()` decides whether a
                    // filtered-only grant may proceed. A denied collection is
                    // simply skipped, the same tolerant treatment an
                    // unresolvable one already gets in this mode (see this
                    // function's own doc).
                    match authorize_search_collection(sc, &collection_id, source.filter_capable())
                        .await
                    {
                        Ok(SearchDecision::RateLimited(refusal)) => {
                            return Err(crate::problem::policy_rate_limited(&refusal));
                        }
                        Ok(SearchDecision::Allow { filter }) => {
                            let caps = asset_capabilities(
                                sc.ctx,
                                sc.tenant_id,
                                sc.catalog_id,
                                &collection_id,
                            )
                            .await;
                            let assets = collection_assets(
                                sc.server,
                                sc.tenant_ext,
                                sc.catalog_ext,
                                coll_ext,
                                &caps,
                            );
                            let contributed = contributed_links(
                                sc.ctx,
                                &ResourceRef {
                                    tenant: sc.tenant_ext,
                                    catalog: sc.catalog_ext,
                                    collection: coll_ext,
                                    item_id: None,
                                    base_url: public_base_url(sc.server),
                                    tenant_id: sc.tenant_id,
                                    catalog_id: sc.catalog_id,
                                    collection_id: &collection_id,
                                },
                                LinkAnchor::Item,
                            )
                            .await;
                            let sidecar = stac_sidecar_docs(
                                sc.ctx,
                                sc.tenant_id,
                                sc.catalog_id,
                                &collection_id,
                                &decl,
                                ids,
                            )
                            .await?;
                            let assets = PageItemAssets::new(
                                sc.server,
                                assets,
                                sc.tenant_ext,
                                sc.catalog_ext,
                                coll_ext,
                                &item_asset_records(
                                    sc.ctx,
                                    sc.tenant_id,
                                    sc.catalog_id,
                                    &collection_id,
                                    &decl,
                                    ids,
                                )
                                .await?,
                            );
                            let projection = derive_projection(decl.projection.as_ref(), decl.srid);
                            Some((
                                coll_idx,
                                ResolvedCollection {
                                    decl,
                                    source,
                                    assets,
                                    policy_filter: filter,
                                    contributed,
                                    sidecar,
                                    projection,
                                },
                            ))
                        }
                        _ => None,
                    }
                }
                Err(_) => None,
            };
        }

        if let Some((_, entry)) = &resolved {
            if let Some(feature) = entry
                .source
                .item(&entry.decl, item_id, entry.policy_filter.as_ref())
                .await
                .map_err(ApiError::from)?
            {
                let mut links = search_item_links(sc.root_path, coll_ext, item_id);
                extend_with_contributed(&mut links, entry.contributed.clone());
                features.push(to_stac_item(
                    feature,
                    coll_ext,
                    entry.decl.datetime.as_deref(),
                    entry.assets.for_item(item_id),
                    links,
                    entry.sidecar.get(item_id),
                    entry.projection.as_ref(),
                ));
            }
        }
        i += 1;
    }

    let next_token = (i < total).then(|| {
        SearchToken::Ids {
            collections: collections.to_vec(),
            ids: ids.to_vec(),
            start: i,
        }
        .encode()
    });
    // `ids` mode never composes a `filter`/`intersects` predicate (this
    // function's own doc, above) — the filter-incapable-collections list
    // `run_cursor_search` reports for the identical scenario has nothing to
    // ever report here, so this is always empty rather than an omission.
    // `ids` mode rides neither `ItemsQuery` nor `Filter` (see `execute_search`'s
    // own doc), so neither capability list can have anything to report.
    Ok(SearchPage {
        features,
        next_token,
        number_matched: None,
        filter_incapable_collections: Vec::new(),
        bbox_incapable_collections: Vec::new(),
    })
}

/// `#181`'s fallthrough metric: one count per collection a `q`-bearing
/// dispatch could NOT serve from a text-capable, fresh derived index —
/// whether that surfaced as a refusal (single-collection) or a reported
/// skip (fan-out). `reason` is a small closed set (`no-search-lane`,
/// `index-not-fresh`, `index-not-text-capable`), so an operator can tell a
/// misrouted catalog apart from a lagging applier without log-diving.
fn count_q_fallthrough(collection_ext: &str, reason: &'static str) {
    metrics::counter!(
        "stac_search_q_fallthrough_total",
        "reason" => reason,
        "collection" => collection_ext.to_string()
    )
    .increment(1);
}

/// `q`-mode `/search` (`#181`): free text against each candidate
/// collection's derived search index, walked in the same fixed alphabetical
/// order the other modes use, until `limit` items are found or the
/// candidates are exhausted. This is the one path in this crate that rides
/// `Router::resolve_search` — the freshness-gated search lane — instead of
/// `resolve_features`, and its dispatch rules are `#181`'s agreement gates:
///
/// - **Index-lane-only, never approximated.** A collection serves `q` only
///   when its search lane resolves to [`SearchResolution::Index`] AND that
///   source advertises [`tellurion_core::SearchSource::text_search_capable`].
///   A [`SearchResolution::Fallback`] (stale/unmeasurable index, or a lane
///   whose primary is no index) means the main chain would serve — and the
///   main chain cannot express free text, so the request is refused by name
///   (`problem::search_index_unavailable`, a `503`: transient by its
///   dominant cause) rather than approximated with a degraded substring
///   scan that would silently answer a different question. A text-incapable
///   index source is the permanent flavor of the same refusal
///   (`CapabilityUnsupported`, like a collection with no `routing.search`
///   at all).
/// - **Fan-out skips are reported, never silent.** More than one candidate
///   collection downgrades each such refusal to a skip recorded in the
///   response's `searchIncapableCollections` — the exact
///   single-vs-fan-out judgment call (and the exact machine-detectable
///   reporting) `run_cursor_search` already applies to filter-incapable
///   collections. An unresolvable id or a policy denial is skipped
///   UNreported, also matching the other modes' pre-existing rule.
/// - **Every fallthrough is counted** ([`count_q_fallthrough`]), skip and
///   refusal alike — the issue's own metric requirement.
///
/// Policy (`#34`): the derived-index query cannot compile a grant filter
/// (`SearchQuery` deliberately carries none), so `lane_supports_filter` is
/// hard `false` here — a subject whose only matching grants require a
/// filter is denied by `policy::authorize_resource` itself rather than
/// served unfiltered index documents the grant never authorized. An
/// unrestricted `Allow` proceeds as usual.
///
/// `number_matched` is always `None` (the index query reports no total) and
/// there is never a `next` token (no cursor to encode) — both documented
/// limits of this slice, per [`execute_search`]'s `q` branch.
async fn run_q_search(
    sc: &SearchContext<'_>,
    collections: &[String],
    q: &str,
    limit: u32,
) -> Result<(Vec<Value>, Vec<String>), ApiError> {
    let single_collection = collections.len() == 1;
    let state = sc.ctx.current();
    let mut features: Vec<Value> = Vec::new();
    let mut search_incapable_collections = Vec::new();

    for coll_ext in collections {
        let remaining = (limit as usize).saturating_sub(features.len());
        if remaining == 0 {
            break;
        }

        let collection_id = match state
            .resolver
            .resolve_collection(sc.catalog_id, coll_ext)
            .await
        {
            Ok(id) => id,
            Err(err) if single_collection => return Err(ApiError::from(err)),
            Err(_) => continue,
        };

        // `#34`: see this function's doc — the index lane can push no grant
        // filter down, so `lane_supports_filter` is hard `false`.
        match authorize_search_collection(sc, &collection_id, false).await {
            Ok(SearchDecision::RateLimited(refusal)) => {
                return Err(crate::problem::policy_rate_limited(&refusal));
            }
            Ok(SearchDecision::Allow { .. }) => {}
            Ok(SearchDecision::Deny) => {
                if single_collection {
                    return Err(crate::problem::policy_denied(sc.credential));
                }
                continue;
            }
            Err(err) => {
                if single_collection {
                    return Err(ApiError::from(err));
                }
                continue;
            }
        }

        let resolution = state
            .router
            .resolve_search(sc.tenant_id, sc.catalog_id, &collection_id)
            .await;
        let (decl, search) = match resolution {
            Ok((decl, SearchResolution::Index(search))) if search.text_search_capable() => {
                (decl, search)
            }
            Ok((_, SearchResolution::Index(_))) => {
                count_q_fallthrough(coll_ext, "index-not-text-capable");
                if single_collection {
                    return Err(ApiError::from(CoreError::CapabilityUnsupported {
                        collection: coll_ext.clone(),
                        capability: "free-text search".to_string(),
                    }));
                }
                search_incapable_collections.push(coll_ext.clone());
                continue;
            }
            Ok((_, SearchResolution::Fallback(_))) => {
                count_q_fallthrough(coll_ext, "index-not-fresh");
                if single_collection {
                    return Err(crate::problem::search_index_unavailable(coll_ext));
                }
                search_incapable_collections.push(coll_ext.clone());
                continue;
            }
            Err(err @ CoreError::CapabilityUnsupported { .. }) => {
                count_q_fallthrough(coll_ext, "no-search-lane");
                if single_collection {
                    return Err(ApiError::from(err));
                }
                search_incapable_collections.push(coll_ext.clone());
                continue;
            }
            Err(err) => {
                if single_collection {
                    return Err(ApiError::from(err));
                }
                continue;
            }
        };

        let query = CoreSearchQuery {
            limit: remaining as u32,
            q: Some(q.to_string()),
        };
        let page = search.search(&decl, &query).await.map_err(ApiError::from)?;

        let caps = asset_capabilities(sc.ctx, sc.tenant_id, sc.catalog_id, &collection_id).await;
        let assets = collection_assets(sc.server, sc.tenant_ext, sc.catalog_ext, coll_ext, &caps);
        // `#202`/`#221`: one sidecar lookup and one asset-record lookup per
        // collection's slice of this page — the free-text lane reads its
        // documents from the derived index, but an Item is an Item, so the
        // same sidecar and the same asset records apply to it.
        let page_ids = page_feature_ids(&page.features_geojson);
        let sidecar = stac_sidecar_docs(
            sc.ctx,
            sc.tenant_id,
            sc.catalog_id,
            &collection_id,
            &decl,
            &page_ids,
        )
        .await?;
        let assets = PageItemAssets::new(
            sc.server,
            assets,
            sc.tenant_ext,
            sc.catalog_ext,
            coll_ext,
            &item_asset_records(
                sc.ctx,
                sc.tenant_id,
                sc.catalog_id,
                &collection_id,
                &decl,
                &page_ids,
            )
            .await?,
        );
        let projection = derive_projection(decl.projection.as_ref(), decl.srid);
        for feature in page.features_geojson {
            let item_id = feature
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let links = search_item_links(sc.root_path, coll_ext, &item_id);
            features.push(to_stac_item(
                feature,
                coll_ext,
                decl.datetime.as_deref(),
                assets.for_item(&item_id),
                links,
                sidecar.get(&item_id),
                projection.as_ref(),
            ));
        }
    }

    Ok((features, search_incapable_collections))
}

/// The ordinary keyset-paged `/search` path — see [`execute_search`]'s doc
/// for the single-vs-fan-out cost distinction and the skip-vs-400
/// capability/validation rule. Walks `collections` starting at
/// `idx`/`cursor`, calling `FeatureSource::items` on each in turn; a
/// collection is exhausted (its own `next_token` comes back `None`) before
/// moving on to the next, never interleaved.
async fn run_cursor_search(
    sc: &SearchContext<'_>,
    collections: &[String],
    mut idx: usize,
    mut cursor: Option<String>,
    request: &SearchRequest,
) -> Result<SearchPage, ApiError> {
    let single_collection = collections.len() == 1;
    let mut features = Vec::new();
    let mut remaining = request.limit;
    let mut number_matched = None;
    // `#255`'s own sibling of the list below: collections this fan-out
    // skipped because it could not honour the request's CRS84 `bbox` against
    // their storage. Kept apart from `filter_incapable_collections` for the
    // reason `StacItemCollectionResponse::bbox_incapable_collections` gives —
    // the two say different things about what the client would have to drop.
    let mut bbox_incapable_collections = Vec::new();
    // External ids this page's fan-out skipped specifically for lacking a
    // capability the merged query needed — compiling the filter/intersects
    // predicate at all, or (`#248`) honouring the `filter-crs` it was
    // declared in; `unservable_filter_reason` owns both. Never a collection
    // skipped for some other reason (unresolvable, access denied), since
    // those are not a claim this response makes false the way silently
    // dropping a collection whose driver just can't filter is.
    // `execute_search`'s own doc explains why the fan-out skips rather than
    // 400s here; this is what makes that skip machine-detectable instead of
    // silent.
    let mut filter_incapable_collections = Vec::new();

    while remaining > 0 && idx < collections.len() {
        let coll_ext = &collections[idx];

        let (collection_id, decl, source) = match resolve_collection_features(sc, coll_ext).await {
            Ok(resolved) => resolved,
            Err(err) if single_collection => return Err(ApiError::from(err)),
            Err(_) => {
                idx += 1;
                cursor = None;
                continue;
            }
        };

        // `#34`: same AND-merge treatment `list_items`/`tellurion_features`
        // give the items-list lane — a single explicitly-named target
        // collection denies with a real 401/403; a fan-out candidate is
        // silently skipped, the same tolerance every other per-collection
        // failure in this loop already gets.
        let policy_filter =
            match authorize_search_collection(sc, &collection_id, source.filter_capable()).await {
                Ok(SearchDecision::RateLimited(refusal)) => {
                    return Err(crate::problem::policy_rate_limited(&refusal));
                }
                Ok(SearchDecision::Allow { filter }) => filter,
                Ok(SearchDecision::Deny) => {
                    if single_collection {
                        return Err(crate::problem::policy_denied(sc.credential));
                    }
                    idx += 1;
                    cursor = None;
                    continue;
                }
                Err(err) => {
                    if single_collection {
                        return Err(ApiError::from(err));
                    }
                    idx += 1;
                    cursor = None;
                    continue;
                }
            };

        // `#255`: the request's own `bbox`, before any filter is composed —
        // a `bbox` is a predicate in its own right, and this refusal has to
        // bite a `bbox`-only request too. Same skip-vs-400 rule the filter
        // refusal below follows.
        if let Some(reason) =
            unservable_bbox_reason(coll_ext, &decl, source.as_ref(), request.bbox.is_some())
        {
            if single_collection {
                return Err(ApiError::from(CoreError::Invalid(reason)));
            }
            bbox_incapable_collections.push(coll_ext.clone());
            idx += 1;
            cursor = None;
            continue;
        }

        let merged_filter = match (compose_filter(&decl, request), policy_filter) {
            (Some(user), Some(grant)) => Some(Filter::And(vec![user, grant])),
            (Some(user), None) => Some(user),
            (None, Some(grant)) => Some(grant),
            (None, None) => None,
        };
        if let Some(reason) = merged_filter.as_ref().and_then(|filter| {
            unservable_filter_reason(coll_ext, &decl, source.as_ref(), request, Some(filter))
        }) {
            if single_collection {
                return Err(ApiError::from(CoreError::Invalid(reason)));
            }
            filter_incapable_collections.push(coll_ext.clone());
            idx += 1;
            cursor = None;
            continue;
        }

        if let Some(filter) = &merged_filter {
            let descriptor = sc
                .ctx
                .current()
                .router
                .collection_descriptor(sc.tenant_id, sc.catalog_id, &collection_id)
                .await;
            let descriptor = match descriptor {
                Ok(d) => d,
                Err(err) if single_collection => return Err(ApiError::from(err)),
                Err(_) => {
                    idx += 1;
                    cursor = None;
                    continue;
                }
            };
            if let Err(err) =
                tellurion_core::filter::validate(filter, &descriptor, decl.schema.as_ref())
            {
                if single_collection {
                    return Err(ApiError::from(err));
                }
                idx += 1;
                cursor = None;
                continue;
            }
        }

        let query = ItemsQuery {
            limit: remaining,
            bbox: request.bbox,
            datetime: request.datetime.clone(),
            token: cursor.clone(),
            filter: merged_filter,
            // `#248`: the resolved `filter-crs`, carried through to the driver
            // that compiles the filter's spatial literals rather than dropped
            // here. `RequestedCrs::Omitted` (no `filter-crs` on the wire) is
            // the value every field of this struct had before `#248`; it
            // compiles byte-for-byte the SQL this lane always produced for a
            // CRS84-stored collection, and (`#247`) a genuine transform for a
            // projected one — the extension's own "always defaults to CRS84"
            // read honestly instead of as a mixed-SRID `500`.
            // `unservable_filter_reason` above has already refused, by name,
            // every collection whose driver could not do that.
            filter_crs: request.filter_crs,
            ..ItemsQuery::default()
        };
        let page = source.items(&decl, &query).await.map_err(ApiError::from)?;

        let caps = asset_capabilities(sc.ctx, sc.tenant_id, sc.catalog_id, &collection_id).await;
        let assets = collection_assets(sc.server, sc.tenant_ext, sc.catalog_ext, coll_ext, &caps);
        // `#186`: contributed once per collection touched, reused for every
        // item of this page slice — same per-collection caching rationale
        // `ResolvedCollection::contributed` documents for the ids mode.
        let contributed = contributed_links(
            sc.ctx,
            &ResourceRef {
                tenant: sc.tenant_ext,
                catalog: sc.catalog_ext,
                collection: coll_ext,
                item_id: None,
                base_url: public_base_url(sc.server),
                tenant_id: sc.tenant_id,
                catalog_id: sc.catalog_id,
                collection_id: &collection_id,
            },
            LinkAnchor::Item,
        )
        .await;

        // `#202`/`#221`: one sidecar lookup and one asset-record lookup per
        // collection's slice of this page, before the per-item loop — see
        // `stac_sidecar_docs` and `item_asset_records`.
        let page_ids = page_feature_ids(&page.features_geojson);
        let sidecar = stac_sidecar_docs(
            sc.ctx,
            sc.tenant_id,
            sc.catalog_id,
            &collection_id,
            &decl,
            &page_ids,
        )
        .await?;
        let assets = PageItemAssets::new(
            sc.server,
            assets,
            sc.tenant_ext,
            sc.catalog_ext,
            coll_ext,
            &item_asset_records(
                sc.ctx,
                sc.tenant_id,
                sc.catalog_id,
                &collection_id,
                &decl,
                &page_ids,
            )
            .await?,
        );

        let returned = page.features_geojson.len();
        let projection = derive_projection(decl.projection.as_ref(), decl.srid);
        for feature in page.features_geojson {
            let item_id = feature
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut links = search_item_links(sc.root_path, coll_ext, &item_id);
            extend_with_contributed(&mut links, contributed.clone());
            features.push(to_stac_item(
                feature,
                coll_ext,
                decl.datetime.as_deref(),
                assets.for_item(&item_id),
                links,
                sidecar.get(&item_id),
                projection.as_ref(),
            ));
        }
        remaining = remaining.saturating_sub(returned as u32);
        if single_collection {
            number_matched = page.number_matched;
        }

        match page.next_token {
            Some(token) => {
                cursor = Some(token);
                break;
            }
            None => {
                idx += 1;
                cursor = None;
            }
        }
    }

    let next_token = (idx < collections.len()).then(|| {
        SearchToken::Cursor {
            collections: collections.to_vec(),
            idx,
            cursor,
        }
        .encode()
    });
    Ok(SearchPage {
        features,
        next_token,
        number_matched,
        filter_incapable_collections,
        bbox_incapable_collections,
    })
}
