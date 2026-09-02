//! Capability traits protocol handlers code against. A driver implements the
//! capabilities it has; the `Router` decides whether a request can proceed at
//! resolve time, never mid-handler. Traits are dyn-compatible (`async-trait`)
//! so the `Router` can hold `Arc<dyn FeatureSource>` / `Arc<dyn TileSource>`
//! without knowing the concrete driver type.

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::config::CollectionDecl;
use crate::crs::RequestedCrs;
use crate::error::{Error, Result};
use crate::filter::Filter;
use crate::tms::TileMatrixSet;

/// Inclusive RFC 3339 datetime bounds; either end may be open. Kept as plain
/// strings here so `tellurion-core` stays free of a datetime dependency —
/// parsing/validation belongs to whichever crate ultimately builds a query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatetimeRange {
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemsQuery {
    pub limit: u32,
    pub bbox: Option<[f64; 4]>,
    pub datetime: Option<DatetimeRange>,
    /// Opaque keyset paging token; drivers never accept an OFFSET.
    pub token: Option<String>,
    /// A parsed CQL2 filter expression (`#33`), combined as AND with
    /// `bbox`/`datetime` above. `None` means no `filter` query parameter was
    /// supplied — the pre-`#33` behavior every existing caller keeps by
    /// default. Already validated (syntax and property names) by the time it
    /// reaches a `FeatureSource`; see `filter::validate`. A source that
    /// declines `filter_capable` below must never be handed a `Some` here —
    /// `tellurion-features`' handler refuses the request before calling
    /// `items` at all.
    pub filter: Option<Filter>,
    /// Requested output CRS for every geometry-valued property in the
    /// response (`crs` query parameter, OGC API Features Part 2 CRS by
    /// Reference, `crate::crs`). `RequestedCrs::Omitted` (the `Default`) is
    /// this crate's pre-Part-2-CRS behavior, byte-for-byte — no SQL
    /// transform, regardless of a collection's storage SRID. Already
    /// validated against this collection's supported CRS set by the caller
    /// (`tellurion-features`' handler, via `crs::resolve`) before reaching a
    /// `FeatureSource`, mirroring `filter`'s own validate-then-compile split.
    /// A source that declines `crs_capable` below is only ever handed a
    /// value `crs::can_serve` calls a no-op for it — the same
    /// deny-before-call obligation `filter_capable` documents for `filter`.
    /// `#227` restated that obligation in terms of the work rather than the
    /// variant: what such a source must never be handed is a request that
    /// would need a transform or an axis swap it cannot perform. On a
    /// projected collection that is `RequestedCrs::Crs84`; `Storage` is the
    /// free one there, since a source that never reprojects already emits
    /// its rows in the storage CRS and so honours it by doing nothing. On a
    /// 4326 collection nothing changed: `Crs84` is free and `Storage` (the
    /// authority axis order) is refused before the call.
    pub crs: RequestedCrs,
    /// The CRS `bbox`'s four numbers are already expressed in, after axis
    /// normalization to that CRS's own longitude-first coordinate order —
    /// see `crs::swap_bbox_axes` — by the same caller that resolves `crs`
    /// above (`bbox-crs` query parameter, Part 2). Meaningful only together
    /// with `bbox`.
    ///
    /// `RequestedCrs::Omitted` here is **not** the "compile exactly what you
    /// always compiled" instruction it is for `crs` above (`#255`). An omitted
    /// `bbox-crs` is a positive statement about four numbers the client already
    /// sent: Part 1 Requirement 23 (`/req/core/fc-bbox-definition`) clause C
    /// interprets them as CRS84, and Part 2 Requirement 8
    /// (`/req/crs/fc-bbox-crs-valid-default-value`) says the same. So a source
    /// must read `Omitted` and `Crs84` identically — against a CRS84-equivalent
    /// storage that is the unchanged behaviour anyway, and against a projected
    /// one it is a genuine transform of the box.
    ///
    /// Like `crs`, a source that declines
    /// [`FeatureSource::crs_capable`](crate::storage::FeatureSource::crs_capable)
    /// is never handed a value that would cost it work: `tellurion-features`'
    /// items handler and `tellurion-stac`'s `unservable_bbox_reason` both refuse
    /// a `bbox` by name, before the call, when honouring it against this
    /// collection would need the transform — the same deny-before-call
    /// obligation `filter`/`crs`/`filter_crs` all document.
    pub bbox_crs: RequestedCrs,
    /// The CRS every spatial literal inside `filter` above is expressed in
    /// (`filter-crs` query parameter, OGC API — Features Part 3: Filtering,
    /// 19-079r2, `#217`). Resolved against this collection's own supported
    /// CRS set by the same caller that resolves `crs`/`bbox_crs`
    /// (`tellurion-features`' handler, via `crs::resolve`), so a source is
    /// never handed a CRS its own descriptor never advertised.
    ///
    /// `RequestedCrs::Omitted` (the `Default`, and the only value any caller
    /// could produce before `#217`) is this workspace's pre-`filter-crs`
    /// behavior, byte-for-byte: every spatial literal compiles exactly as it
    /// always did, regardless of a collection's storage SRID. That is also
    /// what Requirement 7 (`/req/filter/filter-crs-wgs84`) asks for on a
    /// CRS84-native collection — "If a HTTP GET operation ... includes a
    /// `filter` parameter, but no `filter-crs` parameter, the server SHALL
    /// process all geometries in the filter expression using CRS84 ... as
    /// the coordinate reference system" — which is the CRS every filter
    /// compiler in this workspace has always read a spatial literal in.
    ///
    /// A source that declines [`FeatureSource::filter_crs_capable`] must
    /// never be handed anything but `RequestedCrs::Omitted`/
    /// `RequestedCrs::Crs84` here — the same deny-before-call obligation
    /// `filter`/`crs` already document above, and the direct expression of
    /// Requirement 8's own "The server SHALL return an error, if it does not
    /// support the CRS identified in `filter-crs` for the resource."
    pub filter_crs: RequestedCrs,
}

