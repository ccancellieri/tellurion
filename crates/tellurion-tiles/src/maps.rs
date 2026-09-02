//! OGC API — Maps Part 1 (OGC 20-058, v1.0), first slice (`#86`): a single
//! styled-image map resource, `GET .../collections/{cid}/map`.
//!
//! Scope:
//! - TWO render lanes over the one `routing.maps` lane, resolved
//!   independently and in this order (`#37`): a VECTOR collection through
//!   `Router::resolve_maps` (a `TileSource`), or — when nothing in that
//!   lane advertises a `TileSource` at all — a RASTER collection (COG,
//!   Zarr) through `Router::resolve_maps_raster` (a `RasterSource`). See
//!   [`raster_populate`] for the raster half; everything below describes
//!   the vector half unless it says otherwise. Both halves share this
//!   module's parameter parsing, pixel budgets, cache seam and response
//!   headers verbatim — there is exactly one `/map` contract, served two
//!   ways.
//! - Vector collections are rendered from the SAME MVT-first pipeline the
//!   PNG/styled-PNG tile lanes use — a requested `bbox`/`width`/`height`
//!   window is covered by one or more `WebMercatorQuad` tiles (chosen at
//!   whichever zoom level's own resolution best matches the request, see
//!   `crate::mercator::pick_zoom`), each fetched through
//!   [`tellurion_core::AppContext::fetch_mvt`] — the exact same cached,
//!   single-flighted call `crate::handlers::tile`'s own PNG branch makes —
//!   then composited onto one shared output canvas
//!   (`tellurion_render::render_map_window`/`render_map_window_styled`).
//!   No GetFeatureInfo, legends, or multi-collection composites.
//! - Raster collections (`#37`) are composited from the SAME
//!   `RasterSource::raster_tile` call the raster PNG TILE lane makes
//!   (`crate::handlers::raster_tile_response`), one call per covering
//!   `WebMercatorQuad` tile, onto one shared output canvas
//!   (`tellurion_render::render_raster_map_window`, which ends in the same
//!   `encode_rgba_to_png` that lane ends in). Every budget that bounds a
//!   raster TILE therefore bounds a raster MAP unchanged, per covering
//!   tile: the driver's own source-pixel budget, its decode path, and its
//!   remote-read timeout. The collection's validated
//!   `settings.colormap` — and, for Zarr, its array's own fixed
//!   leading-dimension slice — are applied INSIDE that same driver call,
//!   never re-implemented here. A `style` parameter is refused by name on
//!   this lane: a MapLibre style document paints MVT layers, of which a
//!   raster collection has none.
//! - `crs`/`bbox-crs` accept exactly two CRSs: the `WebMercatorQuad` tile
//!   matrix's own CRS (EPSG:3857) and CRS84 (WGS84 longitude/latitude) — the
//!   two-CRS shape `tellurion_core::crs::RequestedCrs` already gives the
//!   features lane, fixed here to the tile grid's CRS instead of a
//!   collection's storage SRID (see [`tellurion_core::MapCrs`]). Their
//!   OMITTED defaults differ, because Maps Part 1 gives them different
//!   ones (`#270`): an omitted `bbox-crs` is CRS84 (Requirement 18 clause
//!   C), an omitted `crs` is this lane's own native CRS (Requirement 35
//!   NOTE 2). See [`parse_crs`], and [`refuse_undeclared_bbox_crs`] for the
//!   guard that keeps the first from silently re-reading a metres `bbox` as
//!   degrees. A `bbox-crs` supplied WITHOUT a `bbox` is ignored — but its
//!   value is still validated (`#291`, the register entry in
//!   `docs/spec-deviations.md`).
//! - PNG output only.
//! - The same style-selection idiom the styled-PNG tile lane uses: an
//!   optional `style` query parameter names a registered MapLibre style
//!   document (`ctx.style_store`); its id joins the cache key exactly the
//!   way `Encoding::PngStyled`'s own style id does (see
//!   [`tellurion_core::Encoding::Map`]'s own doc). Omitted, the collection's
//!   own `StyleConf` (`decl.style`) paints every layer, matching the
//!   unstyled PNG tile lane's own default.
//! - The render is single-flighted and cached through the SAME
//!   byte-budgeted `Arc<dyn TileCache>` every other tile-shaped entry in
//!   this workspace shares (`ctx.get_or_populate`), keyed by
//!   `Encoding::Map` — never a second, separately-budgeted cache. That key
//!   carries which of the two lanes rendered it, and (for the raster lane)
//!   the collection's colormap fingerprint, so a vector render and a raster
//!   render of the same window — and two colormap configurations of the
//!   same raster window — can never answer for one another
//!   ([`tellurion_core::MapLane`]).
//! - The requested OUTPUT image size, and the SOURCE tile count the request
//!   would need to rasterize, are each checked against
//!   [`MAX_MAP_PIXELS`] before any driver call — refused BY NAME
//!   (`"PixelBudgetExceeded"`, the same code the raster tile lane already
//!   uses for its own per-request pixel budget), never silently clamped.
//!
//! `#229`: every query parameter is optional, as Maps Part 1's own Core
//! class requires (`/req/core/map-op` — Core is defined by the request that
//! constrains nothing, and the subsetting/scaling parameters belong to
//! their own classes). Nothing here is defaulted to a convention:
//! - An omitted `bbox` resolves to THIS COLLECTION'S OWN spatial extent,
//!   read off the same `CanonicalDescriptor` `/collections/{cid}` publishes
//!   ([`collection_window`]) — a fact already derived from the data, not an
//!   invented window. A collection whose extent is unknown (or degenerate)
//!   is refused BY NAME (`"CapabilityUnsupported"`, the shape
//!   `crate::handlers::refuse_tile_matrix_set` uses) rather than served a
//!   world bbox it never asked for.
//! - Omitted `width`/`height` resolve to the window's own footprint at the
//!   `WebMercatorQuad` grid's native scale
//!   ([`mercator::native_resolution_m_per_px`]) — again derived, from the
//!   tile matrix set's own published `cellSize` progression and the window
//!   itself, and always aspect-ratio-exact. One supplied without the other
//!   derives its partner from the window's aspect ratio.
//! - Every 200 carries `Content-Crs` and `Content-Bbox`
//!   (`/req/core/map-response` C/D/E) naming the CRS the image was rendered
//!   in and the window it actually covers — the only way a client that
//!   supplied no parameters can georeference what it got back.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;

use tellurion_core::query_params::parse_bbox;
use tellurion_core::{
    AppContext, CollectionDecl, Encoding, Error, Filter, MapCrs, MapLane, MvtFetch, PopulateFuture,
    RasterSource, RasterWindow, TileCoord, TileKey, TileSource, CRS84_URI,
};
use tellurion_render::{
    render_map_window, render_map_window_styled, render_raster_map_window, resolve_layer_paints,
    MapTile, RasterMapTile, RenderStyle,
};

use crate::handlers::{
    authorize_tiles, catalog_of, problem_response, tenant_of, DEFAULT_POINT_RADIUS_PX, PNG_MIME,
};
use crate::mercator;
use crate::tilematrixset::{TILE_SIZE_PX, WEB_MERCATOR_QUAD_CRS};

/// Hard per-request cap this lane checks BOTH the requested OUTPUT image's
/// pixel count (`width * height`) and the effective SOURCE pixel count
/// against (`covering tile count * TILE_SIZE_PX^2` — see [`map`]'s own
/// covering-tiles check, which exists because a request whose output is
/// small but whose bbox spans a huge area at a fine zoom can need to
/// rasterize far more source pixels than its own output ever shows). Same
/// order of magnitude as the raster (COG) tile lane's own per-request
/// SOURCE-pixel budget (`tellurion-cog::driver::MAX_SOURCE_PIXELS`,
/// ~4,000,000 = a 2000x2000 window) — duplicated here, not imported:
/// `tellurion-tiles` is a protocol crate and never depends on a driver
/// crate (drivers sit below protocol crates in this workspace's dependency
/// order — see `docs/driver-authoring.md`), the same "duplicate the
/// constant, cross-reference the sibling in a doc comment" convention
/// `tellurion-cog`'s own `DEST_TILE_SIZE_PX` already follows for this
/// crate's `RENDER_TILE_SIZE_PX`. Refused by name, never silently clamped.
const MAX_MAP_PIXELS: u64 = 4_000_000;

/// `Content-Crs` (Maps Part 1 `/req/core/map-response` C, same header and
/// same `"<" URI ">"` value shape `tellurion-features` already sets for OGC
/// API Features Part 2's own Requirement 15/16) — the CRS this image was
/// actually rendered in. Always sent, including for CRS84 content, which
/// the standard only exempts (`/rec/core/content-crs` recommends sending it
/// anyway).
const CONTENT_CRS_HEADER: HeaderName = HeaderName::from_static("content-crs");
/// `Content-Bbox` (Maps Part 1 `/req/core/map-response` D/E) — the window
/// this image actually covers, four comma-separated numbers in the response
/// CRS (`Content-Crs`), lower-left then upper-right. The only way a client
/// that supplied no `bbox` can learn which window it was given.
const CONTENT_BBOX_HEADER: HeaderName = HeaderName::from_static("content-bbox");

/// A parsed, validated `/collections/{cid}/map` request. `bbox_mercator` is
/// already normalized to `WebMercatorQuad`-native meters regardless of
/// which `bbox-crs` the client supplied (`parse_request`'s own doc) — the
/// single canonical form both tile selection and the cache key use, so two
/// requests naming the same geographic window through different `bbox-crs`
/// values collide into the same cache entry instead of two.
struct MapRequest {
    bbox_mercator: [f64; 4],
    /// The requested OUTPUT image's CRS — independent of `bbox-crs`.
    crs: MapCrs,
    width: u32,
    height: u32,
    style_id: Option<String>,
}

/// Parses and validates every `/collections/{cid}/map` query parameter,
/// returning a boxed [`Response`] (never a bare one — clippy's
/// `result_large_err` flags an un-boxed `Response` in a `Result`'s `Err`
/// arm, the same reason `crate::handlers::TileCoordError` exists as its own
/// small type for the tile lane's own coordinate parsing) for the first
/// named refusal encountered.
///
/// `#229`: every parameter is optional. `collection_window` is this
/// collection's own extent in `WebMercatorQuad` meters — the window an
/// omitted `bbox` resolves to, `None` when the collection has no known
/// extent to derive one from (the caller only computes it when `bbox` was
/// omitted at all; see [`collection_window`]). `cid` is the EXTERNAL
/// collection id, used only to name the collection in that refusal.
fn parse_request(
    query: &HashMap<String, String>,
    cid: &str,
    collection_window: Option<[f64; 4]>,
) -> Result<MapRequest, Box<Response>> {
    // `#270`: whether `bbox-crs` was DECLARED, not just what it resolved
    // to — an omitted one resolves to CRS84 (Requirement 18 clause C) and
    // is the only case [`refuse_undeclared_bbox_crs`] guards.
    let bbox_crs_raw = query.get("bbox-crs").map(String::as_str);
    let bbox_crs = parse_crs(bbox_crs_raw, MapCrs::Crs84)?;
    let crs = parse_crs(query.get("crs").map(String::as_str), MapCrs::WebMercator)?;
    let bbox_mercator = match query.get("bbox") {
        Some(bbox_raw) => {
            let bbox = parse_bbox(bbox_raw)
                .map_err(|error| bad_request("InvalidParameter", error.to_string()))?;
            if bbox[0] >= bbox[2] || bbox[1] >= bbox[3] {
                return Err(bad_request(
                    "InvalidParameter",
                    "'bbox' minimum must be less than its maximum on each axis",
                ));
            }
            match bbox_crs {
                MapCrs::WebMercator => bbox,
                MapCrs::Crs84 => {
                    if bbox_crs_raw.is_none() {
                        refuse_undeclared_bbox_crs(bbox)?;
                    }
                    let (minx, miny) = mercator::forward(bbox[0], bbox[1]);
                    let (maxx, maxy) = mercator::forward(bbox[2], bbox[3]);
                    [minx, miny, maxx, maxy]
                }
            }
        }
        // No `bbox`: this collection's own derived extent, or a named
        // refusal — never a world bbox nobody asked for. A `bbox-crs`
        // supplied here has nothing to qualify and is IGNORED: the
        // response is byte-for-byte the one the same request without it
        // gets, and — `bbox_mercator` being the only thing this arm
        // produces — nothing it could carry reaches the render or the
        // cache key ([`map_key`]). Its VALUE was still validated above,
        // though. OGC 20-058 contradicts itself on this case (Requirement
        // 18 clause F: the parameter "SHALL be ignored"; §13.5: an
        // unsupported CRS in it "will be 400", stated unconditionally),
        // and the recorded decision (`#291`, `docs/spec-deviations.md`) is
        // to ignore the parameter's EFFECT while keeping §13.5's named
        // refusal of a value this server could never serve — ignoring an
        // unused parameter is not accepting a nonsense one.
        None => collection_window.ok_or_else(|| refuse_unknown_extent(cid))?,
    };
    let (width, height) = parse_dimensions(query, bbox_mercator)?;
    if u64::from(width) * u64::from(height) > MAX_MAP_PIXELS {
        return Err(bad_request(
            "PixelBudgetExceeded",
            format!(
                "requested image is {width}x{height} ({} pixels), over this server's {MAX_MAP_PIXELS}-pixel budget",
                u64::from(width) * u64::from(height)
            ),
        ));
    }
    let style_id = query.get("style").cloned();

    Ok(MapRequest {
        bbox_mercator,
        crs,
        width,
        height,
        style_id,
    })
}

/// The `#229` capability-honesty refusal for a `bbox`-less request against a
/// collection whose own spatial extent is unknown — same
/// `CapabilityUnsupported` shape (and same "name the collection, name the
/// capability, name the lane") `crate::handlers::refuse_tile_matrix_set`
/// uses. A world bbox would be an invented answer: this lane would happily
/// render one, and the client would have no way to tell that image apart
/// from a real extent-wide render.
fn refuse_unknown_extent(cid: &str) -> Box<Response> {
    bad_request(
        "CapabilityUnsupported",
        format!(
            "collection '{cid}' does not support capability 'default-extent': no spatial extent is derived for it, so a request with no 'bbox' names no window to render — supply 'bbox'"
        ),
    )
}

/// The output image's size in pixels for one request, over the already
/// resolved `bbox_mercator` window (`#229`).
///
/// Both supplied is the pre-`#229` behavior, unchanged. One supplied
/// derives its partner from the window's own aspect ratio. Neither supplied
/// renders the window at the tile grid's native scale
/// ([`mercator::native_resolution_m_per_px`]) — the honest "nothing was
/// requested" size, since it is the resolution at which the pyramid this
/// lane composites from already stores the window, rather than a
/// convention. Every derived side is at least one pixel; a derived size
/// over [`MAX_MAP_PIXELS`] is refused by the caller's own budget check
/// exactly like a requested one (only reachable via the one-supplied arm —
/// a fully derived window is at most `2 * TILE_SIZE_PX` on its longest
/// side).
fn parse_dimensions(
    query: &HashMap<String, String>,
    bbox_mercator: [f64; 4],
) -> Result<(u32, u32), Box<Response>> {
    let width = parse_dimension(query.get("width"), "width")?;
    let height = parse_dimension(query.get("height"), "height")?;
    let span_x = bbox_mercator[2] - bbox_mercator[0];
    let span_y = bbox_mercator[3] - bbox_mercator[1];
    Ok(match (width, height) {
        (Some(width), Some(height)) => (width, height),
        (Some(width), None) => (width, pixels(f64::from(width) * span_y / span_x)),
        (None, Some(height)) => (pixels(f64::from(height) * span_x / span_y), height),
        (None, None) => {
            let resolution =
                mercator::native_resolution_m_per_px(span_x.max(span_y)).ok_or_else(|| {
                    bad_request(
                        "InvalidParameter",
                        "'bbox' spans no area, so no output size can be derived from it",
                    )
                })?;
            (pixels(span_x / resolution), pixels(span_y / resolution))
        }
    })
}

