//! 3D Tiles 1.1 delivery for 3D places: `tileset.json` discovery plus the
//! Glb tile lane. Driver-agnostic, mirroring `tellurion-tiles`' handler and
//! tenant/catalog-resolution conventions exactly (read `tellurion-tiles::handlers`
//! for the pattern this follows) — the only new work is the extra
//! `places3d` capability check and the extrude-to-glb step at the response
//! boundary, via `tellurion-render` only.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;

use tellurion_core::problem::{Problem, PROBLEM_JSON};
use tellurion_core::{
    mvt_key, AppContext, CollectionDecl, Credential, Encoding, Error, MvtFetch, Places3dConf,
    PopulateFuture, TileCoord, TileKey, TileSource,
};
use tellurion_render::{extrude_mvt_to_glb, volume_mesh_to_glb, ExtrudeParams};

use crate::conformance;

/// `tenant`/`catalog` path parameters carry EXTERNAL ids exactly as the
/// client typed them (`#39`) — every request runs under a
/// `/{tenant}/3dtiles/catalogs/{catalog}` mount; [`resolve_places3d`] turns
/// them (plus a collection's own external id) into the internal ids `Router`
/// and the tile cache key need. A handler that runs with no mount at all
/// (this crate's own unit tests) falls back to [`DEFAULT_TENANT`]/
/// [`DEFAULT_CATALOG`] — the same convention `tellurion-tiles` and
/// `tellurion-features` use.
pub const DEFAULT_TENANT: &str = "public";
pub const DEFAULT_CATALOG: &str = "default";

/// Duplicated from `tellurion-tiles::TILE_CACHE_CONTROL` (same value) rather
/// than imported, so this crate stays independent of its sibling protocol
/// crate.
pub const TILE_CACHE_CONTROL: &str = "public, max-age=86400";

const GLB_SUFFIX: &str = ".glb";
const SUBTREE_SUFFIX: &str = ".subtree";

/// Web Mercator whole-world half-extent in meters (EPSG:3857) — the same
/// constant OGC 17-083r4's WebMercatorQuad TileMatrixSet is built from.
/// Duplicated here rather than imported from `tellurion-tiles` for the same
/// crate-independence reason as [`TILE_CACHE_CONTROL`].
const WEB_MERCATOR_ORIGIN_M: f64 = 20_037_508.342_789_244;
const TILE_SIZE_PX: f64 = 256.0;

/// Lower vertical bound for the tileset's bounding region. There is no
/// per-collection height statistic in config (v0.2 adds no `extent` field to
/// `CollectionDecl`), so this isn't computed from any data — `extrude_mvt_to_glb`
/// never produces a negative Z (both `height` and `min_height` are clamped to
/// `[0, MAX_HEIGHT_METERS]` before `exaggeration`, which is itself required
/// to be positive), so 0 is always a safe lower bound, not a guess.
const WHOLE_WORLD_MIN_HEIGHT_M: f64 = 0.0;

/// Builds the 3D places route table. Mount under whatever prefix the server
/// chooses; paths here are relative to that mount point.
pub fn router() -> axum::Router<Arc<AppContext>> {
    axum::Router::new()
        .route("/collections/{cid}/3dtiles", get(tileset))
        .route(
            "/collections/{cid}/3dtiles/tiles/{tileMatrix}/{tileRow}/{tileCol}",
            get(glb_tile),
        )
        .route(
            "/collections/{cid}/3dtiles/subtrees/{tileMatrix}/{tileRow}/{tileCol}",
            get(subtree_file),
        )
}

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

/// A collection resolved through this request's `(tenant, catalog, cid)`
/// path segments (`#39`) — external ids resolved to internal ones via
/// `AppContext::current().resolver`, then handed to `Router`. `tenant_id`/
/// `catalog_id`/`collection_id` are internal and feed the tile cache key
/// (`mvt_key`/`glb_key`); everything else about the response (hrefs) is
/// built from the ORIGINAL external path segments the client typed, never
/// these. Requires the same capability `tellurion-tiles` requires (3D
/// places are built from MVT, so a driver that can't serve tiles can't serve
/// them either) *and* requires the collection to declare `places3d` config —
/// either failure is refused here, uniformly, before any handler-specific
/// logic runs.
struct ResolvedPlaces3d {
    tenant_id: String,
    catalog_id: String,
    collection_id: String,
    decl: CollectionDecl,
    source: Arc<dyn TileSource>,
    places3d: Places3dConf,
}

async fn resolve_places3d(
    ctx: &AppContext,
    params: &HashMap<String, String>,
    cid: &str,
) -> Option<ResolvedPlaces3d> {
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
    let places3d = decl.places3d.clone()?;
    Some(ResolvedPlaces3d {
        tenant_id,
        catalog_id,
        collection_id,
        decl,
        source,
        places3d,
    })
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

/// The `#34` policy checkpoint every handler in this crate calls right
/// after [`resolve_places3d`] succeeds — mirrors
/// `tellurion-tiles::handlers::authorize_tiles` exactly: `lane_supports_filter`
/// is the resolved `TileSource`'s own `filter_capable()`, so a filtered-only
/// grant is served (its filter pushed into the shared MVT-first fetch,
/// `fetch_mvt` below) when the resolved driver can compile one, and still
/// denied outright when it can't.
///
/// **Solid-geometry interaction (`#70`):** a driver that ALSO advertises
/// `VolumeSource` (`#15`, true solid geometry) bypasses the MVT fetch
/// entirely — so [`glb_tile`] probes for a volume source BEFORE calling this
/// checkpoint and, when one is present, passes THAT source's own
/// `filter_capable()` instead of the MVT `TileSource`'s. A `VolumeSource`
/// that compiles a filter (PostGIS, `sql::build_volume_plan`) is served a
/// filtered-only grant exactly like the MVT lane, its filter pushed into
/// `volume_tile` itself; one that stays at the trait default (`false`)
/// still denies a filtered-only grant outright (fail closed) rather than
/// serving unfiltered solids.
async fn authorize_places(
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
        lane: tellurion_core::PolicyLane::Places3d,
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
            tracing::error!(%error, "policy evaluation failed for a 3D places request");
            Err(problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                "an internal configuration error occurred",
            ))
        }
    }
}

fn to_extrude_params(places3d: &Places3dConf) -> ExtrudeParams {
    ExtrudeParams {
        height_property: places3d.height_property.clone(),
        min_height_property: places3d.min_height_property.clone(),
        default_height: places3d.default_height,
        exaggeration: places3d.exaggeration,
    }
}

/// Shared RFC 9457 problem-details body — same type `tellurion-features` and
/// `tellurion-tiles` serve for their own API errors.
fn problem_response(status: StatusCode, code: &str, detail: impl Into<String>) -> Response {
    let problem = Problem::new(status.as_u16(), code, detail);
    let mut response = (status, axum::Json(problem)).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
    response
}

/// Generic 404 detail for a missing/unresolvable path parameter or
/// collection — deliberately uninformative (same phrasing
/// `tellurion_core::Error::NotFound` uses) so a 404 never leaks whether the
/// collection is absent, mistyped, or simply doesn't declare 3D places.
const NOT_FOUND_DETAIL: &str = "the requested resource was not found";

/// 3D Tiles `geometricError` is the real-world size (in the tileset's
/// units — meters here) a tile's content resolves to; it plays the role
/// `cellSize` plays in a WebMercatorQuad `TileMatrix` (OGC 17-083r4
/// SS5.2.1), so it reuses that formula: halves every zoom, starting from
/// the zoom-0 whole-world cell. Monotonically decreasing in `zoom`.
fn geometric_error_at_zoom(zoom: u8) -> f64 {
    let matrix_side = (1u64 << zoom) as f64;
    (2.0 * WEB_MERCATOR_ORIGIN_M) / TILE_SIZE_PX / matrix_side
}

/// Builds the 3D Tiles 1.1 `tileset.json` body for `decl`/`places3d`. A
/// single implicit-quadtree root tile spans the collection's whole
/// configured zoom range. `content.uri` and `subtrees.uri` use the
/// spec-required `{level}`, `{x}`, `{y}` template tokens verbatim (3D Tiles
/// 1.1 Implicit Tiling requires exactly those names for a `QUADTREE`
/// subdivision scheme, substituted by a client matching that literal text)
/// — placed in `{level}/{y}/{x}` order so the row coordinate (`{y}`)
/// precedes the column coordinate (`{x}`) in the URI, the same
/// `{tileMatrix}/{tileRow}/{tileCol}` convention OGC API Tiles uses and this
/// crate's own axum route now follows. The token *names* stay spec-exact
/// rather than becoming `{tileRow}`/`{tileCol}` — a 3D Tiles client only
/// recognizes the former — but their *position* in the string is this
/// crate's own choice, unrelated to the axum route's own parameter names
/// (`glb_tile`/`subtree_file` parse whatever value a client substitutes
/// there positionally), so reordering the template to agree with the route
/// costs nothing and keeps one coordinate convention across the API.
///
/// The bounding region is always whole-world horizontally (`CollectionDecl`
/// carries no declared extent to narrow it to in this milestone) and
/// vertically bounded by `tellurion_render::MAX_HEIGHT_METERS *
/// places3d.exaggeration` — the exact ceiling `extrude_mvt_to_glb` can ever
/// produce (it clamps a feature's raw height to `MAX_HEIGHT_METERS` *before*
/// multiplying by `exaggeration`), so this declared region can never be
/// smaller than what the content lane actually emits.
///
/// `implicitTiling.subtrees.uri` is backed by [`subtree_file`]: because
/// `subtreeLevels == availableLevels` here, this collection's whole declared
/// zoom range fits inside exactly one subtree (the implicit root's own), so
/// there is exactly one valid subtree address — level 0, x 0, y 0 — and
/// [`subtree_file`] refuses any other coordinate. That one subtree asserts
/// `constant` availability (tiles and content both "might exist, go check")
/// rather than a computed per-tile bitstream; a genuinely empty tile is
/// still discovered precisely and lazily via the content endpoint's 204,
/// exactly like the MVT/PNG lanes already behave.
fn tileset_json(
    base_path: &str,
    decl: &CollectionDecl,
    places3d: &Places3dConf,
) -> serde_json::Value {
    let root_error = geometric_error_at_zoom(decl.tiles.minzoom);
    let levels = u64::from(decl.tiles.maxzoom - decl.tiles.minzoom) + 1;
    let max_height_m = tellurion_render::MAX_HEIGHT_METERS * places3d.exaggeration;

    serde_json::json!({
        "asset": { "version": "1.1" },
        "geometricError": root_error,
        "root": {
            "boundingVolume": {
                "region": [
                    -std::f64::consts::PI,
                    -std::f64::consts::FRAC_PI_2,
                    std::f64::consts::PI,
                    std::f64::consts::FRAC_PI_2,
                    WHOLE_WORLD_MIN_HEIGHT_M,
                    max_height_m,
                ],
            },
            "geometricError": root_error,
            "refine": "REPLACE",
            "content": {
                "uri": format!("{base_path}/tiles/{{level}}/{{y}}/{{x}}.glb"),
            },
            "implicitTiling": {
                "subdivisionScheme": "QUADTREE",
                "subtreeLevels": levels,
                "availableLevels": levels,
                "subtrees": {
                    "uri": format!("{base_path}/subtrees/{{level}}/{{y}}/{{x}}.subtree"),
                },
            },
        },
    })
}

