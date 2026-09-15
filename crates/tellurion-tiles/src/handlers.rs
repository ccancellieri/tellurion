//! OGC API — Tiles Part 1 handlers: tileset discovery + the
//! `{tileMatrix}/{tileRow}/{tileCol}` tile endpoint (OGC API Tiles path
//! order: row before column). Driver-agnostic — every access to storage
//! goes through `AppContext.router`; rasterization goes through
//! `tellurion-render` at the response boundary only.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;

use tellurion_core::problem::{Problem, PROBLEM_JSON};
use tellurion_core::{
    advertised_vector_layers, mvt_key, AppContext, CollectionDecl, Credential, Encoding, Error,
    MvtFetch, PopulateFuture, RasterSource, TileCoord, TileKey, TileMatrixSet, TileSource,
};
use tellurion_render::{
    encode_rgba_to_png, render_mvt_to_png, render_mvt_to_png_styled, resolve_layer_paints,
    style_paints_any_layer, RenderStyle,
};

use crate::tilematrixset;

/// `tenant`/`catalog` path parameters carry EXTERNAL ids exactly as the
/// client typed them (`#39`) — every request runs under a
/// `/{tenant}/tiles/catalogs/{catalog}` mount; [`resolve_tiles`] turns them
/// (plus a collection's own external id) into the internal ids `Router` and
/// the tile cache key need. A handler that runs with no mount at all (this
/// crate's own unit tests) falls back to [`DEFAULT_TENANT`]/
/// [`DEFAULT_CATALOG`] — the same convention `tellurion-features` uses, so
/// every protocol crate resolves consistently regardless of how the server
/// mounts it.
pub const DEFAULT_TENANT: &str = "public";
pub const DEFAULT_CATALOG: &str = "default";

pub const TILE_CACHE_CONTROL: &str = "public, max-age=86400";

const MVT_MIME: &str = "application/vnd.mapbox-vector-tile";
pub(crate) const PNG_MIME: &str = "image/png";
const RENDER_TILE_SIZE_PX: u32 = 256;
/// `StyleConf` (config.rs) has no point-radius field; v0.1 renders every
/// point feature at this fixed radius. Reused by the maps lane's own
/// unstyled render path (`maps.rs`, `#86`) so an unstyled `/map` request
/// draws points at the same default radius the unstyled PNG tile lane does.
pub(crate) const DEFAULT_POINT_RADIUS_PX: f32 = 3.0;

/// OGC API - Maps' own "map" link relation (verified against
/// `opengeospatial/ogcapi-maps`'s `REQ_styled-map_desc-links.adoc`, the
/// requirement that a styled resource link to its rendered map with this
/// exact rel), used here to advertise each registered style's rendered-tile
/// endpoint on the TileSet resource (`#49`) — a client can see every style
/// this collection's tiles can be rendered with, and the exact templated URL
/// to request it, without probing.
pub const MAP_REL: &str = "https://www.opengis.net/def/rel/ogc/1.0/map";

/// Builds the OGC API — Tiles route table. Mount under whatever prefix the
/// server chooses; paths here are relative to that mount point.
///
/// `#190`: the tile-matrix-set path segment is a `{tileMatrixSetId}`
/// parameter rather than a literal `WebMercatorQuad`, resolved against the
/// closed `tellurion_core::TileMatrixSet` registry inside each handler — an
/// id the registry doesn't know is a 404, exactly what the unmatched
/// literal route produced before, so every pre-`#190` URL behaves
/// identically.
pub fn router() -> axum::Router<Arc<AppContext>> {
    axum::Router::new()
        .route("/collections/{cid}/map", get(crate::maps::map))
        .route("/collections/{cid}/tiles", get(tileset_list))
        .route("/collections/{cid}/tiles/{tileMatrixSetId}", get(tileset))
        .route(
            "/collections/{cid}/tiles/{tileMatrixSetId}/{tileMatrix}/{tileRow}/{tileCol}",
            get(tile),
        )
        .route(
            "/collections/{cid}/styles/{styleId}/map/tiles/{tileMatrixSetId}/{tileMatrix}/{tileRow}/{tileCol}",
            get(styled_tile),
        )
        .route("/tileMatrixSets", get(tile_matrix_sets_list))
        .route(
            "/tileMatrixSets/{tileMatrixSetId}",
            get(tile_matrix_set_definition),
        )
}

pub(crate) fn tenant_of(params: &HashMap<String, String>) -> String {
    params
        .get("tenant")
        .cloned()
        .unwrap_or_else(|| DEFAULT_TENANT.to_string())
}

pub(crate) fn catalog_of(params: &HashMap<String, String>) -> String {
    params
        .get("catalog")
        .cloned()
        .unwrap_or_else(|| DEFAULT_CATALOG.to_string())
}

/// Resolves the `{tileMatrixSetId}` path segment against the closed
/// `#190` registry. `None` — an id the registry doesn't know — is a 404 at
/// every call site: before `#190` such a URL matched no route at all, and
/// an unknown-id 404 is byte-identical to that. A handler invoked with no
/// route binding at all (this crate's own unit tests calling handlers as
/// plain functions) falls back to `WebMercatorQuad`, the same convention
/// [`DEFAULT_TENANT`]/[`DEFAULT_CATALOG`] follow.
fn tms_of(params: &HashMap<String, String>) -> Option<TileMatrixSet> {
    match params.get("tileMatrixSetId") {
        Some(id) => TileMatrixSet::from_id(id),
        None => Some(TileMatrixSet::WebMercatorQuad),
    }
}

/// The `#190` capability-honesty refusal, shaped exactly like
/// `tellurion_core::Error::CapabilityUnsupported`'s own message (the shape
/// `Router` refuses a missing write/index/search capability with, and the
/// same `CapabilityUnsupported` problem code `raster_tile_response`'s other
/// by-name refusals already use): names the collection, the requested tile
/// matrix set, and the lane that can't serve it — never a mid-request panic
/// or a silently empty tile.
fn refuse_tile_matrix_set(cid: &str, tms: TileMatrixSet, lane: &str) -> Response {
    problem_response(
        StatusCode::BAD_REQUEST,
        "CapabilityUnsupported",
        format!(
            "collection '{cid}' does not support capability 'tileMatrixSet:{tms}': its resolved {lane} serves only its native WebMercatorQuad grid"
        ),
    )
}

/// A collection resolved through this request's `(tenant, catalog, cid)`
/// path segments (`#39`) — external ids resolved to internal ones via
/// `AppContext::current().resolver`, then handed to `Router`. `tenant_id`/
/// `catalog_id`/`collection_id` are internal and feed the tile cache key
/// (`mvt_key`/`png_key`/`styled_png_key`); everything else about the
/// response (hrefs) is built from the ORIGINAL external path segments the
/// client typed, never these.
struct ResolvedTiles {
    tenant_id: String,
    catalog_id: String,
    collection_id: String,
    decl: CollectionDecl,
    source: Arc<dyn TileSource>,
}

async fn resolve_tiles(
    ctx: &AppContext,
    params: &HashMap<String, String>,
    cid: &str,
) -> Option<ResolvedTiles> {
    let state = ctx.current();
    let tenant_ext = tenant_of(params);
    let catalog_ext = catalog_of(params);
    let tenant_id = state.resolver.resolve_tenant(&tenant_ext).await.ok()?;
    let catalog_id = state
        .resolver
        .resolve_catalog(&tenant_id, &catalog_ext)
        .await
        .ok()?;
    let collection_id = state
        .resolver
        .resolve_collection(&catalog_id, cid)
        .await
        .ok()?;
    let (decl, source) = state
        .router
        .resolve_tiles(&tenant_id, &catalog_id, &collection_id)
        .await
        .ok()?;
    Some(ResolvedTiles {
        tenant_id,
        catalog_id,
        collection_id,
        decl,
        source,
    })
}

/// Raster counterpart of [`ResolvedTiles`] (`#37`) — a collection whose
/// tiles lane serves decoded pixel windows (Cloud-Optimized GeoTIFF) rather
/// than MVT. `tile` only tries this after [`resolve_tiles`] has already
/// failed for the same `(tenant, catalog, cid)`, so a collection with a real
/// vector `TileSource` never pays for this second resolution attempt.
struct ResolvedRaster {
    tenant_id: String,
    catalog_id: String,
    collection_id: String,
    decl: CollectionDecl,
    source: Arc<dyn RasterSource>,
}

async fn resolve_raster(
    ctx: &AppContext,
    params: &HashMap<String, String>,
    cid: &str,
) -> Option<ResolvedRaster> {
    let state = ctx.current();
    let tenant_ext = tenant_of(params);
    let catalog_ext = catalog_of(params);
    let tenant_id = state.resolver.resolve_tenant(&tenant_ext).await.ok()?;
    let catalog_id = state
        .resolver
        .resolve_catalog(&tenant_id, &catalog_ext)
        .await
        .ok()?;
    let collection_id = state
        .resolver
        .resolve_collection(&catalog_id, cid)
        .await
        .ok()?;
    let (decl, source) = state
        .router
        .resolve_raster(&tenant_id, &catalog_id, &collection_id)
        .await
        .ok()?;
    Some(ResolvedRaster {
        tenant_id,
        catalog_id,
        collection_id,
        decl,
        source,
    })
}

/// Extracts a [`Credential`] from `Authorization: Bearer <token>` — mirrors
/// `tellurion-server::app`'s own `extract_credential` exactly (duplicated
/// per protocol crate, not shared — `tellurion-core` stays framework-free,
/// see `auth.rs`'s own module doc). Any other or malformed `Authorization`
/// header is `Credential::None`, same as no header at all.
pub(crate) fn extract_credential(headers: &HeaderMap) -> Credential {
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

/// The `#34` policy checkpoint every handler in this crate calls right
/// after [`resolve_tiles`] succeeds — tileset discovery included, not only
/// tile bytes, since a `tileMatrixSetLimits`/zoom range is itself
/// information about a resource isolation should gate.
///
/// `lane_supports_filter` is the resolved `TileSource`'s own
/// `filter_capable()` — a filtered-only grant is now served (its filter
/// pushed all the way to `fetch_mvt`, then into the tile query) when the
/// resolved driver can compile one (PostGIS), and still denied outright,
/// exactly as before, when it can't (PMTiles' pre-baked archives, or any
/// other driver that never overrides the trait default). Discovery
/// (`tileset_list`/`tileset`) is gated by the same capability as the tile
/// lanes even though it serves no filtered row data itself: a subject who
/// can fetch actual (filtered) tile content for this collection should also
/// see its tileset metadata, and one who can't fetch any content at all
/// gets the same deny here it always did. See `tellurion_core::policy`'s
/// own module doc for the two-gate evaluation this wraps, and
/// `tellurion_core::cache::TileKey`'s own doc for how the returned filter
/// (when `Some`) feeds the tile cache key. `state.authorizer` being `None`
/// skips straight to unrestricted access, same as `tellurion-server`'s own
/// `#17` gate.
///
/// Reused by the maps lane (`maps.rs`, `#86`) under `PolicyLane::Tiles`
/// unchanged — `/collections/{cid}/map` is part of this crate's own
/// protocol root, and `PolicyLane`'s own doc scopes grants per protocol
/// crate, not per endpoint, so a role authorized to read this collection's
/// tiles is authorized to read its rendered maps too.
pub(crate) async fn authorize_tiles(
    ctx: &AppContext,
    headers: &HeaderMap,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
    lane_supports_filter: bool,
) -> Result<Option<tellurion_core::Filter>, Response> {
    let state = ctx.current();
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
    let resource = tellurion_core::policy::ResourceContext {
        tenant_id,
        catalog_id,
        collection_id,
        lane: tellurion_core::PolicyLane::Tiles,
        visibility: &visibility,
    };
    match tellurion_core::policy::authorize_resource(
        &state.config,
        &resource,
        &subject,
        lane_supports_filter,
    ) {
        Ok(tellurion_core::policy::PolicyDecision::Allow { filter }) => Ok(filter),
        Ok(tellurion_core::policy::PolicyDecision::Deny) => {
            let (status, code) = match credential {
                Credential::None => (StatusCode::UNAUTHORIZED, "Unauthorized"),
                Credential::Bearer(_) => (StatusCode::FORBIDDEN, "Forbidden"),
            };
            Err(problem_response(
                status,
                code,
                "the presented credential does not authorize this resource",
            ))
        }
        Err(error) => {
            tracing::error!(%error, "policy evaluation failed for a tiles request");
            Err(problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                "an internal configuration error occurred",
            ))
        }
    }
}

async fn tileset_list(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let Some(cid) = params.get("cid") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // `#37`: report the same `dataType` a client would see by following this
    // entry's own `self` link to `tileset` — a raster-only collection (no
    // vector `TileSource` in its tiles lane) is `"map"` here too, never the
    // hardcoded `"vector"` every collection used to get regardless of what
    // it can actually serve.
    //
    // `#190`: one entry PER tile matrix set the resolved lane actually
    // serves — a vector source advertises whatever grids it declares
    // (`TileSource::supports_tile_matrix_set`: PostGIS both, PMTiles/
    // GeoPackage their native WebMercatorQuad only), a raster lane always
    // its native WebMercatorQuad alone — so this listing is exactly the set
    // of tileset URLs a client can fetch without earning a refusal.
    let (data_type, tile_matrix_sets): (&str, Vec<TileMatrixSet>) =
        match resolve_tileset(&ctx, &params, &headers, cid).await {
            Ok(ResolvedTileset::Vector(resolved)) => (
                "vector",
                TileMatrixSet::ALL
                    .into_iter()
                    .filter(|tms| resolved.source.supports_tile_matrix_set(*tms))
                    .collect(),
            ),
            Ok(ResolvedTileset::Raster(_)) => ("map", vec![TileMatrixSet::WebMercatorQuad]),
            Err(response) => return response,
        };

    let self_path = ctx.current().config.server.public_href(uri.path());
    let tilesets: Vec<serde_json::Value> = tile_matrix_sets
        .into_iter()
        .map(|tms| {
            serde_json::json!({
                "title": cid,
                "dataType": data_type,
                "crs": tilematrixset::crs_of(tms),
                "tileMatrixSetURI": tilematrixset::uri_of(tms),
                "links": [{
                    "rel": "self",
                    "type": "application/json",
                    "href": format!("{}/{}", self_path.trim_end_matches('/'), tms.id()),
                }],
            })
        })
        .collect();
    axum::Json(serde_json::json!({ "tilesets": tilesets })).into_response()
}

/// `decl.tiles.minzoom..=decl.tiles.maxzoom` as the OGC API Tiles
/// `tileMatrixSetLimits` array — shared by the vector and raster tileset
/// bodies, since a collection's configured zoom range means the same thing
/// either way. `#190`: the index bounds come from `tms`'s own matrix
/// dimensions (`WorldCRS84Quad` has twice as many columns as rows at every
/// level), never a square `1 << zoom` assumption.
fn tile_matrix_limits(decl: &CollectionDecl, tms: TileMatrixSet) -> Vec<serde_json::Value> {
    (decl.tiles.minzoom..=decl.tiles.maxzoom)
        .map(|zoom| {
            serde_json::json!({
                "tileMatrix": zoom.to_string(),
                "minTileRow": 0,
                "maxTileRow": tms.matrix_height(zoom) - 1,
                "minTileCol": 0,
                "maxTileCol": tms.matrix_width(zoom) - 1,
            })
        })
        .collect()
}