/// Rounds one derived pixel count to a whole, non-zero, in-range `u32` — a
/// window far thinner than it is wide still gets a one-pixel-tall image
/// rather than a zero-sized (undecodable) one.
fn pixels(value: f64) -> u32 {
    if !value.is_finite() {
        return 1;
    }
    value.round().clamp(1.0, f64::from(u32::MAX)) as u32
}

/// The CRS84 coordinate ranges Requirement 18 clause C's assumed CRS is
/// defined over — longitude `[-180, 180]`, latitude `[-90, 90]`. Used ONLY
/// by [`refuse_undeclared_bbox_crs`]; the bounds are inclusive, so
/// `bbox=-180,-90,180,90` (the whole world, a perfectly ordinary CRS84
/// request) is inside them.
const CRS84_MAX_LON_DEG: f64 = 180.0;
const CRS84_MAX_LAT_DEG: f64 = 90.0;

/// `crs`/`bbox-crs` accept exactly [`WEB_MERCATOR_QUAD_CRS`] (the tile
/// matrix's own CRS) or [`CRS84_URI`] — see this module's own doc for why
/// only these two. `omitted` is what an ABSENT parameter resolves to, and
/// it differs per parameter because Maps Part 1 gives the two different
/// defaults (`#270`):
///
/// - `bbox-crs` omitted is CRS84. Requirement 18
///   (`/req/spatial-subsetting/bbox-crs`) clause C, verbatim from OGC
///   20-058 §13.2.1: "If the bbox-crs is not indicated
///   `https://www.opengis.net/def/crs/OGC/1.3/CRS84` SHALL be assumed."
/// - `crs` omitted is this lane's own native CRS, the `WebMercatorQuad`
///   pyramid every map here composites from. Requirement 35
///   (`/req/crs/crs-definition`) NOTE 2, verbatim from §16.2.1: "The
///   default CRS of the BBOX is
///   `https://www.opengis.net/def/crs/OGC/1.3/CRS84` but the default CRS of
///   the map is the native (storage) CRS." Requirement 18's own NOTE 1 says
///   the same thing from the other side. So this is NOT one default applied
///   inconsistently — the standard asks for exactly these two, and `#270`
///   changed only the first.
///
/// `#291`: [`parse_request`] calls this for `bbox-crs` BEFORE looking at
/// whether `bbox` was supplied at all, deliberately — the value of a
/// `bbox`-less `bbox-crs` is validated (an undeclared CRS is this named
/// refusal either way) even though its effect is then ignored. See the
/// `None` arm of `parse_request`'s `bbox` match and
/// `docs/spec-deviations.md` for the clause pair that decision resolves.
fn parse_crs(raw: Option<&str>, omitted: MapCrs) -> Result<MapCrs, Box<Response>> {
    match raw {
        None => Ok(omitted),
        Some(uri) if uri == WEB_MERCATOR_QUAD_CRS => Ok(MapCrs::WebMercator),
        Some(uri) if uri == CRS84_URI => Ok(MapCrs::Crs84),
        Some(uri) => Err(bad_request(
            "CrsNotSupported",
            format!(
                "unsupported crs '{uri}': this server supports '{WEB_MERCATOR_QUAD_CRS}' or '{CRS84_URI}'"
            ),
        )),
    }
}

/// The guard that pairs with `#270`'s change of the omitted-`bbox-crs`
/// default: a `bbox` supplied WITHOUT a `bbox-crs`, whose coordinates fall
/// outside the CRS84 ranges Requirement 18 clause C makes them degrees in,
/// is refused BY NAME instead of interpreted.
///
/// Before `#270` this lane read an omitted `bbox-crs` as its own
/// `WebMercatorQuad` CRS, so a client sending metres without declaring
/// `bbox-crs` got the window it meant. Under clause C those same numbers
/// are degrees. Without this guard such a client would keep getting a
/// `200`, carrying a wildly different window, with nothing in the response
/// saying so — the exact silent-degradation shape this lane refuses
/// everywhere else (`MAX_MAP_PIXELS`, [`refuse_unknown_extent`]). With it,
/// the one class of client the default change breaks gets a `400` naming
/// the parameter to add.
///
/// The refusal is authorized, not merely pragmatic: OGC 20-058 §13.5
/// ("Error conditions", Spatial Subsetting) says verbatim "If the CRS in
/// the parameter value bbox-crs, subset-crs or center-crs is not supported
/// by the server for this resource, or the parameter value is
/// out-of-range, the status code of the response will be 400." Clamping
/// instead would also be allowed (Permission 4,
/// `/per/spatial-subsetting/map-outside-bounds`, a MAY) — nothing in the
/// document requires the out-of-range case to be processed rather than
/// refused. Clause C is still honoured: the parameter IS assumed to be
/// CRS84, and it is because it is CRS84 that these coordinates are invalid.
///
/// ## The undetectable region, measured rather than assumed
///
/// A `bbox` in `WebMercatorQuad` metres escapes this guard only when all
/// four of its coordinates already sit inside ±180 / ±90 — a 360 m × 180 m
/// rectangle of open ocean centred on 0°N 0°E (±0.001617° lon,
/// ±0.000808° lat).
///
/// `#270`'s issue thread predicted that inside that rectangle "both
/// readings agree to within the pixel". Measured, they do not — anywhere,
/// at any size. Reading the same four numbers as degrees rather than
/// metres multiplies the window's x span by
/// [`mercator::earth_radius_m`]`() * PI / 180 = 20037508.34 / 180 =`
/// **111319.49×**, and that factor is exactly SCALE-INVARIANT: longitude
/// projects linearly, so a 2 mm-wide window and the 360 m-wide largest one
/// that escapes have their x span off by the same 111319.49×. On y the
/// error is
/// never smaller and grows with latitude, because [`mercator::forward`]'s
/// own `ln(tan(...))` is convex away from the equator — 128925.52× at
/// ±50°, and unbounded at ±90 (where `forward` is deliberately unclamped
/// and answers ±inf).
///
/// Worked, at a realistic output size: `bbox=-50,-50,50,50` at 256×256 px
/// is a 100 m × 100 m window read as metres (0.39 m/px) and an
/// 11131.9 km × 12892.6 km window read as degrees (43.5 km/px). Not one
/// pixel of agreement — five orders of magnitude apart.
///
/// So what bounds the residual risk is improbability, not agreement: the
/// misread survives only for a request whose window lies entirely inside
/// that 0.065 km² patch of open ocean in the Gulf of Guinea. That is a
/// real, named limit of this guard, and a different (weaker) argument than
/// the one `#270` was decided on — recorded here rather than left as the
/// claim it replaced.
fn refuse_undeclared_bbox_crs(bbox: [f64; 4]) -> Result<(), Box<Response>> {
    let in_crs84_range = bbox[0] >= -CRS84_MAX_LON_DEG
        && bbox[2] <= CRS84_MAX_LON_DEG
        && bbox[1] >= -CRS84_MAX_LAT_DEG
        && bbox[3] <= CRS84_MAX_LAT_DEG;
    if in_crs84_range {
        return Ok(());
    }
    Err(bad_request(
        "BboxCrsRequired",
        format!(
            "'bbox' was supplied without 'bbox-crs', so its coordinates are read as \
             '{CRS84_URI}' degrees (OGC 20-058 Requirement 18 clause C), but \
             [{}, {}, {}, {}] falls outside the CRS84 ranges \
             (±{CRS84_MAX_LON_DEG} longitude, ±{CRS84_MAX_LAT_DEG} latitude) — \
             supply 'bbox-crs={WEB_MERCATOR_QUAD_CRS}' if these are \
             WebMercatorQuad metres, or 'bbox-crs={CRS84_URI}' with degrees",
            bbox[0], bbox[1], bbox[2], bbox[3]
        ),
    ))
}

/// One optional output dimension (`#229`: optional, per Maps Part 1 — an
/// absent one is derived by [`parse_dimensions`], never refused). A
/// PRESENT one is validated exactly as before: a non-integer or zero value
/// is still a named refusal, never silently replaced by a derived value.
fn parse_dimension(raw: Option<&String>, name: &str) -> Result<Option<u32>, Box<Response>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value: u32 = raw.parse().map_err(|_| {
        bad_request(
            "InvalidParameter",
            format!("'{name}' must be a positive integer"),
        )
    })?;
    if value == 0 {
        return Err(bad_request(
            "InvalidParameter",
            format!("'{name}' must be greater than zero"),
        ));
    }
    Ok(Some(value))
}

/// The URI naming one [`MapCrs`] — the two this lane accepts on `crs`/
/// `bbox-crs` ([`parse_crs`]), reported back on `Content-Crs`.
fn crs_uri(crs: MapCrs) -> &'static str {
    match crs {
        MapCrs::WebMercator => WEB_MERCATOR_QUAD_CRS,
        MapCrs::Crs84 => CRS84_URI,
    }
}

fn bad_request(code: &str, detail: impl Into<String>) -> Box<Response> {
    Box::new(problem_response(StatusCode::BAD_REQUEST, code, detail))
}

/// This request's cache key (`#86`): every parameter that changes the
/// rendered bytes lives on [`Encoding::Map`] itself, `z`/`x`/`y` unused —
/// see that variant's own doc. Reuses the SAME byte-budgeted cache every
/// other tile-shaped entry in this workspace shares.
///
/// `lane` (`#37`) is the render lane that will produce these bytes, and for
/// the raster lane the collection's resolved colormap fingerprint. It is
/// part of the key because it changes the image: the same collection id,
/// window and output size rendered from MVT and rendered from decoded
/// raster samples are two different pictures, and so are two colormap
/// configurations of the same raster window. Without it the first lane to
/// render would answer for the other out of one shared cache.
fn map_key(
    tenant: &str,
    catalog: &str,
    collection: &str,
    request: &MapRequest,
    lane: MapLane,
    policy_fingerprint: Option<u64>,
    properties: Vec<String>,
) -> TileKey {
    TileKey {
        tenant: tenant.to_string(),
        catalog: catalog.to_string(),
        collection: collection.to_string(),
        z: 0,
        x: 0,
        y: 0,
        encoding: Encoding::Map {
            crs: request.crs,
            bbox: request.bbox_mercator.map(f64::to_bits),
            width: request.width,
            height: request.height,
            style: request.style_id.clone(),
            lane,
        },
        // `#190`: fixed — a map window composes from the WebMercatorQuad
        // pyramid regardless of its output `crs`; see `TileKey::tms`'s doc.
        tms: tellurion_core::TileMatrixSet::WebMercatorQuad,
        policy_fingerprint,
        properties,
        // `#113`: `0` here — the real caller (`map`, this module's own
        // handler) overrides it via struct-update syntax after calling in
        // here, the same "pure function defaults to 0" shape `context::
        // mvt_key` uses.
        generation: 0,
    }
}

/// Which of the two capabilities this collection's `routing.maps` lane
/// actually resolved to (`#37`) — the one place the rest of this module
/// branches on, and the discriminator that reaches the cache key as
/// [`MapLane`].
enum MapSource {
    /// A vector `TileSource`: the window is rasterized from covering MVT
    /// tiles (`Router::resolve_maps`).
    Vector(Arc<dyn TileSource>),
    /// A `RasterSource`: the window is composited from decoded raster
    /// windows (`Router::resolve_maps_raster`, `#37`).
    Raster(Arc<dyn RasterSource>),
}

/// A collection resolved through this request's `(tenant, catalog, cid)`
/// path segments, over the maps routing lane (`Router::resolve_maps` /
/// `Router::resolve_maps_raster`) — the maps-lane counterpart of
/// `crate::handlers`'s own `ResolvedTiles`/`ResolvedRaster` pair, kept as
/// ONE type carrying a [`MapSource`] rather than two, because every step
/// between resolution and the cache-populate closure (authorization, window
/// derivation, parameter parsing, zoom choice, covering tiles, both pixel
/// budgets, the key, the response headers) is identical for the two lanes
/// and must stay that way.
struct ResolvedMaps {
    tenant_id: String,
    catalog_id: String,
    collection_id: String,
    decl: CollectionDecl,
    source: MapSource,
}

/// Resolves this request's collection over the maps lane, VECTOR first and
/// RASTER only if the vector capability is absent (`#37`).
///
/// The order, and the "only pay for the second probe when the first
/// refused" shape, are exactly `crate::handlers::tile`'s own
/// `resolve_tiles`-then-`resolve_raster` order — the same rule stated once
/// more here rather than a new one: a collection that has a `TileSource`
/// keeps rendering the way it always did, byte for byte, and a
/// raster-backed collection that used to 404 on this route now resolves.
///
/// `None` — neither capability, or an unknown tenant/catalog/collection —
/// is the caller's 404, unchanged from before this slice. That is
/// deliberate and not a missed named refusal: `resolve_*` returning `Err`
/// is a 404 on EVERY lane in this crate (see `handlers::tile`), and this
/// slice does not move a code an existing deployment already depends on.
/// The named refusals this lane owns are the ones a RESOLVED collection can
/// still earn — an unrenderable window, an unhonourable parameter, an
/// exceeded budget.
async fn resolve_maps(
    ctx: &AppContext,
    params: &HashMap<String, String>,
    cid: &str,
) -> Option<ResolvedMaps> {
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
    let (decl, source) = match state
        .router
        .resolve_maps(&tenant_id, &catalog_id, &collection_id)
        .await
    {
        Ok((decl, source)) => (decl, MapSource::Vector(source)),
        Err(_) => {
            let (decl, source) = state
                .router
                .resolve_maps_raster(&tenant_id, &catalog_id, &collection_id)
                .await
                .ok()?;
            (decl, MapSource::Raster(source))
        }
    };
    Some(ResolvedMaps {
        tenant_id,
        catalog_id,
        collection_id,
        decl,
        source,
    })
}

/// This collection's own spatial extent, in `WebMercatorQuad` meters — the
/// window a request that supplied no `bbox` renders (`#229`).
///
/// Read off `Router::canonical_descriptor`, the SAME merged descriptor
/// `/collections/{cid}` publishes its `extent.spatial.bbox` from, so a
/// parameterless map can never disagree with the extent the collection
/// metadata advertises. That bbox is CRS84 (`tellurion_core::SpatialExtent`
/// own doc) and comes from real data, not the projection — a collection
/// reaching past the Web Mercator latitude bound is clamped to it
/// ([`mercator::MAX_LATITUDE_DEG`]) rather than projected to infinity, the
/// same bound every `WebMercatorQuad` tile of that collection is already
/// clipped to.
///
/// `None` — no descriptor, no extent, or an extent that projects to no area
/// at all (a single-point collection) — is the honest "this collection has
/// no window to default to" answer; the caller turns it into a named
/// refusal ([`refuse_unknown_extent`]). Never a substituted world bbox.
async fn collection_window(
    ctx: &AppContext,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
) -> Option<[f64; 4]> {
    let descriptor = ctx
        .current()
        .router
        .canonical_descriptor(tenant_id, catalog_id, collection_id)
        .await
        .ok()?;
    let [min_lon, min_lat, max_lon, max_lat] = descriptor.extent?.bbox;
    let (minx, miny) = mercator::forward(
        min_lon.clamp(-180.0, 180.0),
        min_lat.clamp(-mercator::MAX_LATITUDE_DEG, mercator::MAX_LATITUDE_DEG),
    );
    let (maxx, maxy) = mercator::forward(
        max_lon.clamp(-180.0, 180.0),
        max_lat.clamp(-mercator::MAX_LATITUDE_DEG, mercator::MAX_LATITUDE_DEG),
    );
    let window = [minx, miny, maxx, maxy];
    if window.iter().any(|value| !value.is_finite()) || minx >= maxx || miny >= maxy {
        return None;
    }
    Some(window)
}