/// This handler is mounted at `.../collections/{cid}/3dtiles` (see `router`
/// below) — `uri.path()` IS that self href, and the correct base for the
/// `content`/`subtrees` template URIs regardless of whether a tenant/catalog
/// prefix is mounted in front of it.
async fn tileset(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let Some(cid) = params.get("cid") else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    let Some(resolved) = resolve_places3d(&ctx, &params, cid).await else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    if let Err(response) = authorize_places(
        &ctx,
        &headers,
        &resolved.tenant_id,
        &resolved.catalog_id,
        &resolved.collection_id,
        resolved.source.filter_capable(),
    )
    .await
    {
        return response;
    }

    let base_path = ctx.current().config.server.public_href(uri.path());
    axum::Json(tileset_json(&base_path, &resolved.decl, &resolved.places3d)).into_response()
}

fn glb_key(
    tenant: &str,
    catalog: &str,
    collection: &str,
    coord: TileCoord,
    policy_fingerprint: Option<u64>,
) -> TileKey {
    TileKey {
        encoding: Encoding::Glb,
        ..mvt_key(tenant, catalog, collection, coord, policy_fingerprint)
    }
}

fn glb_response(bytes: Bytes) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, conformance::MEDIA_TYPE_GLB),
            (header::CACHE_CONTROL, TILE_CACHE_CONTROL),
        ],
        bytes,
    )
        .into_response()
}

async fn glb_tile(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(cid) = params.get("cid") else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    let Some(resolved) = resolve_places3d(&ctx, &params, cid).await else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    let ResolvedPlaces3d {
        tenant_id,
        catalog_id,
        collection_id,
        decl,
        source,
        places3d,
    } = resolved;
    // `#15`/`#70`: this collection's tiles-lane primary driver may advertise
    // true solid geometry instead of the footprint+height fallback below —
    // `Router::resolve_volume` already narrows that driver-wide answer
    // against this collection's own descriptor-derived geometry type, so a
    // footprint+height sibling sharing the same storage entry sees `None`
    // here and falls through to extrusion below. `Err` here can only mean
    // `(tenant_id, catalog_id, collection_id)` stopped resolving between
    // this call and `resolve_places3d`'s own lookup above, which never
    // happens within one request — treated the same as "no volume source"
    // rather than a second 404, since this is a plain capability probe, not
    // a resolve a request depends on succeeding.
    //
    // Probed BEFORE the policy checkpoint, deliberately: when solid geometry
    // would serve this tile, the checkpoint must gate on THAT source's own
    // `filter_capable()`, not the MVT `TileSource`'s — see
    // `authorize_places`'s own doc.
    let volume_source = ctx
        .current()
        .router
        .resolve_volume(&tenant_id, &catalog_id, &collection_id)
        .await
        .ok()
        .flatten();

    let lane_supports_filter = match &volume_source {
        Some(volume_source) => volume_source.filter_capable(),
        None => source.filter_capable(),
    };

    let policy_filter = match authorize_places(
        &ctx,
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        lane_supports_filter,
    )
    .await
    {
        Ok(filter) => filter,
        Err(response) => return response,
    };

    let Some(z_raw) = params.get("tileMatrix") else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    let z: u8 = match z_raw.parse() {
        Ok(v) => v,
        Err(_) => {
            return problem_response(StatusCode::BAD_REQUEST, "InvalidParameter", "invalid zoom")
        }
    };
    if z < decl.tiles.minzoom || z > decl.tiles.maxzoom {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    }

    let Some(row_raw) = params.get("tileRow") else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    let y: u32 = match row_raw.parse() {
        Ok(v) => v,
        Err(_) => {
            return problem_response(
                StatusCode::BAD_REQUEST,
                "InvalidParameter",
                "invalid tile row",
            )
        }
    };

    let Some(col_raw) = params.get("tileCol") else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    let Some(col_part) = col_raw.strip_suffix(GLB_SUFFIX) else {
        return problem_response(
            StatusCode::BAD_REQUEST,
            "InvalidParameter",
            "expected a '.glb' suffix",
        );
    };
    let x: u32 = match col_part.parse() {
        Ok(v) => v,
        Err(_) => {
            return problem_response(
                StatusCode::BAD_REQUEST,
                "InvalidParameter",
                "invalid tile column",
            )
        }
    };

    let matrix_side = 1u64 << z;
    if u64::from(x) >= matrix_side || u64::from(y) >= matrix_side {
        return problem_response(
            StatusCode::BAD_REQUEST,
            "InvalidParameter",
            "tile index out of range",
        );
    }

    let coord = TileCoord { z, x, y };
    let key = TileKey {
        // `#113`: same generation this coordinate's underlying Mvt fetch
        // (below) resolves — one bucket bump covers the Glb entry too.
        // `#190`: the places3d lane is WebMercatorQuad-only (its route
        // carries no tile-matrix-set segment), so the grid is fixed here
        // and in the nested `fetch_mvt` below.
        generation: ctx.tile_generation(
            &collection_id,
            tellurion_core::TileMatrixSet::WebMercatorQuad,
            coord,
        ),
        ..glb_key(
            &tenant_id,
            &catalog_id,
            &collection_id,
            coord,
            policy_filter
                .as_ref()
                .map(tellurion_core::Filter::fingerprint),
        )
    };

    // `#51`: resolved once, here — before `collection_id` moves into the
    // `populate` closure below — so this handler's own Glb-keyed cache entry
    // and the nested `fetch_mvt` sub-fetch's Mvt-keyed entry both get the
    // exact same TTL one `Router` snapshot produced, instead of each
    // independently resolving `AppContext::cache_ttl` and risking two
    // different answers if a config reload lands between them.
    let ttl = ctx.cache_ttl(&collection_id);

    // Mirrors the Png lane's shape in `tellurion-tiles::handlers::tile`:
    // fetch/render both happen inside one `populate` closure keyed by `key`,
    // so N concurrent misses on the same glb tile share one render instead
    // of each running it independently.
    let ctx_for_populate = Arc::clone(&ctx);
    let collection_id_for_cache = collection_id.clone();
    let filter = policy_filter.clone();
    let populate: PopulateFuture = Box::pin(async move {
        // True solid geometry, when this collection's driver has any: skips
        // the MVT fetch and the footprint+height extrusion below entirely.
        // `#70`: `filter` now reaches `volume_tile` the same way it reaches
        // `fetch_mvt` below — the policy checkpoint above gated on THIS
        // source's own `filter_capable()`, so a filtered-only grant already
        // got denied above whenever this source can't apply one; passing
        // `filter` through here on a filter-capable source is what turns
        // that grant into a served, correctly filtered mesh instead of a
        // fail-closed 403.
        if let Some(volume_source) = volume_source {
            return match volume_source
                .volume_tile(&decl, coord, filter.as_ref())
                .await
            {
                Ok(Some(mesh)) => Ok(Bytes::from(volume_mesh_to_glb(
                    &mesh.positions,
                    &mesh.indices,
                ))),
                Ok(None) => Ok(Bytes::new()),
                Err(error) => {
                    tracing::error!(%error, tenant = %tenant_id, catalog = %catalog_id, collection = %collection_id, z, x, y, "volume source failed to produce solid geometry for a 3D places tile");
                    Err(error)
                }
            };
        }

        let mvt_bytes = match ctx_for_populate
            .fetch_mvt(
                &tenant_id,
                &catalog_id,
                &collection_id,
                tellurion_core::TileMatrixSet::WebMercatorQuad,
                coord,
                &decl,
                &source,
                filter.as_ref(),
                ttl,
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

        let extrude_params = to_extrude_params(&places3d);
        // Ear-clip extrusion has no vertex cap and no spatial index (see
        // `tellurion-render::earclip`'s own doc comment: brute-force
        // candidate search, deliberately not a tuned earcut) — its cost
        // grows superlinearly with a single footprint's vertex count, so an
        // unsimplified large polygon (a coastline, an admin boundary, a
        // complex building footprint) can push one call well past a
        // millisecond, same territory as the raster lane's rasterize step
        // (see `tellurion-tiles::handlers::tile`'s offload comment for the
        // measured numbers). It runs on the blocking pool for the same
        // reason: `mvt_bytes`/`extrude_params` are owned values moved into
        // the closure rather than borrowed across the `.await`, so nothing
        // here holds a lock or pooled connection while it runs, and a
        // dropped `populate` future (timeout/shed) just lets the detached
        // blocking task finish and discards its result.
        let glb_bytes = tokio::task::spawn_blocking(move || {
            extrude_mvt_to_glb(mvt_bytes.as_ref(), &extrude_params)
        })
        .await
        .map_err(|join_error| {
            tracing::error!(error = %join_error, tenant = %tenant_id, catalog = %catalog_id, collection = %collection_id, z, x, y, "glb extrude task failed to complete");
            Error::Storage(Box::new(join_error))
        })?
        .map_err(|error| {
            tracing::error!(%error, tenant = %tenant_id, catalog = %catalog_id, collection = %collection_id, z, x, y, "failed to extrude glb tile");
            Error::Storage(Box::new(error))
        })?;
        Ok(Bytes::from(glb_bytes))
    });

    let result = match ttl {
        Some(ttl) => ctx.get_or_populate_with_ttl(key, populate, ttl).await,
        None => {
            ctx.get_or_populate(&collection_id_for_cache, key, populate)
                .await
        }
    };
    match result {
        Ok(bytes) if bytes.is_empty() => StatusCode::NO_CONTENT.into_response(),
        Ok(bytes) => glb_response(bytes),
        Err(_) => problem_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalServerError",
            "an internal storage error occurred",
        ),
    }
}

