//! OGC API — Tiles Part 1 conformance class URIs (OGC 20-057, v1.0), plus
//! OGC API — Maps Part 1 (OGC 20-058, v1.0) — both approved standards.
//! Verified 2026-07: each `.../conf/...` URI below 302-redirects from
//! `www.opengis.net` to its own anchor in the published standard text
//! (`docs.ogc.org/is/{20-057,20-058}/{...}.html#conf_...`), the same
//! resolvable-registered-URI bar every other conformance constant in this
//! workspace is held to.

pub const CONFORMANCE_TILES_CORE: &str = "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/core";
pub const CONFORMANCE_TILESET: &str = "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/tileset";
pub const CONFORMANCE_TILESETS_LIST: &str =
    "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/tilesets-list";
pub const CONFORMANCE_MVT: &str = "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/mvt";
pub const CONFORMANCE_PNG: &str = "http://www.opengis.net/spec/ogcapi-tiles-1/1.0/conf/png";

/// OGC API — Maps Part 1: Core (`#86`) — the `/collections/{cid}/map`
/// resource itself: one rendered image per request. `maps::map` serves it
/// for a VECTOR collection (rasterized from the existing MVT-first tile
/// pipeline) and, since `#37`, for a RASTER one (COG/Zarr, composited from
/// the same `RasterSource::raster_tile` windows the raster PNG tile lane
/// decodes) — see `maps.rs`'s own module doc for the full scope of both.
///
/// `#37` deliberately did NOT add
/// `.../ogcapi-maps-1/1.0/conf/collection-map` alongside this, even though
/// the collection documents now carry that class's own Requirement 46 link
/// (`/req/collection-map/desc-links`, the `map` rel — see
/// `tellurion-server`'s `MapsLinkContributor`). That class also carries
/// Requirement 47 (`/req/collection-map/desc-crs`): "The crs property in
/// the collection object of a geospatial collection SHALL contain URI or
/// safe CURIEs for the list of CRSs supported by the server for that
/// collection." A raster collection's document here carries no such list,
/// so the class is not declared. One honoured requirement out of three is
/// not a conformance class.
///
/// `#229`: Core is the class defined by the request that constrains
/// NOTHING — `bbox`/`bbox-crs` belong to Spatial Subsetting and
/// `width`/`height` to Scaling, so a Core implementation has to answer
/// `GET .../map` with no query parameters at all
/// (`/req/core/map-op`). `maps::map` now does, deriving its window from the
/// collection's own extent and its size from the tile grid's own native
/// scale, and refusing by name where neither can be derived. It also sets
/// the `Content-Crs` and `Content-Bbox` response headers
/// (`/req/core/map-response` C/D/E) that let a client georeference an image
/// it supplied no parameters for. The default output CRS is this lane's own
/// storage CRS — the `WebMercatorQuad` pyramid every map here composites
/// from (`/req/core/map-response` B) — and is always named explicitly on
/// `Content-Crs`.
pub const CONFORMANCE_MAPS_CORE: &str = "http://www.opengis.net/spec/ogcapi-maps-1/1.0/conf/core";
/// OGC API — Maps Part 1: Coordinate Reference System (`#229`) — the `crs`
/// query parameter selecting the output CRS of a map
/// (`/req/crs/crs-definition`, `/req/crs/map-success`). `maps::parse_crs`
/// accepts CRS84 (the value the class requires of every implementation) and
/// this lane's own `WebMercatorQuad` CRS, refuses anything else by name
/// with a 400, and `maps::output_bbox`/`build_projector` reproject the
/// rendered window into whichever was asked for — the response content is
/// consistent with the requested CRS, and `Content-Crs` names it.
///
/// Deliberately NOT accompanied by `.../conf/spatial-subsetting` or
/// `.../conf/scaling`, which the same lane's `bbox`/`width`/`height`
/// support might suggest: Spatial Subsetting additionally requires the
/// `subset`/`subset-crs` (Requirement 19) and `center`/`center-crs`
/// (Requirement 20) parameters, and Scaling additionally requires
/// `scale-denominator`. Neither is implemented, so neither class is
/// declared — see `#229`.
///
/// `#270` closed ONE of Spatial Subsetting's requirements — Requirement 18
/// clause C, the CRS84 default for an omitted `bbox-crs`, which this lane
/// used to read as its own native CRS — and that changes nothing here.
/// Three of the class's requirements remain unimplemented, and honouring
/// one more of a class is not permission to claim the class: the same
/// arithmetic `#37` applied to `conf/collection-map` (one honoured
/// requirement out of three, so not declared). Nor does `#270` touch what
/// THIS class claims: `conf/crs` is about the OUTPUT `crs` parameter,
/// whose omitted default is the native (storage) CRS per Requirement 35
/// NOTE 2, and which `#270` deliberately left alone — the two parameters
/// have different defaults in the standard, so fixing one is not a reason
/// to move the other.
pub const CONFORMANCE_MAPS_CRS: &str = "http://www.opengis.net/spec/ogcapi-maps-1/1.0/conf/crs";
/// OGC API — Maps Part 1: PNG — the one image format this slice serves.
pub const CONFORMANCE_MAPS_PNG: &str = "http://www.opengis.net/spec/ogcapi-maps-1/1.0/conf/png";