/// The mercator-meters bbox of the request's OUTPUT canvas, reprojected
/// into `crs`'s own units when `crs` differs from the native
/// `WebMercatorQuad` CRS — a pure function of `bbox_mercator`/`crs`, so it
/// is never itself part of the cache key (see [`map_key`]'s own doc).
fn output_bbox(bbox_mercator: [f64; 4], crs: MapCrs) -> [f64; 4] {
    match crs {
        MapCrs::WebMercator => bbox_mercator,
        MapCrs::Crs84 => {
            let (minlon, minlat) = mercator::inverse(bbox_mercator[0], bbox_mercator[1]);
            let (maxlon, maxlat) = mercator::inverse(bbox_mercator[2], bbox_mercator[3]);
            [minlon, minlat, maxlon, maxlat]
        }
    }
}

/// Builds the per-vertex projection for one covering tile (`tile_bounds_m`,
/// mercator meters) into the shared output canvas: tile-local `[0, 1]`
/// normalized coordinates (see [`tellurion_render`]'s own `paint_mvt_onto`
/// doc) to destination pixel coordinates, reprojecting through
/// [`mercator::inverse`] first when the OUTPUT crs is CRS84.
fn build_projector(
    tile_bounds_m: [f64; 4],
    crs: MapCrs,
    bbox_out: [f64; 4],
    width: u32,
    height: u32,
) -> impl Fn(f64, f64) -> (f32, f32) {
    let [tminx, tminy, tmaxx, tmaxy] = tile_bounds_m;
    let tile_w = tmaxx - tminx;
    let tile_h = tmaxy - tminy;
    let [ominx, ominy, omaxx, omaxy] = bbox_out;
    let out_w = omaxx - ominx;
    let out_h = omaxy - ominy;
    let width = f64::from(width);
    let height = f64::from(height);
    move |nx: f64, ny: f64| {
        let mx = tminx + nx * tile_w;
        let my = tmaxy - ny * tile_h;
        let (ox, oy) = match crs {
            MapCrs::WebMercator => (mx, my),
            MapCrs::Crs84 => mercator::inverse(mx, my),
        };
        let px = ((ox - ominx) / out_w * width) as f32;
        let py = ((omaxy - oy) / out_h * height) as f32;
        (px, py)
    }
}

/// The half-open destination pixel rectangle `[x0, y0, x1, y1)` one covering
/// tile can write into, clamped to the output canvas (`#37`).
///
/// Exists so compositing N covering tiles costs one pass over the canvas
/// between them rather than N full passes: `tellurion_render::
/// render_raster_map_window` only samples inside this rectangle.
///
/// Two opposite corners bound the tile exactly, with no need to project all
/// four: both mappings from mercator metres into the output CRS are
/// strictly increasing on their own axis (longitude is linear in mercator
/// X, and [`mercator::inverse`]'s latitude is monotonic in mercator Y).
/// Rounded OUTWARDS (floor the low corner, ceil the high one) so no
/// destination pixel whose centre falls inside the tile is missed at a
/// seam; a pixel that lands just outside is rejected by the sampler itself,
/// which returns normalized coordinates outside `[0, 1)` for it.
fn dest_rect(
    tile_bounds_m: [f64; 4],
    crs: MapCrs,
    bbox_out: [f64; 4],
    width: u32,
    height: u32,
) -> [u32; 4] {
    let [tminx, tminy, tmaxx, tmaxy] = output_bbox(tile_bounds_m, crs);
    let [ominx, ominy, omaxx, omaxy] = bbox_out;
    let out_w = omaxx - ominx;
    let out_h = omaxy - ominy;
    if !(out_w.is_finite() && out_h.is_finite()) || out_w <= 0.0 || out_h <= 0.0 {
        return [0, 0, 0, 0];
    }
    let to_px = |offset: f64, span: f64, extent: u32| {
        (offset / span * f64::from(extent)).clamp(0.0, f64::from(extent))
    };
    let x0 = to_px(tminx - ominx, out_w, width).floor() as u32;
    let x1 = to_px(tmaxx - ominx, out_w, width).ceil() as u32;
    // Rows run southward from the window's north edge, so the tile's HIGH
    // y bounds its LOW row — the same axis flip `build_projector` makes.
    let y0 = to_px(omaxy - tmaxy, out_h, height).floor() as u32;
    let y1 = to_px(omaxy - tminy, out_h, height).ceil() as u32;
    [x0.min(width), y0.min(height), x1.min(width), y1.min(height)]
}

/// The exact inverse of [`build_projector`] for the raster lane (`#37`):
/// a destination pixel CENTRE, in output pixel coordinates, back to the
/// covering tile's own normalized `[0, 1]` tile-local space.
///
/// The vector lane pushes geometry forward (tile-local -> pixels) because
/// it draws vertices; the raster lane pulls samples backward (pixels ->
/// tile-local) because it fills pixels. Same two coordinate systems, same
/// two CRS cases, opposite direction — written out rather than derived
/// numerically so the raster lane's georeferencing is checkable against the
/// vector lane's line by line.
fn build_sampler(
    tile_bounds_m: [f64; 4],
    crs: MapCrs,
    bbox_out: [f64; 4],
    width: u32,
    height: u32,
) -> impl Fn(f64, f64) -> (f64, f64) {
    let [tminx, tminy, tmaxx, tmaxy] = tile_bounds_m;
    let tile_w = tmaxx - tminx;
    let tile_h = tmaxy - tminy;
    let [ominx, ominy, omaxx, omaxy] = bbox_out;
    let out_w = omaxx - ominx;
    // A destination row is measured DOWN from the window's north edge,
    // exactly as `build_projector` measures it.
    let out_h = omaxy - ominy;
    let width = f64::from(width);
    let height = f64::from(height);
    move |px: f64, py: f64| {
        let ox = ominx + px / width * out_w;
        let oy = omaxy - py / height * out_h;
        let (mx, my) = match crs {
            MapCrs::WebMercator => (ox, oy),
            MapCrs::Crs84 => mercator::forward(ox, oy),
        };
        ((mx - tminx) / tile_w, (tmaxy - my) / tile_h)
    }
}

/// The `#37` named refusal for a `style` parameter on the RASTER lane.
///
/// A MapLibre style document assigns paint to MVT `source-layer` names
/// (`tellurion_render::resolve_layer_paints`), and a raster collection has
/// no MVT layers at all — so honouring `style` here is not something this
/// lane does badly, it is something it cannot do. Ignoring the parameter
/// and returning the unstyled image would be a 200 the client cannot tell
/// apart from a styled one, which is exactly the silent degradation this
/// workspace refuses; the same `CapabilityUnsupported` shape
/// `crate::handlers::refuse_tile_matrix_set` uses says so instead.
fn refuse_raster_style(cid: &str, style_id: &str) -> Response {
    problem_response(
        StatusCode::BAD_REQUEST,
        "CapabilityUnsupported",
        format!(
            "collection '{cid}' is served by a raster source, which does not support capability 'styled-map': a style document paints vector tile layers and this collection has none, so style '{style_id}' cannot be applied — omit 'style'"
        ),
    )
}

/// The 200 for one rendered window (`#229`): the PNG bytes plus the two
/// headers Maps Part 1's Core class requires of every map response —
/// `Content-Crs` (the CRS the image is in) and `Content-Bbox` (the window
/// it actually covers, in that CRS). Both are pure functions of the already
/// resolved [`MapRequest`], so a cache HIT carries them exactly like a
/// miss; neither ever joins the cache key.
///
/// Header values are built from a closed set of ASCII URIs
/// ([`crs_uri`]) and finite `f64`s, so `HeaderValue::from_str` cannot
/// realistically fail; a failure drops that one header rather than failing
/// a rendered response over a metadata header (the same treatment
/// `tellurion-features`' own `set_content_crs` gives the identical case).
fn map_response(bytes: Bytes, request: &MapRequest) -> Response {
    let mut response = (StatusCode::OK, [(header::CONTENT_TYPE, PNG_MIME)], bytes).into_response();
    let uri = crs_uri(request.crs);
    if let Ok(value) = HeaderValue::from_str(&format!("<{uri}>")) {
        response.headers_mut().insert(CONTENT_CRS_HEADER, value);
    }
    let [minx, miny, maxx, maxy] = output_bbox(request.bbox_mercator, request.crs);
    if let Ok(value) = HeaderValue::from_str(&format!("{minx},{miny},{maxx},{maxy}")) {
        response.headers_mut().insert(CONTENT_BBOX_HEADER, value);
    }
    response
}