/// Whether `style_id`'s document paints anything on a tileset made of
/// `layers` — the gate on each `map`-rel link [`tileset_vector_body`]
/// advertises (`#245`).
///
/// The predicate itself is `tellurion_render::style_paints_any_layer`, the
/// same one `tellurion-server`'s `StylesLinkContributor` applies for the
/// identical decision on Collection documents (`#220`) — one rule, so the
/// set of styles a client sees on the TileSet resource and the set it sees
/// on the STAC/Features Collection can never disagree.
///
/// A style that fails to load, or that the store no longer has, answers
/// `false`: a link is a promise, and a promise that cannot be checked is not
/// made. The load failure is warned about rather than propagated — the
/// tileset itself is perfectly servable without that one link.
fn style_applies(ctx: &Arc<AppContext>, style_id: &str, layers: &BTreeSet<String>) -> bool {
    match ctx.style_store.load(style_id) {
        Ok(Some(doc)) => style_paints_any_layer(&doc, layers),
        Ok(None) => false,
        Err(error) => {
            tracing::warn!(%error, style = %style_id, "failed to load style; advertising no map link for it");
            false
        }
    }
}

/// `.../collections/{cid}/tiles/{tileMatrixSetId}` — the resolved-vector
/// tileset body; `tms` (`#190`) picks whose grid the limits, URIs, and
/// hrefs describe, with `WebMercatorQuad` producing the pre-`#190` body
/// byte-for-byte.
async fn tileset_vector_body(
    ctx: &Arc<AppContext>,
    resolved: ResolvedTiles,
    cid: &str,
    tms: TileMatrixSet,
    uri: &axum::http::Uri,
) -> Response {
    let decl = &resolved.decl;
    let limits = tile_matrix_limits(decl, tms);

    // This handler is mounted at `.../collections/{cid}/tiles/{tmsId}`
    // (see `router` below) — `uri.path()` IS that self href; the catalog
    // root's `tileMatrixSets/{tmsId}` sibling is three segments up from
    // `collections/{cid}/tiles/{tmsId}`.
    let self_path = ctx.current().config.server.public_href(uri.path());
    let catalog_root = self_path
        .strip_suffix(&format!("/collections/{cid}/tiles/{}", tms.id()))
        .unwrap_or(&self_path)
        .to_string();

    // `#49`: the real MVT source-layer name(s) a client must reference in a
    // style's `source-layer` to draw this collection — never guessed from
    // the collection's public id. A driver that can report this cheaply
    // (PMTiles, from its own `vector_layers` metadata) wins; every other
    // driver in this workspace (PostGIS) embeds exactly `external_id()` into
    // `ST_AsMVT` (see `tellurion-postgis::sql::build_mvt_plan`), which is
    // also the only name this handler could ever honestly fall back to — see
    // `TileSource::vector_layers`'s own doc for why that is never the
    // internal id.
    // `#245`: the resolution and its `external_id()` fallback now live in
    // `tellurion_core::advertised_vector_layers`, shared with the style
    // applicability check below and with `tellurion-server`'s own
    // `StylesLinkContributor` — one answer to "which MVT layers does this
    // collection have", so the names advertised here and the styles
    // advertised for them can never be computed from different sets.
    let layer_names = advertised_vector_layers(decl, resolved.source.as_ref()).await;
    let applicable_layers: BTreeSet<String> = layer_names.iter().cloned().collect();
    // `#85`: the resolved vector-tile property allowlist, so a client can
    // discover exactly which properties a style can draw on from this
    // resource alone, without probing an actual tile first. Applies to every
    // reported layer name identically — `tile_properties` is a per-
    // collection setting, not per-layer, and every driver in this workspace
    // that can report more than one layer name (PMTiles) is outside this
    // slice's own scope (PostGIS and the embedded GeoPackage driver, `#85`'s
    // "First slice", both ever report exactly one). Omitted entirely (not an
    // empty array) when the allowlist is empty — the pk-only default, so a
    // collection that never sets `tile_properties` gets byte-for-byte the
    // pre-`#85` layer object.
    let properties = decl.tile_properties.clone();
    let layers: Vec<_> = layer_names
        .into_iter()
        .map(|name| {
            let mut layer = serde_json::json!({ "id": name, "dataType": "vector" });
            if !properties.is_empty() {
                layer["properties"] = serde_json::json!(properties);
            }
            layer
        })
        .collect();

    // `#49`: one `map`-rel link per registered style (`ctx.style_store` —
    // global across collections, see its own doc comment) pointing at this
    // collection's rendered-tile endpoint for that style, so a client can
    // see every styled-map option without probing. Re-sorted here rather
    // than trusting `list`'s own order: `StyleStore::list` only promises
    // "every registered id", not a stable order, across implementations.
    //
    // `#245`: "per registered style" is now "per *applicable* registered
    // style". The style registry is global but a MapLibre style document is
    // not — `tellurion_render::resolve_layer_paints` keys every layer's
    // paint by `source-layer`, so a style naming none of the layer names
    // resolved above paints nothing on this collection and its `map` link
    // led to a blank tile. `#220` fixed exactly this on the link-contributor
    // side; this is the same predicate (`style_paints_any_layer`), applied
    // to the resource that describes the tileset itself. A style that fails
    // to load, or that is missing from the store, is not advertised either:
    // an unverifiable claim is not made.
    let mut links = vec![
        serde_json::json!({
            "rel": "self",
            "type": "application/json",
            "href": self_path,
        }),
        serde_json::json!({
            "rel": "tileMatrixSet",
            "type": "application/json",
            "href": format!("{catalog_root}/tileMatrixSets/{}", tms.id()),
        }),
        serde_json::json!({
            "rel": "item",
            "type": MVT_MIME,
            "href": format!("{self_path}/{{tileMatrix}}/{{tileRow}}/{{tileCol}}.mvt"),
            "templated": true,
        }),
        serde_json::json!({
            "rel": "item",
            "type": PNG_MIME,
            "href": format!("{self_path}/{{tileMatrix}}/{{tileRow}}/{{tileCol}}.png"),
            "templated": true,
        }),
    ];
    match ctx.style_store.list() {
        Ok(mut style_ids) => {
            style_ids.sort();
            for style_id in style_ids {
                if !style_applies(ctx, &style_id, &applicable_layers) {
                    continue;
                }
                links.push(serde_json::json!({
                    "rel": MAP_REL,
                    "type": PNG_MIME,
                    "title": style_id,
                    "templated": true,
                    "href": format!(
                        "{catalog_root}/collections/{cid}/styles/{style_id}/map/tiles/{}/{{tileMatrix}}/{{tileRow}}/{{tileCol}}",
                        tms.id()
                    ),
                }));
            }
        }
        Err(error) => {
            tracing::warn!(%error, "style store failed to list styles for the tileset's styled-map links");
        }
    }

    let body = serde_json::json!({
        "tileMatrixSetId": tms.id(),
        "tileMatrixSetURI": tilematrixset::uri_of(tms),
        "dataType": "vector",
        "crs": tilematrixset::crs_of(tms),
        // OGC API Tiles Part 1's own `mediaTypes` field ("Media types
        // available for the tiles") — every resolved TileSource in this
        // workspace serves both encodings via content negotiation (`tile`'s
        // MVT/PNG branches share one driver call), so both are always true.
        "mediaTypes": [MVT_MIME, PNG_MIME],
        "layers": layers,
        "tileMatrixSetLimits": limits,
        "links": links,
    });
    axum::Json(body).into_response()
}

/// `.../collections/{cid}/tiles/WebMercatorQuad` — the raster (`#37`)
/// counterpart of [`tileset_vector_body`], reached only after
/// `resolve_tiles` has already refused the collection (no vector
/// `TileSource` in its tiles lane), the same resolution order [`tile`]'s
/// own MVT/raster fallback uses. Describes the collection honestly instead
/// of the vector defaults: `PNG_MIME` is the only media type (this driver
/// never produces MVT — [`raster_tile_response`] refuses it outright),
/// `layers` stays empty (a decoded pixel window carries no source-layer
/// concept to name), and `dataType` is `"map"` — the OGC TileSet metadata
/// schema's own term for a tileset made of rendered images, as opposed to
/// `"vector"`. No styled-map links either: every registered style paints an
/// MVT source layer (`styled_tile`'s own `fetch_mvt`-based pipeline), which
/// a raster collection has none of — styling/colormaps are out of this
/// lane's scope entirely.
fn tileset_raster_body(
    ctx: &Arc<AppContext>,
    resolved: &ResolvedRaster,
    cid: &str,
    uri: &axum::http::Uri,
) -> Response {
    let decl = &resolved.decl;
    // `#190`: the raster lane is native-grid only — `tileset` refuses any
    // other tile matrix set by name before calling in here, so this body
    // only ever describes WebMercatorQuad.
    let limits = tile_matrix_limits(decl, TileMatrixSet::WebMercatorQuad);

    let self_path = ctx.current().config.server.public_href(uri.path());
    let catalog_root = self_path
        .strip_suffix(&format!("/collections/{cid}/tiles/WebMercatorQuad"))
        .unwrap_or(&self_path)
        .to_string();

    let links = vec![
        serde_json::json!({
            "rel": "self",
            "type": "application/json",
            "href": self_path,
        }),
        serde_json::json!({
            "rel": "tileMatrixSet",
            "type": "application/json",
            "href": format!("{catalog_root}/tileMatrixSets/WebMercatorQuad"),
        }),
        serde_json::json!({
            "rel": "item",
            "type": PNG_MIME,
            "href": format!("{self_path}/{{tileMatrix}}/{{tileRow}}/{{tileCol}}.png"),
            "templated": true,
        }),
    ];

    let body = serde_json::json!({
        "tileMatrixSetId": tilematrixset::WEB_MERCATOR_QUAD_ID,
        "tileMatrixSetURI": tilematrixset::WEB_MERCATOR_QUAD_URI,
        "dataType": "map",
        "crs": tilematrixset::WEB_MERCATOR_QUAD_CRS,
        "mediaTypes": [PNG_MIME],
        "layers": [],
        "tileMatrixSetLimits": limits,
        "links": links,
    });
    axum::Json(body).into_response()
}

/// Which capability a collection's tiles lane actually resolved to (`#37`)
/// — the outcome [`resolve_tileset`] hands back to both `tileset` (the full
/// TileSet body) and `tileset_list` (that same tileset's one-line entry), so
/// the two endpoints report the same `dataType` for the same collection
/// through the same resolution order rather than each guessing on its own.
enum ResolvedTileset {
    Vector(ResolvedTiles),
    Raster(ResolvedRaster),
}

/// Resolves `.../collections/{cid}/tiles*`'s collection and authorizes it,
/// falling back from vector to raster exactly the way `tile` itself does
/// (`#37`): a collection whose tiles lane implements `TileSource` resolves
/// as [`ResolvedTileset::Vector`]; one whose lane only ever advertises
/// `RasterSource` (no vector `TileSource` anywhere in it — see
/// [`resolve_tiles`]'s own doc for why a mixed lane never reaches this
/// branch, `tiles_source` already prefers any vector-capable entry over
/// raster ones) resolves as [`ResolvedTileset::Raster`]. Neither exists →
/// 404. Shared by [`tileset`] and [`tileset_list`] so both endpoints agree.
async fn resolve_tileset(
    ctx: &AppContext,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
    cid: &str,
) -> Result<ResolvedTileset, Response> {
    if let Some(resolved) = resolve_tiles(ctx, params, cid).await {
        authorize_tiles(
            ctx,
            headers,
            &resolved.tenant_id,
            &resolved.catalog_id,
            &resolved.collection_id,
            resolved.source.filter_capable(),
        )
        .await?;
        return Ok(ResolvedTileset::Vector(resolved));
    }

    // `#37`: this collection's tiles lane never implements `TileSource`
    // (every existing vector collection resolves above, so this only runs
    // for one that doesn't) — try the raster capability before giving up
    // with a 404, the same resolution order `tile` itself uses.
    let Some(resolved) = resolve_raster(ctx, params, cid).await else {
        return Err(StatusCode::NOT_FOUND.into_response());
    };
    // `RasterSource` has no `filter_capable` concept (see its own doc) —
    // the same conservative default `raster_tile_response` gives a raster
    // collection's actual tile lane.
    authorize_tiles(
        ctx,
        headers,
        &resolved.tenant_id,
        &resolved.catalog_id,
        &resolved.collection_id,
        false,
    )
    .await?;
    Ok(ResolvedTileset::Raster(resolved))
}

async fn tileset(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let Some(cid) = params.get("cid").cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // `#190`: an id outside the closed registry is a 404 (the pre-`#190`
    // unmatched-route answer); a registered id the resolved lane can't
    // serve is the same by-name capability refusal the tile lanes give.
    let Some(tms) = tms_of(&params) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match resolve_tileset(&ctx, &params, &headers, &cid).await {
        Ok(ResolvedTileset::Vector(resolved)) => {
            if !resolved.source.supports_tile_matrix_set(tms) {
                return refuse_tile_matrix_set(&cid, tms, "tile source");
            }
            tileset_vector_body(&ctx, resolved, &cid, tms, &uri).await
        }
        Ok(ResolvedTileset::Raster(resolved)) => {
            if tms != TileMatrixSet::WebMercatorQuad {
                return refuse_tile_matrix_set(&cid, tms, "raster source");
            }
            tileset_raster_body(&ctx, &resolved, &cid, &uri)
        }
        Err(response) => response,
    }
}

async fn tile_matrix_sets_list(
    State(ctx): State<Arc<AppContext>>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let self_path = ctx.current().config.server.public_href(uri.path());
    // `#190`: every registry entry, in `TileMatrixSet::ALL`'s own
    // advertisement order — this listing and `tile_matrix_set_definition`
    // below both walk the same closed registry, so a listed id always
    // resolves and an unlisted one never does.
    let sets: Vec<serde_json::Value> = TileMatrixSet::ALL
        .into_iter()
        .map(|tms| {
            serde_json::json!({
                "id": tms.id(),
                "uri": tilematrixset::uri_of(tms),
                "links": [{
                    "rel": "self",
                    "href": format!("{}/{}", self_path.trim_end_matches('/'), tms.id()),
                }],
            })
        })
        .collect();
    axum::Json(serde_json::json!({ "tileMatrixSets": sets })).into_response()
}