/// `subt` in ASCII, the 3D Tiles 1.1 subtree binary format's magic number
/// (spec: stored little-endian, which is simply these four bytes in file
/// order).
const SUBTREE_MAGIC: [u8; 4] = *b"subt";
const SUBTREE_VERSION: u32 = 1;

/// Builds the one subtree binary document this crate ever serves: `tileset_json`'s
/// [`subtree_file`] doc comment explains why there is exactly one (`subtreeLevels
/// == availableLevels`). Every availability is `constant` — no bitstream buffer
/// is needed (per the 3D Tiles 1.1 spec's own worked example: "The tile
/// availability can be encoded by setting `tileAvailability.constant` to `1`,
/// without needing an explicit bitstream, because all tiles in the subtree are
/// available") — so `binaryByteLength` is 0 and the file is header + JSON only.
/// `tileAvailability`/`contentAvailability` both assert `1` ("might exist, go
/// check"; a genuinely empty tile is still discovered precisely via the
/// content endpoint's 204); `childSubtreeAvailability` asserts `0` because
/// this subtree already covers the collection's entire declared zoom range,
/// so no further subtree can exist beneath it.
fn build_subtree_binary() -> Vec<u8> {
    let doc = serde_json::json!({
        "tileAvailability": { "constant": 1 },
        "contentAvailability": [{ "constant": 1 }],
        "childSubtreeAvailability": { "constant": 0 },
    });
    let mut json_bytes =
        serde_json::to_vec(&doc).expect("subtree JSON body is built from static shapes only");
    // Spec: the JSON chunk is padded with trailing spaces to end on an
    // 8-byte boundary.
    while !json_bytes.len().is_multiple_of(8) {
        json_bytes.push(b' ');
    }

    let mut out = Vec::with_capacity(24 + json_bytes.len());
    out.extend_from_slice(&SUBTREE_MAGIC);
    out.extend_from_slice(&SUBTREE_VERSION.to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes()); // binary byte length: no buffer
    out.extend_from_slice(&json_bytes);
    out
}

fn subtree_response(bytes: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, conformance::MEDIA_TYPE_SUBTREE),
            (header::CACHE_CONTROL, TILE_CACHE_CONTROL),
        ],
        bytes,
    )
        .into_response()
}