/// `GET .../collections/{cid}/map` (`#86`) — OGC API Maps Part 1's own
/// single styled-image resource. See this module's own doc for the full
/// first-slice scope.
pub async fn map(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(cid) = params.get("cid") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(resolved) = resolve_maps(&ctx, &params, cid).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let ResolvedMaps {
        tenant_id,
        catalog_id,
        collection_id,
        decl,
        source,
    } = resolved;

    let filter_capable = match &source {
        MapSource::Vector(source) => source.filter_capable(),
        // `RasterSource` has no `filter_capable` concept (see its own doc):
        // a raster collection has no queryable attributes for a `#34` grant
        // filter to narrow. A filtered-only grant is therefore denied
        // outright — the same conservative default
        // `crate::handlers::raster_tile_response` already gives a raster
        // TILE request, not a new rule for this lane.
        MapSource::Raster(_) => false,
    };
    let policy_filter = match authorize_tiles(
        &ctx,
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
        filter_capable,
    )
    .await
    {
        Ok(filter) => filter,
        Err(response) => return response,
    };

    // Only a request that omitted `bbox` entirely pays for the descriptor
    // round trip its default window comes from (`#229`) — an explicit
    // `bbox` needs no extent at all, and this lane's hot path is the
    // explicit one.
    let window = if query.contains_key("bbox") {
        None
    } else {
        collection_window(&ctx, &tenant_id, &catalog_id, &collection_id).await
    };
    let request = match parse_request(&query, cid, window) {
        Ok(request) => request,
        Err(response) => return *response,
    };

    // `#37`: refused BY NAME, before any driver call, rather than rendering
    // an unstyled image the client cannot tell apart from a styled one —
    // see [`refuse_raster_style`].
    if let (MapSource::Raster(_), Some(style_id)) = (&source, request.style_id.as_deref()) {
        return refuse_raster_style(cid, style_id);
    }

    let res_x = (request.bbox_mercator[2] - request.bbox_mercator[0]) / f64::from(request.width);
    let res_y = (request.bbox_mercator[3] - request.bbox_mercator[1]) / f64::from(request.height);
    let zoom = mercator::pick_zoom(res_x.min(res_y), decl.tiles.minzoom, decl.tiles.maxzoom);
    let (min_col, max_col, min_row, max_row) =
        mercator::covering_tiles(request.bbox_mercator, zoom);

    // Refused by name before any driver call — see [`MAX_MAP_PIXELS`]'s own
    // doc for why this SOURCE-side check exists alongside the OUTPUT-side
    // one `parse_request` already made.
    let tile_count =
        (u64::from(max_col - min_col) + 1).saturating_mul(u64::from(max_row - min_row) + 1);
    let source_pixels = tile_count.saturating_mul(u64::from(TILE_SIZE_PX).pow(2));
    if source_pixels > MAX_MAP_PIXELS {
        return problem_response(
            StatusCode::BAD_REQUEST,
            "PixelBudgetExceeded",
            format!(
                "requested window needs to rasterize {source_pixels} source pixels across {tile_count} tiles, over this server's {MAX_MAP_PIXELS}-pixel budget"
            ),
        );
    }

    let coords: Vec<TileCoord> = (min_row..=max_row)
        .flat_map(|row| {
            (min_col..=max_col).map(move |col| TileCoord {
                z: zoom,
                x: col,
                y: row,
            })
        })
        .collect();

    // `#37`: which lane is about to render, and — on the raster lane — the
    // colormap that will classify its samples. Both change the bytes, so
    // both are cache-key material; see [`map_key`]'s own doc. The
    // fingerprint is read off the same `decl.settings.colormap`
    // `crate::handlers::raster_tile_response` folds into `Encoding::
    // PngRaster`, through the same `ColormapConf::fingerprint`.
    let lane = match &source {
        MapSource::Vector(_) => MapLane::Vector,
        MapSource::Raster(_) => MapLane::Raster(
            decl.settings
                .colormap
                .as_ref()
                .map(tellurion_core::ColormapConf::fingerprint),
        ),
    };
    let key = TileKey {
        // `#113`: this window's generation, resolved over the union of
        // buckets its mercator-meters bbox intersects — see
        // `AppContext::tile_generation_for_bbox_mercator`'s own doc; the
        // `Encoding::Map` variant has no single pyramid coordinate to
        // resolve one ancestor bucket from the way every other tile-shaped
        // key does.
        generation: ctx.tile_generation_for_bbox_mercator(&collection_id, request.bbox_mercator),
        ..map_key(
            &tenant_id,
            &catalog_id,
            &collection_id,
            &request,
            lane,
            policy_filter.as_ref().map(Filter::fingerprint),
            decl.tile_properties.clone(),
        )
    };

    let style_id_for_error = request.style_id.clone();
    let ctx_for_populate = Arc::clone(&ctx);
    let tenant_id_owned = tenant_id.clone();
    let catalog_id_owned = catalog_id.clone();
    let collection_id_owned = collection_id.clone();
    let decl_owned = decl.clone();
    let filter_for_populate = policy_filter.clone();
    let crs = request.crs;
    let bbox_mercator = request.bbox_mercator;
    let width = request.width;
    let height = request.height;
    let style_id = request.style_id.clone();
    let is_raster = matches!(source, MapSource::Raster(_));

    let populate: PopulateFuture = match source {
        MapSource::Raster(source) => raster_populate(
            source,
            decl_owned,
            coords,
            crs,
            bbox_mercator,
            width,
            height,
        ),
        MapSource::Vector(source_owned) => Box::pin(async move {
            let mut tiles_data: Vec<(TileCoord, Bytes)> = Vec::with_capacity(coords.len());
            for coord in coords {
                match ctx_for_populate
                    .fetch_mvt(
                        &tenant_id_owned,
                        &catalog_id_owned,
                        &collection_id_owned,
                        // `#190`: the maps compositor always reads the
                        // WebMercatorQuad pyramid — see `map_key`'s `tms` note.
                        tellurion_core::TileMatrixSet::WebMercatorQuad,
                        coord,
                        &decl_owned,
                        &source_owned,
                        filter_for_populate.as_ref(),
                        None,
                    )
                    .await
                {
                    MvtFetch::Hit(bytes) => tiles_data.push((coord, bytes)),
                    MvtFetch::Empty => {}
                    MvtFetch::Failed => {
                        return Err(Error::Storage(
                            "mvt tile source failed to produce a tile to render".into(),
                        ))
                    }
                }
            }

            // A style id is resolved here, inside `populate`, not before —
            // exactly the styled-PNG tile lane's own choice (see
            // `crate::handlers::styled_tile`'s doc): a cache hit skips style
            // validation entirely, and a missing style is recovered from the
            // outer `Error::NotFound` into a named 404, never surfacing from
            // inside this future directly.
            let paints = match &style_id {
                Some(id) => {
                    let style_doc = ctx_for_populate
                    .style_store
                    .load(id)
                    .map_err(|error| {
                        tracing::error!(%error, style_id = %id, "style store failed to load a registered style");
                        error
                    })?
                    .ok_or(Error::NotFound)?;
                    // Resolved at the SAME zoom this compositor is fetching its
                    // source tiles from (`#174`) — `pick_zoom`'s own answer for
                    // this request's resolution, not a fixed one: a style's
                    // zoom-driven `step`/`interpolate` paint expressions
                    // describe how the map looks at a given zoom, and the map
                    // window is drawn out of that zoom's tiles. `zoom` is
                    // derived from the bbox/width/height already folded into
                    // this response's cache key, so it never varies for one key.
                    Some(resolve_layer_paints(&style_doc, f64::from(zoom)))
                }
                None => None,
            };

            let bbox_out = output_bbox(bbox_mercator, crs);
            let png_bytes =
                tokio::task::spawn_blocking(move || -> tellurion_core::Result<Vec<u8>> {
                    let map_tiles: Vec<MapTile<'_>> = tiles_data
                        .iter()
                        .map(|(coord, bytes)| {
                            let bounds = mercator::tile_bounds_m(coord.z, coord.x, coord.y);
                            let project = build_projector(bounds, crs, bbox_out, width, height);
                            MapTile {
                                mvt: bytes.as_ref(),
                                project: Box::new(project),
                            }
                        })
                        .collect();

                    let result = match &paints {
                        Some(paints) => {
                            render_map_window_styled(width, height, paints, None, &map_tiles)
                        }
                        None => {
                            let style = RenderStyle::new(
                                &decl_owned.style.fill,
                                &decl_owned.style.stroke,
                                decl_owned.style.stroke_width as f32,
                                DEFAULT_POINT_RADIUS_PX,
                            )
                            .map_err(|error| Error::Storage(Box::new(error)))?;
                            render_map_window(width, height, &style, &map_tiles)
                        }
                    };
                    result.map_err(|error| Error::Storage(Box::new(error)))
                })
                .await
                .map_err(|join_error| Error::Storage(Box::new(join_error)))??;

            Ok(Bytes::from(png_bytes))
        }),
    };

    match ctx.get_or_populate(&collection_id, key, populate).await {
        Ok(bytes) => map_response(bytes, &request),
        // Vector lane only: the ONE source of `Error::NotFound` inside that
        // populate is a `style` id the store does not hold. The raster lane
        // has no style lookup at all (it refuses `style` up front), so a
        // `NotFound` reaching here from it would be some other driver's and
        // must not be reported as a missing style.
        Err(err) if !is_raster && matches!(err.as_ref(), Error::NotFound) => problem_response(
            StatusCode::NOT_FOUND,
            "NotFound",
            format!(
                "style '{}' not found",
                style_id_for_error.unwrap_or_default()
            ),
        ),
        // `#37`, raster lane: honouring this window would have taken the
        // driver over its OWN per-request source-pixel budget for one
        // covering tile (`tellurion-cog::driver::MAX_SOURCE_PIXELS` and its
        // Zarr counterpart). A client-correctable 400 naming the budget —
        // never a 500, and never a ballooned read of the whole source.
        // Identical treatment, and identical problem code, to the raster
        // TILE lane's own (`crate::handlers::raster_tile_response`).
        Err(err) if is_raster && matches!(err.as_ref(), Error::Invalid(_)) => problem_response(
            StatusCode::BAD_REQUEST,
            "PixelBudgetExceeded",
            err.to_string(),
        ),
        // `#37`, raster lane: the collection's configured colormap cannot be
        // honoured against this raster's real band layout (or a Zarr array
        // was given none at all, which has no visual meaning) — the driver's
        // own capability-mismatch refusal, surfaced by name exactly as the
        // raster TILE lane surfaces it.
        Err(err) if is_raster && matches!(err.as_ref(), Error::Config(_)) => problem_response(
            StatusCode::BAD_REQUEST,
            "CapabilityUnsupported",
            err.to_string(),
        ),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The RASTER half of this lane's cache-populate closure (`#37`) — the
/// counterpart of the vector half inlined in [`map`] above.
///
/// One [`RasterSource::raster_tile`] call per covering `WebMercatorQuad`
/// tile: the SAME call `crate::handlers::raster_tile_response` makes for a
/// single PNG tile, with the same `CollectionDecl` it would pass. That is
/// what makes every budget on this lane a REUSED budget rather than a
/// second one — the driver's per-request source-pixel cap, its decode path,
/// and its remote-read timeout all apply per covering tile, unchanged and
/// un-duplicated here, and the collection's validated colormap (and, for
/// Zarr, its array's fixed leading-dimension slice) is applied inside that
/// same call. This function never reads a source window of its own.
///
/// `Ok(None)` from a covering tile means it does not intersect the raster's
/// extent at all — the same "legitimately empty" convention `MvtFetch::
/// Empty` carries on the vector half. Such a tile contributes nothing, and
/// a window that intersects no data at all yields a fully transparent PNG
/// of the requested size, exactly as the vector half yields one for a
/// window whose covering tiles are all empty. That is not a degraded
/// answer: `Content-Bbox` on the response names the window it covers, so a
/// client can tell an empty window from a wrong one.
///
/// Errors are NOT swallowed: a driver refusal (`Error::Invalid` for its own
/// pixel budget, `Error::Config` for a colormap its band layout cannot
/// honour) propagates out of here untouched, and [`map`]'s own match turns
/// it into a named problem response.
fn raster_populate(
    source: Arc<dyn RasterSource>,
    decl: CollectionDecl,
    coords: Vec<TileCoord>,
    crs: MapCrs,
    bbox_mercator: [f64; 4],
    width: u32,
    height: u32,
) -> PopulateFuture {
    Box::pin(async move {
        let mut windows: Vec<(TileCoord, RasterWindow)> = Vec::with_capacity(coords.len());
        for coord in coords {
            if let Some(window) = source.raster_tile(&decl, coord).await? {
                windows.push((coord, window));
            }
        }

        let bbox_out = output_bbox(bbox_mercator, crs);
        // Same rasterize-is-real-CPU-work rationale as every other render in
        // this workspace — offloaded the same way, for the same reason.
        let png_bytes = tokio::task::spawn_blocking(move || -> tellurion_core::Result<Vec<u8>> {
            let tiles: Vec<RasterMapTile<'_>> = windows
                .iter()
                .map(|(coord, window)| {
                    let bounds = mercator::tile_bounds_m(coord.z, coord.x, coord.y);
                    RasterMapTile {
                        rgba: &window.rgba,
                        width: window.width,
                        height: window.height,
                        dest: dest_rect(bounds, crs, bbox_out, width, height),
                        sample: Box::new(build_sampler(bounds, crs, bbox_out, width, height)),
                    }
                })
                .collect();
            render_raster_map_window(width, height, &tiles)
                .map_err(|error| Error::Storage(Box::new(error)))
        })
        .await
        .map_err(|join_error| Error::Storage(Box::new(join_error)))??;

        Ok(Bytes::from(png_bytes))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::to_bytes;
    use geozero::mvt::{tile, Message, Tile};
    use tellurion_core::{
        AppConfig, CatalogSource, DriverFactory, MokaTileCache, PhysicalCollection, Registry,
        Resolver, Result as CoreResult, Router, SpatialExtent, StaticResolver, StorageDecl,
        StorageDriver, StyleStore, TileCache,
    };

    use crate::tilematrixset::{TILE_SIZE_PX, WEB_MERCATOR_ORIGIN};

    const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    struct EmptyCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for EmptyCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![])
        }
    }

    /// A catalog that DOES report the `demo` table, with a CRS84 spatial
    /// extent — the one source `#229`'s `bbox`-less window comes from
    /// (`Router::canonical_descriptor`'s own `extent`). [`EmptyCatalog`]
    /// reports no table at all, which is exactly the "no extent is known"
    /// case the named refusal covers, so both fixtures are needed.
    struct ExtentCatalog {
        bbox: [f64; 4],
    }

    #[async_trait::async_trait]
    impl CatalogSource for ExtentCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![PhysicalCollection {
                name: "demo".to_string(),
                geometry_column: Some("geom".to_string()),
                primary_key: Some("id".to_string()),
                srid: Some(4326),
                geometry_type: Some("POINT".to_string()),
            }])
        }

        async fn extent(
            &self,
            _physical: &PhysicalCollection,
        ) -> CoreResult<Option<SpatialExtent>> {
            Ok(Some(SpatialExtent { bbox: self.bbox }))
        }
    }

    fn cmd(id: u32, count: u32) -> u32 {
        id | (count << 3)
    }
    fn zz(n: i32) -> u32 {
        ((n << 1) ^ (n >> 31)) as u32
    }
    fn move_to(dx: i32, dy: i32) -> Vec<u32> {
        vec![cmd(1, 1), zz(dx), zz(dy)]
    }

    /// One point feature, layer `"pts"`, at the tile's own local center
    /// (extent 100, point at `(50, 50)`) — every covering tile in these
    /// tests carries exactly this content, so a composited render always has
    /// the same known geometry per tile regardless of which tile it is.
    fn point_tile_bytes() -> Bytes {
        let mut layer = tile::Layer {
            version: 2,
            name: "pts".to_string(),
            extent: Some(100),
            ..Default::default()
        };
        let mut feature = tile::Feature {
            geometry: move_to(50, 50),
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

    /// Always answers [`point_tile_bytes`] for any coordinate, counting how
    /// many times `mvt_tile` actually ran — the single-flight/cache-key
    /// tests below assert on this directly. `delay`, when non-zero, holds
    /// the call open long enough for N concurrent requests to overlap
    /// in-flight (mirrors `crate::handlers`' own `FakeTileSource::
    /// with_delay`).
    struct FakeTileSource {
        calls: AtomicUsize,
        delay: std::time::Duration,
    }

    impl FakeTileSource {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                delay: std::time::Duration::ZERO,
            }
        }

        fn with_delay(delay: std::time::Duration) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                delay,
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl TileSource for FakeTileSource {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<Bytes>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            Ok(Some(point_tile_bytes()))
        }
    }

    struct FakeDriver {
        tiles: Arc<FakeTileSource>,
        /// `Some` mounts an [`ExtentCatalog`] reporting this CRS84 bbox;
        /// `None` mounts [`EmptyCatalog`], leaving the collection with no
        /// derived extent at all.
        extent: Option<[f64; 4]>,
    }

    impl StorageDriver for FakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            match self.extent {
                Some(bbox) => Arc::new(ExtentCatalog { bbox }),
                None => Arc::new(EmptyCatalog),
            }
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::clone(&self.tiles) as Arc<dyn TileSource>)
        }
    }

    struct FakeFactory {
        tiles: Arc<FakeTileSource>,
        extent: Option<[f64; 4]>,
    }

    impl DriverFactory for FakeFactory {
        fn name(&self) -> &str {
            "fake"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FakeDriver {
                tiles: Arc::clone(&self.tiles),
                extent: self.extent,
            }))
        }
    }

    struct FakeStyleStore {
        styles: HashMap<String, serde_json::Value>,
    }

    impl StyleStore for FakeStyleStore {
        fn load(&self, id: &str) -> CoreResult<Option<serde_json::Value>> {
            Ok(self.styles.get(id).cloned())
        }

        fn list(&self) -> CoreResult<Vec<String>> {
            Ok(self.styles.keys().cloned().collect())
        }
    }

    /// A MapLibre Style JSON document with one `circle` layer painting the
    /// given MVT `source_layer` a flat opaque color — same shape
    /// `crate::handlers`' own styled-lane tests use.
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

    /// `tiles.minzoom == tiles.maxzoom == 0` pins every request in this
    /// module to zoom 0 regardless of `pick_zoom`'s own resolution math, so
    /// [`whole_world_bbox`] always resolves to exactly the single root tile
    /// `(0, 0, 0)` — deterministic covering-tile counts for every test here.
    fn test_ctx(
        tiles: Arc<FakeTileSource>,
        styles: HashMap<String, serde_json::Value>,
    ) -> Arc<AppContext> {
        test_ctx_with_extent(tiles, styles, None)
    }

    /// [`test_ctx`], with `extent` deciding whether this collection has a
    /// derived spatial extent at all (`#229`) — `Some` is the collection a
    /// `bbox`-less request renders, `None` the one it is refused by name
    /// for.
    fn test_ctx_with_extent(
        tiles: Arc<FakeTileSource>,
        styles: HashMap<String, serde_json::Value>,
        extent: Option<[f64; 4]>,
    ) -> Arc<AppContext> {
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(10_000_000));
        test_ctx_with_extent_and_cache(tiles, styles, extent, cache)
    }

    /// [`test_ctx_with_extent`], with the [`TileCache`] supplied by the
    /// caller — for the tests that observe the cache itself (`#291`'s
    /// key-fragmentation test wraps it in a [`MapKeySpyCache`]).
    fn test_ctx_with_extent_and_cache(
        tiles: Arc<FakeTileSource>,
        styles: HashMap<String, serde_json::Value>,
        extent: Option<[f64; 4]>,
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
    tiles: { minzoom: 0, maxzoom: 0, caps: {} }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory { tiles, extent }));
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

    fn cid_path(cid: &str) -> Path<HashMap<String, String>> {
        Path(HashMap::from([("cid".to_string(), cid.to_string())]))
    }

    fn query(pairs: &[(&str, &str)]) -> Query<HashMap<String, String>> {
        Query(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    /// The whole projected-world bbox, in `WebMercatorQuad`-native meters —
    /// combined with [`test_ctx`]'s `minzoom == maxzoom == 0`, always
    /// resolves to exactly one covering tile. `#270`: metres, so every
    /// caller pairs it with an explicit `bbox-crs` naming this lane's own
    /// CRS; an undeclared one would now be refused by name.
    fn whole_world_bbox() -> String {
        format!(
            "{},{},{},{}",
            -WEB_MERCATOR_ORIGIN, -WEB_MERCATOR_ORIGIN, WEB_MERCATOR_ORIGIN, WEB_MERCATOR_ORIGIN
        )
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// The `Content-Bbox` header's four numbers, in whatever CRS
    /// `Content-Crs` names for that response — the only wire-visible view
    /// of the window a request actually resolved to, and therefore the only
    /// honest place to assert how a `bbox` was READ.
    fn content_bbox(response: &Response) -> Vec<f64> {
        response
            .headers()
            .get(CONTENT_BBOX_HEADER)
            .expect("/req/core/map-response D: every 200 carries Content-Bbox")
            .to_str()
            .unwrap()
            .split(',')
            .map(|value| value.parse().unwrap())
            .collect()
    }

    // -- named refusals -------------------------------------------------

    /// `#229`: a collection with no derived extent has no honest window to
    /// render a `bbox`-less request over, so it is refused BY NAME —
    /// never quietly served a world-extent image it never asked for.
    #[tokio::test]
    async fn map_without_bbox_refuses_by_name_when_no_extent_is_known() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[("width", "10"), ("height", "10")]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["code"], "CapabilityUnsupported");
        let detail = json["detail"].as_str().unwrap();
        assert!(detail.contains("demo"), "{detail}");
        assert!(detail.contains("default-extent"), "{detail}");
    }

    /// `#229`: `width`/`height` belong to the Scaling class, not Core — a
    /// request naming only a window renders it at the tile grid's own
    /// native scale, which for the whole world is exactly one tile.
    #[tokio::test]
    async fn map_without_width_or_height_renders_the_window_at_the_grids_native_scale() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let pixmap = tiny_skia::Pixmap::decode_png(&body).unwrap();
        assert_eq!(
            (pixmap.width(), pixmap.height()),
            (TILE_SIZE_PX, TILE_SIZE_PX)
        );
    }

    /// One dimension supplied, the other derived from the window's own
    /// aspect ratio (`#229`) — a 2:1 window asked for 100 columns gets 50
    /// rows, never a squashed square.
    #[tokio::test]
    async fn map_derives_the_missing_dimension_from_the_windows_aspect_ratio() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let bbox = format!(
            "{},{},{},{}",
            -WEB_MERCATOR_ORIGIN,
            -WEB_MERCATOR_ORIGIN / 2.0,
            WEB_MERCATOR_ORIGIN,
            WEB_MERCATOR_ORIGIN / 2.0
        );
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", &bbox),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "100"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let pixmap = tiny_skia::Pixmap::decode_png(&body).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (100, 50));
    }

    #[tokio::test]
    async fn map_rejects_a_bbox_whose_minimum_is_not_less_than_its_maximum() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[("bbox", "10,10,5,20"), ("width", "10"), ("height", "10")]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(response).await["code"], "InvalidParameter");
    }

    #[tokio::test]
    async fn map_rejects_an_unsupported_crs() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("width", "10"),
                ("height", "10"),
                ("crs", "http://www.opengis.net/def/crs/EPSG/0/2154"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_json(response).await["code"], "CrsNotSupported");
    }

    #[tokio::test]
    async fn map_refuses_an_over_budget_output_size_by_name_rather_than_clamping() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "3000"),
                ("height", "3000"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["code"], "PixelBudgetExceeded");
        assert!(json["detail"].as_str().unwrap().contains("3000x3000"));
    }

    #[tokio::test]
    async fn map_refuses_an_unknown_style_by_name() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "10"),
                ("height", "10"),
                ("style", "does-not-exist"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let json = body_json(response).await;
        assert_eq!(json["code"], "NotFound");
        assert!(json["detail"].as_str().unwrap().contains("does-not-exist"));
    }

    #[tokio::test]
    async fn unknown_collection_is_not_found() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("missing"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("width", "10"),
                ("height", "10"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -- render correctness -----------------------------------------------

    #[tokio::test]
    async fn map_renders_a_deterministic_non_empty_png_for_a_known_bbox() {
        let tiles = Arc::new(FakeTileSource::new());
        let ctx = test_ctx(Arc::clone(&tiles), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "100"),
                ("height", "100"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            PNG_MIME
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[0..8], &PNG_MAGIC);

        let pixmap = tiny_skia::Pixmap::decode_png(&body).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (100, 100));
        // The single covering tile's point sits at its own local center,
        // which the whole-world bbox maps to the output canvas's own
        // center.
        assert!(
            pixmap.pixel(50, 50).unwrap().alpha() > 0,
            "the point feature must be visible at the canvas center"
        );
        assert_eq!(
            pixmap.pixel(2, 2).unwrap().alpha(),
            0,
            "an untouched corner must stay transparent"
        );
        assert_eq!(tiles.call_count(), 1);
    }

    #[tokio::test]
    async fn map_styled_request_paints_using_the_resolved_style_not_the_collections_default() {
        let mut styles = HashMap::new();
        styles.insert("basic".to_string(), circle_style_doc("pts", "#ff0000"));
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), styles);
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "100"),
                ("height", "100"),
                ("style", "basic"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let pixmap = tiny_skia::Pixmap::decode_png(&body).unwrap();
        let pixel = pixmap.pixel(50, 50).unwrap().demultiply();
        assert_eq!(
            (pixel.red(), pixel.green(), pixel.blue()),
            (255, 0, 0),
            "the resolved style's circle-color must paint the point, not the collection's default fill"
        );
    }

    // -- `#229`: the parameterless Core request ---------------------------

    /// The request Maps Part 1's Core class is defined by: no query
    /// parameters at all. It renders the collection's OWN extent — the same
    /// `CanonicalDescriptor.extent` `/collections/{cid}` publishes — at the
    /// grid's native scale, and reports both back on `Content-Bbox`/
    /// `Content-Crs` so the client can georeference what it got.
    #[tokio::test]
    async fn parameterless_map_renders_the_collections_own_extent() {
        let tiles = Arc::new(FakeTileSource::new());
        let extent = [-10.0, -5.0, 10.0, 5.0];
        let ctx = test_ctx_with_extent(Arc::clone(&tiles), HashMap::new(), Some(extent));
        let response = map(
            State(ctx),
            cid_path("demo"),
            Query(HashMap::new()),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            PNG_MIME
        );
        assert_eq!(
            response.headers().get(CONTENT_CRS_HEADER).unwrap(),
            &format!("<{WEB_MERCATOR_QUAD_CRS}>")
        );

        // `Content-Bbox` is the collection's extent, projected — not a
        // world bbox, and not the request's (there was none).
        let (expected_minx, expected_miny) = mercator::forward(extent[0], extent[1]);
        let (expected_maxx, expected_maxy) = mercator::forward(extent[2], extent[3]);
        let reported: Vec<f64> = response
            .headers()
            .get(CONTENT_BBOX_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .split(',')
            .map(|value| value.parse().unwrap())
            .collect();
        for (reported, expected) in
            reported
                .iter()
                .zip([expected_minx, expected_miny, expected_maxx, expected_maxy])
        {
            assert!((reported - expected).abs() < 1e-6, "{reported} {expected}");
        }
        assert!(
            reported[2] - reported[0] < 2.0 * WEB_MERCATOR_ORIGIN,
            "a derived window must be the collection's own extent, never the whole world"
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[0..8], &PNG_MAGIC);
        let pixmap = tiny_skia::Pixmap::decode_png(&body).unwrap();
        // Native scale: the longest side lands between one and two tiles,
        // and the aspect ratio matches the projected extent's own.
        let longest = pixmap.width().max(pixmap.height());
        assert!(
            (TILE_SIZE_PX..=2 * TILE_SIZE_PX).contains(&longest),
            "{longest}"
        );
        let expected_ratio = (expected_maxx - expected_minx) / (expected_maxy - expected_miny);
        let actual_ratio = f64::from(pixmap.width()) / f64::from(pixmap.height());
        assert!(
            (actual_ratio - expected_ratio).abs() / expected_ratio < 0.02,
            "aspect ratio {actual_ratio} should track the extent's own {expected_ratio}"
        );
        assert_eq!(tiles.call_count(), 1);
    }

    /// The output CRS the client asked for is the one `Content-Crs` names,
    /// and `Content-Bbox` is expressed in it (`/req/core/map-response`
    /// C/D/E) — degrees for CRS84, not the meters the window is resolved
    /// in internally.
    #[tokio::test]
    async fn map_reports_its_window_in_the_requested_output_crs() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", "-20,-10,20,10"),
                ("bbox-crs", CRS84_URI),
                ("crs", CRS84_URI),
                ("width", "40"),
                ("height", "20"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_CRS_HEADER).unwrap(),
            &format!("<{CRS84_URI}>")
        );
        let reported: Vec<f64> = response
            .headers()
            .get(CONTENT_BBOX_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .split(',')
            .map(|value| value.parse().unwrap())
            .collect();
        for (reported, expected) in reported.iter().zip([-20.0, -10.0, 20.0, 10.0]) {
            assert!((reported - expected).abs() < 1e-6, "{reported} {expected}");
        }
    }

    /// A present-but-invalid dimension is still refused by name — `#229`
    /// made the parameters optional, not lenient.
    #[tokio::test]
    async fn map_still_refuses_a_zero_or_non_numeric_dimension() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        for (name, value) in [("width", "0"), ("height", "wide")] {
            let response = map(
                State(Arc::clone(&ctx)),
                cid_path("demo"),
                query(&[
                    ("bbox", &whole_world_bbox()),
                    ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                    (name, value),
                ]),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let json = body_json(response).await;
            assert_eq!(json["code"], "InvalidParameter");
            assert!(json["detail"].as_str().unwrap().contains(name));
        }
    }

    // -- `bbox-crs` default + guard (`#270`) --------------------------------

    /// Requirement 18 (`/req/spatial-subsetting/bbox-crs`) clause C: "If the
    /// bbox-crs is not indicated `https://www.opengis.net/def/crs/OGC/1.3/
    /// CRS84` SHALL be assumed." A `bbox` whose numbers are plausible
    /// degrees is therefore read as degrees — proven against the WIRE, not
    /// against an internal value: `crs=CRS84` makes `Content-Bbox` report
    /// the window back in degrees, and it comes back as the very degrees
    /// that were sent. Under the pre-`#270` reading these same four numbers
    /// were metres, and 20 metres east of Greenwich is 0.00018°, so this
    /// assertion cannot pass by accident.
    #[tokio::test]
    async fn map_reads_an_omitted_bbox_crs_as_crs84() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", "-20,-10,20,10"),
                ("crs", CRS84_URI),
                ("width", "40"),
                ("height", "20"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let reported = content_bbox(&response);
        for (reported, expected) in reported.iter().zip([-20.0, -10.0, 20.0, 10.0]) {
            assert!(
                (reported - expected).abs() < 1e-6,
                "an omitted 'bbox-crs' must be read as CRS84 degrees, so the window \
                 reported back in CRS84 must be the degrees that were sent: \
                 got {reported}, want {expected}"
            );
        }
    }

    /// The other half of `#270`: an omitted `bbox-crs` whose `bbox` cannot
    /// be CRS84 is REFUSED BY NAME rather than interpreted. The refusal has
    /// to name `bbox-crs` itself and say what to supply — a bare "invalid
    /// bbox" would leave the client exactly as stuck as the silent wrong
    /// window this replaces.
    #[tokio::test]
    async fn map_refuses_an_omitted_bbox_crs_whose_bbox_cannot_be_crs84() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("width", "64"),
                ("height", "64"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "a WebMercator-magnitude bbox with no 'bbox-crs' must not be read as degrees"
        );
        let json = body_json(response).await;
        assert_eq!(json["code"], "BboxCrsRequired");
        let detail = json["detail"].as_str().unwrap();
        assert!(
            detail.contains("bbox-crs"),
            "the refusal must name the parameter the client has to add: {detail}"
        );
        assert!(
            detail.contains(WEB_MERCATOR_QUAD_CRS),
            "the refusal must name the value to supply for metres: {detail}"
        );
    }

    /// The guard is scoped to the OMITTED case only — a client that
    /// declares `bbox-crs` has said what its numbers mean, and both
    /// declarations keep working exactly as they did before `#270`. Same
    /// four numbers under the two declarations must land on two different
    /// windows, or the declaration is not reaching the parse at all.
    #[tokio::test]
    async fn map_honours_an_explicitly_declared_bbox_crs_in_either_crs() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        // Metres, declared: accepted despite being far outside ±180/±90.
        let mercator = map(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "64"),
                ("height", "64"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(
            mercator.status(),
            StatusCode::OK,
            "a DECLARED metres bbox-crs is unaffected by the omitted-case guard"
        );
        let mercator_window = content_bbox(&mercator);

        // Degrees, declared: the same numbers a CRS84 client would send.
        let degrees = map(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            query(&[
                ("bbox", "-20,-10,20,10"),
                ("bbox-crs", CRS84_URI),
                ("width", "40"),
                ("height", "20"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(degrees.status(), StatusCode::OK);
        let degrees_window = content_bbox(&degrees);

        // Both report in this lane's default output CRS (metres), so the
        // two windows are directly comparable — and must not be the same.
        assert!(
            (mercator_window[2] - degrees_window[2]).abs() > 1.0,
            "the declared bbox-crs is not reaching the parse: {mercator_window:?} \
             vs {degrees_window:?}"
        );
        // ...and the same numbers declared as metres are NOT what the
        // omitted case produces, which is the whole point of `#270`.
        assert!(
            (mercator_window[2] - WEB_MERCATOR_ORIGIN).abs() < 1.0,
            "declared metres must survive the parse verbatim: {mercator_window:?}"
        );
    }

    /// The exact CRS84 world bbox is INSIDE the ranges, not outside them —
    /// `bbox=-180,-90,180,90` with no `bbox-crs` is the single most
    /// ordinary CRS84 request there is, and a guard that refused it would
    /// have converted the silent wrong answer into a loud wrong answer.
    /// This is the boundary the range check's inclusive comparison holds.
    #[tokio::test]
    async fn map_accepts_the_exact_crs84_world_bbox_with_no_bbox_crs() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", "-180,-90,180,90"),
                ("width", "64"),
                ("height", "64"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Requirement 18 clause F ("If the bbox parameter is not used, the
    /// bbox-crs SHALL be ignored") — and with it, the guard. A request with
    /// no `bbox` at all resolves to the collection's own extent and must
    /// not be dragged into a `bbox-crs` refusal it named no coordinates
    /// for.
    #[tokio::test]
    async fn map_without_a_bbox_is_not_touched_by_the_bbox_crs_guard() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[("width", "32"), ("height", "32")]),
            HeaderMap::new(),
        )
        .await;
        // `test_ctx`'s collection has no derived extent, so this is the
        // `#229` extent refusal — NOT the `#270` bbox-crs one. Either way
        // it must not be `BboxCrsRequired`.
        let json = body_json(response).await;
        assert_ne!(
            json["code"], "BboxCrsRequired",
            "clause F: with no 'bbox' there is nothing for 'bbox-crs' to qualify"
        );
    }

    /// Everything wire-visible about one response — status, every header in
    /// order, and the body bytes. The `#291` identity tests compare on this
    /// rather than on a chosen subset, because "the response is byte-for-byte
    /// what the same request without the parameter produces" is a claim about
    /// the whole response, not about the headers someone remembered to check.
    async fn response_fingerprint(
        response: Response,
    ) -> (StatusCode, Vec<(String, Vec<u8>)>, Bytes) {
        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
            .collect();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, headers, body)
    }

    /// `#291`: OGC 20-058 contradicts itself on a `bbox`-less `bbox-crs`.
    /// Requirement 18 clause F, verbatim: "If the bbox parameter is not
    /// used, the bbox-crs SHALL be ignored." §13.5, verbatim and stated
    /// unconditionally: "If the CRS in the parameter value bbox-crs,
    /// subset-crs or center-crs is not supported by the server for this
    /// resource, or the parameter value is out-of-range, the status code of
    /// the response will be 400." The recorded decision
    /// (`docs/spec-deviations.md`) takes clause F for the parameter's
    /// EFFECT: a supported `bbox-crs` with no `bbox` changes nothing, and
    /// the response — status, every header, every body byte — is exactly
    /// the one the same request without it gets.
    #[tokio::test]
    async fn a_bbox_less_bbox_crs_is_ignored_and_the_response_is_byte_identical() {
        let ctx = test_ctx_with_extent(
            Arc::new(FakeTileSource::new()),
            HashMap::new(),
            Some([-10.0, -5.0, 10.0, 5.0]),
        );
        let without = map(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            query(&[("width", "32"), ("height", "16")]),
            HeaderMap::new(),
        )
        .await;
        let baseline = response_fingerprint(without).await;
        assert_eq!(baseline.0, StatusCode::OK);

        for declared in [CRS84_URI, WEB_MERCATOR_QUAD_CRS] {
            let with = map(
                State(Arc::clone(&ctx)),
                cid_path("demo"),
                query(&[("width", "32"), ("height", "16"), ("bbox-crs", declared)]),
                HeaderMap::new(),
            )
            .await;
            let observed = response_fingerprint(with).await;
            assert_eq!(
                observed.0, baseline.0,
                "clause F (`#291`): 'bbox-crs={declared}' with no 'bbox' must not \
                 change the status"
            );
            assert_eq!(
                observed.1, baseline.1,
                "clause F (`#291`): 'bbox-crs={declared}' with no 'bbox' must not \
                 change a single header"
            );
            assert_eq!(
                observed.2, baseline.2,
                "clause F (`#291`): 'bbox-crs={declared}' with no 'bbox' must not \
                 change a single body byte"
            );
        }
    }

    /// A [`TileCache`] that records every DISTINCT [`Encoding::Map`] key any
    /// request reaches it with, delegating the caching itself to a real
    /// [`MokaTileCache`]. Source-tile (`Encoding::Mvt`) keys are deliberately
    /// not recorded: the map compositor's `fetch_mvt` serves repeat renders
    /// out of the SAME shared cache, so a fragmented map key would not show
    /// up in source fetch counts — only in the map keys themselves, which is
    /// exactly what `#291`'s fragmentation test asserts on.
    struct MapKeySpyCache {
        inner: MokaTileCache,
        map_keys: std::sync::Mutex<Vec<TileKey>>,
    }

    impl MapKeySpyCache {
        fn new() -> Self {
            Self {
                inner: MokaTileCache::with_byte_budget(10_000_000),
                map_keys: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn record(&self, key: &TileKey) {
            if !matches!(key.encoding, Encoding::Map { .. }) {
                return;
            }
            let mut keys = self.map_keys.lock().unwrap();
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }

        fn distinct_map_keys(&self) -> usize {
            self.map_keys.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl TileCache for MapKeySpyCache {
        async fn get(&self, key: &TileKey) -> Option<Bytes> {
            self.record(key);
            self.inner.get(key).await
        }

        async fn insert(&self, key: TileKey, value: Bytes) {
            self.record(&key);
            self.inner.insert(key, value).await;
        }

        async fn get_or_populate(
            &self,
            key: TileKey,
            populate: PopulateFuture,
        ) -> Result<Bytes, Arc<tellurion_core::Error>> {
            self.record(&key);
            self.inner.get_or_populate(key, populate).await
        }

        async fn get_or_populate_with_ttl(
            &self,
            key: TileKey,
            populate: PopulateFuture,
            ttl: std::time::Duration,
        ) -> Result<Bytes, Arc<tellurion_core::Error>> {
            self.record(&key);
            self.inner
                .get_or_populate_with_ttl(key, populate, ttl)
                .await
        }
    }

    /// The cache half of the same identity (`#291`): an ignored parameter
    /// must not fragment the cache. The `bbox`-less baseline populates one
    /// [`Encoding::Map`] entry; the same request with each supported
    /// `bbox-crs` declared must reach the cache under EXACTLY that key — one
    /// distinct map key across all three requests — or the key has grown a
    /// discriminator the response bytes do not depend on. Asserted on the
    /// keys themselves (via [`MapKeySpyCache`]) rather than on source fetch
    /// counts, which the shared MVT-entry cache would keep flat even under a
    /// fragmented map key.
    #[tokio::test]
    async fn a_bbox_less_bbox_crs_does_not_fragment_the_cache() {
        let spy = Arc::new(MapKeySpyCache::new());
        let ctx = test_ctx_with_extent_and_cache(
            Arc::new(FakeTileSource::new()),
            HashMap::new(),
            Some([-10.0, -5.0, 10.0, 5.0]),
            Arc::clone(&spy) as Arc<dyn TileCache>,
        );
        let baseline = map(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            query(&[("width", "32"), ("height", "16")]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(baseline.status(), StatusCode::OK);
        assert_eq!(
            spy.distinct_map_keys(),
            1,
            "the baseline render must have used exactly one map cache key"
        );

        for declared in [CRS84_URI, WEB_MERCATOR_QUAD_CRS] {
            let with = map(
                State(Arc::clone(&ctx)),
                cid_path("demo"),
                query(&[("width", "32"), ("height", "16"), ("bbox-crs", declared)]),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(with.status(), StatusCode::OK);
        }
        assert_eq!(
            spy.distinct_map_keys(),
            1,
            "an ignored 'bbox-crs' fragmented the cache: the same window reached \
             the cache under more than one map key"
        );
    }

    /// The other half of `#291`'s decision, §13.5's side: ignoring an unused
    /// parameter is not accepting a nonsense one. An undeclared CRS in
    /// `bbox-crs` is refused BY NAME whether or not `bbox` is present — the
    /// collection here HAS a derived extent, so the value is the only thing
    /// wrong with this request, and the refusal must name it and the two
    /// values this server does serve (`#270`'s named-refusal contract,
    /// unchanged).
    #[tokio::test]
    async fn a_bbox_less_bbox_crs_with_an_undeclared_crs_is_still_refused_by_name() {
        let ctx = test_ctx_with_extent(
            Arc::new(FakeTileSource::new()),
            HashMap::new(),
            Some([-10.0, -5.0, 10.0, 5.0]),
        );
        let bogus = "http://www.opengis.net/def/crs/EPSG/0/2154";
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[("width", "32"), ("height", "16"), ("bbox-crs", bogus)]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "§13.5 (`#291`): a CRS this server cannot serve is never accepted, \
             even on a parameter whose effect would be ignored"
        );
        let json = body_json(response).await;
        assert_eq!(json["code"], "CrsNotSupported");
        let detail = json["detail"].as_str().unwrap();
        assert!(
            detail.contains(bogus),
            "the refusal must name the value it refuses: {detail}"
        );
        assert!(
            detail.contains(WEB_MERCATOR_QUAD_CRS) && detail.contains(CRS84_URI),
            "the refusal must name the two CRSs this server does serve: {detail}"
        );
    }

    /// `#291`'s regression guard: WITH a `bbox`, nothing moved. An
    /// undeclared CRS on `bbox-crs` is the same named refusal `#270`
    /// established, bbox or no bbox.
    #[tokio::test]
    async fn map_rejects_an_unsupported_bbox_crs_when_bbox_is_present() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let bogus = "http://www.opengis.net/def/crs/EPSG/0/2154";
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[
                ("bbox", "-20,-10,20,10"),
                ("bbox-crs", bogus),
                ("width", "40"),
                ("height", "20"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["code"], "CrsNotSupported");
        assert!(
            json["detail"].as_str().unwrap().contains(bogus),
            "the refusal must still name the value: {}",
            json["detail"]
        );
    }

    /// `#270`'s guard fires on the OUTPUT CRS's own parameter being absent,
    /// never on `crs`. An omitted `crs` still means this lane's native CRS
    /// (Requirement 35 NOTE 2: "the default CRS of the map is the native
    /// (storage) CRS"), which is what `Content-Crs` names — changing
    /// `bbox-crs`'s default must not have moved this one.
    #[tokio::test]
    async fn an_omitted_output_crs_is_still_this_lanes_native_crs() {
        let ctx = test_ctx(Arc::new(FakeTileSource::new()), HashMap::new());
        let response = map(
            State(ctx),
            cid_path("demo"),
            query(&[("bbox", "-20,-10,20,10"), ("width", "40"), ("height", "20")]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_CRS_HEADER).unwrap(),
            &format!("<{WEB_MERCATOR_QUAD_CRS}>"),
            "an omitted 'crs' is the native CRS, not CRS84 — `#270` moved only 'bbox-crs'"
        );
    }

    /// The raster half of the lane shares `parse_request` verbatim (`#37`),
    /// so it must show the same two behaviours — one fix, not one fix per
    /// lane. A COG-backed collection: undeclared metres refused by name,
    /// the same window with `bbox-crs` declared renders.
    #[tokio::test]
    async fn the_raster_lane_shares_the_bbox_crs_default_and_its_guard() {
        let bbox = tile_window_bbox(COG_QUADRANT);
        let ctx = raster_ctx(
            "cog",
            &cog_fixture(),
            &cog_collections(PRIMARY_STOPS_YAML, COG_QUADRANT.z),
        );
        let refused = map(
            State(Arc::clone(&ctx)),
            cid_path(COG_CID),
            query(&[("bbox", &bbox), ("width", "64"), ("height", "64")]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        let json = body_json(refused).await;
        assert_eq!(json["code"], "BboxCrsRequired");
        assert!(json["detail"].as_str().unwrap().contains("bbox-crs"));

        let declared = map(
            State(ctx),
            cid_path(COG_CID),
            query(&[
                ("bbox", &bbox),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "64"),
                ("height", "64"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(
            declared.status(),
            StatusCode::OK,
            "the raster lane must accept the same declared bbox-crs the vector lane does"
        );
    }

    // -- cache key / single-flight ------------------------------------------

    #[tokio::test]
    async fn second_identical_map_request_is_served_from_cache_without_refetching() {
        let tiles = Arc::new(FakeTileSource::new());
        let ctx = test_ctx(Arc::clone(&tiles), HashMap::new());
        let bbox = whole_world_bbox();
        let first = map(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            Query(HashMap::from([
                ("bbox".to_string(), bbox.clone()),
                ("bbox-crs".to_string(), WEB_MERCATOR_QUAD_CRS.to_string()),
                ("width".to_string(), "64".to_string()),
                ("height".to_string(), "64".to_string()),
            ])),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let second = map(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            Query(HashMap::from([
                ("bbox".to_string(), bbox),
                ("bbox-crs".to_string(), WEB_MERCATOR_QUAD_CRS.to_string()),
                ("width".to_string(), "64".to_string()),
                ("height".to_string(), "64".to_string()),
            ])),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(
            tiles.call_count(),
            1,
            "a second identical /map request must not refetch the covering tile's MVT bytes"
        );
    }

    /// Two Map-encoded cache entries (styled vs. unstyled) over the exact
    /// same window are distinct — proven by their rendered bytes differing —
    /// even though both resolve to the same single covering tile, whose
    /// `Encoding::Mvt`-keyed bytes are fetched only ONCE and reused by
    /// both: the outer `Map` key partitions by style, but the underlying
    /// cached MVT fetch does not, exactly the same sharing the styled-PNG
    /// tile lane already gets from `Encoding::Mvt` regardless of style.
    #[tokio::test]
    async fn styled_and_unstyled_requests_over_the_same_window_do_not_share_a_cache_entry() {
        let tiles = Arc::new(FakeTileSource::new());
        let mut styles = HashMap::new();
        styles.insert("basic".to_string(), circle_style_doc("pts", "#ff0000"));
        let ctx = test_ctx(Arc::clone(&tiles), styles);

        let unstyled = map(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "64"),
                ("height", "64"),
            ]),
            HeaderMap::new(),
        )
        .await;
        let styled = map(
            State(Arc::clone(&ctx)),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "64"),
                ("height", "64"),
                ("style", "basic"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(unstyled.status(), StatusCode::OK);
        assert_eq!(styled.status(), StatusCode::OK);
        let unstyled_body = to_bytes(unstyled.into_body(), usize::MAX).await.unwrap();
        let styled_body = to_bytes(styled.into_body(), usize::MAX).await.unwrap();
        assert_ne!(
            unstyled_body, styled_body,
            "a styled and an unstyled render over the same window must not collide in the cache"
        );
        assert_eq!(
            tiles.call_count(),
            1,
            "the single covering tile's MVT bytes are cached at the Encoding::Mvt \
             level and reused across both the styled and unstyled Map entries"
        );
    }

    /// The `#23`-style regression this lane must not reintroduce (see
    /// `crate::handlers::concurrent_png_misses_on_one_tile_coalesce_to_a_single_rasterize`'s
    /// own doc for the original bug): N concurrent misses on the exact same
    /// `/map` window must coalesce into exactly one render, not N.
    #[tokio::test]
    async fn concurrent_identical_map_requests_coalesce_to_a_single_render() {
        let tiles = Arc::new(FakeTileSource::with_delay(
            std::time::Duration::from_millis(30),
        ));
        let ctx = test_ctx(Arc::clone(&tiles), HashMap::new());

        let mut handles = Vec::new();
        for _ in 0..16 {
            let ctx = Arc::clone(&ctx);
            handles.push(tokio::spawn(async move {
                let response = map(
                    State(ctx),
                    cid_path("demo"),
                    query(&[
                        ("bbox", &whole_world_bbox()),
                        ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                        ("width", "64"),
                        ("height", "64"),
                    ]),
                    HeaderMap::new(),
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(
            tiles.call_count(),
            1,
            "16 concurrent identical /map requests must coalesce into exactly one MVT fetch"
        );
    }

    // -- `#37`: the RASTER maps lane ------------------------------------
    //
    // Every case below drives a REAL raster driver — `tellurion-cog`'s or
    // `tellurion-zarr`'s — through the real `DriverFactory` ->
    // `StorageDriver` -> `RasterSource` contract a booted server routes
    // through, over the real `Router`, the real byte-budgeted cache and the
    // real `map` handler. A fake `RasterSource` would pin nothing about the
    // three things this lane's whole claim is that it REUSES: the driver's
    // own per-request source-pixel budget, its colormap classification, and
    // (for Zarr) its fixed leading-dimension slice selection. The
    // dev-dependency arrow this needs is the same one `tellurion-render`'s
    // colormap goldens already established, and it points the same way:
    // dev-only, drivers still know nothing about this crate.

    /// `tellurion-cog`'s own committed single-band gradient fixture: a
    /// 32x32 `Gray` GeoTIFF spanning CRS84 `[-1.28, 1.28]` on both axes,
    /// whose 16x16 bottom-right block carries every value in `0..=255`
    /// exactly once (see that crate's `examples/gen_fixture.rs`). Reached by
    /// relative path rather than copied here, exactly as
    /// `tellurion-render`'s goldens and `tellurion-server`'s COG binary
    /// proof already reach for it.
    fn cog_fixture() -> String {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tellurion-cog/tests/fixtures/gray_gradient.tif")
            .to_string_lossy()
            .into_owned()
    }

    /// The `WebMercatorQuad` tile at `z8/x128/y128` — CRS84 `lon [0,
    /// 1.40625], lat [-1.405, 0]`, the fixture's bottom-right quadrant and
    /// therefore the one tile that carries its full 256-value range. The
    /// same tile `tellurion-render`'s own COG colormap goldens pin, chosen
    /// for the same reason.
    const COG_QUADRANT: TileCoord = TileCoord {
        z: 8,
        x: 128,
        y: 128,
    };

    /// `bbox` (mercator metres — so every caller must declare
    /// `bbox-crs=WEB_MERCATOR_QUAD_CRS` alongside it, `#270`) for one
    /// whole tile, inset by a metre on each side.
    ///
    /// The inset is load-bearing, not cosmetic: `mercator::covering_tiles`
    /// floors each edge into a tile index, so a bbox whose maximum lands
    /// exactly on a tile boundary covers the NEXT tile as well. Insetting
    /// keeps these cases at exactly one covering tile, which is what makes
    /// their driver-call counts and their pixel content predictable.
    fn tile_window_bbox(coord: TileCoord) -> String {
        let [minx, miny, maxx, maxy] = mercator::tile_bounds_m(coord.z, coord.x, coord.y);
        format!(
            "{},{},{},{}",
            minx + 1.0,
            miny + 1.0,
            maxx - 1.0,
            maxy - 1.0
        )
    }

    /// An explicit stop list in primary colours no built-in ramp ever
    /// produces, at values the COG fixture is known to carry exactly — so
    /// "this image was classified by THIS colormap" is checkable by looking
    /// for one specific RGBA, not merely by two renders differing.
    const PRIMARY_STOPS_YAML: &str = "{ kind: stops, stops: [ \
        { value: 0.0, rgba: [255, 0, 0, 255] }, \
        { value: 128.0, rgba: [0, 255, 0, 255] }, \
        { value: 255.0, rgba: [0, 0, 255, 255] } ] }";
    const VIRIDIS_YAML: &str = "{ kind: ramp, ramp: viridis, min: 0.0, max: 255.0 }";
    /// `viridis`' own `t = 0` control point — a colour
    /// [`PRIMARY_STOPS_YAML`] can never produce, and vice versa.
    const VIRIDIS_MIN_RGBA: [u8; 4] = [68, 1, 84, 255];
    const PRIMARY_MIN_RGBA: [u8; 4] = [255, 0, 0, 255];

    /// Every process-wide `url_env` a raster fixture is handed to the
    /// driver through gets its own uniquely-numbered name, and the write is
    /// serialized against the `DriverFactory::build` that reads it: cargo
    /// runs this binary's tests on several threads and the process
    /// environment is one shared table however distinct the names in it
    /// are. Same shape, same reason, as `tellurion-render`'s own colormap
    /// goldens.
    fn raster_router(driver: &str, locator: &str, config_yaml: &str) -> (AppConfig, Registry) {
        use std::sync::Mutex;
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        static ENV_LOCK: Mutex<()> = Mutex::new(());

        let url_env = format!(
            "TELLURION_TILES_MAPS_RASTER_SRC_{}",
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let yaml = format!(
            "storages: [ {{ id: main, driver: {driver}, url_env: {url_env} }} ]\n\
             tenants: [ {{ id: public }} ]\n\
             catalogs: [ {{ id: default, tenant: public }} ]\n\
             {config_yaml}"
        );
        let config: AppConfig = serde_yaml::from_str(&yaml)
            .unwrap_or_else(|error| panic!("raster test config: {error}\n{yaml}"));
        config.validate().unwrap();

        let mut registry = Registry::new();
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::env::set_var(&url_env, locator);
        match driver {
            "cog" => registry.register(Arc::new(tellurion_cog::CogDriverFactory::new())),
            "zarr" => registry.register(Arc::new(tellurion_zarr::ZarrDriverFactory::new())),
            other => panic!("unknown raster driver '{other}'"),
        }
        (config, registry)
    }

    /// An `AppContext` over a real raster driver, sharing `cache` — passed
    /// in rather than built here so a cache-key-separation case can point
    /// two differently-configured deployments at the SAME byte-budgeted
    /// cache, which is the only way to prove two keys do not collide in it.
    fn raster_ctx_with_cache(
        driver: &str,
        locator: &str,
        config_yaml: &str,
        cache: Arc<dyn TileCache>,
    ) -> Arc<AppContext> {
        let (config, registry) = raster_router(driver, locator, config_yaml);
        let router = Router::build(&config, &registry).unwrap();
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

    fn fresh_cache() -> Arc<dyn TileCache> {
        Arc::new(MokaTileCache::with_byte_budget(10_000_000))
    }

    fn raster_ctx(driver: &str, locator: &str, config_yaml: &str) -> Arc<AppContext> {
        raster_ctx_with_cache(driver, locator, config_yaml, fresh_cache())
    }

    /// One raster-backed collection pinned to a single zoom level, with
    /// `colormap` spliced in verbatim.
    ///
    /// The collection id is the driver's own logical name for the store —
    /// a COG's file stem, a Zarr array's directory name — because a
    /// collection that overrides neither `table` nor `geometry`/`pk` is
    /// resolved against `CatalogSource::collections` by name
    /// (`Router::effective_decl`), exactly as a real deployment's config
    /// does (see `tellurion-server/tests/cog_binary.rs`'s own config).
    fn raster_collection(id: &str, colormap_yaml: &str, zoom: u8) -> String {
        format!(
            "collections:\n\
             \x20 - id: {id}\n\
             \x20   catalog: default\n\
             \x20   storage: main\n\
             \x20   tiles: {{ minzoom: {zoom}, maxzoom: {zoom}, caps: {{}} }}\n\
             \x20   settings: {{ colormap: {colormap_yaml} }}\n"
        )
    }

    /// The COG fixture's own collection id — see [`raster_collection`].
    const COG_CID: &str = "gray_gradient";

    fn cog_collections(colormap_yaml: &str, zoom: u8) -> String {
        raster_collection(COG_CID, colormap_yaml, zoom)
    }

    async fn png_body(response: Response) -> Vec<u8> {
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    fn distinct_colors(pixmap: &tiny_skia::Pixmap) -> std::collections::BTreeSet<[u8; 4]> {
        pixmap
            .pixels()
            .iter()
            .map(|p| {
                let p = p.demultiply();
                [p.red(), p.green(), p.blue(), p.alpha()]
            })
            .collect()
    }

    /// GATE 1 + 2 + 3: a COG-backed collection — no vector `TileSource`
    /// anywhere in its maps lane — serves a real `/map`, in the requested
    /// window, with both Maps Part 1 Core response headers
    /// (`/req/core/map-response` C/D/E), classified by the collection's own
    /// configured colormap.
    #[tokio::test]
    async fn a_cog_backed_collection_serves_a_real_map_with_both_content_headers() {
        let ctx = raster_ctx(
            "cog",
            &cog_fixture(),
            &cog_collections(PRIMARY_STOPS_YAML, COG_QUADRANT.z),
        );
        let response = map(
            State(ctx),
            cid_path(COG_CID),
            query(&[
                ("bbox", &tile_window_bbox(COG_QUADRANT)),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "128"),
                ("height", "128"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_CRS_HEADER).unwrap(),
            &format!("<{WEB_MERCATOR_QUAD_CRS}>"),
            "/req/core/map-response C: every 200 names the CRS it was rendered in"
        );
        let reported: Vec<f64> = response
            .headers()
            .get(CONTENT_BBOX_HEADER)
            .expect("/req/core/map-response D: every 200 carries Content-Bbox")
            .to_str()
            .unwrap()
            .split(',')
            .map(|value| value.parse().unwrap())
            .collect();
        let [minx, miny, maxx, maxy] =
            mercator::tile_bounds_m(COG_QUADRANT.z, COG_QUADRANT.x, COG_QUADRANT.y);
        for (reported, expected) in
            reported
                .iter()
                .zip([minx + 1.0, miny + 1.0, maxx - 1.0, maxy - 1.0])
        {
            assert!((reported - expected).abs() < 1e-6, "{reported} {expected}");
        }

        let body = png_body(response).await;
        assert_eq!(&body[0..8], &PNG_MAGIC);
        let pixmap = tiny_skia::Pixmap::decode_png(&body).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (128, 128));
        let colors = distinct_colors(&pixmap);
        assert!(
            colors.len() >= 32,
            "the composited window has only {} distinct colours — a blank or \
             single-colour image would pass a mere 200 assertion while proving \
             nothing reached the render path",
            colors.len()
        );
        assert!(
            colors.contains(&PRIMARY_MIN_RGBA),
            "the collection's configured stop for sample 0 must appear verbatim: \
             the colormap is applied by the SAME driver call the raster tile lane \
             makes, so its absence means the configuration never reached it"
        );
    }

    /// GATE 3: the SAME window under two different collection colormaps
    /// renders two different images, each carrying a colour only its own
    /// colormap can produce. Without this, "the colormap is applied" is a
    /// claim a fully-transparent image would also satisfy.
    #[tokio::test]
    async fn a_cog_map_is_classified_by_the_collections_own_colormap() {
        let bbox = tile_window_bbox(COG_QUADRANT);
        let mut rendered = Vec::new();
        for colormap in [PRIMARY_STOPS_YAML, VIRIDIS_YAML] {
            let ctx = raster_ctx(
                "cog",
                &cog_fixture(),
                &cog_collections(colormap, COG_QUADRANT.z),
            );
            let response = map(
                State(ctx),
                cid_path(COG_CID),
                query(&[
                    ("bbox", &bbox),
                    ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                    ("width", "64"),
                    ("height", "64"),
                ]),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            rendered.push(png_body(response).await);
        }
        assert_ne!(
            rendered[0], rendered[1],
            "two different colormaps produced byte-identical maps — the configured \
             colormap is not reaching this lane's render path at all"
        );
        let stops = distinct_colors(&tiny_skia::Pixmap::decode_png(&rendered[0]).unwrap());
        let viridis = distinct_colors(&tiny_skia::Pixmap::decode_png(&rendered[1]).unwrap());
        assert!(stops.contains(&PRIMARY_MIN_RGBA) && !stops.contains(&VIRIDIS_MIN_RGBA));
        assert!(viridis.contains(&VIRIDIS_MIN_RGBA) && !viridis.contains(&PRIMARY_MIN_RGBA));
    }

    /// GATE 5: a window whose covering tiles would take the DRIVER over its
    /// own per-request source-pixel budget is refused BY NAME, with the
    /// same problem code the raster TILE lane already uses — never a
    /// clamped window, never a whole-source read, never an opaque 500.
    ///
    /// Reached through this lane's own SOURCE-side budget, which is checked
    /// before any driver call at all: a window spanning far more of the
    /// pyramid than its output size shows.
    #[tokio::test]
    async fn a_raster_map_over_the_source_pixel_budget_is_refused_by_name() {
        let ctx = raster_ctx("cog", &cog_fixture(), &cog_collections(VIRIDIS_YAML, 12));
        let response = map(
            State(ctx),
            cid_path(COG_CID),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "16"),
                ("height", "16"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["code"], "PixelBudgetExceeded");
        let detail = json["detail"].as_str().unwrap();
        assert!(detail.contains("budget"), "{detail}");
    }

    /// GATE 5: `style` on the raster lane is refused BY NAME rather than
    /// silently ignored. An ignored `style` would be a 200 the client
    /// cannot tell apart from a styled render.
    #[tokio::test]
    async fn a_raster_map_refuses_a_style_parameter_by_name() {
        let ctx = raster_ctx(
            "cog",
            &cog_fixture(),
            &cog_collections(VIRIDIS_YAML, COG_QUADRANT.z),
        );
        let response = map(
            State(ctx),
            cid_path(COG_CID),
            query(&[
                ("bbox", &tile_window_bbox(COG_QUADRANT)),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "32"),
                ("height", "32"),
                ("style", "basic"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["code"], "CapabilityUnsupported");
        let detail = json["detail"].as_str().unwrap();
        assert!(
            detail.contains("styled-map") && detail.contains("basic"),
            "{detail}"
        );
    }

    /// GATE 5: a colormap this GeoTIFF's band layout cannot honour is the
    /// driver's own capability refusal, surfaced BY NAME as a 400 rather
    /// than an opaque 500 — the identical treatment the raster TILE lane
    /// gives the identical driver error.
    #[tokio::test]
    async fn a_raster_map_surfaces_a_driver_capability_refusal_by_name() {
        let rgb_fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tellurion-cog/tests/fixtures/tiled_rgb.tif")
            .to_string_lossy()
            .into_owned();
        let ctx = raster_ctx(
            "cog",
            &rgb_fixture,
            &raster_collection("tiled_rgb", VIRIDIS_YAML, 0),
        );
        let response = map(
            State(ctx),
            cid_path("tiled_rgb"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "32"),
                ("height", "32"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["code"], "CapabilityUnsupported");
        assert!(
            json["detail"].as_str().unwrap().contains("colormap"),
            "{}",
            json["detail"]
        );
    }

    /// GATE 6, empty extent (the covered-window half): a window the raster
    /// genuinely does not reach is a transparent image of the requested
    /// size, not a refusal and not a guessed fill — and `Content-Bbox` says
    /// which window that was, which is what lets a client tell an empty
    /// answer apart from a wrong one.
    #[tokio::test]
    async fn a_raster_map_over_a_window_the_source_never_covers_is_transparent() {
        let ctx = raster_ctx(
            "cog",
            &cog_fixture(),
            &cog_collections(VIRIDIS_YAML, COG_QUADRANT.z),
        );
        // The antipodal quadrant: nowhere near the fixture's own
        // `[-1.28, 1.28]` degree extent.
        let far = TileCoord { z: 8, x: 8, y: 8 };
        let response = map(
            State(ctx),
            cid_path(COG_CID),
            query(&[
                ("bbox", &tile_window_bbox(far)),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "32"),
                ("height", "32"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(CONTENT_BBOX_HEADER).is_some());
        let body = png_body(response).await;
        let pixmap = tiny_skia::Pixmap::decode_png(&body).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (32, 32));
        assert!(
            pixmap.pixels().iter().all(|p| p.alpha() == 0),
            "a window outside the raster's extent must be transparent, never a \
             guessed colour"
        );
    }

    // -- `#37`: Zarr ----------------------------------------------------

    /// A private, self-cleaning temp directory holding one hand-built Zarr
    /// v2 store. Built here rather than committed for the same reason
    /// `tellurion-render`'s own Zarr goldens build theirs: a small Zarr v2
    /// store is a handful of tiny files, cheaper to write than to store in
    /// git. The directory's own NAME is the collection id, because that is
    /// what `ZarrStore::logical_name` reports as the physical collection.
    struct ZarrFixture {
        dir: std::path::PathBuf,
    }

    impl ZarrFixture {
        /// A 16x16 single-band `u8` array chunked 8x8, raw, whose sample at
        /// `(y, x)` is `y * 16 + x` — a bijection onto `0..=255`, so one
        /// map over its whole extent carries every possible byte value
        /// exactly once. Declares the Web Mercator world extent, so the
        /// `z0/x0/y0` tile covers it exactly.
        fn gradient_2d() -> Self {
            let dir = Self::new_dir(ZARR_CID);
            std::fs::write(
                dir.join(".zarray"),
                r#"{"zarr_format":2,"shape":[16,16],"chunks":[8,8],"dtype":"|u1","compressor":null,"fill_value":0,"order":"C"}"#,
            )
            .unwrap();
            std::fs::write(dir.join(".zattrs"), ZARR_WORLD_ZATTRS).unwrap();
            for chunk_y in 0..2u32 {
                for chunk_x in 0..2u32 {
                    let mut chunk = Vec::with_capacity(64);
                    for row in 0..8u32 {
                        for col in 0..8u32 {
                            chunk.push(((chunk_y * 8 + row) * 16 + (chunk_x * 8 + col)) as u8);
                        }
                    }
                    std::fs::write(dir.join(format!("{chunk_y}.{chunk_x}")), &chunk).unwrap();
                }
            }
            Self { dir }
        }

        /// A 3D array with a leading `time` dimension of length 2 (chunked
        /// 1, so each step is its own chunk) over a trailing 8x8 `(y, x)`:
        /// step 0 is the constant `50`, step 1 the constant `200`.
        /// `tellurion:fixed_index` selects `fixed`, so which constant the
        /// rendered map shows says, unambiguously, which slice the driver
        /// read.
        fn fixed_slice_3d(fixed: u64) -> Self {
            let dir = Self::new_dir(ZARR_CID);
            std::fs::write(
                dir.join(".zarray"),
                r#"{"zarr_format":2,"shape":[2,8,8],"chunks":[1,4,4],"dtype":"|u1","compressor":null,"fill_value":0,"order":"C"}"#,
            )
            .unwrap();
            std::fs::write(
                dir.join(".zattrs"),
                format!(
                    r#"{{"tellurion:extent_crs84":[-180.0,-85.0511287798066,180.0,85.0511287798066],"tellurion:fixed_index":[{fixed}]}}"#
                ),
            )
            .unwrap();
            for (step, value) in [(0u32, 50u8), (1u32, 200u8)] {
                for chunk_y in 0..2u32 {
                    for chunk_x in 0..2u32 {
                        std::fs::write(
                            dir.join(format!("{step}.{chunk_y}.{chunk_x}")),
                            [value; 16],
                        )
                        .unwrap();
                    }
                }
            }
            Self { dir }
        }

        fn new_dir(name: &str) -> std::path::PathBuf {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir()
                .join(format!(
                    "tellurion-tiles-maps-zarr-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::SeqCst)
                ))
                .join(name);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn locator(&self) -> String {
            self.dir.to_string_lossy().into_owned()
        }
    }

    impl Drop for ZarrFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const ZARR_CID: &str = "zarr_demo";
    const ZARR_WORLD_ZATTRS: &str =
        r#"{"tellurion:extent_crs84":[-180.0,-85.0511287798066,180.0,85.0511287798066]}"#;
    /// Explicit stops at the two constants the fixed-slice fixture carries,
    /// in colours nothing else in these cases can produce.
    const SLICE_STOPS_YAML: &str = "{ kind: stops, stops: [ \
        { value: 50.0, rgba: [255, 0, 0, 255] }, \
        { value: 200.0, rgba: [0, 0, 255, 255] } ] }";
    const SLICE_LOW_RGBA: [u8; 4] = [255, 0, 0, 255];
    const SLICE_HIGH_RGBA: [u8; 4] = [0, 0, 255, 255];

    /// GATE 1 + 3: a Zarr-backed collection resolves the maps lane through
    /// `RasterSource` and serves a real, colormap-classified map — the same
    /// proof the COG case makes, over the other raster driver, because
    /// "raster maps work" must not mean "one driver's raster maps work".
    #[tokio::test]
    async fn a_zarr_backed_collection_serves_a_real_map_through_the_raster_lane() {
        let fixture = ZarrFixture::gradient_2d();
        let ctx = raster_ctx(
            "zarr",
            &fixture.locator(),
            &raster_collection(ZARR_CID, PRIMARY_STOPS_YAML, 0),
        );
        let response = map(
            State(ctx),
            cid_path(ZARR_CID),
            query(&[
                ("bbox", &tile_window_bbox(TileCoord { z: 0, x: 0, y: 0 })),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "64"),
                ("height", "64"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_CRS_HEADER).unwrap(),
            &format!("<{WEB_MERCATOR_QUAD_CRS}>")
        );
        assert!(response.headers().get(CONTENT_BBOX_HEADER).is_some());
        let body = png_body(response).await;
        assert_eq!(&body[0..8], &PNG_MAGIC);
        let pixmap = tiny_skia::Pixmap::decode_png(&body).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (64, 64));
        let colors = distinct_colors(&pixmap);
        assert!(
            colors.len() >= 32,
            "only {} distinct colours — the Zarr array's own gradient did not reach \
             the composited window",
            colors.len()
        );
        assert!(colors.contains(&PRIMARY_MIN_RGBA));
    }

    /// GATE 3, the fixed-slice half: the leading-dimension slice the array's
    /// own `.zattrs` selects is the one the MAP shows. The two fixtures
    /// differ only in `tellurion:fixed_index`, and each renders exclusively
    /// its own slice's colour — so a lane that ignored the declaration, or
    /// read slice 0 unconditionally, fails here rather than passing on a
    /// 200.
    #[tokio::test]
    async fn a_zarr_map_shows_the_arrays_own_fixed_leading_dimension_slice() {
        for (fixed, expected, forbidden) in [
            (1u64, SLICE_HIGH_RGBA, SLICE_LOW_RGBA),
            (0u64, SLICE_LOW_RGBA, SLICE_HIGH_RGBA),
        ] {
            let fixture = ZarrFixture::fixed_slice_3d(fixed);
            let ctx = raster_ctx(
                "zarr",
                &fixture.locator(),
                &raster_collection(ZARR_CID, SLICE_STOPS_YAML, 0),
            );
            let response = map(
                State(ctx),
                cid_path(ZARR_CID),
                query(&[
                    ("bbox", &tile_window_bbox(TileCoord { z: 0, x: 0, y: 0 })),
                    ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                    ("width", "32"),
                    ("height", "32"),
                ]),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = png_body(response).await;
            let colors = distinct_colors(&tiny_skia::Pixmap::decode_png(&body).unwrap());
            assert!(
                colors.contains(&expected),
                "fixed_index [{fixed}]: the selected slice's own colour {expected:?} is \
                 absent from the rendered map"
            );
            assert!(
                !colors.contains(&forbidden),
                "fixed_index [{fixed}]: the OTHER slice's colour {forbidden:?} reached \
                 the rendered map"
            );
        }
    }

    /// GATE 3: a Zarr array given no colormap at all has no visual meaning
    /// of its own — the driver refuses by name and this lane surfaces that
    /// refusal as a named 400, never a guessed default scaling.
    #[tokio::test]
    async fn a_zarr_map_without_a_configured_colormap_is_refused_by_name() {
        let fixture = ZarrFixture::gradient_2d();
        let (config, registry) = raster_router(
            "zarr",
            &fixture.locator(),
            &format!(
                "collections:\n\
                 \x20 - id: {ZARR_CID}\n\
                 \x20   catalog: default\n\
                 \x20   storage: main\n\
                 \x20   tiles: {{ minzoom: 0, maxzoom: 0, caps: {{}} }}\n"
            ),
        );
        let router = Router::build(&config, &registry).unwrap();
        let style_store: Arc<dyn StyleStore> = Arc::new(FakeStyleStore {
            styles: HashMap::new(),
        });
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            fresh_cache(),
            style_store,
        ));
        let response = map(
            State(ctx),
            cid_path(ZARR_CID),
            query(&[
                ("bbox", &tile_window_bbox(TileCoord { z: 0, x: 0, y: 0 })),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "32"),
                ("height", "32"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["code"], "CapabilityUnsupported");
        assert!(
            json["detail"].as_str().unwrap().contains("colormap"),
            "{}",
            json["detail"]
        );
    }

    // -- `#37`: capability honesty --------------------------------------

    /// A catalog that reports the collection but knows no extent for it —
    /// the "this collection has no window to default to" case
    /// [`collection_window`] answers `None` for. [`ExtentCatalog`] can't
    /// express it (it always has a bbox) and [`EmptyCatalog`] reports no
    /// collection at all, which fails resolution outright.
    struct NoExtentCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for NoExtentCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![PhysicalCollection {
                name: "demo".to_string(),
                geometry_column: None,
                primary_key: None,
                srid: Some(4326),
                geometry_type: None,
            }])
        }
    }

    /// A raster source that would happily serve any tile — present only so
    /// the collection RESOLVES through the raster maps lane, which is what
    /// makes the extent refusal below a refusal of the WINDOW rather than
    /// of the capability.
    struct AlwaysCoveringRasterSource;

    #[async_trait::async_trait]
    impl RasterSource for AlwaysCoveringRasterSource {
        async fn raster_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
        ) -> CoreResult<Option<RasterWindow>> {
            Ok(Some(RasterWindow {
                width: 1,
                height: 1,
                rgba: vec![255, 255, 255, 255],
            }))
        }
    }

    /// `raster` mounts [`AlwaysCoveringRasterSource`]; `false` mounts
    /// neither capability at all — the "unsupported driver" fixture.
    struct BareDriver {
        raster: bool,
    }

    impl StorageDriver for BareDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(NoExtentCatalog)
        }

        fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
            self.raster
                .then(|| Arc::new(AlwaysCoveringRasterSource) as Arc<dyn RasterSource>)
        }
    }

    struct BareFactory {
        raster: bool,
    }

    impl DriverFactory for BareFactory {
        fn name(&self) -> &str {
            "bare"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(BareDriver {
                raster: self.raster,
            }))
        }
    }

    fn bare_ctx(raster: bool) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: bare, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    tiles: { minzoom: 0, maxzoom: 0, caps: {} }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(BareFactory { raster }));
        let router = Router::build(&config, &registry).unwrap();
        let style_store: Arc<dyn StyleStore> = Arc::new(FakeStyleStore {
            styles: HashMap::new(),
        });
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            fresh_cache(),
            style_store,
        ))
    }

    /// GATE 6, unsupported driver: a storage advertising NEITHER `TileSource`
    /// nor `RasterSource` does not serve a map — and answers exactly what it
    /// answered before this slice existed, a `404`.
    ///
    /// This is deliberately not a named 400. A `resolve_*` failure is a 404
    /// on every lane in this crate (`handlers::tile` included), and rule 1
    /// says an unconfigured deployment behaves byte-for-byte as it did. The
    /// capability honesty this slice owes such a collection is instead that
    /// nothing ADVERTISES the route for it — see
    /// `tellurion-server`'s own `MapsLinkContributor` tests.
    #[tokio::test]
    async fn a_collection_whose_driver_has_neither_capability_serves_no_map() {
        let response = map(
            State(bare_ctx(false)),
            cid_path("demo"),
            query(&[
                ("bbox", &whole_world_bbox()),
                ("width", "32"),
                ("height", "32"),
            ]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// GATE 6, empty extent (the no-derivable-window half): a RASTER
    /// collection whose extent is unknown is refused BY NAME for a
    /// `bbox`-less request, exactly as a vector one already is — never
    /// handed a world bbox it never asked for and never a whole-source read.
    #[tokio::test]
    async fn a_raster_map_without_bbox_refuses_by_name_when_no_extent_is_known() {
        let response = map(
            State(bare_ctx(true)),
            cid_path("demo"),
            query(&[("width", "16"), ("height", "16")]),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["code"], "CapabilityUnsupported");
        let detail = json["detail"].as_str().unwrap();
        assert!(detail.contains("default-extent"), "{detail}");
    }

    // -- `#37`: cache-key separation ------------------------------------

    /// The vector-lane counterpart of [`raster_ctx_with_cache`]: a
    /// `FakeTileSource`-backed collection under a CHOSEN id, pinned to a
    /// chosen zoom, over a SHARED cache — everything a collision test needs
    /// to make the two lanes' keys agree on every field except the one
    /// under test.
    fn vector_ctx_named(
        tiles: Arc<FakeTileSource>,
        cid: &str,
        zoom: u8,
        cache: Arc<dyn TileCache>,
    ) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]\n\
             tenants: [ {{ id: public }} ]\n\
             catalogs: [ {{ id: default, tenant: public }} ]\n\
             collections:\n\
             \x20 - id: {cid}\n\
             \x20   catalog: default\n\
             \x20   storage: main\n\
             \x20   table: demo\n\
             \x20   geometry: geom\n\
             \x20   pk: id\n\
             \x20   tiles: {{ minzoom: {zoom}, maxzoom: {zoom}, caps: {{}} }}\n"
        ))
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            tiles,
            extent: None,
        }));
        let router = Router::build(&config, &registry).unwrap();
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

    /// GATE 6, cache-key separation (lane): two deployments of the SAME
    /// tenant/catalog/collection id, one vector-backed and one COG-backed,
    /// over ONE shared byte-budgeted cache and the identical window and
    /// output size. Every other component of the key is equal by
    /// construction, so if the lane were not part of the key the second
    /// request would be served the first's cached bytes — and this
    /// assertion would find them byte-identical.
    #[tokio::test]
    async fn a_raster_map_and_a_vector_map_of_the_same_window_do_not_share_a_cache_entry() {
        let cache = fresh_cache();
        let bbox = tile_window_bbox(COG_QUADRANT);
        let params = || {
            query(&[
                ("bbox", bbox.as_str()),
                ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                ("width", "64"),
                ("height", "64"),
            ])
        };

        let tiles = Arc::new(FakeTileSource::new());
        let vector = vector_ctx_named(
            Arc::clone(&tiles),
            COG_CID,
            COG_QUADRANT.z,
            Arc::clone(&cache),
        );
        let vector_response =
            map(State(vector), cid_path(COG_CID), params(), HeaderMap::new()).await;
        assert_eq!(vector_response.status(), StatusCode::OK);
        let vector_body = png_body(vector_response).await;

        let raster = raster_ctx_with_cache(
            "cog",
            &cog_fixture(),
            &cog_collections(PRIMARY_STOPS_YAML, COG_QUADRANT.z),
            Arc::clone(&cache),
        );
        let raster_response =
            map(State(raster), cid_path(COG_CID), params(), HeaderMap::new()).await;
        assert_eq!(raster_response.status(), StatusCode::OK);
        let raster_body = png_body(raster_response).await;

        assert_ne!(
            vector_body, raster_body,
            "a raster map and a vector map of the same window collided in the shared \
             cache: whichever rendered first is now answering for both lanes"
        );
        assert_eq!(
            tiles.call_count(),
            1,
            "the raster request must not have reached the vector tile source at all"
        );
        let raster_colors = distinct_colors(&tiny_skia::Pixmap::decode_png(&raster_body).unwrap());
        assert!(
            raster_colors.contains(&PRIMARY_MIN_RGBA),
            "the raster answer must be the raster render, not the vector one"
        );
    }

    /// GATE 6, cache-key separation (colormap): the SAME collection, the
    /// same window, one shared cache, two colormap configurations — the
    /// shape a config reload produces, which the tile cache deliberately
    /// does NOT participate in. Without the colormap fingerprint in the key
    /// the second render is the first's bytes under a stale colormap,
    /// forever.
    #[tokio::test]
    async fn two_colormaps_over_the_same_raster_window_do_not_share_a_cache_entry() {
        let cache = fresh_cache();
        let bbox = tile_window_bbox(COG_QUADRANT);
        let mut rendered = Vec::new();
        for colormap in [PRIMARY_STOPS_YAML, VIRIDIS_YAML] {
            let ctx = raster_ctx_with_cache(
                "cog",
                &cog_fixture(),
                &cog_collections(colormap, COG_QUADRANT.z),
                Arc::clone(&cache),
            );
            let response = map(
                State(ctx),
                cid_path(COG_CID),
                query(&[
                    ("bbox", bbox.as_str()),
                    ("bbox-crs", WEB_MERCATOR_QUAD_CRS),
                    ("width", "64"),
                    ("height", "64"),
                ]),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            rendered.push(png_body(response).await);
        }
        assert_ne!(
            rendered[0], rendered[1],
            "two colormap configurations of the same window collided in the shared \
             cache: a config reload would keep serving the previous colormap's bytes"
        );
    }

    /// The key itself, asserted directly — the property the two end-to-end
    /// cases above exercise, stated where a reader can check it without
    /// running a driver.
    #[test]
    fn the_map_cache_key_separates_lanes_and_colormaps() {
        let request = MapRequest {
            bbox_mercator: [0.0, 0.0, 1.0, 1.0],
            crs: MapCrs::WebMercator,
            width: 16,
            height: 16,
            style_id: None,
        };
        let key = |lane| map_key("t", "c", "demo", &request, lane, None, Vec::new());
        let vector = key(MapLane::Vector);
        let raster_none = key(MapLane::Raster(None));
        let raster_a = key(MapLane::Raster(Some(1)));
        let raster_b = key(MapLane::Raster(Some(2)));
        assert_ne!(vector, raster_none);
        assert_ne!(raster_none, raster_a);
        assert_ne!(raster_a, raster_b);
    }
}