async fn tile_matrix_set_definition(Path(params): Path<HashMap<String, String>>) -> Response {
    match tms_of(&params) {
        Some(tms) => axum::Json(tilematrixset::document_for(tms)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TileFormat {
    Mvt,
    Png,
}

fn split_suffix(raw: &str) -> (&str, Option<&str>) {
    match raw.rsplit_once('.') {
        Some((base, "mvt")) => (base, Some("mvt")),
        Some((base, "png")) => (base, Some("png")),
        _ => (raw, None),
    }
}

/// Suffix on the `y` segment wins outright; then the standard OGC API `f`
/// query parameter (`?f=mvt`/`?f=png`); otherwise MVT is the default unless
/// the client asked for PNG and did not also accept MVT via `Accept`.
fn negotiate_format(
    suffix: Option<&str>,
    query_format: Option<&str>,
    headers: &HeaderMap,
) -> TileFormat {
    match suffix {
        Some("png") => return TileFormat::Png,
        Some("mvt") => return TileFormat::Mvt,
        _ => {}
    }
    match query_format {
        Some("png") => return TileFormat::Png,
        Some("mvt") => return TileFormat::Mvt,
        _ => {}
    }
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if accept.contains(PNG_MIME) && !accept.contains(MVT_MIME) {
        TileFormat::Png
    } else {
        TileFormat::Mvt
    }
}

fn png_key(
    tenant: &str,
    catalog: &str,
    collection: &str,
    coord: TileCoord,
    policy_fingerprint: Option<u64>,
) -> TileKey {
    TileKey {
        encoding: Encoding::Png,
        ..mvt_key(tenant, catalog, collection, coord, policy_fingerprint)
    }
}

fn styled_png_key(
    tenant: &str,
    catalog: &str,
    collection: &str,
    coord: TileCoord,
    style_id: &str,
    policy_fingerprint: Option<u64>,
) -> TileKey {
    TileKey {
        encoding: Encoding::PngStyled(style_id.to_string()),
        ..mvt_key(tenant, catalog, collection, coord, policy_fingerprint)
    }
}

/// The raster (COG) PNG lane's own cache key (`#37`/`#92`) — like `png_key`,
/// but carrying the collection's resolved colormap fingerprint
/// (`ColormapConf::fingerprint`), or `None` when no colormap is configured,
/// so a config reload that changes a collection's colormap never serves the
/// previous colormap's cached bytes for the same tile (see
/// `tellurion_core::cache::Encoding::PngRaster`'s own doc).
fn raster_png_key(
    tenant: &str,
    catalog: &str,
    collection: &str,
    coord: TileCoord,
    colormap_fingerprint: Option<u64>,
    policy_fingerprint: Option<u64>,
) -> TileKey {
    TileKey {
        encoding: Encoding::PngRaster(colormap_fingerprint),
        ..mvt_key(tenant, catalog, collection, coord, policy_fingerprint)
    }
}

/// Shared RFC 9457 problem-details body — same type `tellurion-features`
/// serves for its API errors. Reused by the maps lane (`maps.rs`, `#86`)
/// so every named refusal in this crate shares one problem+json shape.
pub(crate) fn problem_response(
    status: StatusCode,
    code: &str,
    detail: impl Into<String>,
) -> Response {
    let problem = Problem::new(status.as_u16(), code, detail);
    let mut response = (status, axum::Json(problem)).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
    response
}

fn tile_response(content_type: &'static str, bytes: Bytes) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, TILE_CACHE_CONTROL),
        ],
        bytes,
    )
        .into_response()
}

/// Small `Err` type for [`parse_tile_coord`] — a bare `Response` is over a
/// hundred bytes and clippy (rightly) flags returning that by value as the
/// error variant of a `Result`; this stays a couple of machine words and
/// builds the actual `Response` only once the caller decides to bail out.
enum TileCoordError {
    NotFound,
    BadRequest(&'static str),
}

impl IntoResponse for TileCoordError {
    fn into_response(self) -> Response {
        match self {
            TileCoordError::NotFound => StatusCode::NOT_FOUND.into_response(),
            TileCoordError::BadRequest(message) => {
                (StatusCode::BAD_REQUEST, message).into_response()
            }
        }
    }
}

/// Parses `tileMatrix`/`tileRow`/`tileCol` path params (OGC API Tiles order
/// — row before column) against `decl`'s configured zoom range and `tms`'s
/// index bounds (`#190`: `WorldCRS84Quad` admits twice as many columns as
/// rows at every level, so the bounds come from the grid itself, never a
/// square `1 << z` assumption), splitting any `.mvt`/`.png` suffix off
/// `tileCol` first — the last path segment in this order. Shared by the
/// unstyled and styled tile handlers so both routes enforce identical
/// coordinate rules.
fn parse_tile_coord(
    params: &HashMap<String, String>,
    decl: &CollectionDecl,
    tms: TileMatrixSet,
) -> Result<(TileCoord, Option<String>), TileCoordError> {
    let z_raw = params.get("tileMatrix").ok_or(TileCoordError::NotFound)?;
    let z: u8 = z_raw
        .parse()
        .map_err(|_| TileCoordError::BadRequest("invalid zoom"))?;
    if z < decl.tiles.minzoom || z > decl.tiles.maxzoom {
        return Err(TileCoordError::NotFound);
    }

    let row_raw = params.get("tileRow").ok_or(TileCoordError::NotFound)?;
    let row: u32 = row_raw
        .parse()
        .map_err(|_| TileCoordError::BadRequest("invalid tile row"))?;

    let col_raw = params.get("tileCol").ok_or(TileCoordError::NotFound)?;
    let (col_part, suffix) = split_suffix(col_raw);
    let col: u32 = col_part
        .parse()
        .map_err(|_| TileCoordError::BadRequest("invalid tile column"))?;

    if u64::from(col) >= tms.matrix_width(z) || u64::from(row) >= tms.matrix_height(z) {
        return Err(TileCoordError::BadRequest("tile index out of range"));
    }

    Ok((TileCoord { z, x: col, y: row }, suffix.map(str::to_string)))
}

async fn tile(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(cid) = params.get("cid") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // `#190`: resolve the grid before anything else — an unknown id is the
    // same 404 the unmatched literal route used to give.
    let Some(tms) = tms_of(&params) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(resolved) = resolve_tiles(&ctx, &params, cid).await else {
        // `#37`: this collection's tiles lane never implements `TileSource`
        // (every existing vector collection resolves above, so this only
        // runs for one that doesn't) — try the raster capability before
        // giving up with a 404.
        return match resolve_raster(&ctx, &params, cid).await {
            Some(resolved) => {
                raster_tile_response(&ctx, resolved, &params, &query, &headers, tms).await
            }
            None => StatusCode::NOT_FOUND.into_response(),
        };
    };
    let ResolvedTiles {
        tenant_id,
        catalog_id,
        collection_id,
        decl,
        source,
    } = resolved;
    // `#190` capability honesty, at resolve time: a driver that can only
    // serve its native grid refuses this request by name here, before any
    // coordinate parsing or driver call — never a mid-request error or an
    // empty tile.
    if !source.supports_tile_matrix_set(tms) {
        return refuse_tile_matrix_set(cid, tms, "tile source");
    }
    let policy_filter = match authorize_tiles(
        &ctx,
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        source.filter_capable(),
    )
    .await
    {
        Ok(filter) => filter,
        Err(response) => return response,
    };

    let (coord, suffix) = match parse_tile_coord(&params, &decl, tms) {
        Ok(parsed) => parsed,
        Err(error) => return error.into_response(),
    };
    let format = negotiate_format(
        suffix.as_deref(),
        query.get("f").map(String::as_str),
        &headers,
    );

    match format {
        TileFormat::Mvt => {
            match ctx
                .fetch_mvt(
                    &tenant_id,
                    &catalog_id,
                    &collection_id,
                    tms,
                    coord,
                    &decl,
                    &source,
                    policy_filter.as_ref(),
                    None,
                )
                .await
            {
                MvtFetch::Hit(bytes) => tile_response(MVT_MIME, bytes),
                MvtFetch::Empty => StatusCode::NO_CONTENT.into_response(),
                MvtFetch::Failed => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        TileFormat::Png => {
            // The whole mvt-fetch + rasterize pipeline lives inside one
            // `populate` closure keyed by `key` (the Png cache key), so
            // moka's single-flight coalesces N concurrent misses on the same
            // tile into exactly one rasterize — not just one driver fetch.
            let key = TileKey {
                // `#190`: the grid identity, same value the nested
                // `fetch_mvt` folds into its Mvt entry.
                tms,
                // `#113`: this tile's bucket generation, so a write-reactive
                // bump forces a fresh render here too — same value
                // `fetch_mvt` resolves for the underlying Mvt entry this
                // Png entry renders from.
                generation: ctx.tile_generation(&collection_id, tms, coord),
                ..png_key(
                    &tenant_id,
                    &catalog_id,
                    &collection_id,
                    coord,
                    policy_filter
                        .as_ref()
                        .map(tellurion_core::Filter::fingerprint),
                )
            };
            let ctx_for_populate = Arc::clone(&ctx);
            let tenant_id_owned = tenant_id.clone();
            let catalog_id_owned = catalog_id.clone();
            let collection_id_owned = collection_id.clone();
            let filter_for_populate = policy_filter.clone();
            let populate: PopulateFuture = Box::pin(async move {
                let mvt_bytes = match ctx_for_populate
                    .fetch_mvt(
                        &tenant_id_owned,
                        &catalog_id_owned,
                        &collection_id_owned,
                        tms,
                        coord,
                        &decl,
                        &source,
                        filter_for_populate.as_ref(),
                        None,
                    )
                    .await
                {
                    MvtFetch::Hit(bytes) => bytes,
                    MvtFetch::Empty => return Ok(Bytes::new()),
                    MvtFetch::Failed => {
                        return Err(Error::Storage(
                            "mvt tile source failed to produce a tile to render".into(),
                        ))
                    }
                };

                let style = RenderStyle::new(
                    &decl.style.fill,
                    &decl.style.stroke,
                    decl.style.stroke_width as f32,
                    DEFAULT_POINT_RADIUS_PX,
                )
                .map_err(|error| Error::Storage(Box::new(error)))?;

                // Rasterizing is measured, non-trivial CPU work (low-single-digit
                // milliseconds for a moderately busy basemap tile, low double digits
                // for a dense one — see the #29 offload decision), so it runs on the
                // blocking pool instead of occupying an async worker thread for the
                // duration. `mvt_bytes`/`style` are owned values moved into the
                // closure, not borrowed from anything held across the `.await`, so no
                // lock or pooled connection stays alive while it runs; if this
                // `populate` future is dropped (request timeout/shed) before the
                // blocking task finishes, the task keeps running to completion on its
                // own thread and its result is simply discarded — nothing here to
                // poison.
                let png_bytes = tokio::task::spawn_blocking(move || {
                    render_mvt_to_png(mvt_bytes.as_ref(), &style, RENDER_TILE_SIZE_PX)
                })
                .await
                .map_err(|join_error| Error::Storage(Box::new(join_error)))?
                .map_err(|error| Error::Storage(Box::new(error)))?;
                Ok(Bytes::from(png_bytes))
            });

            match ctx.get_or_populate(&collection_id, key, populate).await {
                Ok(bytes) if bytes.is_empty() => StatusCode::NO_CONTENT.into_response(),
                Ok(bytes) => tile_response(PNG_MIME, bytes),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    }
}

/// The raster (Cloud-Optimized GeoTIFF) lane (`#37`): PNG only, served from
/// decoded pixel windows rather than a rasterized MVT tile — `tile` calls
/// this only after `resolve_tiles` has already refused the collection (no
/// vector `TileSource` anywhere in its tiles lane). Same single-flight
/// cache-populate shape as the vector Png lane (`tile`'s own `TileFormat::
/// Png` branch): fetch, then encode, inside one `populate` closure keyed by
/// the same `png_key` a vector collection's PNG lane uses — the cache
/// itself doesn't distinguish where a PNG tile's bytes came from.
async fn raster_tile_response(
    ctx: &Arc<AppContext>,
    resolved: ResolvedRaster,
    params: &HashMap<String, String>,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    tms: TileMatrixSet,
) -> Response {
    let ResolvedRaster {
        tenant_id,
        catalog_id,
        collection_id,
        decl,
        source,
    } = resolved;
    // `#190` capability honesty: every raster driver in this workspace
    // (COG, Zarr) decodes against its archive-native WebMercatorQuad
    // pyramid — `RasterSource` declares no other grid, so any other tile
    // matrix set is refused here by name, at resolve time, mirroring the
    // vector lane's own `supports_tile_matrix_set` gate.
    if tms != TileMatrixSet::WebMercatorQuad {
        return refuse_tile_matrix_set(decl.external_id(), tms, "raster source");
    }
    // `RasterSource` has no `filter_capable` concept (see its own doc) — a
    // filtered-only grant is denied outright, the same conservative default
    // any other driver without the capability already gets.
    let policy_filter =
        match authorize_tiles(ctx, headers, &tenant_id, &catalog_id, &collection_id, false).await {
            Ok(filter) => filter,
            Err(response) => return response,
        };

    let (coord, suffix) = match parse_tile_coord(params, &decl, tms) {
        Ok(parsed) => parsed,
        Err(error) => return error.into_response(),
    };
    let format = negotiate_format(
        suffix.as_deref(),
        query.get("f").map(String::as_str),
        headers,
    );
    if format == TileFormat::Mvt {
        return problem_response(
            StatusCode::BAD_REQUEST,
            "CapabilityUnsupported",
            "this collection serves raster tiles; MVT is not available",
        );
    }

    // `#92`: folded into the cache key so a config reload that changes this
    // collection's colormap never serves the previous colormap's cached
    // bytes for the same tile — see `raster_png_key`'s own doc.
    let colormap_fingerprint = decl
        .settings
        .colormap
        .as_ref()
        .map(tellurion_core::ColormapConf::fingerprint);
    let key = TileKey {
        // `#113`: same generation resolution as the vector Png lane above.
        // `#190`: `tms` is always `WebMercatorQuad` past the refusal above,
        // so `raster_png_key`'s default grid is already the right one.
        generation: ctx.tile_generation(&collection_id, tms, coord),
        ..raster_png_key(
            &tenant_id,
            &catalog_id,
            &collection_id,
            coord,
            colormap_fingerprint,
            policy_filter
                .as_ref()
                .map(tellurion_core::Filter::fingerprint),
        )
    };
    let populate: PopulateFuture = Box::pin(async move {
        let window = match source.raster_tile(&decl, coord).await? {
            Some(window) => window,
            None => return Ok(Bytes::new()),
        };
        // Same rasterize-is-real-CPU-work rationale as the vector Png lane
        // — offloaded the same way, for the same reason.
        let png_bytes = tokio::task::spawn_blocking(move || {
            encode_rgba_to_png(&window.rgba, window.width, window.height)
        })
        .await
        .map_err(|join_error| Error::Storage(Box::new(join_error)))?
        .map_err(|error| Error::Storage(Box::new(error)))?;
        Ok(Bytes::from(png_bytes))
    });

    match ctx.get_or_populate(&collection_id, key, populate).await {
        Ok(bytes) if bytes.is_empty() => StatusCode::NO_CONTENT.into_response(),
        Ok(bytes) => tile_response(PNG_MIME, bytes),
        // `#37`: the driver refused because honoring this tile would exceed
        // its own per-request source-pixel budget — a client-correctable 400,
        // never a 500 or a ballooned read.
        Err(error) if matches!(error.as_ref(), Error::Invalid(_)) => problem_response(
            StatusCode::BAD_REQUEST,
            "PixelBudgetExceeded",
            error.to_string(),
        ),
        // `#92`: a raster whose band layout can't support its collection's
        // configured colormap refuses here by name (`tellurion-cog`'s own
        // capability-mismatch check) — the same 400 shape as any other
        // capability this driver can't honor, not an opaque 500.
        Err(error) if matches!(error.as_ref(), Error::Config(_)) => problem_response(
            StatusCode::BAD_REQUEST,
            "CapabilityUnsupported",
            error.to_string(),
        ),
        Err(_) => problem_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalServerError",
            "an internal storage error occurred",
        ),
    }
}

/// GET .../styles/{styleId}/map/tiles/{tileMatrixSetId}/{tileMatrix}/{tileRow}/{tileCol} — the
/// PngStyled lane: same MVT-first cache pattern as the unstyled PNG lane
/// (`tile`), but the raster paint comes from a resolved style document
/// instead of the collection's single `StyleConf`, and the cache key carries
/// the style id so two styles over the same tile never collide. Styling
/// only affects rasterization, so MVT is not an offerable format here — a
/// request that negotiates to MVT (suffix, `f=mvt`, or `Accept`) is a 400.
async fn styled_tile(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(cid) = params.get("cid") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(style_id) = params.get("styleId") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // `#190`: same grid resolution + capability gate as the unstyled lane —
    // styling only changes rasterization, not which grids the underlying
    // MVT fetch can honor.
    let Some(tms) = tms_of(&params) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(resolved) = resolve_tiles(&ctx, &params, cid).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let ResolvedTiles {
        tenant_id,
        catalog_id,
        collection_id,
        decl,
        source,
    } = resolved;
    if !source.supports_tile_matrix_set(tms) {
        return refuse_tile_matrix_set(cid, tms, "tile source");
    }
    let policy_filter = match authorize_tiles(
        &ctx,
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        source.filter_capable(),
    )
    .await
    {
        Ok(filter) => filter,
        Err(response) => return response,
    };

    let (coord, suffix) = match parse_tile_coord(&params, &decl, tms) {
        Ok(parsed) => parsed,
        Err(error) => return error.into_response(),
    };
    let format = negotiate_format(
        suffix.as_deref(),
        query.get("f").map(String::as_str),
        &headers,
    );
    if format == TileFormat::Mvt {
        return (
            StatusCode::BAD_REQUEST,
            "styled tiles are raster-only; MVT cannot be styled",
        )
            .into_response();
    }

    // Same single-flight shape as the unstyled Png lane: mvt-fetch, style
    // lookup and rasterize all happen inside one `populate` closure keyed by
    // `key`, so a cache hit skips style validation entirely (matching the
    // pre-single-flight behavior exactly) and N concurrent misses share one
    // rasterize. `Error::NotFound` from a missing style id is recovered into
    // the same 404 problem body this lane returned before.
    let key = TileKey {
        // `#190`: the grid identity, same value the nested `fetch_mvt`
        // folds into its Mvt entry.
        tms,
        // `#113`: same generation resolution as the vector Png lane.
        generation: ctx.tile_generation(&collection_id, tms, coord),
        ..styled_png_key(
            &tenant_id,
            &catalog_id,
            &collection_id,
            coord,
            style_id,
            policy_filter
                .as_ref()
                .map(tellurion_core::Filter::fingerprint),
        )
    };
    let ctx_for_populate = Arc::clone(&ctx);
    let tenant_id_owned = tenant_id.clone();
    let catalog_id_owned = catalog_id.clone();
    let collection_id_owned = collection_id.clone();
    let style_id_owned = style_id.to_string();
    let filter_for_populate = policy_filter.clone();
    let populate: PopulateFuture = Box::pin(async move {
        let mvt_bytes = match ctx_for_populate
            .fetch_mvt(
                &tenant_id_owned,
                &catalog_id_owned,
                &collection_id_owned,
                tms,
                coord,
                &decl,
                &source,
                filter_for_populate.as_ref(),
                None,
            )
            .await
        {
            MvtFetch::Hit(bytes) => bytes,
            MvtFetch::Empty => return Ok(Bytes::new()),
            MvtFetch::Failed => {
                return Err(Error::Storage(
                    "mvt tile source failed to produce a tile to render".into(),
                ))
            }
        };

        let style_doc = ctx_for_populate
            .style_store
            .load(&style_id_owned)
            .map_err(|error| {
                tracing::error!(%error, style_id = %style_id_owned, "style store failed to load a registered style");
                error
            })?
            .ok_or(Error::NotFound)?;

        // Resolved AT THIS TILE'S OWN ZOOM (`#174`): a style's zoom-driven
        // `step`/`interpolate` paint expressions describe how the map looks
        // per zoom level, and this lane serves exactly one tile at one
        // zoom, so `coord.z` is the zoom those expressions are asking
        // about. The zoom is already part of `key` (it is part of the tile
        // coordinate), so two zooms can never share one cached rendering.
        let paints = resolve_layer_paints(&style_doc, f64::from(coord.z));
        // Same rasterize-is-real-CPU-work rationale as the unstyled Png lane
        // in `tile` above — offloaded the same way, for the same reason.
        let png_bytes = tokio::task::spawn_blocking(move || {
            render_mvt_to_png_styled(mvt_bytes.as_ref(), &paints, None, RENDER_TILE_SIZE_PX)
        })
        .await
        .map_err(|join_error| Error::Storage(Box::new(join_error)))?
        .map_err(|error| Error::Storage(Box::new(error)))?;
        Ok(Bytes::from(png_bytes))
    });

    match ctx.get_or_populate(&collection_id, key, populate).await {
        Ok(bytes) if bytes.is_empty() => StatusCode::NO_CONTENT.into_response(),
        Ok(bytes) => tile_response(PNG_MIME, bytes),
        Err(err) if matches!(err.as_ref(), Error::NotFound) => problem_response(
            StatusCode::NOT_FOUND,
            "NotFound",
            format!("style '{style_id}' not found"),
        ),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use axum::http::HeaderValue;
    use geozero::mvt::{tile, Message, Tile};
    use tellurion_core::{
        AppConfig, CatalogSource, DriverFactory, MokaTileCache, PhysicalCollection, Registry,
        Resolver, Result as CoreResult, Router, StaticResolver, StorageDecl, StorageDriver,
        StyleStore, TileCache,
    };

    /// A `CatalogSource` that reports no collections — every fake driver in
    /// this module's tests is exercised through handlers, not through
    /// `Router::validate_catalog`, so this is present only to satisfy the
    /// trait.
    struct EmptyCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for EmptyCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![])
        }
    }

    struct FakeTileSource {
        tiles: Mutex<HashMap<(u8, u32, u32), Option<Bytes>>>,
        calls: AtomicUsize,
        /// Artificial delay before returning, so a test can spawn N
        /// concurrent requests and be sure they overlap in-flight rather
        /// than serializing one after another.
        delay: std::time::Duration,
        /// `#34`: whether this source advertises `filter_capable()` —
        /// `false` (the trait default) for every existing fixture in this
        /// module unless built via [`Self::with_filter_capable`].
        filter_capable: bool,
        /// `#190`: whether this source advertises `WorldCRS84Quad` support
        /// (the PostGIS shape) — `false` (the trait default: native
        /// WebMercatorQuad only) for every existing fixture unless built
        /// via [`Self::with_tms_capable`].
        tms_capable: bool,
        /// Whether this source can serve the resolved collection. This
        /// fixture is normally capable; the opt-out proves a source-level
        /// metadata gate is a 404 at the protocol boundary, never a tile
        /// fetch that later becomes a 500.
        tile_capable: bool,
        /// `#190`: the grid the last `mvt_tile_in` call actually carried —
        /// lets a test assert what reached the driver, the same seam
        /// `last_filter_fingerprint` provides for filters.
        last_tms: Mutex<Option<TileMatrixSet>>,
        /// `#34`: the fingerprint of the last filter `mvt_tile` actually
        /// received (`None` for an unfiltered call) — lets a test assert
        /// exactly what reached the driver, not just what the cache key
        /// carries.
        last_filter_fingerprint: Mutex<Option<u64>>,
    }

    impl FakeTileSource {
        fn new() -> Self {
            Self {
                tiles: Mutex::new(HashMap::new()),
                calls: AtomicUsize::new(0),
                delay: std::time::Duration::ZERO,
                filter_capable: false,
                tms_capable: false,
                tile_capable: true,
                last_tms: Mutex::new(None),
                last_filter_fingerprint: Mutex::new(None),
            }
        }

        fn with_delay(delay: std::time::Duration) -> Self {
            Self {
                delay,
                ..Self::new()
            }
        }

        /// `#34`: a variant that advertises `filter_capable() == true`, so
        /// the tiles lane's policy checkpoint can push a matched grant's
        /// filter down to it instead of denying outright.
        fn with_filter_capable() -> Self {
            Self {
                filter_capable: true,
                ..Self::new()
            }
        }

        /// `#190`: a variant that advertises both registered tile matrix
        /// sets — the PostGIS shape — so the handlers' resolve-time grid
        /// gate lets a `WorldCRS84Quad` request through to the driver.
        fn with_tms_capable() -> Self {
            Self {
                tms_capable: true,
                ..Self::new()
            }
        }

        fn without_collection_tile_capability() -> Self {
            Self {
                tile_capable: false,
                ..Self::new()
            }
        }

        fn last_tms(&self) -> Option<TileMatrixSet> {
            *self.last_tms.lock().unwrap()
        }

        fn set(&self, coord: TileCoord, value: Option<Bytes>) {
            self.tiles
                .lock()
                .unwrap()
                .insert((coord.z, coord.x, coord.y), value);
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn last_filter_fingerprint(&self) -> Option<u64> {
            *self.last_filter_fingerprint.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl TileSource for FakeTileSource {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            coord: TileCoord,
            filter: Option<&tellurion_core::Filter>,
        ) -> tellurion_core::Result<Option<Bytes>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_filter_fingerprint.lock().unwrap() =
                filter.map(tellurion_core::Filter::fingerprint);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            Ok(self
                .tiles
                .lock()
                .unwrap()
                .get(&(coord.z, coord.x, coord.y))
                .cloned()
                .flatten())
        }

        fn tile_capable(&self, _collection: &CollectionDecl) -> bool {
            self.tile_capable
        }

        fn filter_capable(&self) -> bool {
            self.filter_capable
        }

        fn supports_tile_matrix_set(&self, tms: TileMatrixSet) -> bool {
            self.tms_capable || tms == TileMatrixSet::WebMercatorQuad
        }

        /// `#190`: the tms-capable override the real PostGIS driver has —
        /// records which grid arrived, then serves from the same fixture
        /// map (these tests assert routing/caching/refusal shape, not
        /// envelope math, which has its own tests in `tellurion_core::tms`
        /// and `tellurion-postgis::sql`).
        async fn mvt_tile_in(
            &self,
            collection: &CollectionDecl,
            tms: TileMatrixSet,
            coord: TileCoord,
            filter: Option<&tellurion_core::Filter>,
        ) -> tellurion_core::Result<Option<Bytes>> {
            if !self.supports_tile_matrix_set(tms) {
                return Err(Error::CapabilityUnsupported {
                    collection: collection.id.clone(),
                    capability: format!("tileMatrixSet:{tms}"),
                });
            }
            *self.last_tms.lock().unwrap() = Some(tms);
            self.mvt_tile(collection, coord, filter).await
        }
    }

    struct FakeDriver {
        tiles: Arc<FakeTileSource>,
    }

    impl StorageDriver for FakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::clone(&self.tiles) as Arc<dyn TileSource>)
        }
    }

    struct FakeFactory {
        tiles: Arc<FakeTileSource>,
    }

    impl DriverFactory for FakeFactory {
        fn name(&self) -> &str {
            "fake"
        }

        fn build(&self, _decl: &StorageDecl) -> tellurion_core::Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FakeDriver {
                tiles: Arc::clone(&self.tiles),
            }))
        }
    }

    /// A `RasterSource` that never actually serves a window — every test
    /// using this only exercises tileset *discovery* (`tileset`), never the
    /// tile lane itself, so `Ok(None)` (an in-range-but-uncovered tile) is
    /// enough.
    struct FakeRasterSource;

    #[async_trait::async_trait]
    impl RasterSource for FakeRasterSource {
        async fn raster_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
        ) -> tellurion_core::Result<Option<tellurion_core::RasterWindow>> {
            Ok(None)
        }
    }

    /// Reports exactly one physical collection named `"demo"` — unlike
    /// [`EmptyCatalog`], `Router::effective_decl` needs a real match here:
    /// this fixture's own `CollectionDecl` leaves `table`/`geometry`/`pk`
    /// unset (a raster collection has no such concept, the same shape
    /// `tellurion-cog`'s own config uses), so it never takes the
    /// fully-pinned fast path and always derives from the catalog instead.
    struct FakeRasterCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for FakeRasterCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![PhysicalCollection {
                name: "demo".to_string(),
                geometry_column: None,
                primary_key: None,
                srid: None,
                geometry_type: None,
            }])
        }
    }

    /// A driver that only ever advertises `raster_source()` — never
    /// `tile_source()` — the same shape `tellurion-cog`'s own driver has
    /// (see its module doc), so `resolve_tiles` refuses this collection and
    /// `tileset`/`tile` must fall back to [`resolve_raster`].
    struct FakeRasterDriver;

    impl StorageDriver for FakeRasterDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeRasterCatalog)
        }

        fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
            Some(Arc::new(FakeRasterSource))
        }
    }

    struct FakeRasterFactory;

    impl DriverFactory for FakeRasterFactory {
        fn name(&self) -> &str {
            "fake_raster"
        }

        fn build(&self, _decl: &StorageDecl) -> tellurion_core::Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FakeRasterDriver))
        }
    }

    /// In-memory `StyleStore` so styled-lane tests don't need real files on
    /// disk; mirrors `FileStyleStore`'s contract (`Ok(None)` for an
    /// unregistered id).
    struct FakeStyleStore {
        styles: HashMap<String, serde_json::Value>,
    }

    impl StyleStore for FakeStyleStore {
        fn load(&self, id: &str) -> tellurion_core::Result<Option<serde_json::Value>> {
            Ok(self.styles.get(id).cloned())
        }

        fn list(&self) -> tellurion_core::Result<Vec<String>> {
            Ok(self.styles.keys().cloned().collect())
        }
    }

    /// Wraps a real cache and counts how many times a `populate` future for
    /// `watch_encoding` specifically actually runs (i.e. how many times a
    /// `get_or_populate` caller on that encoding's key became the
    /// single-flight leader for a miss). Scoped to one encoding rather than
    /// every key, because the Png/PngStyled/Glb lanes each call `fetch_mvt`
    /// from inside their own leader's `populate` — which itself calls
    /// `get_or_populate` again for the *Mvt* key — so an unscoped counter
    /// would double-count one Png-lane render as two populate calls. A
    /// minimal test double for proving a render lane's `populate` closure —
    /// mvt-fetch plus rasterize/extrude — runs exactly once under N
    /// concurrent misses, independent of the `metrics` crate.
    struct CountingCache {
        inner: Arc<dyn TileCache>,
        watch_encoding: Encoding,
        populate_calls: Arc<AtomicUsize>,
    }

    impl CountingCache {
        fn new(
            inner: Arc<dyn TileCache>,
            watch_encoding: Encoding,
        ) -> (Arc<Self>, Arc<AtomicUsize>) {
            let populate_calls = Arc::new(AtomicUsize::new(0));
            let cache = Arc::new(Self {
                inner,
                watch_encoding,
                populate_calls: Arc::clone(&populate_calls),
            });
            (cache, populate_calls)
        }
    }

    #[async_trait::async_trait]
    impl TileCache for CountingCache {
        async fn get(&self, key: &TileKey) -> Option<Bytes> {
            self.inner.get(key).await
        }

        async fn insert(&self, key: TileKey, value: Bytes) {
            self.inner.insert(key, value).await;
        }

        async fn get_or_populate(
            &self,
            key: TileKey,
            populate: PopulateFuture,
        ) -> Result<Bytes, Arc<Error>> {
            let calls =
                (key.encoding == self.watch_encoding).then(|| Arc::clone(&self.populate_calls));
            let tracked: PopulateFuture = Box::pin(async move {
                if let Some(calls) = calls {
                    calls.fetch_add(1, Ordering::SeqCst);
                }
                populate.await
            });
            self.inner.get_or_populate(key, tracked).await
        }

        /// Handlers route every real request through `AppContext::get_or_populate`
        /// (`#46`), which resolves to this entry point whenever the collection
        /// has materialized settings (always, in these tests) — so the
        /// coalescing tests below only stay meaningful if this counts calls
        /// the same way the plain override above does.
        async fn get_or_populate_with_ttl(
            &self,
            key: TileKey,
            populate: PopulateFuture,
            ttl: std::time::Duration,
        ) -> Result<Bytes, Arc<Error>> {
            let calls =
                (key.encoding == self.watch_encoding).then(|| Arc::clone(&self.populate_calls));
            let tracked: PopulateFuture = Box::pin(async move {
                if let Some(calls) = calls {
                    calls.fetch_add(1, Ordering::SeqCst);
                }
                populate.await
            });
            self.inner.get_or_populate_with_ttl(key, tracked, ttl).await
        }
    }

    fn test_context(tiles: Arc<FakeTileSource>) -> Arc<AppContext> {
        test_context_with_styles(tiles, HashMap::new())
    }

    fn test_context_with_styles(
        tiles: Arc<FakeTileSource>,
        styles: HashMap<String, serde_json::Value>,
    ) -> Arc<AppContext> {
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        test_context_with_cache(tiles, styles, cache)
    }

    /// Same fixture as [`test_context_with_styles`], with the cache injected
    /// rather than always a bare `MokaTileCache` — lets a test wrap it (e.g.
    /// in [`CountingCache`]) to observe single-flight behavior directly.
    fn test_context_with_cache(
        tiles: Arc<FakeTileSource>,
        styles: HashMap<String, serde_json::Value>,
        cache: Arc<dyn TileCache>,
    ) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory { tiles }));
        let router = Router::build(&config, &registry).unwrap();
        let style_store: Arc<dyn StyleStore> = Arc::new(FakeStyleStore { styles });
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ))
    }

    /// A collection whose only storage is [`FakeRasterDriver`] — no
    /// `TileSource` anywhere in its tiles lane, so `resolve_tiles` refuses
    /// it and every test using this context exercises the raster (`#37`)
    /// fallback path instead.
    fn test_context_raster() -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake_raster, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeRasterFactory));
        let router = Router::build(&config, &registry).unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FakeStyleStore {
            styles: HashMap::new(),
        });
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ))
    }

    /// `#34`: same fixture shape as [`test_context_with_cache`], but built
    /// from a caller-supplied full config document (so a test can add
    /// `auth:`/`policy:` sections) rather than the fixed one baked into that
    /// function — and, unlike it, builds a real authorizer from
    /// `config.auth` instead of hardcoding `None`.
    fn test_context_with_config(tiles: Arc<FakeTileSource>, config_yaml: &str) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory { tiles }));
        let router = Router::build(&config, &registry).unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FakeStyleStore {
            styles: HashMap::new(),
        });
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer = tellurion_core::build_authorizer(&config.auth)
            .expect("no bearer principal in this fixture reads a token_env");
        Arc::new(AppContext::new(
            config,
            router,
            resolver,
            authorizer,
            cache,
            style_store,
        ))
    }

    /// `row`/`col` follow the OGC API Tiles path order (`tileRow` before
    /// `tileCol`); `col` carries any `.mvt`/`.png` suffix since it's the last
    /// path segment.
    fn path(cid: &str, z: u8, row: u32, col: &str) -> Path<HashMap<String, String>> {
        Path(HashMap::from([
            ("cid".to_string(), cid.to_string()),
            ("tileMatrix".to_string(), z.to_string()),
            ("tileRow".to_string(), row.to_string()),
            ("tileCol".to_string(), col.to_string()),
        ]))
    }

    fn path_with_tenant(
        tenant: &str,
        cid: &str,
        z: u8,
        row: u32,
        col: &str,
    ) -> Path<HashMap<String, String>> {
        let Path(mut params) = path(cid, z, row, col);
        params.insert("tenant".to_string(), tenant.to_string());
        Path(params)
    }

    fn cid_path(cid: &str) -> Path<HashMap<String, String>> {
        Path(HashMap::from([("cid".to_string(), cid.to_string())]))
    }

    /// `#190`: [`path`] with an explicit `{tileMatrixSetId}` binding, the
    /// way the real param route always provides one — [`path`] itself
    /// deliberately omits it to keep exercising `tms_of`'s unit-test
    /// fallback (WebMercatorQuad) alongside every pre-`#190` test.
    fn path_with_tms(
        cid: &str,
        tms_id: &str,
        z: u8,
        row: u32,
        col: &str,
    ) -> Path<HashMap<String, String>> {
        let Path(mut params) = path(cid, z, row, col);
        params.insert("tileMatrixSetId".to_string(), tms_id.to_string());
        Path(params)
    }

    fn styled_path(
        cid: &str,
        style_id: &str,
        z: u8,
        row: u32,
        col: &str,
    ) -> Path<HashMap<String, String>> {
        let Path(mut params) = path(cid, z, row, col);
        params.insert("styleId".to_string(), style_id.to_string());
        Path(params)
    }

    /// A MapLibre Style JSON document with a single `circle` layer painting
    /// the given MVT `source_layer` a flat opaque color — enough for the
    /// styled-lane tests to assert a specific pixel color.
    fn circle_style_doc(source_layer: &str, color_hex: &str) -> serde_json::Value {
        serde_json::json!({
            "version": 8,
            "layers": [{
                "id": format!("{source_layer}-circle"),
                "type": "circle",
                "source-layer": source_layer,
                "paint": { "circle-color": color_hex, "circle-radius": 4 },
            }],
        })
    }

    fn no_query() -> Query<HashMap<String, String>> {
        Query(HashMap::new())
    }

    fn query_f(value: &str) -> Query<HashMap<String, String>> {
        Query(HashMap::from([("f".to_string(), value.to_string())]))
    }

    fn headers_with_accept(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, HeaderValue::from_str(value).unwrap());
        headers
    }

    fn headers_with_bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    fn fake_mvt_bytes() -> Bytes {
        Bytes::from_static(b"fake-mvt-bytes")
    }

    /// A minimal, genuinely valid single-point MVT tile (geometry command
    /// stream `[9, 50, 34]` decodes to tile-local point `(25, 17)` — the same
    /// vector used in geozero's own published test suite).
    fn valid_mvt_bytes() -> Bytes {
        let mut layer = tile::Layer {
            version: 2,
            name: "demo".to_string(),
            extent: Some(4096),
            ..Default::default()
        };
        let mut feature = tile::Feature {
            geometry: vec![9, 50, 34],
            ..Default::default()
        };
        feature.set_type(tile::GeomType::Point);
        layer.features.push(feature);
        Bytes::from(
            Tile {
                layers: vec![layer],
            }
            .encode_to_vec(),
        )
    }

    const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    fn mvt_cmd(id: u32, count: u32) -> u32 {
        id | (count << 3)
    }
    fn mvt_zz(n: i32) -> u32 {
        ((n << 1) ^ (n >> 31)) as u32
    }

    /// A single closed quad (4 vertices) feature at `(cx, cy)` -- cheap to
    /// build, but real enough for the rasterizer to do a fill and a stroke
    /// per feature (see `dense_mvt_bytes`, which repeats this many times
    /// over).
    fn quad_feature(cx: i32, cy: i32, half: i32) -> tile::Feature {
        let geometry = vec![
            mvt_cmd(1, 1),
            mvt_zz(cx - half),
            mvt_zz(cy - half),
            mvt_cmd(2, 3),
            mvt_zz(2 * half),
            mvt_zz(0),
            mvt_zz(0),
            mvt_zz(2 * half),
            mvt_zz(-2 * half),
            mvt_zz(0),
            mvt_cmd(7, 1),
        ];
        let mut feature = tile::Feature {
            geometry,
            ..Default::default()
        };
        feature.set_type(tile::GeomType::Polygon);
        feature
    }

    /// A synthetic MVT tile with `n_features` small quad polygons scattered
    /// across the tile extent -- dense enough that rasterizing it takes
    /// several milliseconds (see the offload comment on `tile`'s Png branch
    /// above for the measured numbers this is based on). Used only to make
    /// the render step in
    /// [`png_render_does_not_starve_the_async_runtime_thread`] take long
    /// enough to observe, not to assert on rendered pixels.
    fn dense_mvt_bytes(n_features: usize) -> Bytes {
        let features: Vec<tile::Feature> = (0..n_features)
            .map(|i| {
                let cx = (i as i32 * 37) % 4096;
                let cy = (i as i32 * 53) % 4096;
                quad_feature(cx, cy, 12)
            })
            .collect();
        let layer = tile::Layer {
            version: 2,
            name: "dense".to_string(),
            extent: Some(4096),
            features,
            ..Default::default()
        };
        Bytes::from(
            Tile {
                layers: vec![layer],
            }
            .encode_to_vec(),
        )
    }

    /// A minimal, genuinely valid single-point MVT tile whose one layer is
    /// named `layer_name` — used by the styled-lane tests, which need a
    /// known `source-layer` name for the style document to target.
    fn valid_mvt_bytes_named(layer_name: &str) -> Bytes {
        let mut layer = tile::Layer {
            version: 2,
            name: layer_name.to_string(),
            extent: Some(4096),
            ..Default::default()
        };
        let mut feature = tile::Feature {
            geometry: vec![9, 50, 34],
            ..Default::default()
        };
        feature.set_type(tile::GeomType::Point);
        layer.features.push(feature);
        Bytes::from(
            Tile {
                layers: vec![layer],
            }
            .encode_to_vec(),
        )
    }

    #[tokio::test]
    async fn mvt_cache_hit_skips_the_driver() {
        let tiles = Arc::new(FakeTileSource::new());
        let ctx = test_context(Arc::clone(&tiles));
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        ctx.cache
            .insert(
                mvt_key(DEFAULT_TENANT, DEFAULT_CATALOG, "demo", coord, None),
                fake_mvt_bytes(),
            )
            .await;

        let response = tile(
            State(Arc::clone(&ctx)),
            path("demo", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            MVT_MIME
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, fake_mvt_bytes());
        assert_eq!(tiles.call_count(), 0);
    }

    #[tokio::test]
    async fn mvt_miss_populates_the_cache() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(fake_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let response = tile(
            State(Arc::clone(&ctx)),
            path("demo", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(tiles.call_count(), 1);
        let cached = ctx
            .cache
            .get(&mvt_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                None,
            ))
            .await;
        assert_eq!(cached, Some(fake_mvt_bytes()));
    }

    #[tokio::test]
    async fn concurrent_misses_on_one_tile_coalesce_to_a_single_driver_fetch() {
        let tiles = Arc::new(FakeTileSource::with_delay(
            std::time::Duration::from_millis(30),
        ));
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(fake_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let mut handles = Vec::new();
        for _ in 0..16 {
            let ctx = Arc::clone(&ctx);
            handles.push(tokio::spawn(async move {
                tile(
                    State(ctx),
                    path("demo", 1, 0, "0"),
                    no_query(),
                    HeaderMap::new(),
                )
                .await
            }));
        }

        for handle in handles {
            let response = handle.await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        assert_eq!(
            tiles.call_count(),
            1,
            "16 concurrent requests for one missing tile must hit the driver exactly once"
        );
    }

    #[tokio::test]
    async fn empty_mvt_tile_returns_204() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 3, x: 2, y: 2 };
        tiles.set(coord, None);
        let ctx = test_context(Arc::clone(&tiles));

        let response = tile(
            State(Arc::clone(&ctx)),
            path("demo", 3, 2, "2"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn collection_ineligible_for_tiles_is_neither_advertised_nor_fetched() {
        let tiles = Arc::new(FakeTileSource::without_collection_tile_capability());
        let ctx = test_context(Arc::clone(&tiles));
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let tileset_response = tileset(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            HeaderMap::new(),
            uri,
        )
        .await;
        assert_eq!(tileset_response.status(), StatusCode::NOT_FOUND);

        let tile_response = tile(
            State(ctx),
            path("demo", 1, 0, "0.mvt"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(tile_response.status(), StatusCode::NOT_FOUND);
        assert_eq!(tiles.call_count(), 0);
    }

    #[tokio::test]
    async fn png_request_populates_the_mvt_cache_first() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let response = tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.png"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            PNG_MIME
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[0..8], &PNG_MAGIC);

        let mvt_cached = ctx
            .cache
            .get(&mvt_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                None,
            ))
            .await;
        assert!(
            mvt_cached.is_some_and(|bytes| !bytes.is_empty()),
            "the MVT entry must be populated as a side effect of the PNG-first lane"
        );
        assert_eq!(tiles.call_count(), 1, "driver hit exactly once (MVT probe)");
    }

    #[tokio::test]
    async fn second_png_request_is_served_from_cache_without_rerendering() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let _first = tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.png"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        let second = tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.png"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(
            tiles.call_count(),
            1,
            "second request must not touch the driver again"
        );
    }

    /// The regression #23 fixes: before routing the Png lane through
    /// `get_or_populate`, N concurrent misses on the same not-yet-rendered
    /// PNG each rasterized independently even though the underlying MVT
    /// fetch was already coalesced (#3) — `tiles.call_count() == 1` alone
    /// would not have caught that, since MVT-level coalescing already
    /// guaranteed it regardless of the Png lane's own cache usage. Wrapping
    /// the cache in `CountingCache` observes the `populate` closure itself
    /// (mvt-fetch + rasterize together) directly, which is what changed.
    #[tokio::test]
    async fn concurrent_png_misses_on_one_tile_coalesce_to_a_single_rasterize() {
        let tiles = Arc::new(FakeTileSource::with_delay(
            std::time::Duration::from_millis(30),
        ));
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let (counting_cache, populate_calls) = CountingCache::new(
            Arc::new(MokaTileCache::with_byte_budget(10_000_000)),
            Encoding::Png,
        );
        let ctx = test_context_with_cache(Arc::clone(&tiles), HashMap::new(), counting_cache);

        let mut handles = Vec::new();
        for _ in 0..16 {
            let ctx = Arc::clone(&ctx);
            handles.push(tokio::spawn(async move {
                let response = tile(
                    State(ctx),
                    path("demo", 2, 1, "1.png"),
                    no_query(),
                    HeaderMap::new(),
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()
            }));
        }

        let mut bodies = Vec::new();
        for handle in handles {
            bodies.push(handle.await.unwrap());
        }

        let first = &bodies[0];
        assert!(
            bodies.iter().all(|body| body == first),
            "all 16 concurrent PNG requests for the same tile must return identical bytes"
        );
        assert_eq!(
            populate_calls.load(Ordering::SeqCst),
            1,
            "16 concurrent misses on one PNG tile must rasterize exactly once"
        );
    }

    /// Regression proof for the #29 offload decision. On a single-threaded
    /// (`current_thread`) runtime there is exactly one OS thread available to
    /// poll async tasks; a synchronous, in-line rasterize would occupy that
    /// thread for its entire duration with nowhere else for any other task
    /// to run in the meantime. A concurrently spawned task that increments a
    /// counter and immediately yields, in a tight loop, can only rack up
    /// meaningful progress *during* the render call if the actual CPU-bound
    /// rasterize work has moved off this runtime's thread onto the blocking
    /// pool (`tokio::task::spawn_blocking`) -- a near-zero count here would
    /// mean that offload silently regressed back to inline execution.
    #[tokio::test(flavor = "current_thread")]
    async fn png_render_does_not_starve_the_async_runtime_thread() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(dense_mvt_bytes(600)));
        let ctx = test_context(Arc::clone(&tiles));

        let progress = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let progress_task = {
            let progress = Arc::clone(&progress);
            let stop = Arc::clone(&stop);
            tokio::spawn(async move {
                while !stop.load(Ordering::Relaxed) {
                    progress.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            })
        };
        // Let the progress task actually start before the render begins, so
        // its startup scheduling isn't what the assertion below is counting.
        tokio::task::yield_now().await;

        let response = tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.png"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        stop.store(true, Ordering::Relaxed);
        progress_task.await.unwrap();

        let yields = progress.load(Ordering::Relaxed);
        assert!(
            yields > 10,
            "the progress task made too little headway while the render was \
             in flight ({yields} yields) -- rasterize looks like it ran \
             in-line on this single-threaded runtime instead of on the \
             blocking pool"
        );
    }

    #[tokio::test]
    async fn zoom_beyond_collection_maxzoom_is_not_found() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = tile(
            State(ctx),
            path("demo", 9, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn xy_beyond_matrix_size_is_bad_request() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        // matrix side at z=2 is 4; tileRow=10 is out of range.
        let response = tile(
            State(ctx),
            path("demo", 2, 10, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_collection_is_not_found() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = tile(
            State(ctx),
            path("missing", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn tile_resolves_the_tenant_from_the_path_when_present() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(TileCoord { z: 1, x: 0, y: 0 }, Some(fake_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        // "demo" is only registered under the "public" tenant; a mismatched
        // tenant in the path must not leak into another tenant's collection.
        let wrong_tenant = tile(
            State(Arc::clone(&ctx)),
            path_with_tenant("other-tenant", "demo", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(wrong_tenant.status(), StatusCode::NOT_FOUND);

        let matching_tenant = tile(
            State(ctx),
            path_with_tenant(DEFAULT_TENANT, "demo", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(matching_tenant.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn missing_accept_header_defaults_to_mvt() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(TileCoord { z: 1, x: 0, y: 0 }, Some(fake_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            MVT_MIME
        );
    }

    #[tokio::test]
    async fn accept_header_selects_png_when_only_png_is_offered() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(TileCoord { z: 1, x: 0, y: 0 }, Some(valid_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_accept(PNG_MIME),
        )
        .await;

        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            PNG_MIME
        );
    }

    #[tokio::test]
    async fn suffix_takes_precedence_over_accept_header() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(TileCoord { z: 1, x: 0, y: 0 }, Some(fake_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        // Accept asks for PNG only, but the `.mvt` suffix must win.
        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0.mvt"),
            no_query(),
            headers_with_accept(PNG_MIME),
        )
        .await;

        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            MVT_MIME
        );
    }

    #[tokio::test]
    async fn query_f_png_selects_png_without_a_suffix_or_accept_header() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(TileCoord { z: 1, x: 0, y: 0 }, Some(valid_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0"),
            query_f("png"),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            PNG_MIME
        );
    }

    #[tokio::test]
    async fn query_f_mvt_selects_mvt_over_a_png_only_accept_header() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(TileCoord { z: 1, x: 0, y: 0 }, Some(fake_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0"),
            query_f("mvt"),
            headers_with_accept(PNG_MIME),
        )
        .await;

        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            MVT_MIME
        );
    }

    #[tokio::test]
    async fn suffix_takes_precedence_over_query_f() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(TileCoord { z: 1, x: 0, y: 0 }, Some(fake_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0.mvt"),
            query_f("png"),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            MVT_MIME
        );
    }

    #[tokio::test]
    async fn tileset_metadata_reports_configured_zoom_range() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["tileMatrixSetId"], "WebMercatorQuad");
        assert_eq!(json["tileMatrixSetLimits"].as_array().unwrap().len(), 6); // z0..=5
    }

    /// `#49`: every resolved `TileSource` in this workspace can serve both
    /// encodings via the same content-negotiated `/{tileMatrix}/{tileRow}/
    /// {tileCol}` route (`tile`'s own two-branch format negotiation) — the
    /// TileSet resource must say so up front, in the OGC-defined
    /// `mediaTypes` field, so a client never has to probe.
    #[tokio::test]
    async fn tileset_advertises_both_vector_and_raster_media_types() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let media_types: Vec<&str> = json["mediaTypes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(media_types, vec![MVT_MIME, PNG_MIME]);
    }

    /// `#85`: the TileSet resource's `layers[]` entry lists exactly the
    /// resolved `tile_properties` allowlist, so a client can discover which
    /// properties a style can draw on without probing an actual tile.
    #[tokio::test]
    async fn tileset_lists_exactly_the_projected_properties() {
        let ctx = test_context_with_config(
            Arc::new(FakeTileSource::new()),
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
    settings:
      tile_properties: [name, pop]
"#,
        );
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let properties: Vec<&str> = json["layers"][0]["properties"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(properties, vec!["name", "pop"]);
    }

    /// No-regression guard (`#85`): a collection with no `tile_properties`
    /// declared anywhere in the settings chain gets byte-for-byte the
    /// pre-`#85` layer object — no `properties` key at all, not an empty
    /// array.
    #[tokio::test]
    async fn tileset_omits_the_properties_key_when_no_allowlist_is_declared() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["layers"][0].get("properties").is_none());
    }

    #[tokio::test]
    async fn tileset_advertises_a_typed_template_for_each_tile_format() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let item_links: Vec<&serde_json::Value> = json["links"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|link| link["rel"] == "item")
            .collect();

        assert_eq!(item_links.len(), 2);
        assert_eq!(item_links[0]["type"], MVT_MIME);
        assert_eq!(item_links[1]["type"], PNG_MIME);
        assert_eq!(item_links[0]["templated"], true);
        assert_eq!(item_links[1]["templated"], true);
        assert!(item_links[0]["href"]
            .as_str()
            .unwrap()
            .ends_with("/{tileMatrix}/{tileRow}/{tileCol}.mvt"));
        assert!(item_links[1]["href"]
            .as_str()
            .unwrap()
            .ends_with("/{tileMatrix}/{tileRow}/{tileCol}.png"));
    }

    #[tokio::test]
    async fn configured_public_base_is_used_for_tileset_links_and_templates() {
        let ctx = test_context_with_config(
            Arc::new(FakeTileSource::new()),
            r#"
server: { public_base_url: "https://maps.example.test/tellurion/" }
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
"#,
        );
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let hrefs: Vec<&str> = json["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| link["href"].as_str().unwrap())
            .collect();

        assert_eq!(
            hrefs,
            vec![
                "https://maps.example.test/tellurion/collections/demo/tiles/WebMercatorQuad",
                "https://maps.example.test/tellurion/tileMatrixSets/WebMercatorQuad",
                "https://maps.example.test/tellurion/collections/demo/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.mvt",
                "https://maps.example.test/tellurion/collections/demo/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.png",
            ]
        );
    }

    /// `#37`: a raster-only collection's tileset body must describe raster
    /// capabilities, not the vector defaults — PNG is the only media type,
    /// `layers` stays empty (a decoded pixel window has no source-layer
    /// concept), and `dataType` says `"map"`, never `"vector"`.
    #[tokio::test]
    async fn tileset_for_a_raster_only_collection_reports_png_only_and_no_vector_layers() {
        let ctx = test_context_raster();
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["dataType"], "map");
        let media_types: Vec<&str> = json["mediaTypes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(media_types, vec![PNG_MIME]);
        assert!(
            json["layers"].as_array().unwrap().is_empty(),
            "a raster collection must advertise no vector layers"
        );

        let item_links: Vec<&serde_json::Value> = json["links"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|link| link["rel"] == "item")
            .collect();
        assert_eq!(
            item_links.len(),
            1,
            "only a PNG item link, never an MVT one"
        );
        assert_eq!(item_links[0]["type"], PNG_MIME);
    }

    #[tokio::test]
    async fn tileset_for_a_raster_only_collection_still_reports_the_configured_zoom_range() {
        let ctx = test_context_raster();
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["tileMatrixSetId"], "WebMercatorQuad");
        assert_eq!(json["tileMatrixSetLimits"].as_array().unwrap().len(), 6); // z0..=5
    }

    /// `#37` follow-up: the list endpoint (`.../collections/{cid}/tiles`)
    /// must keep reporting `"vector"` for a collection with a real
    /// `TileSource` — unchanged from before [`resolve_tileset`] existed.
    #[tokio::test]
    async fn tileset_list_for_a_vector_collection_still_reports_data_type_vector() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let uri = OriginalUri(axum::http::Uri::from_static("/collections/demo/tiles"));
        let response = tileset_list(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let tilesets = json["tilesets"].as_array().unwrap();
        assert_eq!(tilesets.len(), 1);
        assert_eq!(tilesets[0]["dataType"], "vector");
    }

    /// `#37` follow-up: the list endpoint must tell the same truth the
    /// single-tileset endpoint already tells (`tileset_for_a_raster_only_
    /// collection_reports_png_only_and_no_vector_layers`) — a raster-only
    /// collection's entry says `dataType: "map"`, not the hardcoded
    /// `"vector"` every collection's entry used to get regardless of what
    /// its tiles lane can actually serve.
    #[tokio::test]
    async fn tileset_list_for_a_raster_only_collection_reports_data_type_map() {
        let ctx = test_context_raster();
        let uri = OriginalUri(axum::http::Uri::from_static("/collections/demo/tiles"));
        let response = tileset_list(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let tilesets = json["tilesets"].as_array().unwrap();
        assert_eq!(tilesets.len(), 1);
        assert_eq!(tilesets[0]["dataType"], "map");
    }

    /// `#49`: a driver with no cheaper answer (the default `TileSource::
    /// vector_layers` impl `FakeTileSource` never overrides) falls back to
    /// the collection's external id — never a hardcoded guess, and never the
    /// internal id.
    #[tokio::test]
    async fn tileset_falls_back_to_the_external_id_as_the_layer_name_when_the_driver_reports_none()
    {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json["layers"],
            serde_json::json!([{ "id": "demo", "dataType": "vector" }])
        );
    }

    /// `#49` acceptance: a collection whose `external_id` genuinely differs
    /// from its internal `id` must advertise the EXTERNAL id as the layer
    /// name — the one name a client can actually derive from the API.
    #[tokio::test]
    async fn tileset_layer_name_uses_the_external_id_not_the_internal_id_when_they_differ() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: internal-alias-marker
    external_id: public-demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            tiles: Arc::new(FakeTileSource::new()),
        }));
        let router = Router::build(&config, &registry).unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FakeStyleStore {
            styles: HashMap::new(),
        });
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));

        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/public-demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("public-demo"), HeaderMap::new(), uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json["layers"],
            serde_json::json!([{ "id": "public-demo", "dataType": "vector" }]),
            "the internal id must never appear as a layer name: {json}"
        );
    }

    /// A `TileSource` whose `vector_layers` answer is configurable — proves
    /// the tileset handler surfaces every real name a driver reports,
    /// plural, rather than assuming a collection ever carries exactly one
    /// (`#49`, the multi-layer PMTiles archive acceptance case).
    struct MultiLayerTileSource {
        layers: Vec<String>,
    }

    #[async_trait::async_trait]
    impl TileSource for MultiLayerTileSource {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&tellurion_core::Filter>,
        ) -> tellurion_core::Result<Option<Bytes>> {
            Ok(None)
        }

        async fn vector_layers(
            &self,
            _collection: &CollectionDecl,
        ) -> tellurion_core::Result<Option<Vec<String>>> {
            Ok(Some(self.layers.clone()))
        }
    }

    struct MultiLayerDriver {
        tiles: Arc<MultiLayerTileSource>,
    }

    impl StorageDriver for MultiLayerDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::clone(&self.tiles) as Arc<dyn TileSource>)
        }
    }

    struct MultiLayerFactory {
        tiles: Arc<MultiLayerTileSource>,
    }

    impl DriverFactory for MultiLayerFactory {
        fn name(&self) -> &str {
            "fake"
        }

        fn build(&self, _decl: &StorageDecl) -> tellurion_core::Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(MultiLayerDriver {
                tiles: Arc::clone(&self.tiles),
            }))
        }
    }

    /// A collection whose driver reports `layers` as its real MVT layer
    /// names, with `styles` in the registry — the fixture both the layer-name
    /// test and the `#245` applicability tests below share, so applicability
    /// is always checked against the SAME reported names the resource
    /// advertises.
    fn multi_layer_ctx(
        layers: &[&str],
        styles: HashMap<String, serde_json::Value>,
    ) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let tiles = Arc::new(MultiLayerTileSource {
            layers: layers.iter().map(|l| l.to_string()).collect(),
        });
        let mut registry = Registry::new();
        registry.register(Arc::new(MultiLayerFactory { tiles }));
        let router = Router::build(&config, &registry).unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FakeStyleStore { styles });
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ))
    }

    /// The TileSet body for `ctx`'s single `demo` collection.
    async fn tileset_json(ctx: Arc<AppContext>) -> serde_json::Value {
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// Titles of the `map`-rel links a TileSet body advertises — one per
    /// style the resource says this collection can be rendered with.
    fn map_link_titles(json: &serde_json::Value) -> Vec<&str> {
        json["links"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|link| link["rel"] == MAP_REL)
            .map(|link| link["title"].as_str().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn tileset_reports_every_real_layer_name_a_driver_advertises() {
        let json = tileset_json(multi_layer_ctx(
            &["world", "quadrant", "leaf"],
            HashMap::new(),
        ))
        .await;
        let layer_ids: Vec<&str> = json["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|layer| layer["id"].as_str().unwrap())
            .collect();
        assert_eq!(layer_ids, vec!["world", "quadrant", "leaf"]);
        assert!(json["layers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|layer| layer["dataType"] == "vector"));
    }

    /// `#49`: no registered style means no `map`-rel links — the tileset
    /// resource must not fabricate a styled-map option that doesn't exist.
    #[tokio::test]
    async fn tileset_has_no_map_links_when_no_styles_are_registered() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["links"]
            .as_array()
            .unwrap()
            .iter()
            .all(|link| link["rel"] != MAP_REL));
    }

    /// `#49`: one `map`-rel link per registered style, each pointing at the
    /// exact templated href that style's own styled-tile route serves.
    #[tokio::test]
    async fn tileset_advertises_a_map_link_per_registered_style() {
        let mut styles = HashMap::new();
        styles.insert("basic".to_string(), circle_style_doc("demo", "#ff0000"));
        styles.insert("dark".to_string(), circle_style_doc("demo", "#00ff00"));
        let ctx = test_context_with_styles(Arc::new(FakeTileSource::new()), styles);
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WebMercatorQuad",
        ));
        let response = tileset(State(ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        let map_links: Vec<&serde_json::Value> = json["links"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|link| link["rel"] == MAP_REL)
            .collect();
        let titles: Vec<&str> = map_links
            .iter()
            .map(|link| link["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, vec!["basic", "dark"]);
        assert_eq!(
            map_links[0]["href"],
            "/collections/demo/styles/basic/map/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}"
        );
        assert_eq!(map_links[0]["type"], PNG_MIME);
        assert_eq!(map_links[0]["templated"], true);
    }

    // -- styled-map link applicability (`#245`) ---------------------------
    //
    // `#49` advertised one `map` link per *registered* style. The registry is
    // global (`tellurion-styles`' own doc) but a style document is not: a
    // MapLibre style paints per `source-layer`, so a style naming none of a
    // collection's MVT layers renders a blank tile. `#220` closed exactly
    // this on the link-contributor side; these are the same rule on the
    // TileSet resource, through the same shared predicate.

    /// A style targeting some other collection's source layer paints nothing
    /// here, so the resource must not offer it. Proven against the driver's
    /// REAL reported layer names (not the collection's external id), which is
    /// what makes this the same set the resource's own `layers[]` advertises.
    #[tokio::test]
    async fn tileset_does_not_advertise_a_style_that_paints_none_of_its_layers() {
        let styles = HashMap::from([
            ("mine".to_string(), circle_style_doc("quadrant", "#ff0000")),
            (
                "somebody-elses".to_string(),
                circle_style_doc("another-collections-layer", "#00ff00"),
            ),
        ]);
        let json = tileset_json(multi_layer_ctx(&["world", "quadrant", "leaf"], styles)).await;

        assert_eq!(
            map_link_titles(&json),
            vec!["mine"],
            "only a style that paints one of this tileset's own layers may be advertised: {json}"
        );
        // And the advertised layer names really are the set applicability was
        // checked against — the two can never be computed from different
        // answers, which is the drift `advertised_vector_layers` exists to
        // prevent.
        let layer_ids: Vec<&str> = json["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|layer| layer["id"].as_str().unwrap())
            .collect();
        assert_eq!(layer_ids, vec!["world", "quadrant", "leaf"]);
    }

    /// A background-only style names no `source-layer` at all, so it paints
    /// nothing on any collection — the empty intersection, not a special
    /// case.
    #[tokio::test]
    async fn tileset_does_not_advertise_a_style_with_no_source_layer_at_all() {
        let styles = HashMap::from([(
            "bg-only".to_string(),
            serde_json::json!({
                "version": 8,
                "layers": [ { "id": "bg", "type": "background" } ]
            }),
        )]);
        let json = tileset_json(multi_layer_ctx(&["world"], styles)).await;
        assert!(
            map_link_titles(&json).is_empty(),
            "a style with nothing to paint is a link to a blank tile: {json}"
        );
    }

    /// The fallback path: a driver that reports no layer names at all is
    /// advertised under `external_id()` (`TileSource::vector_layers`'s own
    /// documented fallback), and applicability is checked against exactly
    /// that name — so the single-layer PostGIS/GeoPackage shape every
    /// existing deployment runs keeps advertising the styles it always did.
    #[tokio::test]
    async fn a_style_targeting_the_external_id_is_still_advertised_without_driver_layer_metadata() {
        let mut styles = HashMap::new();
        styles.insert("basic".to_string(), circle_style_doc("demo", "#ff0000"));
        styles.insert(
            "elsewhere".to_string(),
            circle_style_doc("not-this-collection", "#00ff00"),
        );
        // `FakeTileSource` never overrides `vector_layers`, so the resource
        // falls back to the collection's external id, `demo`.
        let ctx = test_context_with_styles(Arc::new(FakeTileSource::new()), styles);
        let json = tileset_json(ctx).await;
        assert_eq!(json["layers"][0]["id"], "demo");
        assert_eq!(map_link_titles(&json), vec!["basic"]);
    }

    fn tms_path(id: &str) -> Path<HashMap<String, String>> {
        Path(HashMap::from([(
            "tileMatrixSetId".to_string(),
            id.to_string(),
        )]))
    }

    #[tokio::test]
    async fn tile_matrix_set_definition_covers_the_full_zoom_range() {
        let response = tile_matrix_set_definition(tms_path("WebMercatorQuad")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], "WebMercatorQuad");
        assert_eq!(json["tileMatrices"].as_array().unwrap().len(), 25);
    }

    /// `#190`: `/tileMatrixSets/WorldCRS84Quad` serves the second registry
    /// entry's own full definition — 2x1 at level 0, CRS84 CRS — and an id
    /// outside the closed registry stays the 404 the unmatched literal
    /// route always produced.
    #[tokio::test]
    async fn tile_matrix_set_definition_serves_world_crs84_quad_and_404s_unknown_ids() {
        let response = tile_matrix_set_definition(tms_path("WorldCRS84Quad")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], "WorldCRS84Quad");
        assert_eq!(json["crs"], tilematrixset::WORLD_CRS84_QUAD_CRS);
        assert_eq!(json["tileMatrices"][0]["matrixWidth"], 2);
        assert_eq!(json["tileMatrices"][0]["matrixHeight"], 1);

        let unknown = tile_matrix_set_definition(tms_path("NoSuchQuad")).await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }

    /// `#190`: the listing advertises BOTH registry entries (strengthened
    /// from the pre-`#190` single-entry assertion), each with a resolvable
    /// self link.
    #[tokio::test]
    async fn tile_matrix_sets_listing_advertises_both_registered_sets() {
        let uri = OriginalUri(axum::http::Uri::from_static("/tileMatrixSets"));
        let response =
            tile_matrix_sets_list(State(test_context(Arc::new(FakeTileSource::new()))), uri).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let sets = json["tileMatrixSets"].as_array().unwrap();
        let ids: Vec<&str> = sets.iter().map(|s| s["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["WebMercatorQuad", "WorldCRS84Quad"]);
        assert_eq!(
            sets[1]["links"][0]["href"],
            "/tileMatrixSets/WorldCRS84Quad"
        );
        assert_eq!(sets[1]["uri"], tilematrixset::WORLD_CRS84_QUAD_URI);
    }

    #[tokio::test]
    async fn styled_lane_populates_the_mvt_cache_first() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(valid_mvt_bytes_named("demo")));
        let mut styles = HashMap::new();
        styles.insert("basic".to_string(), circle_style_doc("demo", "#ff0000"));
        let ctx = test_context_with_styles(Arc::clone(&tiles), styles);

        let response = styled_tile(
            State(Arc::clone(&ctx)),
            styled_path("demo", "basic", 2, 1, "1.png"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            PNG_MIME
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[0..8], &PNG_MAGIC);

        let mvt_cached = ctx
            .cache
            .get(&mvt_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                None,
            ))
            .await;
        assert!(
            mvt_cached.is_some_and(|bytes| !bytes.is_empty()),
            "the MVT entry must be populated as a side effect of the styled lane"
        );
        assert_eq!(tiles.call_count(), 1, "driver hit exactly once (MVT probe)");
    }

    #[tokio::test]
    async fn distinct_style_ids_produce_distinct_cache_entries_over_the_same_tile() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(valid_mvt_bytes_named("demo")));
        let mut styles = HashMap::new();
        styles.insert("basic".to_string(), circle_style_doc("demo", "#ff0000"));
        styles.insert("dark".to_string(), circle_style_doc("demo", "#00ff00"));
        let ctx = test_context_with_styles(Arc::clone(&tiles), styles);

        let basic = styled_tile(
            State(Arc::clone(&ctx)),
            styled_path("demo", "basic", 2, 1, "1.png"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        let dark = styled_tile(
            State(Arc::clone(&ctx)),
            styled_path("demo", "dark", 2, 1, "1.png"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(basic.status(), StatusCode::OK);
        assert_eq!(dark.status(), StatusCode::OK);
        let basic_body = axum::body::to_bytes(basic.into_body(), usize::MAX)
            .await
            .unwrap();
        let dark_body = axum::body::to_bytes(dark.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_ne!(
            basic_body, dark_body,
            "different styles over the same tile must render different pixels"
        );

        let basic_cached = ctx
            .cache
            .get(&styled_png_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                "basic",
                None,
            ))
            .await;
        let dark_cached = ctx
            .cache
            .get(&styled_png_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                "dark",
                None,
            ))
            .await;
        assert_eq!(basic_cached.as_deref(), Some(basic_body.as_ref()));
        assert_eq!(dark_cached.as_deref(), Some(dark_body.as_ref()));
        assert_eq!(
            tiles.call_count(),
            1,
            "both styles must share the one cached MVT probe"
        );
    }

    /// Renders the SAME MVT bytes under the SAME style at two zoom levels
    /// either side of a `step` breakpoint, and asserts the served PNGs
    /// differ (`#174`). This is the end of the wire that `resolve_layer_
    /// paints`' own unit tests cannot reach: it proves this handler passes
    /// the tile's real zoom in, rather than a constant that happens to make
    /// the resolver's tests pass.
    ///
    /// The paired control below is what makes it a real check rather than a
    /// coincidence — with the zoom expression replaced by a flat color, the
    /// same two zooms must render byte-identical output. Without that half,
    /// "the two bodies differ" could be true because the zoom leaked into
    /// the raster path somewhere it has no business being.
    #[tokio::test]
    async fn a_zoom_stepped_style_renders_differently_either_side_of_its_breakpoint() {
        // Both inside this fixture's own `tiles: { minzoom: 0, maxzoom: 5 }`
        // range, and either side of the style's breakpoint at zoom 4.
        let low = TileCoord { z: 2, x: 1, y: 1 };
        let high = TileCoord { z: 5, x: 1, y: 1 };

        // Same MVT bytes at both coordinates, so the ONLY input that
        // differs between the two renders below is the zoom.
        let render = |style: serde_json::Value, coord: TileCoord| async move {
            let tiles = Arc::new(FakeTileSource::new());
            tiles.set(coord, Some(valid_mvt_bytes_named("demo")));
            let mut styles = HashMap::new();
            styles.insert("stepped".to_string(), style);
            let ctx = test_context_with_styles(Arc::clone(&tiles), styles);
            let response = styled_tile(
                State(ctx),
                styled_path(
                    "demo",
                    "stepped",
                    coord.z,
                    coord.y,
                    &format!("{}.png", coord.x),
                ),
                no_query(),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
        };

        let stepped = serde_json::json!({
            "version": 8,
            "layers": [{
                "id": "demo-circle",
                "type": "circle",
                "source-layer": "demo",
                "paint": {
                    "circle-color": ["step", ["zoom"], "#ff0000", 4, "#00ff00"],
                    "circle-radius": 4,
                },
            }],
        });
        let below = render(stepped.clone(), low).await;
        let above = render(stepped, high).await;
        assert_ne!(
            below, above,
            "a `step` on zoom must change the served tile across its own breakpoint — \
             identical bytes here mean the tile's zoom never reached the style resolver"
        );

        let flat = circle_style_doc("demo", "#ff0000");
        let flat_below = render(flat.clone(), low).await;
        let flat_above = render(flat, high).await;
        assert_eq!(
            flat_below, flat_above,
            "a style with no zoom expression must render identically at every zoom"
        );
    }

    #[tokio::test]
    async fn unknown_style_id_is_a_404_problem_json() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(valid_mvt_bytes_named("demo")));
        let ctx = test_context_with_styles(Arc::clone(&tiles), HashMap::new());

        let response = styled_tile(
            State(ctx),
            styled_path("demo", "missing", 2, 1, "1.png"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "NotFound");
    }

    #[tokio::test]
    async fn styled_route_rejects_mvt_suffix() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(
            TileCoord { z: 2, x: 1, y: 1 },
            Some(valid_mvt_bytes_named("demo")),
        );
        let mut styles = HashMap::new();
        styles.insert("basic".to_string(), circle_style_doc("demo", "#ff0000"));
        let ctx = test_context_with_styles(Arc::clone(&tiles), styles);

        let response = styled_tile(
            State(ctx),
            styled_path("demo", "basic", 2, 1, "1.mvt"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn styled_route_rejects_f_mvt_query_param() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(
            TileCoord { z: 2, x: 1, y: 1 },
            Some(valid_mvt_bytes_named("demo")),
        );
        let mut styles = HashMap::new();
        styles.insert("basic".to_string(), circle_style_doc("demo", "#ff0000"));
        let ctx = test_context_with_styles(Arc::clone(&tiles), styles);

        let response = styled_tile(
            State(ctx),
            styled_path("demo", "basic", 2, 1, "1"),
            query_f("mvt"),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn styled_route_empty_mvt_tile_returns_204_and_caches_empty() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 3, x: 2, y: 2 };
        tiles.set(coord, None);
        let mut styles = HashMap::new();
        styles.insert("basic".to_string(), circle_style_doc("demo", "#ff0000"));
        let ctx = test_context_with_styles(Arc::clone(&tiles), styles);

        let response = styled_tile(
            State(Arc::clone(&ctx)),
            styled_path("demo", "basic", 3, 2, "2.png"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cached = ctx
            .cache
            .get(&styled_png_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                "basic",
                None,
            ))
            .await;
        assert_eq!(cached, Some(Bytes::new()));
    }

    // -- `#34` authorization policy layer ----------------------------------

    const AUTH_ONLY_TILES_CONFIG: &str = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
auth:
  bearer_tokens:
    - { token: member-token, tenants: [public] }
"#;

    const RBAC_TILES_CONFIG: &str = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
auth:
  bearer_tokens:
    - { token: no-role-token, tenants: [public] }
    - { token: reader-token, tenants: [public], roles: { public: [reader] } }
    - token: filtered-token
      tenants: [public]
      roles: { public: [filtered-reader] }
      claims: { org: acme }
policy:
  roles:
    - name: reader
      grants:
        - scope: { collections: [demo] }
          lanes: [tiles]
    - name: filtered-reader
      grants:
        - scope: { collections: [demo] }
          lanes: [tiles]
          filter: "org = {{claims.org}}"
"#;

    #[tokio::test]
    async fn no_credential_against_a_private_collection_is_401_when_auth_is_configured() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context_with_config(tiles, AUTH_ONLY_TILES_CONFIG);

        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_tenant_member_reads_tiles_unrestricted_with_no_policy_configured() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context_with_config(tiles, AUTH_ONLY_TILES_CONFIG);

        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_bearer("member-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tileset_discovery_is_also_gated_by_isolation() {
        let tiles = Arc::new(FakeTileSource::new());
        let ctx = test_context_with_config(tiles, AUTH_ONLY_TILES_CONFIG);

        let response = tileset(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            HeaderMap::new(),
            OriginalUri("/collections/demo/tiles/WebMercatorQuad".parse().unwrap()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let allowed = tileset(
            State(ctx),
            cid_path("demo"),
            headers_with_bearer("member-token"),
            OriginalUri("/collections/demo/tiles/WebMercatorQuad".parse().unwrap()),
        )
        .await;
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rbac_active_denies_a_member_with_no_matching_role() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context_with_config(tiles, RBAC_TILES_CONFIG);

        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_bearer("no-role-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn rbac_active_allows_an_unconditional_grant() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context_with_config(tiles, RBAC_TILES_CONFIG);

        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_bearer("reader-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// `FakeTileSource::new()` never overrides `filter_capable` (stays at
    /// the trait default, `false`) — a driver that can't compile a filter
    /// still denies a filtered-only grant outright rather than serving
    /// unfiltered (see `authorize_tiles`'s own doc). A filter-capable
    /// driver's own coverage lives in the `#34` tile-lane ABAC pushdown
    /// tests further down this file.
    #[tokio::test]
    async fn a_filtered_only_grant_denies_the_tiles_lane_rather_than_serving_unfiltered() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context_with_config(tiles, RBAC_TILES_CONFIG);

        let response = tile(
            State(ctx),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_bearer("filtered-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn styled_tile_is_also_gated_by_the_same_policy_check() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 3, x: 2, y: 2 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let mut styles = HashMap::new();
        styles.insert("basic".to_string(), circle_style_doc("demo", "#ff0000"));
        let config: AppConfig = serde_yaml::from_str(AUTH_ONLY_TILES_CONFIG).unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            tiles: Arc::clone(&tiles),
        }));
        let router = Router::build(&config, &registry).unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FakeStyleStore { styles });
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer = tellurion_core::build_authorizer(&config.auth)
            .expect("no bearer principal in this fixture reads a token_env");
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            authorizer,
            cache,
            style_store,
        ));

        let denied = styled_tile(
            State(Arc::clone(&ctx)),
            styled_path("demo", "basic", 3, 2, "2.png"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let allowed = styled_tile(
            State(ctx),
            styled_path("demo", "basic", 3, 2, "2.png"),
            no_query(),
            headers_with_bearer("member-token"),
        )
        .await;
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    // -- `#34` tile-lane ABAC pushdown + cache-key fingerprint --------------

    /// Three tokens, one filtered role: `acme-token-a`/`acme-token-b` both
    /// hold `filtered-reader` with the same `org` claim (so their grant
    /// substitutes to the identical effective filter), `globex-token` holds
    /// the same role with a different `org` claim (a different effective
    /// filter), and `unconditional-token` holds a role with no filter at
    /// all.
    const FILTERED_FINGERPRINT_TILES_CONFIG: &str = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
auth:
  bearer_tokens:
    - token: acme-token-a
      tenants: [public]
      roles: { public: [filtered-reader] }
      claims: { org: acme }
    - token: acme-token-b
      tenants: [public]
      roles: { public: [filtered-reader] }
      claims: { org: acme }
    - token: globex-token
      tenants: [public]
      roles: { public: [filtered-reader] }
      claims: { org: globex }
    - token: unconditional-token
      tenants: [public]
      roles: { public: [full-reader] }
policy:
  roles:
    - name: filtered-reader
      grants:
        - scope: { collections: [demo] }
          lanes: [tiles]
          filter: "org = {{claims.org}}"
    - name: full-reader
      grants:
        - scope: { collections: [demo] }
          lanes: [tiles]
"#;

    /// A driver that CAN compile a filter must actually be served a
    /// filtered-only grant, not denied — and the tile it renders must land
    /// in the cache under a key carrying that filter's own fingerprint.
    #[tokio::test]
    async fn a_filtered_grant_on_a_filter_capable_driver_is_served_and_fingerprints_the_cache_key()
    {
        let tiles = Arc::new(FakeTileSource::with_filter_capable());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context_with_config(Arc::clone(&tiles), FILTERED_FINGERPRINT_TILES_CONFIG);

        let response = tile(
            State(Arc::clone(&ctx)),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_bearer("acme-token-a"),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "a filter-capable driver must serve a filtered-only grant, not deny it"
        );

        let expected_fingerprint = tellurion_core::filter::parse_text("org = 'acme'")
            .unwrap()
            .fingerprint();
        assert_eq!(
            tiles.last_filter_fingerprint(),
            Some(expected_fingerprint),
            "the substituted grant filter must reach the driver's own mvt_tile call"
        );

        let cached = ctx
            .cache
            .get(&mvt_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                Some(expected_fingerprint),
            ))
            .await;
        assert!(
            cached.is_some(),
            "the tile must be cached under a key carrying the filter's own fingerprint"
        );
    }

    /// Two different subjects (different tokens, so different `Subject`s)
    /// whose grants resolve to the same effective filter text must share
    /// one cache entry — the second request never re-hits the driver.
    #[tokio::test]
    async fn two_subjects_with_the_same_effective_filter_share_one_cache_entry() {
        let tiles = Arc::new(FakeTileSource::with_filter_capable());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context_with_config(Arc::clone(&tiles), FILTERED_FINGERPRINT_TILES_CONFIG);

        let first = tile(
            State(Arc::clone(&ctx)),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_bearer("acme-token-a"),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(tiles.call_count(), 1);

        let second = tile(
            State(Arc::clone(&ctx)),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_bearer("acme-token-b"),
        )
        .await;
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(
            tiles.call_count(),
            1,
            "two subjects resolving to the same effective filter must share one cache entry"
        );
    }

    /// Two subjects whose grants resolve to DIFFERENT effective filters must
    /// never collide — each gets its own driver hit and its own cache entry.
    #[tokio::test]
    async fn subjects_with_different_effective_filters_get_different_cache_entries() {
        let tiles = Arc::new(FakeTileSource::with_filter_capable());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context_with_config(Arc::clone(&tiles), FILTERED_FINGERPRINT_TILES_CONFIG);

        let acme = tile(
            State(Arc::clone(&ctx)),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_bearer("acme-token-a"),
        )
        .await;
        assert_eq!(acme.status(), StatusCode::OK);
        assert_eq!(tiles.call_count(), 1);

        let globex = tile(
            State(Arc::clone(&ctx)),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_bearer("globex-token"),
        )
        .await;
        assert_eq!(globex.status(), StatusCode::OK);
        assert_eq!(
            tiles.call_count(),
            2,
            "a different effective filter must never reuse another subject's cache entry"
        );
    }

    /// An unconditional grant (unrestricted access) must produce the exact
    /// same `policy_fingerprint: None` cache key this lane always built,
    /// even against a filter-capable driver — public/unfiltered traffic
    /// never pays for a per-subject cache split it doesn't need.
    #[tokio::test]
    async fn an_unconditional_grant_keeps_the_pre_policy_cache_key_unchanged() {
        let tiles = Arc::new(FakeTileSource::with_filter_capable());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(valid_mvt_bytes()));
        let ctx = test_context_with_config(Arc::clone(&tiles), FILTERED_FINGERPRINT_TILES_CONFIG);

        let response = tile(
            State(Arc::clone(&ctx)),
            path("demo", 1, 0, "0"),
            no_query(),
            headers_with_bearer("unconditional-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            tiles.last_filter_fingerprint(),
            None,
            "an unconditional grant must reach the driver with no filter at all"
        );

        let cached = ctx
            .cache
            .get(&mvt_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                None,
            ))
            .await;
        assert!(
            cached.is_some(),
            "unrestricted access must still populate the byte-identical pre-`#34` cache key \
             (policy_fingerprint: None), so public/anonymous traffic keeps sharing entries"
        );
    }

    /// `#92`: two collections whose only difference is which colormap they
    /// resolve to must never collide in the tile cache — the fingerprint is
    /// what a config reload relies on to keep a stale colormap's cached
    /// bytes from answering under the same key (see `raster_png_key`'s own
    /// doc).
    #[test]
    fn raster_png_key_folds_the_colormap_fingerprint_into_the_encoding() {
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        let none = raster_png_key("public", "default", "demo", coord, None, None);
        let fp_a = raster_png_key("public", "default", "demo", coord, Some(1), None);
        let fp_b = raster_png_key("public", "default", "demo", coord, Some(2), None);

        assert_eq!(none.encoding, Encoding::PngRaster(None));
        assert_ne!(
            none, fp_a,
            "an unconfigured colormap must not collide with a configured one"
        );
        assert_ne!(fp_a, fp_b, "two different colormaps must not collide");
    }

    // -- `#190` WorldCRS84Quad tile matrix set ------------------------------

    /// `#190` capability honesty: a driver that never overrides
    /// `supports_tile_matrix_set` (PMTiles/GeoPackage shape) earns a clean,
    /// by-name `CapabilityUnsupported` refusal at resolve time — the driver
    /// is never called, and the body names the grid.
    #[tokio::test]
    async fn world_crs84_tile_on_a_non_capable_driver_is_refused_by_name() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(TileCoord { z: 1, x: 0, y: 0 }, Some(valid_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let response = tile(
            State(ctx),
            path_with_tms("demo", "WorldCRS84Quad", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            PROBLEM_JSON
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // `Problem::new` keeps the HTTP reason phrase as `title`; the
        // machine-readable refusal name travels in its `code` field.
        assert_eq!(json["code"], "CapabilityUnsupported");
        let detail = json["detail"].as_str().unwrap();
        assert!(
            detail.contains("WorldCRS84Quad") && detail.contains("demo"),
            "the refusal must name the grid and the collection: {detail}"
        );
        assert_eq!(tiles.call_count(), 0, "the driver must never be reached");
    }

    /// `#190`: an id outside the closed registry is a plain 404 — exactly
    /// what the pre-`#190` unmatched literal route produced.
    #[tokio::test]
    async fn unknown_tile_matrix_set_id_is_not_found() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = tile(
            State(ctx),
            path_with_tms("demo", "NoSuchQuad", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// `#190`: on a tms-capable (PostGIS-shaped) driver the WorldCRS84Quad
    /// lane serves MVT — the grid reaches the driver, and the wider CRS84
    /// column range (level 1 has FOUR columns where mercator has two) is
    /// accepted by the coordinate gate.
    #[tokio::test]
    async fn world_crs84_tile_on_a_capable_driver_is_served_with_the_grid() {
        let tiles = Arc::new(FakeTileSource::with_tms_capable());
        // Column 3 at level 1: valid in WorldCRS84Quad (matrixWidth 4),
        // out of range in WebMercatorQuad (matrixWidth 2).
        let coord = TileCoord { z: 1, x: 3, y: 0 };
        tiles.set(coord, Some(fake_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let response = tile(
            State(ctx),
            path_with_tms("demo", "WorldCRS84Quad", 1, 0, "3"),
            no_query(),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(tiles.last_tms(), Some(TileMatrixSet::WorldCrs84Quad));
        assert_eq!(tiles.call_count(), 1);
    }

    /// `#190`: CRS84 indices past the grid's own (non-square) bounds are a
    /// 400 — row bound `2^z`, column bound `2^(z+1)`.
    #[tokio::test]
    async fn world_crs84_indices_beyond_the_grid_are_bad_request() {
        let ctx = test_context(Arc::new(FakeTileSource::with_tms_capable()));
        let row_out = tile(
            State(Arc::clone(&ctx)),
            path_with_tms("demo", "WorldCRS84Quad", 1, 2, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(row_out.status(), StatusCode::BAD_REQUEST);

        let col_out = tile(
            State(ctx),
            path_with_tms("demo", "WorldCRS84Quad", 1, 0, "4"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(col_out.status(), StatusCode::BAD_REQUEST);
    }

    /// `#190` cache identity: the same `z`/`x`/`y` requested on both grids
    /// hits the driver twice and lands in two distinct cache entries — the
    /// grid partitions the cache exactly like `encoding` does.
    #[tokio::test]
    async fn the_two_grids_never_share_a_cache_entry_at_the_same_coordinate() {
        let tiles = Arc::new(FakeTileSource::with_tms_capable());
        let coord = TileCoord { z: 1, x: 0, y: 0 };
        tiles.set(coord, Some(fake_mvt_bytes()));
        let ctx = test_context(Arc::clone(&tiles));

        let mercator = tile(
            State(Arc::clone(&ctx)),
            path_with_tms("demo", "WebMercatorQuad", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(mercator.status(), StatusCode::OK);
        assert_eq!(tiles.call_count(), 1);

        let crs84 = tile(
            State(Arc::clone(&ctx)),
            path_with_tms("demo", "WorldCRS84Quad", 1, 0, "0"),
            no_query(),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(crs84.status(), StatusCode::OK);
        assert_eq!(
            tiles.call_count(),
            2,
            "the CRS84 request must miss the mercator entry and hit the driver again"
        );

        let mercator_key = mvt_key(DEFAULT_TENANT, DEFAULT_CATALOG, "demo", coord, None);
        let crs84_key = TileKey {
            tms: TileMatrixSet::WorldCrs84Quad,
            ..mercator_key.clone()
        };
        assert!(ctx.cache.get(&mercator_key).await.is_some());
        assert!(ctx.cache.get(&crs84_key).await.is_some());
    }

    /// `#190`: the per-collection tilesets listing advertises exactly what
    /// the resolved source can serve — both grids for a tms-capable
    /// (PostGIS-shaped) source, WebMercatorQuad alone for everything else.
    #[tokio::test]
    async fn tileset_list_advertises_exactly_the_grids_the_source_serves() {
        let uri = OriginalUri(axum::http::Uri::from_static("/collections/demo/tiles"));

        let capable_ctx = test_context(Arc::new(FakeTileSource::with_tms_capable()));
        let response = tileset_list(
            State(capable_ctx),
            cid_path("demo"),
            HeaderMap::new(),
            uri.clone(),
        )
        .await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let uris: Vec<&str> = json["tilesets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["tileMatrixSetURI"].as_str().unwrap())
            .collect();
        assert_eq!(
            uris,
            vec![
                tilematrixset::WEB_MERCATOR_QUAD_URI,
                tilematrixset::WORLD_CRS84_QUAD_URI
            ]
        );

        let native_ctx = test_context(Arc::new(FakeTileSource::new()));
        let response =
            tileset_list(State(native_ctx), cid_path("demo"), HeaderMap::new(), uri).await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let uris: Vec<&str> = json["tilesets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["tileMatrixSetURI"].as_str().unwrap())
            .collect();
        assert_eq!(uris, vec![tilematrixset::WEB_MERCATOR_QUAD_URI]);
    }

    /// `#190`: the WorldCRS84Quad tileset body reports the grid's own
    /// non-square index bounds, and a non-capable source's tileset request
    /// for that grid gets the same by-name refusal the tile lane gives.
    #[tokio::test]
    async fn world_crs84_tileset_reports_grid_bounds_and_refuses_non_capable_sources() {
        let mut params = HashMap::from([("cid".to_string(), "demo".to_string())]);
        params.insert("tileMatrixSetId".to_string(), "WorldCRS84Quad".to_string());
        let uri = OriginalUri(axum::http::Uri::from_static(
            "/collections/demo/tiles/WorldCRS84Quad",
        ));

        let capable_ctx = test_context(Arc::new(FakeTileSource::with_tms_capable()));
        let response = tileset(
            State(capable_ctx),
            Path(params.clone()),
            HeaderMap::new(),
            uri.clone(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["tileMatrixSetId"], "WorldCRS84Quad");
        assert_eq!(json["crs"], tilematrixset::WORLD_CRS84_QUAD_CRS);
        let level0 = &json["tileMatrixSetLimits"][0];
        assert_eq!(level0["maxTileCol"], 1, "two columns at level 0");
        assert_eq!(level0["maxTileRow"], 0, "one row at level 0");

        let native_ctx = test_context(Arc::new(FakeTileSource::new()));
        let refused = tileset(State(native_ctx), Path(params), HeaderMap::new(), uri).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    }
}