impl Default for ItemsQuery {
    fn default() -> Self {
        Self {
            limit: 10,
            bbox: None,
            datetime: None,
            token: None,
            filter: None,
            crs: RequestedCrs::Omitted,
            bbox_crs: RequestedCrs::Omitted,
            filter_crs: RequestedCrs::Omitted,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeaturePage {
    pub features_geojson: Vec<serde_json::Value>,
    pub number_matched: Option<u64>,
    pub next_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileCoord {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

#[async_trait::async_trait]
pub trait FeatureSource: Send + Sync {
    async fn items(&self, collection: &CollectionDecl, query: &ItemsQuery) -> Result<FeaturePage>;

    /// `filter` is a `#34` ABAC grant filter (AND-merged with nothing else —
    /// a single-item lookup has no other filter surface to combine with),
    /// `None` when the requesting subject's access is unrestricted. A driver
    /// that declines [`filter_capable`](Self::filter_capable) is never
    /// handed `Some` here, the same contract `ItemsQuery::filter` documents
    /// for the items-list lane — the caller (each protocol crate's own
    /// policy checkpoint) refuses the request before calling `item` at all
    /// when a filter is required but the resolved source can't apply one.
    /// An item that exists but the filter excludes comes back `Ok(None)`,
    /// indistinguishable from an absent id — never a distinct "found but
    /// filtered" signal, so a caller can never leak that a hidden item
    /// exists.
    async fn item(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
    ) -> Result<Option<serde_json::Value>>;

    /// Whether this source can evaluate a `Filter` passed via
    /// `ItemsQuery::filter` (`#33`). A `runtime_checkable`-style capability
    /// marker in the same spirit as `StorageDriver::feature_source`/
    /// `tile_source` being `Option`-shaped, but simpler: filtering is a
    /// refinement of `FeatureSource` (a source can serve items — paging,
    /// bbox, datetime — without being able to compile an arbitrary filter
    /// expression), not a capability with its own resolve entry point, so it
    /// lives as a boolean marker directly on the trait a caller already
    /// holds an `Arc<dyn FeatureSource>` for, rather than a fourth
    /// `StorageDriver::*_source` method. Default `false`. PostGIS overrides
    /// this to `true` (full SQL compilation with bound parameters, see
    /// `tellurion-postgis::sql::compile_filter`); FlatGeobuf stays at the
    /// default (attribute filtering is out of scope for its lane, `#33`).
    /// `tellurion-features`' handler checks this before ever calling `items`
    /// with a non-`None` filter, refusing with a 400 naming the unsupported
    /// capability rather than letting a driver silently ignore or
    /// partially-evaluate a filter it cannot compile.
    fn filter_capable(&self) -> bool {
        false
    }

    /// The CQL2 (1.0, OGC 21-065r2) conformance classes this source's own
    /// filter compiler satisfies (`#105`) — richer than
    /// [`filter_capable`](Self::filter_capable) above: a compiler can accept
    /// one predicate shape and refuse another by name (comparison yes,
    /// `S_INTERSECTS` no, temporal no), which a single `true`/`false` marker
    /// cannot express.
    /// See `filter::CQL2_CONFORMANCE_CLASSES`'s own doc for the full set a
    /// driver could ever declare a subset of, and each overriding driver's
    /// own implementation for exactly which classes it earns and why.
    ///
    /// Declared independently of `filter_capable` rather than derived from
    /// it: `filter_capable` is the hot-path gate `tellurion-features`'/
    /// `tellurion-stac`'s policy checkpoint and every `items`/`item`/
    /// `mvt_tile` call already consults before compiling anything at all,
    /// so it stays the cheap, allocation-free `bool` it always was; this
    /// method builds a `Vec` and is read only for collection-metadata
    /// exposure (`descriptor::canonical::CanonicalCapabilities::
    /// cql2_conformance_classes`) and the workspace-level landing-page
    /// intersection (`crate::router::Router::cql2_conformance_classes`),
    /// both far off any per-item request path. Every overriding driver is
    /// expected to keep the two answers consistent (`filter_capable() ==
    /// !cql2_conformance_classes().is_empty()`) — each one's own test module
    /// pins that invariant. Default empty, matching `filter_capable`'s own
    /// default `false` — FlatGeobuf, GeoParquet, and the memory driver
    /// never override either.
    fn cql2_conformance_classes(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Whether this source can reproject geometry coordinates to a requested
    /// CRS (`ItemsQuery::crs`/`::bbox_crs`, OGC API Features Part 2 CRS by
    /// Reference). Same `runtime_checkable`-style capability marker as
    /// `filter_capable`, and the same reasoning: reprojection is a
    /// refinement of `FeatureSource`, not a capability with its own resolve
    /// entry point. Default `false`; PostGIS overrides this to `true`
    /// (`ST_Transform`/`ST_FlipCoordinates` in SQL, see `tellurion-postgis::
    /// sql`). `tellurion-features`' handler folds this into `crs::can_serve`
    /// before honoring a `crs`/`bbox-crs` that would need real work, refusing
    /// with a 400 naming what the collection *is* served in rather than
    /// silently ignoring the request.
    ///
    /// A source answering `false` here does not thereby serve CRS84 — it
    /// serves its **storage** CRS, unchanged, on every request including one
    /// with no `crs` parameter at all. `crs::content_crs_uri` is what says
    /// so on the wire (`#227`); before that issue the header asserted CRS84
    /// regardless, so a projected collection answered with metres under a
    /// URI naming degrees and no client could tell.
    fn crs_capable(&self) -> bool {
        false
    }

    /// Whether this source can evaluate a `filter`'s own spatial literals in
    /// a CRS the client names rather than in the one its compiler hardcodes
    /// (`ItemsQuery::filter_crs`, `filter-crs` query parameter, OGC API —
    /// Features Part 3: Filtering 19-079r2, `#217`). Same
    /// `runtime_checkable`-style marker as [`filter_capable`](Self::
    /// filter_capable)/[`crs_capable`](Self::crs_capable) above, for the same
    /// reason: it refines `FeatureSource` rather than being a capability with
    /// its own resolve entry point. Default `false`; PostGIS overrides it to
    /// `true` (`ST_Transform`/`ST_FlipCoordinates` around the bound literal,
    /// see `tellurion-postgis::sql::geometry_literal_expr`).
    ///
    /// **Deliberately independent of [`crs_capable`](Self::crs_capable),
    /// not derived from it.** Reprojecting *output* geometry and
    /// transforming an *input* filter literal are different pieces of work,
    /// and `#217` exists precisely because PostGIS had the first without the
    /// second: it answered `crs_capable` `true`, which makes Requirement 8
    /// (`/req/filter/filter-crs-param`, conditional on "Server supports
    /// additional coordinate reference systems") fire, while `filter-crs`
    /// stayed a reserved-but-inert parameter name. Collapsing the two would
    /// make the deployment's Part 3 claim
    /// ([`crate::router::Router::filtering_conformance_classes`], which reads
    /// this) true only by coincidence, and would silently re-claim
    /// `filter-crs` for the next driver that learns to reproject output.
    ///
    /// A source that answers `false` here is never handed a
    /// `ItemsQuery::filter_crs` other than `RequestedCrs::Omitted`/
    /// `RequestedCrs::Crs84`; the caller refuses the request by name first,
    /// exactly as it already does for `crs`/`bbox-crs` against a source
    /// whose `crs_capable` is `false`.
    fn filter_crs_capable(&self) -> bool {
        false
    }

    /// Single-feature counterpart of `ItemsQuery::crs` above — `item` itself
    /// takes no query struct, so a requested output CRS travels as its own
    /// parameter here rather than widening every implementer's `item`
    /// signature for a capability only PostGIS ever exercises.
    /// `requested_crs` is already validated against this collection's
    /// supported CRS set by the caller (`tellurion-features`' handler),
    /// exactly like `ItemsQuery::crs`. The default implementation ignores
    /// `requested_crs` and calls `item` — correct for every driver whose
    /// `crs_capable` stays `false`, since the caller already refuses a
    /// non-default CRS before this is ever reached; PostGIS overrides both
    /// methods together.
    async fn item_with_crs(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
        _requested_crs: RequestedCrs,
    ) -> Result<Option<serde_json::Value>> {
        self.item(collection, id, filter).await
    }
}

#[async_trait::async_trait]
pub trait TileSource: Send + Sync {
    /// `Ok(None)` means an empty tile (valid, nothing to draw), distinct from
    /// `Err` (a real failure resolving/rendering the tile). `filter` is a
    /// `#34` ABAC grant filter, pushed into the tile query exactly the way
    /// `FeatureSource::item`'s own `filter` parameter is — `None` when the
    /// requesting subject's access is unrestricted. A driver that declines
    /// [`filter_capable`](Self::filter_capable) is never handed `Some` here;
    /// the caller (`tellurion-tiles`/`tellurion-places`' own policy
    /// checkpoint) denies the request before calling `mvt_tile` at all when
    /// a filter is required but the resolved source can't apply one.
    async fn mvt_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<Bytes>>;

    /// Whether this source can serve tiles for this effective collection.
    /// The default preserves the existing driver-wide capability contract;
    /// a source whose support depends on metadata derived into
    /// [`CollectionDecl`] can narrow it before Tiles resources are exposed.
    fn tile_capable(&self, _collection: &CollectionDecl) -> bool {
        true
    }

    /// Whether this source's envelope math can honor `tms` (`#190`) — the
    /// tile-grid counterpart of [`filter_capable`](Self::filter_capable),
    /// same default-`false`-shaped marker and same reasoning: serving a
    /// second grid is a refinement of "can serve tiles at all," not a
    /// capability with its own resolve entry point. The default admits only
    /// `WebMercatorQuad`, every driver's native grid — honest for a
    /// pre-baked archive (PMTiles) or any driver whose tile math assumes
    /// mercator (the embedded GeoPackage MVT lane). PostGIS overrides this
    /// to accept `WorldCRS84Quad` too, because it computes each tile's
    /// envelope per request in SQL and can just as well build a CRS84 one.
    /// `tellurion-tiles`' handlers consult this at resolve time and refuse
    /// an unsupported grid by name BEFORE any tile method runs, the same
    /// deny-before-call contract `filter_capable` documents.
    fn supports_tile_matrix_set(&self, tms: TileMatrixSet) -> bool {
        tms == TileMatrixSet::WebMercatorQuad
    }

    /// [`mvt_tile`](Self::mvt_tile) parameterized by tile matrix set
    /// (`#190`) — `coord`'s `z`/`x`/`y` are indices into `tms`'s own grid.
    /// A separate defaulted method rather than a new `mvt_tile` parameter so
    /// every native-grid-only driver (and every test double) keeps its
    /// existing implementation untouched, the same widening pattern
    /// `FeatureSource::item_with_crs` uses. The default delegates the
    /// native grid and refuses anything else with the router's own
    /// `CapabilityUnsupported` shape, by name — belt-and-braces only: the
    /// handlers' resolve-time [`supports_tile_matrix_set`](
    /// Self::supports_tile_matrix_set) check refuses first, so this error
    /// is never a request's primary refusal path. An overriding driver
    /// (PostGIS) must keep the two methods consistent.
    async fn mvt_tile_in(
        &self,
        collection: &CollectionDecl,
        tms: TileMatrixSet,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<Bytes>> {
        if tms != TileMatrixSet::WebMercatorQuad {
            return Err(Error::CapabilityUnsupported {
                collection: collection.id.clone(),
                capability: format!("tileMatrixSet:{tms}"),
            });
        }
        self.mvt_tile(collection, coord, filter).await
    }

    /// Whether this source can evaluate a `#34` ABAC grant `Filter` inside
    /// `mvt_tile` — the tile-lane counterpart of
    /// [`FeatureSource::filter_capable`], same default-`false` marker and
    /// same reasoning (filtering is a refinement of "can serve tiles at
    /// all," not a capability with its own resolve entry point). PostGIS
    /// overrides this to `true` (same `tellurion-postgis::sql::compile_filter`
    /// compiler the features lane uses, reused here for the MVT query's own
    /// `WHERE` clause); PMTiles stays at the default — a pre-baked archive
    /// has no query to filter, only whole tiles.
    fn filter_capable(&self) -> bool {
        false
    }

    /// The real MVT source-layer name(s) `mvt_tile` embeds for `collection`,
    /// when this driver can report them without probing tile content
    /// (advertised on the OGC API Tiles `TileSet` resource's `layers`
    /// array). `Ok(None)` is the honest default for a driver with no such
    /// metadata concept — a caller that needs a name to advertise falls back
    /// to `collection.external_id()`, the name every single-layer driver in
    /// this workspace (PostGIS's `ST_AsMVT`) actually writes into the tile.
    /// Never `collection.id`: that is the internal id, which must never
    /// reach a response body. PMTiles overrides this to read its archive's
    /// own `vector_layers` metadata, since an archive's real layer names can
    /// differ entirely from the collection's public id.
    async fn vector_layers(&self, _collection: &CollectionDecl) -> Result<Option<Vec<String>>> {
        Ok(None)
    }
}

/// The MVT layer names a client must reference in a style's `source-layer`
/// to draw `collection` — [`TileSource::vector_layers`] resolved through the
/// fallback that trait method's own doc prescribes, in one place (`#245`).
///
/// Two callers need this answer and must never disagree about it: the OGC
/// API Tiles TileSet resource, which advertises the names
/// (`tellurion_tiles::handlers::tileset_vector_body`), and the style
/// applicability check that decides which styles are advertised for the same
/// collection (`tellurion-server`'s `StylesLinkContributor`, and the TileSet
/// resource's own styled-map links). `#220` introduced the second copy of
/// this logic; keeping two would let the advertised layer names and the
/// advertised styles drift apart, which is precisely the disagreement the
/// applicability check exists to prevent.
///
/// `Ok(None)`, `Ok(Some([]))` and a driver error all resolve to
/// `collection.external_id()` — the only name this workspace could honestly
/// fall back to (PostGIS embeds exactly that into `ST_AsMVT`), and never
/// `collection.id`, which is internal and must not reach a response body. A
/// probe failure is warned about rather than propagated: layer names are
/// metadata on a resource that is otherwise servable.
pub async fn advertised_vector_layers(
    collection: &CollectionDecl,
    source: &dyn TileSource,
) -> Vec<String> {
    match source.vector_layers(collection).await {
        Ok(Some(names)) if !names.is_empty() => names,
        Ok(_) => vec![collection.external_id().to_string()],
        Err(error) => {
            tracing::warn!(%error, collection = %collection.external_id(), "failed to read vector layer names; falling back to the collection's external id");
            vec![collection.external_id().to_string()]
        }
    }
}

/// A decoded pixel window for one PNG-lane raster tile, already resampled to
/// the destination tile's own pixel dimensions (`#37`, Cloud-Optimized
/// GeoTIFF serving) — the raster counterpart of [`TileSource::mvt_tile`]'s
/// pre-encoded MVT bytes. A raster collection has no vector intermediate to
/// hand back, so this capability returns decoded samples instead of bytes
/// ready to serve directly; only `tellurion-render`'s PNG encoder turns them
/// into a response body, at the same request boundary `mvt_tile`'s bytes
/// are rasterized at.
#[derive(Debug, Clone, PartialEq)]
pub struct RasterWindow {
    pub width: u32,
    pub height: u32,
    /// Row-major, straight (non-premultiplied) RGBA8 — `width * height * 4`
    /// bytes. Every implementer widens its native band layout (grayscale,
    /// RGB, ...) to RGBA here so the PNG lane never branches on source band
    /// count; a destination pixel outside the source raster's real extent
    /// (edge-of-coverage, or a tile only partially covered) carries alpha
    /// `0` rather than a guessed color.
    pub rgba: Vec<u8>,
}

#[async_trait::async_trait]
pub trait RasterSource: Send + Sync {
    /// `Ok(None)`: `coord`'s tile does not intersect this raster's extent at
    /// all — a legitimately empty tile, the same convention
    /// [`TileSource::mvt_tile`] uses for an empty MVT tile. `Err(Error::
    /// Invalid)`: the tile does intersect, but honoring it would require
    /// reading more source pixels than this driver's own per-request budget
    /// allows — refused rather than served via a ballooned read. No `filter`
    /// parameter: unlike vector tiles, a raster collection has no queryable
    /// attributes for a `#34` grant filter to narrow, the same reasoning
    /// [`TileSource::filter_capable`]'s doc gives for a pre-baked PMTiles
    /// archive.
    async fn raster_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
    ) -> Result<Option<RasterWindow>>;
}

/// One driver's answer to "true solid geometry for this tile" (`#15`): a
/// triangle mesh assembled directly from the backend's own 3D coordinates —
/// a CityJSON store, a BIM export, a PostGIS `PolyhedralSurface`/`Solid`
/// column, ... — rather than the footprint+height extrusion the places3d
/// lane falls back to when no driver in a collection's tiles lane advertises
/// [`VolumeSource`]. Coordinates follow the same tile-local convention
/// `tellurion-render::extrude_mvt_to_glb` documents: X/Y roughly `[0, 1]`
/// (the tile's own footprint), Z in whatever real-world units the backend
/// carries (conventionally meters), never normalized against a tile extent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VolumeMesh {
    pub positions: Vec<[f64; 3]>,
    /// Triangle indices into `positions`, three per triangle — the same
    /// flat layout a glTF `POSITION`/`indices` accessor pair expects.
    pub indices: Vec<u32>,
}

#[async_trait::async_trait]
pub trait VolumeSource: Send + Sync {
    /// `Ok(None)` means an empty tile (valid, nothing to draw) — same
    /// convention as [`TileSource::mvt_tile`], distinct from `Err` (a real
    /// failure resolving the solid geometry). `filter` is a `#34` ABAC grant
    /// filter, pushed into the volume query the same way
    /// [`TileSource::mvt_tile`]'s own `filter` parameter is (`#70`): `None`
    /// when the requesting subject's access is unrestricted. A source that
    /// declines [`filter_capable`](Self::filter_capable) is never handed
    /// `Some` here; the places3d lane's own policy checkpoint denies the
    /// request before calling `volume_tile` at all when a filter is
    /// required but the resolved source can't apply one.
    async fn volume_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<VolumeMesh>>;

    /// Whether this source can evaluate a `#34` ABAC grant `Filter` inside
    /// `volume_tile` — the volume-lane counterpart of
    /// [`TileSource::filter_capable`], same default-`false` marker and same
    /// reasoning (filtering is a refinement of "can serve solid geometry at
    /// all," not a capability with its own resolve entry point). PostGIS
    /// overrides this to `true` (the same `tellurion-postgis::sql::
    /// compile_filter` compiler the tiles lane uses, reused for the volume
    /// query's own `WHERE` clause).
    fn filter_capable(&self) -> bool {
        false
    }
}

/// Geometry-column type names that carry true 3D solid geometry rather than
/// a flat 2D shape — the same three names `tellurion-postgis::volume::
/// VolumeGeometryKind` accepts (`#41`), duplicated here as a plain string
/// check so [`crate::router::Router::resolve_volume`] (`#70`) can decide,
/// per collection, whether a driver-wide [`VolumeSource`] answer actually
/// applies to *this* collection's own geometry column — without
/// `tellurion-core` depending on any driver crate. A footprint+height
/// `places3d` collection sharing a storage entry with a genuinely solid one
/// reports some other type name here (e.g. `"POLYGON"`) and must still fall
/// through to the places3d extrusion path.
pub fn is_volume_capable_geometry_type(type_name: &str) -> bool {
    matches!(type_name, "POLYHEDRALSURFACE" | "TIN" | "MULTIPOLYGON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl() -> CollectionDecl {
        serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main").unwrap()
    }

    /// A `TileSource` whose `vector_layers` answer is whatever the test
    /// hands it — the three shapes [`advertised_vector_layers`] resolves.
    struct Reports(Result<Option<Vec<String>>>);

    #[async_trait::async_trait]
    impl TileSource for Reports {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> Result<Option<Bytes>> {
            Ok(None)
        }

        async fn vector_layers(&self, _collection: &CollectionDecl) -> Result<Option<Vec<String>>> {
            match &self.0 {
                Ok(names) => Ok(names.clone()),
                Err(_) => Err(Error::Storage("boom".into())),
            }
        }
    }

    #[tokio::test]
    async fn a_driver_that_reports_real_layer_names_is_taken_at_its_word() {
        let source = Reports(Ok(Some(vec!["world".into(), "leaf".into()])));
        assert_eq!(
            advertised_vector_layers(&decl(), &source).await,
            vec!["world".to_string(), "leaf".to_string()]
        );
    }

    /// `Ok(None)` (no such metadata concept), `Ok(Some([]))` (nothing to
    /// report) and a probe failure all resolve to the same honest fallback —
    /// the collection's EXTERNAL id, which is the name every single-layer
    /// driver in this workspace actually embeds in the tile.
    #[tokio::test]
    async fn every_no_answer_falls_back_to_the_collections_external_id() {
        for answer in [
            Reports(Ok(None)),
            Reports(Ok(Some(vec![]))),
            Reports(Err(Error::Storage("boom".into()))),
        ] {
            assert_eq!(
                advertised_vector_layers(&decl(), &answer).await,
                vec!["demo".to_string()]
            );
        }
    }
}