/// GET .../3dtiles/subtrees/{tileMatrix}/{tileRow}/{tileCol}.subtree — the availability
/// document a spec-compliant 3D Tiles client resolves before requesting any
/// glb content (see [`tileset_json`]'s doc comment). Refuses every
/// coordinate but the implicit root's own (`0/0/0.subtree`): with
/// `subtreeLevels == availableLevels`, this collection's whole declared zoom
/// range fits inside exactly one subtree, so no other coordinate names a
/// real one.
async fn subtree_file(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(cid) = params.get("cid") else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    let Some(resolved) = resolve_places3d(&ctx, &params, cid).await else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    if let Err(response) = authorize_places(
        &ctx,
        &headers,
        &resolved.tenant_id,
        &resolved.catalog_id,
        &resolved.collection_id,
        resolved.source.filter_capable(),
    )
    .await
    {
        return response;
    }

    let Some(tile_matrix) = params.get("tileMatrix") else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    let Some(tile_row) = params.get("tileRow") else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    let Some(col_raw) = params.get("tileCol") else {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    };
    let Some(tile_col) = col_raw.strip_suffix(SUBTREE_SUFFIX) else {
        return problem_response(
            StatusCode::BAD_REQUEST,
            "InvalidParameter",
            "expected a '.subtree' suffix",
        );
    };

    if tile_matrix != "0" || tile_row != "0" || tile_col != "0" {
        return problem_response(StatusCode::NOT_FOUND, "NotFound", NOT_FOUND_DETAIL);
    }

    subtree_response(build_subtree_binary())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use geozero::mvt::{tile, Message, Tile};
    use tellurion_core::{
        observability::scope_request, AppConfig, CatalogSource, DriverFactory, FileStyleStore,
        MokaTileCache, PhysicalCollection, Registry, Resolver, Result as CoreResult, Router,
        StaticResolver, StorageDecl, StorageDriver, StyleStore, TileCache, VolumeMesh,
        VolumeSource,
    };

    /// A `CatalogSource` that reports no collections — this module's tests
    /// exercise handlers directly, not `Router::validate_catalog`, so this
    /// is present only to satisfy the trait.
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
        /// `#34`: the fingerprint of the last filter `mvt_tile` actually
        /// received (`None` for an unfiltered call).
        last_filter_fingerprint: Mutex<Option<u64>>,
    }

    impl FakeTileSource {
        fn new() -> Self {
            Self {
                tiles: Mutex::new(HashMap::new()),
                calls: AtomicUsize::new(0),
                delay: std::time::Duration::ZERO,
                filter_capable: false,
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
        /// the places3d lane's policy checkpoint can push a matched grant's
        /// filter down to it instead of denying outright.
        fn with_filter_capable() -> Self {
            Self {
                filter_capable: true,
                ..Self::new()
            }
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

        fn filter_capable(&self) -> bool {
            self.filter_capable
        }
    }

    /// `#15`/`#70`: a `VolumeSource` test double — reports whatever mesh (or
    /// absence, or forced error) was configured per tile coordinate, counts
    /// calls, and records the last filter it actually received, the same
    /// shape `FakeTileSource` already uses for the MVT lane.
    struct FakeVolumeSource {
        meshes: Mutex<HashMap<(u8, u32, u32), Option<VolumeMesh>>>,
        calls: AtomicUsize,
        error: bool,
        /// Same purpose as `FakeTileSource::delay`: lets a test spawn N
        /// concurrent requests and be sure they overlap in-flight.
        delay: std::time::Duration,
        /// `#70`: whether this source advertises `filter_capable()` — `false`
        /// (the trait default) for every existing fixture in this module
        /// unless built via [`Self::with_filter_capable`].
        filter_capable: bool,
        /// `#70`: the fingerprint of the last filter `volume_tile` actually
        /// received (`None` for an unfiltered call).
        last_filter_fingerprint: Mutex<Option<u64>>,
    }

    impl FakeVolumeSource {
        fn new() -> Self {
            Self {
                meshes: Mutex::new(HashMap::new()),
                calls: AtomicUsize::new(0),
                error: false,
                delay: std::time::Duration::ZERO,
                filter_capable: false,
                last_filter_fingerprint: Mutex::new(None),
            }
        }

        fn erroring() -> Self {
            Self {
                error: true,
                ..Self::new()
            }
        }

        fn with_delay(delay: std::time::Duration) -> Self {
            Self {
                delay,
                ..Self::new()
            }
        }

        /// `#70`: a variant that advertises `filter_capable() == true`, so
        /// the places3d lane's policy checkpoint can push a matched grant's
        /// filter down to it instead of denying outright.
        fn with_filter_capable() -> Self {
            Self {
                filter_capable: true,
                ..Self::new()
            }
        }

        fn set(&self, coord: TileCoord, value: Option<VolumeMesh>) {
            self.meshes
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
    impl VolumeSource for FakeVolumeSource {
        async fn volume_tile(
            &self,
            _collection: &CollectionDecl,
            coord: TileCoord,
            filter: Option<&tellurion_core::Filter>,
        ) -> tellurion_core::Result<Option<VolumeMesh>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_filter_fingerprint.lock().unwrap() =
                filter.map(tellurion_core::Filter::fingerprint);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            if self.error {
                return Err(Error::Timeout);
            }
            Ok(self
                .meshes
                .lock()
                .unwrap()
                .get(&(coord.z, coord.x, coord.y))
                .cloned()
                .flatten())
        }

        fn filter_capable(&self) -> bool {
            self.filter_capable
        }
    }

    struct FakeDriver {
        tiles: Arc<FakeTileSource>,
        volume: Option<Arc<FakeVolumeSource>>,
    }

    impl StorageDriver for FakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::clone(&self.tiles) as Arc<dyn TileSource>)
        }

        fn volume_source(&self) -> Option<Arc<dyn VolumeSource>> {
            self.volume
                .as_ref()
                .map(|volume| Arc::clone(volume) as Arc<dyn VolumeSource>)
        }
    }

    struct FakeFactory {
        tiles: Arc<FakeTileSource>,
        volume: Option<Arc<FakeVolumeSource>>,
    }

    impl DriverFactory for FakeFactory {
        fn name(&self) -> &str {
            "fake"
        }

        fn build(&self, _decl: &StorageDecl) -> tellurion_core::Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FakeDriver {
                tiles: Arc::clone(&self.tiles),
                volume: self.volume.clone(),
            }))
        }
    }

    fn test_context(tiles: Arc<FakeTileSource>) -> Arc<AppContext> {
        build_test_context(FakeFactory {
            tiles,
            volume: None,
        })
    }

    /// `#34`: a minimal single-collection context built from a
    /// caller-supplied full config document (so a test can add `auth:`/
    /// `policy:` sections), with a real authorizer built from `config.auth`
    /// instead of the fixed `None` every other builder in this module uses.
    fn test_context_with_config(tiles: Arc<FakeTileSource>, config_yaml: &str) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            tiles,
            volume: None,
        }));
        let router = Router::build(&config, &registry).unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
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

    /// Same fixture as [`test_context`], plus a driver that also advertises
    /// `VolumeSource` (`#15`) — for tests proving the places lane consumes
    /// real solid geometry when a driver offers it, instead of running the
    /// footprint+height extrusion fallback.
    fn test_context_with_volume(
        tiles: Arc<FakeTileSource>,
        volume: Arc<FakeVolumeSource>,
    ) -> Arc<AppContext> {
        build_test_context(FakeFactory {
            tiles,
            volume: Some(volume),
        })
    }

    /// [`test_context_with_config`] plus a driver that also advertises
    /// `VolumeSource` — for proving the glb lane's policy checkpoint fails
    /// closed when solid geometry (which has no filter seam) would serve
    /// the tile.
    fn test_context_with_volume_and_config(
        tiles: Arc<FakeTileSource>,
        volume: Arc<FakeVolumeSource>,
        config_yaml: &str,
    ) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            tiles,
            volume: Some(volume),
        }));
        let router = Router::build(&config, &registry).unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
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

    /// `#34`/`#41`/`#70`: a filtered-only grant cannot be honored by a
    /// `VolumeSource` that stays at the trait default `filter_capable() ==
    /// false` (`FakeVolumeSource::new()`, never overridden here) — so the
    /// checkpoint must deny it outright (fail closed), never serve
    /// unfiltered solids. See `glb_filtered_grant_on_a_filter_capable_
    /// volume_source_is_served_and_pushes_the_filter_through` below for the
    /// filter-capable counterpart.
    #[tokio::test]
    async fn glb_filtered_grant_is_denied_when_the_volume_source_cannot_filter() {
        let tiles = Arc::new(FakeTileSource::new());
        let volume = Arc::new(FakeVolumeSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        volume.set(coord, Some(one_triangle_mesh(42.0)));
        let ctx =
            test_context_with_volume_and_config(tiles, Arc::clone(&volume), RBAC_PLACES_CONFIG);

        let response = glb_tile(
            State(ctx),
            path("demo", 2, 1, "1.glb"),
            headers_with_bearer("filtered-token"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            volume.call_count(),
            0,
            "a denied subject must never reach the volume source"
        );
    }

    /// The unconditional-grant counterpart: the fail-closed rule only bites
    /// filtered grants; a subject whose grant carries no filter still gets
    /// real solid geometry.
    #[tokio::test]
    async fn glb_unconditional_grant_still_reaches_solid_geometry() {
        let tiles = Arc::new(FakeTileSource::new());
        let volume = Arc::new(FakeVolumeSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        volume.set(coord, Some(one_triangle_mesh(21.0)));
        let ctx =
            test_context_with_volume_and_config(tiles, Arc::clone(&volume), RBAC_PLACES_CONFIG);

        let response = glb_tile(
            State(ctx),
            path("demo", 2, 1, "1.glb"),
            headers_with_bearer("reader-token"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(volume.call_count(), 1);
    }

    /// `#70`: a `VolumeSource` that DOES advertise `filter_capable()` must
    /// actually be served a filtered-only grant — the checkpoint gates on
    /// this source's own capability, not the MVT `TileSource`'s — and the
    /// substituted grant filter must reach `volume_tile` itself.
    #[tokio::test]
    async fn glb_filtered_grant_on_a_filter_capable_volume_source_is_served_and_pushes_the_filter_through(
    ) {
        let tiles = Arc::new(FakeTileSource::new());
        let volume = Arc::new(FakeVolumeSource::with_filter_capable());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        volume.set(coord, Some(one_triangle_mesh(42.0)));
        let ctx =
            test_context_with_volume_and_config(tiles, Arc::clone(&volume), RBAC_PLACES_CONFIG);

        let response = glb_tile(
            State(ctx),
            path("demo", 2, 1, "1.glb"),
            headers_with_bearer("filtered-token"),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "a filter-capable volume source must serve a filtered-only grant, not deny it"
        );
        let expected_fingerprint = tellurion_core::filter::parse_text("org = 'acme'")
            .unwrap()
            .fingerprint();
        assert_eq!(
            volume.last_filter_fingerprint(),
            Some(expected_fingerprint),
            "the substituted grant filter must reach the volume source's own volume_tile call"
        );
    }

    fn build_test_context(factory: FakeFactory) -> Arc<AppContext> {
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        build_test_context_with_cache(factory, cache, "")
    }

    /// Same fixture as [`build_test_context`], parameterized on the
    /// `TileCache` implementation and an extra YAML block appended to the
    /// `demo` collection's declaration (`#51`: lets a test configure
    /// `cache_ttl_s` and swap in a TTL-recording `TileCache` double — neither
    /// is observable through the default `MokaTileCache` fixture, whose own
    /// `get_or_populate_with_ttl` just drops the TTL and delegates to the
    /// plain entry point, as documented on `MokaTileCache` itself).
    fn build_test_context_with_cache(
        factory: FakeFactory,
        cache: Arc<dyn TileCache>,
        demo_extra_yaml: &str,
    ) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
    places3d: {{ height_property: height }}
{demo_extra_yaml}
  - id: flat
    catalog: default
    storage: main
    table: flat
    geometry: geom
    pk: id
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
  - id: tall
    catalog: default
    storage: main
    table: tall
    geometry: geom
    pk: id
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
    places3d: {{ height_property: height, exaggeration: 2.0 }}
"#
        ))
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(factory));
        let router = Router::build(&config, &registry).unwrap();
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
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

    /// `#51`: a `TileCache` double that records the `ttl` argument every
    /// `get_or_populate_with_ttl` call actually received, keyed by
    /// `TileKey::encoding` — proves both this collection's Glb entry and its
    /// nested Mvt sub-fetch entry (`fetch_mvt`, called from inside the Glb
    /// `populate` closure) receive the same, correctly-resolved TTL. Same
    /// shape as `tellurion-core::context`'s own `RecordingCache` used for
    /// the `#46` TTL-aware entry point tests.
    struct RecordingCache {
        ttls_by_encoding: Mutex<HashMap<Encoding, Duration>>,
        plain_calls: AtomicUsize,
    }

    impl RecordingCache {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                ttls_by_encoding: Mutex::new(HashMap::new()),
                plain_calls: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait::async_trait]
    impl TileCache for RecordingCache {
        async fn get(&self, _key: &TileKey) -> Option<Bytes> {
            None
        }

        async fn insert(&self, _key: TileKey, _value: Bytes) {}

        async fn get_or_populate(
            &self,
            _key: TileKey,
            populate: PopulateFuture,
        ) -> Result<Bytes, Arc<Error>> {
            self.plain_calls.fetch_add(1, Ordering::SeqCst);
            populate.await.map_err(Arc::new)
        }

        async fn get_or_populate_with_ttl(
            &self,
            key: TileKey,
            populate: PopulateFuture,
            ttl: Duration,
        ) -> Result<Bytes, Arc<Error>> {
            self.ttls_by_encoding
                .lock()
                .unwrap()
                .insert(key.encoding, ttl);
            populate.await.map_err(Arc::new)
        }
    }

    struct TimedCache;

    #[async_trait::async_trait]
    impl TileCache for TimedCache {
        async fn get(&self, _key: &TileKey) -> Option<Bytes> {
            None
        }

        async fn insert(&self, _key: TileKey, _value: Bytes) {}

        async fn get_or_populate_with_ttl(
            &self,
            _key: TileKey,
            populate: PopulateFuture,
            _ttl: Duration,
        ) -> Result<Bytes, Arc<Error>> {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let value = populate.await.map_err(Arc::new)?;
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(value)
        }
    }

    /// `row`/`col` follow the OGC API Tiles path order (`tileRow` before
    /// `tileCol`); `col` carries any `.glb` suffix since it's the last path
    /// segment.
    fn path(cid: &str, z: u8, row: u32, col: &str) -> Path<HashMap<String, String>> {
        Path(HashMap::from([
            ("cid".to_string(), cid.to_string()),
            ("tileMatrix".to_string(), z.to_string()),
            ("tileRow".to_string(), row.to_string()),
            ("tileCol".to_string(), col.to_string()),
        ]))
    }

    fn cid_path(cid: &str) -> Path<HashMap<String, String>> {
        Path(HashMap::from([("cid".to_string(), cid.to_string())]))
    }

    /// `tileset` is mounted at `.../collections/{cid}/3dtiles` (see
    /// `router`), and this crate's own unit tests run with no server mount
    /// in front of it — so the `OriginalUri` a real request would carry is
    /// exactly this relative path.
    fn tileset_uri(cid: &str) -> OriginalUri {
        OriginalUri(
            format!("/collections/{cid}/3dtiles")
                .parse()
                .expect("test-built URI is always well-formed"),
        )
    }

    fn subtree_path(
        cid: &str,
        tile_matrix: &str,
        tile_row: &str,
        tile_col: &str,
    ) -> Path<HashMap<String, String>> {
        Path(HashMap::from([
            ("cid".to_string(), cid.to_string()),
            ("tileMatrix".to_string(), tile_matrix.to_string()),
            ("tileRow".to_string(), tile_row.to_string()),
            ("tileCol".to_string(), tile_col.to_string()),
        ]))
    }

    /// Encodes an MVT geometry command header: 3 low bits are the command id
    /// (1 = MoveTo, 2 = LineTo, 7 = ClosePath), the rest is the repeat count.
    fn cmd(id: u32, count: u32) -> u32 {
        id | (count << 3)
    }
    fn zz(n: i32) -> u32 {
        ((n << 1) ^ (n >> 31)) as u32
    }

    /// A single-ring 10x10 square (0,0)-(10,10) MVT polygon feature carrying
    /// one numeric property under `height_property`. Enough geometry for
    /// `extrude_mvt_to_glb` to produce a real, non-placeholder mesh; hole
    /// handling itself is `tellurion-render`'s own test responsibility, not
    /// this crate's.
    fn square_polygon_mvt(height_property: &str, height: f64) -> Bytes {
        let geometry = vec![
            cmd(1, 1),
            zz(0),
            zz(0),
            cmd(2, 3),
            zz(10),
            zz(0),
            zz(0),
            zz(10),
            zz(-10),
            zz(0),
            cmd(7, 1),
        ];
        let keys = vec![height_property.to_string()];
        let values = vec![tile::Value {
            double_value: Some(height),
            ..Default::default()
        }];
        let mut feature = tile::Feature {
            geometry,
            tags: vec![0, 0],
            ..Default::default()
        };
        feature.set_type(tile::GeomType::Polygon);
        let layer = tile::Layer {
            version: 2,
            name: "buildings".to_string(),
            extent: Some(100),
            features: vec![feature],
            keys,
            values,
        };
        Bytes::from(
            Tile {
                layers: vec![layer],
            }
            .encode_to_vec(),
        )
    }

    /// A single closed `n`-vertex polygon (points on a circle, so it's
    /// always simple/non-self-intersecting regardless of `n`) carrying one
    /// numeric property under `height_property`. Ear-clip has no vertex cap
    /// and no spatial index (see `tellurion-render::earclip`'s own doc
    /// comment), so a large, unsimplified polygon like this is exactly the
    /// realistic-worst-case input the #29 offload decision is about. Only
    /// used by [`glb_render_does_not_starve_the_async_runtime_thread`] to
    /// make the extrude call take long enough to observe — not to assert on
    /// specific mesh output.
    fn big_ngon_mvt(height_property: &str, height: f64, n: usize) -> Bytes {
        let (cx, cy, radius) = (2048i32, 2048i32, 2000f64);
        let points: Vec<(i32, i32)> = (0..n)
            .map(|i| {
                let theta = 2.0 * std::f64::consts::PI * (i as f64) / (n as f64);
                (
                    cx + (radius * theta.cos()) as i32,
                    cy + (radius * theta.sin()) as i32,
                )
            })
            .collect();

        let mut geometry = vec![cmd(1, 1), zz(points[0].0), zz(points[0].1)];
        let mut cursor = points[0];
        let mut line_to = vec![cmd(2, (n - 1) as u32)];
        for &(x, y) in &points[1..] {
            line_to.push(zz(x - cursor.0));
            line_to.push(zz(y - cursor.1));
            cursor = (x, y);
        }
        geometry.extend(line_to);
        geometry.push(cmd(7, 1));

        let keys = vec![height_property.to_string()];
        let values = vec![tile::Value {
            double_value: Some(height),
            ..Default::default()
        }];
        let mut feature = tile::Feature {
            geometry,
            tags: vec![0, 0],
            ..Default::default()
        };
        feature.set_type(tile::GeomType::Polygon);
        let layer = tile::Layer {
            version: 2,
            name: "buildings".to_string(),
            extent: Some(4096),
            features: vec![feature],
            keys,
            values,
        };
        Bytes::from(
            Tile {
                layers: vec![layer],
            }
            .encode_to_vec(),
        )
    }

    const GLB_MAGIC: [u8; 4] = *b"glTF";

    /// Asserts a response carries the shared RFC 9457 problem-details body:
    /// `application/problem+json` content type plus `type`/`title`/`status`/
    /// `detail`/`code` fields, with `code` and `status` matching the given
    /// values.
    async fn assert_problem_json(response: Response, status: StatusCode, code: &str) {
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["type"], "about:blank");
        assert_eq!(json["status"], status.as_u16());
        assert_eq!(json["code"], code);
        assert!(json["title"].is_string());
        assert!(json["detail"].is_string());
    }

    #[tokio::test]
    async fn tileset_requires_places3d_config() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = tileset(
            State(ctx),
            cid_path("flat"),
            HeaderMap::new(),
            tileset_uri("flat"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_problem_json(response, StatusCode::NOT_FOUND, "NotFound").await;
    }

    #[tokio::test]
    async fn tileset_unknown_collection_is_not_found() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = tileset(
            State(ctx),
            cid_path("missing"),
            HeaderMap::new(),
            tileset_uri("missing"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_problem_json(response, StatusCode::NOT_FOUND, "NotFound").await;
    }

    #[tokio::test]
    async fn tileset_json_shape_has_asset_version_and_template_content_uri() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = tileset(
            State(ctx),
            cid_path("demo"),
            HeaderMap::new(),
            tileset_uri("demo"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["asset"]["version"], "1.1");
        assert_eq!(json["root"]["refine"], "REPLACE");
        assert_eq!(
            json["root"]["content"]["uri"], "/collections/demo/3dtiles/tiles/{level}/{y}/{x}.glb",
            "3D Tiles 1.1 implicit tiling requires the {{level}}/{{x}}/{{y}} token names verbatim, \
             reordered row-first (level/y/x) to match this crate's OGC API Tiles-style axum route"
        );
        assert_eq!(
            json["root"]["implicitTiling"]["subdivisionScheme"],
            "QUADTREE"
        );
        assert_eq!(
            json["root"]["implicitTiling"]["subtrees"]["uri"],
            "/collections/demo/3dtiles/subtrees/{level}/{y}/{x}.subtree"
        );
        assert_eq!(
            json["root"]["boundingVolume"]["region"]
                .as_array()
                .unwrap()
                .len(),
            6
        );
        // "demo" declares no explicit exaggeration (defaults to 1.0), so the
        // bounding region's upper height bound is exactly the render crate's
        // own clamp ceiling.
        assert_eq!(
            json["root"]["boundingVolume"]["region"][5],
            tellurion_render::MAX_HEIGHT_METERS
        );
    }

    #[tokio::test]
    async fn configured_public_base_is_used_for_3d_content_and_subtree_templates() {
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
    places3d: { height_property: height }
"#,
        );
        let response = tileset(
            State(ctx),
            cid_path("demo"),
            HeaderMap::new(),
            tileset_uri("demo"),
        )
        .await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            json["root"]["content"]["uri"],
            "https://maps.example.test/tellurion/collections/demo/3dtiles/tiles/{level}/{y}/{x}.glb"
        );
        assert_eq!(
            json["root"]["implicitTiling"]["subtrees"]["uri"],
            "https://maps.example.test/tellurion/collections/demo/3dtiles/subtrees/{level}/{y}/{x}.subtree"
        );
    }

    #[tokio::test]
    async fn tileset_bounding_region_height_scales_with_exaggeration() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = tileset(
            State(ctx),
            cid_path("tall"),
            HeaderMap::new(),
            tileset_uri("tall"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // "tall" declares places3d.exaggeration = 2.0 — the region's upper
        // bound must scale with it, since extrude_mvt_to_glb clamps a raw
        // height to MAX_HEIGHT_METERS *before* multiplying by exaggeration,
        // so the tallest content it can ever produce is exactly that product.
        assert_eq!(
            json["root"]["boundingVolume"]["region"][5],
            tellurion_render::MAX_HEIGHT_METERS * 2.0
        );
    }

    #[test]
    fn geometric_error_is_monotonically_decreasing_with_zoom() {
        let errors: Vec<f64> = (0..=10u8).map(geometric_error_at_zoom).collect();
        for pair in errors.windows(2) {
            assert!(
                pair[1] < pair[0],
                "geometricError must strictly decrease as zoom increases: {} then {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[tokio::test]
    async fn glb_request_populates_the_mvt_cache_first() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));
        let ctx = test_context(Arc::clone(&tiles));

        let response = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            conformance::MEDIA_TYPE_GLB
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[0..4], &GLB_MAGIC);

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
            "the MVT entry must be populated as a side effect of the Glb-first lane"
        );
        assert_eq!(tiles.call_count(), 1, "driver hit exactly once (MVT probe)");
    }

    /// `#51`: before this fix, `fetch_mvt`'s inner sub-fetch used a plain
    /// get/insert pair — the collection's configured `cache_ttl_s` reached
    /// the outer Glb-keyed entry but never the inner Mvt-keyed one. With
    /// `RecordingCache` standing in for the real cache, this proves the Mvt
    /// entry now receives the exact same TTL as the Glb entry.
    #[tokio::test]
    async fn glb_sub_fetch_mvt_entry_honors_the_collections_configured_ttl() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));

        let cache = RecordingCache::new();
        let ctx = build_test_context_with_cache(
            FakeFactory {
                tiles,
                volume: None,
            },
            Arc::clone(&cache) as Arc<dyn TileCache>,
            "    settings: { cache_ttl_s: 45 }",
        );

        let response = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let ttls = cache.ttls_by_encoding.lock().unwrap();
        assert_eq!(
            ttls.get(&Encoding::Mvt),
            Some(&Duration::from_secs(45)),
            "the inner Mvt sub-fetch must be inserted with the collection's configured cache_ttl_s"
        );
        assert_eq!(
            ttls.get(&Encoding::Glb),
            Some(&Duration::from_secs(45)),
            "the outer Glb entry must receive the same TTL as the inner Mvt entry"
        );
        assert_eq!(
            cache.plain_calls.load(Ordering::SeqCst),
            0,
            "a collection with a configured cache_ttl_s must never fall back to the plain, non-TTL entry point"
        );
    }

    #[tokio::test]
    async fn glb_cache_work_is_exclusive_from_volume_query_time() {
        let tiles = Arc::new(FakeTileSource::new());
        let volume = Arc::new(FakeVolumeSource::with_delay(Duration::from_millis(20)));
        let ctx = build_test_context_with_cache(
            FakeFactory {
                tiles,
                volume: Some(volume),
            },
            Arc::new(TimedCache),
            "    settings: { cache_ttl_s: 45 }",
        );

        let started = tokio::time::Instant::now();
        let (response, snapshot) = scope_request(glb_tile(
            State(ctx),
            path("demo", 2, 1, "1.glb"),
            HeaderMap::new(),
        ))
        .await;
        let total = started.elapsed();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(snapshot.cache() >= Duration::from_millis(40));
        assert!(snapshot.query() >= Duration::from_millis(20));
        assert_eq!(
            snapshot.routing() + snapshot.cache() + snapshot.query() + snapshot.encode(total),
            total
        );
    }

    #[tokio::test]
    async fn glb_and_mvt_cache_entries_hold_distinct_content() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));
        let ctx = test_context(Arc::clone(&tiles));

        let _response = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
            HeaderMap::new(),
        )
        .await;

        let glb_cached = ctx
            .cache
            .get(&glb_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                None,
            ))
            .await
            .expect("glb entry present");
        let mvt_cached = ctx
            .cache
            .get(&mvt_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                None,
            ))
            .await
            .expect("mvt entry present");
        assert_ne!(
            glb_cached, mvt_cached,
            "Glb and Mvt cache entries for the same coord must hold distinct content"
        );
        assert_eq!(&glb_cached[0..4], &GLB_MAGIC);
    }

    #[tokio::test]
    async fn second_glb_request_is_served_from_cache_without_rerendering() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));
        let ctx = test_context(Arc::clone(&tiles));

        let _first = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
            HeaderMap::new(),
        )
        .await;
        let second = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
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

    /// Regression for #23: this crate's own `fetch_mvt` (unlike
    /// `tellurion-tiles`') never coalesced via `get_or_populate` on its own —
    /// before wrapping the whole mvt-fetch + extrude pipeline in one
    /// `get_or_populate` call keyed by the glb key, N concurrent misses on
    /// the same coord raced the driver independently. `tiles.call_count() ==
    /// 1` is a real discriminator here (unlike the Png lane in
    /// `tellurion-tiles`, this crate had no prior MVT-level coalescing to
    /// mask the gap).
    #[tokio::test]
    async fn concurrent_glb_misses_on_one_tile_coalesce_to_a_single_extrude() {
        let tiles = Arc::new(FakeTileSource::with_delay(
            std::time::Duration::from_millis(30),
        ));
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));
        let ctx = test_context(Arc::clone(&tiles));

        let mut handles = Vec::new();
        for _ in 0..16 {
            let ctx = Arc::clone(&ctx);
            handles.push(tokio::spawn(async move {
                let response =
                    glb_tile(State(ctx), path("demo", 2, 1, "1.glb"), HeaderMap::new()).await;
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
            "all 16 concurrent glb requests for the same tile must return identical bytes"
        );
        assert_eq!(
            tiles.call_count(),
            1,
            "16 concurrent misses on one glb tile must hit the driver exactly once"
        );
    }

    /// Regression proof for the #29 offload decision, mirroring
    /// `tellurion-tiles`' `png_render_does_not_starve_the_async_runtime_thread`.
    /// On a single-threaded (`current_thread`) runtime there is exactly one
    /// OS thread available to poll async tasks; an in-line, synchronous
    /// extrude would occupy that thread for its entire duration with nowhere
    /// else for any other task to run. A concurrently spawned task that
    /// increments a counter and immediately yields, in a tight loop, can
    /// only advance *during* the extrude call if the actual CPU-bound work
    /// has moved off this runtime's thread onto the blocking pool
    /// (`tokio::task::spawn_blocking`).
    ///
    /// This asserts liveness, not a magnitude: any nonzero number of
    /// increments observed strictly between the render call starting and
    /// finishing proves the runtime thread was free to run something else
    /// while the extrude was in flight, which is only possible if the
    /// extrude is not running inline on that same thread. A regression back
    /// to inline execution would occupy the thread for the extrude's whole
    /// duration, so the delta would be deterministically zero — never just
    /// small. An earlier version of this test asserted the delta was above
    /// a fixed count (`> 10`); under a saturated host (e.g. a parallel
    /// `cargo build` competing for every core) the scheduler can hand this
    /// test's worker thread only a couple of timeslices for the whole
    /// render, which drove a false failure with no actual regression. The
    /// `> 0` check below still fails deterministically on the regression it
    /// guards against, and needs only one scheduling interleave to pass.
    #[tokio::test(flavor = "current_thread")]
    async fn glb_render_does_not_starve_the_async_runtime_thread() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(big_ngon_mvt("height", 20.0, 900)));
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
        // its startup scheduling isn't counted as progress "during" the
        // render below.
        tokio::task::yield_now().await;
        let before = progress.load(Ordering::Relaxed);

        let response = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let during = progress.load(Ordering::Relaxed) - before;

        stop.store(true, Ordering::Relaxed);
        progress_task.await.unwrap();

        assert!(
            during > 0,
            "the progress task made no headway while the extrude was in \
             flight -- ear-clip extrude looks like it ran in-line on this \
             single-threaded runtime instead of on the blocking pool"
        );
    }

    #[tokio::test]
    async fn empty_mvt_tile_returns_204_for_glb() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 3, x: 2, y: 2 };
        tiles.set(coord, None);
        let ctx = test_context(Arc::clone(&tiles));

        let response = glb_tile(State(ctx), path("demo", 3, 2, "2.glb"), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn glb_route_requires_places3d_config() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(
            TileCoord { z: 1, x: 0, y: 0 },
            Some(square_polygon_mvt("height", 5.0)),
        );
        let ctx = test_context(Arc::clone(&tiles));

        let response = glb_tile(State(ctx), path("flat", 1, 0, "0.glb"), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_problem_json(response, StatusCode::NOT_FOUND, "NotFound").await;
    }

    #[tokio::test]
    async fn glb_zoom_beyond_collection_maxzoom_is_not_found() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = glb_tile(State(ctx), path("demo", 9, 0, "0.glb"), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_problem_json(response, StatusCode::NOT_FOUND, "NotFound").await;
    }

    #[tokio::test]
    async fn glb_xy_beyond_matrix_size_is_bad_request() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        // matrix side at z=2 is 4; tileRow=10 is out of range.
        let response = glb_tile(State(ctx), path("demo", 2, 10, "0.glb"), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_problem_json(response, StatusCode::BAD_REQUEST, "InvalidParameter").await;
    }

    #[tokio::test]
    async fn glb_missing_suffix_is_bad_request() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = glb_tile(State(ctx), path("demo", 1, 0, "0"), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_problem_json(response, StatusCode::BAD_REQUEST, "InvalidParameter").await;
    }

    #[tokio::test]
    async fn glb_unknown_collection_is_not_found() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = glb_tile(State(ctx), path("missing", 1, 0, "0.glb"), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_problem_json(response, StatusCode::NOT_FOUND, "NotFound").await;
    }

    const SUBTREE_MAGIC_BYTES: [u8; 4] = *b"subt";

    #[tokio::test]
    async fn subtree_root_returns_a_well_formed_binary_with_constant_availability() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = subtree_file(
            State(ctx),
            subtree_path("demo", "0", "0", "0.subtree"),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            conformance::MEDIA_TYPE_SUBTREE
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        assert_eq!(&body[0..4], &SUBTREE_MAGIC_BYTES, "magic");
        assert_eq!(
            u32::from_le_bytes(body[4..8].try_into().unwrap()),
            1,
            "version"
        );
        let json_len = u64::from_le_bytes(body[8..16].try_into().unwrap()) as usize;
        let binary_len = u64::from_le_bytes(body[16..24].try_into().unwrap()) as usize;
        assert_eq!(binary_len, 0, "constant availability needs no buffer");
        assert_eq!(
            body.len(),
            24 + json_len,
            "no trailing bytes past the JSON chunk"
        );
        assert_eq!(json_len % 8, 0, "JSON chunk must end on an 8-byte boundary");

        let doc: serde_json::Value = serde_json::from_slice(&body[24..24 + json_len]).unwrap();
        assert_eq!(doc["tileAvailability"]["constant"], 1);
        assert_eq!(doc["contentAvailability"][0]["constant"], 1);
        assert_eq!(doc["childSubtreeAvailability"]["constant"], 0);
    }

    #[tokio::test]
    async fn subtree_non_root_coordinate_is_not_found() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = subtree_file(
            State(ctx),
            subtree_path("demo", "1", "0", "0.subtree"),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_problem_json(response, StatusCode::NOT_FOUND, "NotFound").await;
    }

    #[tokio::test]
    async fn subtree_missing_suffix_is_bad_request() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = subtree_file(
            State(ctx),
            subtree_path("demo", "0", "0", "0"),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_problem_json(response, StatusCode::BAD_REQUEST, "InvalidParameter").await;
    }

    #[tokio::test]
    async fn subtree_requires_places3d_config() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = subtree_file(
            State(ctx),
            subtree_path("flat", "0", "0", "0.subtree"),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_problem_json(response, StatusCode::NOT_FOUND, "NotFound").await;
    }

    #[tokio::test]
    async fn subtree_unknown_collection_is_not_found() {
        let ctx = test_context(Arc::new(FakeTileSource::new()));
        let response = subtree_file(
            State(ctx),
            subtree_path("missing", "0", "0", "0.subtree"),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_problem_json(response, StatusCode::NOT_FOUND, "NotFound").await;
    }

    /// Parses just enough of a glb response body to read its JSON chunk —
    /// enough for the `#15` tests below to check the volume-source lane's
    /// output reflects raw mesh coordinates, not the extrusion lane's own
    /// clamp/exaggeration transform.
    fn glb_json_chunk(body: &[u8]) -> serde_json::Value {
        assert_eq!(&body[0..4], &GLB_MAGIC, "glb magic");
        let json_len = u32::from_le_bytes(body[12..16].try_into().unwrap()) as usize;
        serde_json::from_slice(&body[20..20 + json_len]).unwrap()
    }

    fn one_triangle_mesh(z: f64) -> VolumeMesh {
        VolumeMesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, z]],
            indices: vec![0, 1, 2],
        }
    }

    /// `#15`: the places lane must consume a driver's `VolumeSource` in
    /// preference to the MVT+extrusion path — the seam this issue asks for.
    #[tokio::test]
    async fn glb_tile_consumes_the_volume_source_when_the_driver_advertises_one() {
        let tiles = Arc::new(FakeTileSource::new());
        let volume = Arc::new(FakeVolumeSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        volume.set(coord, Some(one_triangle_mesh(42.0)));
        let ctx = test_context_with_volume(Arc::clone(&tiles), Arc::clone(&volume));

        let response = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            conformance::MEDIA_TYPE_GLB
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let doc = glb_json_chunk(&body);
        assert_eq!(
            doc["accessors"][0]["max"][2], 42.0,
            "the volume source's own Z must pass through untouched — no extrusion clamp or \
             exaggeration applied"
        );

        assert_eq!(
            volume.call_count(),
            1,
            "the volume source must be consulted"
        );
        assert_eq!(
            tiles.call_count(),
            0,
            "advertising VolumeSource must skip the MVT fetch and extrusion path entirely"
        );
    }

    /// `#15`: an empty answer from a driver that *does* advertise
    /// `VolumeSource` means "no solid content at this coordinate" — the same
    /// 204 convention `TileSource::mvt_tile`'s `Ok(None)` uses — not a
    /// signal to try extrusion instead.
    #[tokio::test]
    async fn glb_tile_empty_volume_answer_is_204_without_touching_extrusion() {
        let tiles = Arc::new(FakeTileSource::new());
        let volume = Arc::new(FakeVolumeSource::new());
        let coord = TileCoord { z: 3, x: 2, y: 2 };
        volume.set(coord, None);
        let ctx = test_context_with_volume(Arc::clone(&tiles), Arc::clone(&volume));

        let response = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 3, 2, "2.glb"),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            tiles.call_count(),
            0,
            "an empty volume answer must not fall through to MVT+extrusion"
        );
    }

    /// `#15`: a volume source failure is a real failure, surfaced the same
    /// way an MVT source failure already is (`MvtFetch::Failed`) — never
    /// silently retried through extrusion.
    #[tokio::test]
    async fn glb_tile_volume_source_error_is_internal_server_error() {
        let tiles = Arc::new(FakeTileSource::new());
        let volume = Arc::new(FakeVolumeSource::erroring());
        let ctx = test_context_with_volume(Arc::clone(&tiles), Arc::clone(&volume));

        let response = glb_tile(State(ctx), path("demo", 1, 0, "0.glb"), HeaderMap::new()).await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            tiles.call_count(),
            0,
            "a volume source error must not fall through to MVT+extrusion"
        );
    }

    /// `#15`: capability-detection's negative case, spelled out explicitly
    /// (distinct from the pre-existing MVT-cache-focused tests above, which
    /// happen to cover the same code path but were written before this
    /// capability existed) — a driver that never advertises `VolumeSource`
    /// (`test_context`'s ordinary fixture) must still run the
    /// footprint+height extrusion path exactly as it did before `#15`.
    #[tokio::test]
    async fn glb_tile_falls_back_to_extrusion_when_the_driver_has_no_volume_source() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));
        let ctx = test_context(Arc::clone(&tiles));

        let response = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            tiles.call_count(),
            1,
            "no VolumeSource means the MVT+extrusion path must run, same as before #15"
        );
    }

    /// Volume-lane counterpart of
    /// `concurrent_glb_misses_on_one_tile_coalesce_to_a_single_extrude`: the
    /// same `get_or_populate` coalescing must hold for the volume-source
    /// branch, not just the extrusion one.
    #[tokio::test]
    async fn concurrent_glb_misses_on_one_volume_tile_coalesce_to_a_single_volume_tile_call() {
        let tiles = Arc::new(FakeTileSource::new());
        let volume = Arc::new(FakeVolumeSource::with_delay(
            std::time::Duration::from_millis(30),
        ));
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        volume.set(coord, Some(one_triangle_mesh(5.0)));
        let ctx = test_context_with_volume(Arc::clone(&tiles), Arc::clone(&volume));

        let mut handles = Vec::new();
        for _ in 0..16 {
            let ctx = Arc::clone(&ctx);
            handles.push(tokio::spawn(async move {
                let response =
                    glb_tile(State(ctx), path("demo", 2, 1, "1.glb"), HeaderMap::new()).await;
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
            "all 16 concurrent glb requests for the same tile must return identical bytes"
        );
        assert_eq!(
            volume.call_count(),
            1,
            "16 concurrent misses on one glb tile must hit the volume source exactly once"
        );
    }

    // -- `#70` per-collection geometry-type gating ---------------------------

    /// `#70`: a `CatalogSource` double that reports one real physical row per
    /// name it was given — table name plus `geometry_type` — so
    /// `Router::resolve_volume`'s own per-collection geometry-type check
    /// actually has something to compare against, instead of hitting the
    /// "descriptor derivation failed, trust the driver-wide signal" fallback
    /// every other fixture in this module relies on (`EmptyCatalog` reports
    /// no physical rows at all).
    struct GeometryTypeCatalog {
        geometry_types: HashMap<String, String>,
    }

    #[async_trait::async_trait]
    impl CatalogSource for GeometryTypeCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(self
                .geometry_types
                .iter()
                .map(|(name, geometry_type)| PhysicalCollection {
                    name: name.clone(),
                    geometry_column: Some("geom".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(3857),
                    geometry_type: Some(geometry_type.clone()),
                })
                .collect())
        }
    }

    struct GeometryTypedDriver {
        tiles: Arc<FakeTileSource>,
        volume: Arc<FakeVolumeSource>,
        catalog: Arc<GeometryTypeCatalog>,
    }

    impl StorageDriver for GeometryTypedDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::clone(&self.catalog) as Arc<dyn CatalogSource>
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::clone(&self.tiles) as Arc<dyn TileSource>)
        }

        fn volume_source(&self) -> Option<Arc<dyn VolumeSource>> {
            Some(Arc::clone(&self.volume) as Arc<dyn VolumeSource>)
        }
    }

    struct GeometryTypedFactory {
        tiles: Arc<FakeTileSource>,
        volume: Arc<FakeVolumeSource>,
        catalog: Arc<GeometryTypeCatalog>,
    }

    impl DriverFactory for GeometryTypedFactory {
        fn name(&self) -> &str {
            "fake"
        }

        fn build(&self, _decl: &StorageDecl) -> tellurion_core::Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(GeometryTypedDriver {
                tiles: Arc::clone(&self.tiles),
                volume: Arc::clone(&self.volume),
                catalog: Arc::clone(&self.catalog),
            }))
        }
    }

    /// `#70`: two `places3d` collections sharing one storage entry/driver —
    /// "footprint" reports a flat `POLYGON` geometry_type, "solid" reports
    /// `POLYHEDRALSURFACE` — reproducing the issue's own scenario: a
    /// footprint+height collection sharing a storage entry with a genuinely
    /// solid one must regain the MVT+extrusion fallback, while the solid
    /// collection keeps serving true solids.
    fn test_context_with_volume_geometry_types(
        tiles: Arc<FakeTileSource>,
        volume: Arc<FakeVolumeSource>,
    ) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: footprint
    catalog: default
    storage: main
    table: footprint
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
    places3d: { height_property: height }
  - id: solid
    catalog: default
    storage: main
    table: solid
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
    places3d: { height_property: height }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut geometry_types = HashMap::new();
        geometry_types.insert("footprint".to_string(), "POLYGON".to_string());
        geometry_types.insert("solid".to_string(), "POLYHEDRALSURFACE".to_string());
        let catalog = Arc::new(GeometryTypeCatalog { geometry_types });

        let mut registry = Registry::new();
        registry.register(Arc::new(GeometryTypedFactory {
            tiles,
            volume,
            catalog,
        }));
        let router = Router::build(&config, &registry).unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
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

    /// `#70` deliverable 1: a footprint+height collection sharing a storage
    /// entry with a genuinely solid one must regain the MVT+extrusion
    /// fallback — the whole point of surfacing the per-collection
    /// geometry-type fact instead of trusting the driver-wide `VolumeSource`
    /// signal alone.
    #[tokio::test]
    async fn glb_tile_footprint_collection_regains_extrusion_fallback_despite_a_solid_sibling() {
        let tiles = Arc::new(FakeTileSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));
        let volume = Arc::new(FakeVolumeSource::new());
        // The volume source is driver-wide and would happily answer for ANY
        // collection it's asked about — proving the fallback below comes
        // from `resolve_volume`'s own geometry-type check, not from the
        // fake simply never being consulted.
        volume.set(coord, Some(one_triangle_mesh(99.0)));
        let ctx = test_context_with_volume_geometry_types(Arc::clone(&tiles), Arc::clone(&volume));

        let response = glb_tile(
            State(Arc::clone(&ctx)),
            path("footprint", 2, 1, "1.glb"),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[0..4], &GLB_MAGIC);
        assert_eq!(
            tiles.call_count(),
            1,
            "a flat geometry_type must route this collection through MVT+extrusion"
        );
        assert_eq!(
            volume.call_count(),
            0,
            "the driver-wide volume source must never be consulted for a collection whose own \
             geometry column isn't solid"
        );
    }

    /// Counterpart of the above: the "solid" sibling on the SAME storage
    /// entry must still serve true solid geometry, unaffected by the
    /// footprint collection's own fallback.
    #[tokio::test]
    async fn glb_tile_solid_sibling_still_serves_true_solid_geometry() {
        let tiles = Arc::new(FakeTileSource::new());
        let volume = Arc::new(FakeVolumeSource::new());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        volume.set(coord, Some(one_triangle_mesh(99.0)));
        let ctx = test_context_with_volume_geometry_types(Arc::clone(&tiles), Arc::clone(&volume));

        let response = glb_tile(
            State(Arc::clone(&ctx)),
            path("solid", 2, 1, "1.glb"),
            HeaderMap::new(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let doc = glb_json_chunk(&body);
        assert_eq!(doc["accessors"][0]["max"][2], 99.0);
        assert_eq!(volume.call_count(), 1);
        assert_eq!(
            tiles.call_count(),
            0,
            "a genuinely solid collection must skip MVT+extrusion entirely"
        );
    }

    // -- `#34` authorization policy layer ------------------------------------

    fn headers_with_bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    const AUTH_ONLY_PLACES_CONFIG: &str = r#"
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
    places3d: { height_property: height }
auth:
  bearer_tokens:
    - { token: member-token, tenants: [public] }
"#;

    const RBAC_PLACES_CONFIG: &str = r#"
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
    places3d: { height_property: height }
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
          lanes: [places3d]
    - name: filtered-reader
      grants:
        - scope: { collections: [demo] }
          lanes: [places3d]
          filter: "org = {{claims.org}}"
"#;

    #[tokio::test]
    async fn no_credential_against_a_private_collection_is_401_when_auth_is_configured() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(
            TileCoord { z: 2, x: 1, y: 1 },
            Some(square_polygon_mvt("height", 20.0)),
        );
        let ctx = test_context_with_config(tiles, AUTH_ONLY_PLACES_CONFIG);

        let response = glb_tile(State(ctx), path("demo", 2, 1, "1.glb"), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_tenant_member_reads_glb_tiles_unrestricted_with_no_policy_configured() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(
            TileCoord { z: 2, x: 1, y: 1 },
            Some(square_polygon_mvt("height", 20.0)),
        );
        let ctx = test_context_with_config(tiles, AUTH_ONLY_PLACES_CONFIG);

        let response = glb_tile(
            State(ctx),
            path("demo", 2, 1, "1.glb"),
            headers_with_bearer("member-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tileset_discovery_is_also_gated_by_isolation() {
        let tiles = Arc::new(FakeTileSource::new());
        let ctx = test_context_with_config(tiles, AUTH_ONLY_PLACES_CONFIG);

        let denied = tileset(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            HeaderMap::new(),
            tileset_uri("demo"),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let allowed = tileset(
            State(ctx),
            cid_path("demo"),
            headers_with_bearer("member-token"),
            tileset_uri("demo"),
        )
        .await;
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn subtree_file_is_also_gated_by_isolation() {
        let tiles = Arc::new(FakeTileSource::new());
        let ctx = test_context_with_config(tiles, AUTH_ONLY_PLACES_CONFIG);

        let response = subtree_file(
            State(ctx),
            subtree_path("demo", "0", "0", "0.subtree"),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rbac_active_denies_a_member_with_no_matching_role() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(
            TileCoord { z: 2, x: 1, y: 1 },
            Some(square_polygon_mvt("height", 20.0)),
        );
        let ctx = test_context_with_config(tiles, RBAC_PLACES_CONFIG);

        let response = glb_tile(
            State(ctx),
            path("demo", 2, 1, "1.glb"),
            headers_with_bearer("no-role-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    /// `FakeTileSource::new()` never overrides `filter_capable` (stays at
    /// the trait default, `false`) — a driver that can't compile a filter
    /// still denies a filtered-only grant outright rather than serving
    /// unfiltered (see `authorize_places`'s own doc). A filter-capable
    /// driver's own coverage lives in the `#34` places3d ABAC pushdown
    /// tests further down this file.
    #[tokio::test]
    async fn a_filtered_only_grant_denies_the_places3d_lane_rather_than_serving_unfiltered() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(
            TileCoord { z: 2, x: 1, y: 1 },
            Some(square_polygon_mvt("height", 20.0)),
        );
        let ctx = test_context_with_config(tiles, RBAC_PLACES_CONFIG);

        let response = glb_tile(
            State(ctx),
            path("demo", 2, 1, "1.glb"),
            headers_with_bearer("filtered-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn rbac_active_allows_an_unconditional_grant() {
        let tiles = Arc::new(FakeTileSource::new());
        tiles.set(
            TileCoord { z: 2, x: 1, y: 1 },
            Some(square_polygon_mvt("height", 20.0)),
        );
        let ctx = test_context_with_config(tiles, RBAC_PLACES_CONFIG);

        let response = glb_tile(
            State(ctx),
            path("demo", 2, 1, "1.glb"),
            headers_with_bearer("reader-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    // -- `#34` places3d ABAC pushdown + cache-key fingerprint ----------------

    /// Mirrors `tellurion-tiles::handlers::tests::FILTERED_FINGERPRINT_TILES_CONFIG`.
    const FILTERED_FINGERPRINT_PLACES_CONFIG: &str = r#"
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
    places3d: { height_property: height }
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
          lanes: [places3d]
          filter: "org = {{claims.org}}"
    - name: full-reader
      grants:
        - scope: { collections: [demo] }
          lanes: [places3d]
"#;

    /// A driver that CAN compile a filter must actually be served a
    /// filtered-only grant, and the tile it renders must land in the cache
    /// under a key carrying that filter's own fingerprint.
    #[tokio::test]
    async fn a_filtered_grant_on_a_filter_capable_driver_is_served_and_fingerprints_the_cache_key()
    {
        let tiles = Arc::new(FakeTileSource::with_filter_capable());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));
        let ctx = test_context_with_config(tiles.clone(), FILTERED_FINGERPRINT_PLACES_CONFIG);

        let response = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
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
            .get(&glb_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                Some(expected_fingerprint),
            ))
            .await;
        assert!(
            cached.is_some(),
            "the glb tile must be cached under a key carrying the filter's own fingerprint"
        );
    }

    /// Two different subjects whose grants resolve to the same effective
    /// filter must share one cache entry — the second request never re-hits
    /// the driver.
    #[tokio::test]
    async fn two_subjects_with_the_same_effective_filter_share_one_cache_entry() {
        let tiles = Arc::new(FakeTileSource::with_filter_capable());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));
        let ctx = test_context_with_config(tiles.clone(), FILTERED_FINGERPRINT_PLACES_CONFIG);

        let first = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
            headers_with_bearer("acme-token-a"),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(tiles.call_count(), 1);

        let second = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
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
    /// never collide.
    #[tokio::test]
    async fn subjects_with_different_effective_filters_get_different_cache_entries() {
        let tiles = Arc::new(FakeTileSource::with_filter_capable());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));
        let ctx = test_context_with_config(tiles.clone(), FILTERED_FINGERPRINT_PLACES_CONFIG);

        let acme = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
            headers_with_bearer("acme-token-a"),
        )
        .await;
        assert_eq!(acme.status(), StatusCode::OK);
        assert_eq!(tiles.call_count(), 1);

        let globex = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
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

    /// An unconditional grant must produce the exact same
    /// `policy_fingerprint: None` cache key this lane always built, even
    /// against a filter-capable driver.
    #[tokio::test]
    async fn an_unconditional_grant_keeps_the_pre_policy_cache_key_unchanged() {
        let tiles = Arc::new(FakeTileSource::with_filter_capable());
        let coord = TileCoord { z: 2, x: 1, y: 1 };
        tiles.set(coord, Some(square_polygon_mvt("height", 20.0)));
        let ctx = test_context_with_config(tiles.clone(), FILTERED_FINGERPRINT_PLACES_CONFIG);

        let response = glb_tile(
            State(Arc::clone(&ctx)),
            path("demo", 2, 1, "1.glb"),
            headers_with_bearer("unconditional-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(tiles.last_filter_fingerprint(), None);

        let cached = ctx
            .cache
            .get(&glb_key(
                DEFAULT_TENANT,
                DEFAULT_CATALOG,
                "demo",
                coord,
                None,
            ))
            .await;
        assert!(
            cached.is_some(),
            "unrestricted access must still populate the byte-identical pre-`#34` cache key"
        );
    }
}
