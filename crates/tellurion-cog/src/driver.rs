//! The `cog` `DriverFactory`, and the `CatalogSource` + `RasterSource`
//! implementation backing it. Read-only, same shape as `tellurion-pmtiles`'
//! own driver: `FeatureSource`/`TileSource` are never implemented — a
//! collection routed to a `cog` storage on the `features` lane, or asking
//! for MVT on the `tiles` lane, fails with the router's ordinary
//! missing-capability refusal, never a stub.
//!
//! ## Storage config
//!
//! A `cog` storage reuses `StorageDecl.url_env` exactly as `postgis`/
//! `pmtiles` do: the named environment variable holds the GeoTIFF's
//! location — either a local filesystem path, or an `http(s)://` URL read
//! entirely through ranged GET requests (`#37` slice 2, `reader.rs`'s
//! `CogSource`/`remote.rs`'s `HttpRangeReader`). [`CogDriverFactory::build`]
//! decides which by the locator's own scheme; nothing else in config names
//! it.
//!
//! ## Metadata vs. pixel reads
//!
//! `DriverFactory::build` is synchronous and does no I/O — it only captures
//! the configured source (for `Remote`, that means building the HTTP
//! client, never probing the URL). Metadata (`reader::CogMeta`, tags/
//! GeoKeys/overview pyramid) is parsed lazily on first use via a `tokio::
//! sync::OnceCell`, offloaded to the blocking pool the same way every pixel
//! read is — see `reader.rs`'s own doc for why every read reopens the
//! source rather than keeping a decoder alive across requests, and for why
//! that choice extends to `Remote` without adding a byte-range cache
//! alongside it: a cache here would only ever hold header/IFD bytes (never
//! the served tile — that goes through this workspace's own byte-budgeted
//! tile cache already, at the response boundary), and `remote.rs`'s own
//! window buffering already keeps a single request's own ranged-GET count
//! small without needing state to survive past that request.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use tellurion_http_source::RangeObject;

use tellurion_core::{
    CatalogSource, CollectionDecl, DriverFactory, Error as CoreError, PhysicalCollection,
    ProjectionFacts, RasterSource, RasterWindow, Result as CoreResult, SpatialExtent, StorageDecl,
    StorageDriver, TileCoord,
};

use crate::colormap::ResolvedColormap;
use crate::error::CogError;
use crate::reader::{self, Bands, CogMeta, CogSource};
use crate::remote::RemoteCogSource;
use crate::tiling;

/// Destination tile size, pixels — matches `tellurion-tiles`' own
/// `RENDER_TILE_SIZE_PX` (256, the OGC API Tiles `WebMercatorQuad` tile
/// size every PNG in this workspace is rendered at). Duplicated rather than
/// shared: `tellurion-tiles`' own constant is crate-private, and this driver
/// crate has no dependency on `tellurion-tiles` to reach it through (drivers
/// sit below the protocol crates in this workspace's dependency order).
pub(crate) const DEST_TILE_SIZE_PX: u32 = 256;

/// Hard per-request cap on how many SOURCE pixels (at whichever overview
/// level `tiling::select_overview` picks) one tile request may read — the
/// bound that keeps a request over a huge, coarse raster from ballooning
/// into an unbounded read instead of refusing. ~16x the 256x256 destination
/// tile's own pixel count, generous enough that a well-chosen overview level
/// never trips it, tight enough that a misconfigured/degenerate window
/// (a raster whose overviews skip too coarse a step, say) still fails fast.
///
/// This is a pixel-COUNT budget, not a byte budget, deliberately: every
/// band layout `reader::read_window` reads — `Gray`/`Palette` (1 source
/// byte/pixel), `Rgb` (3), `Rgba` (4) — is widened to a flat 4-byte RGBA8
/// destination pixel regardless, so the widest possible ratio (`Gray` or
/// `Palette`'s 1 -> 4, a 4x expansion) has been this same budget's worst
/// case since before this crate served any categorical format at all — a
/// paletted source's own index -> RGBA expansion (`#37` categorical
/// authoring) is exactly that SAME ratio, not a new or larger one, so no
/// separate multiplier is needed here (see `decode_budget_bounds_the_worst_case_widen_ratio`
/// below, which pins this down rather than leaving it asserted only in
/// prose).
pub(crate) const MAX_SOURCE_PIXELS: u64 = 4_000_000;

/// Process-wide bound for COG operations that execute on Tokio's blocking
/// pool. The permit moves into the blocking closure, so cancelling the
/// future awaiting it never re-admits work while the underlying I/O is
/// still running.
pub(crate) const MAX_CONCURRENT_BLOCKING_COG_OPERATIONS: usize = 4;

static BLOCKING_COG_LIMIT: LazyLock<Arc<tokio::sync::Semaphore>> = LazyLock::new(|| {
    Arc::new(tokio::sync::Semaphore::new(
        MAX_CONCURRENT_BLOCKING_COG_OPERATIONS,
    ))
});

/// Runs one blocking COG operation while keeping its admission permit owned
/// by the blocking closure itself. Tokio cannot abort a `spawn_blocking`
/// task once it has started, so retaining the permit there is the only
/// truthful representation of active raster work after request cancellation.
pub(crate) async fn run_blocking_cog<T, F>(operation: F) -> Result<T, CogError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, CogError> + Send + 'static,
{
    let permit = Arc::clone(&BLOCKING_COG_LIMIT)
        .acquire_owned()
        .await
        .map_err(|error| CogError::Decode(error.to_string()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
    .await
    .map_err(|join_error| CogError::Decode(join_error.to_string()))?
}

#[cfg(test)]
pub(crate) fn blocking_cog_available_permits() -> usize {
    BLOCKING_COG_LIMIT.available_permits()
}

/// `#37`: refuses a read whose clamped window (`width * height` source
/// pixels) exceeds `budget` — pulled out of `raster_tile_inner` as its own
/// pure function so a test can exercise the refusal directly, without
/// needing a fixture large enough to actually trip
/// [`MAX_SOURCE_PIXELS`]'s real value.
///
/// `#254`: takes an already-summed source-pixel count rather than a
/// `width`/`height` pair, because the `cog-mosaic` driver (`mosaic.rs`)
/// checks it ONCE per request over the total across every source it selected
/// for that tile. Deliberately the same [`MAX_SOURCE_PIXELS`] budget, not a
/// second one: the issue's bound is a per-REQUEST budget, so a mosaic of 32
/// sources may not quietly read 32 times what a single COG is allowed to.
pub(crate) fn check_pixel_budget_total(requested: u64, budget: u64) -> Result<(), CogError> {
    if requested > budget {
        return Err(CogError::PixelBudgetExceeded { requested, budget });
    }
    Ok(())
}

/// Registers the `cog` driver.
#[derive(Default)]
pub struct CogDriverFactory;

impl CogDriverFactory {
    pub fn new() -> Self {
        Self
    }

    /// Builds a COG storage driver from an already-authorized byte-range
    /// object. The object owns all remote transport policy and identity.
    pub fn build_range_object(&self, object: Arc<dyn RangeObject>) -> Arc<dyn StorageDriver> {
        Arc::new(CogDriverImpl {
            backend: Arc::new(CogBackend::new(CogSource::Remote(
                RemoteCogSource::from_range_object(object),
            ))),
        })
    }
}

impl DriverFactory for CogDriverFactory {
    fn name(&self) -> &str {
        "cog"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let raw = std::env::var(&decl.url_env).map_err(|_| {
            CoreError::Config(format!(
                "storage '{}': environment variable '{}' is not set",
                decl.id, decl.url_env
            ))
        })?;
        let source = parse_source(&decl.id, &decl.url_env, &raw)?;
        Ok(Arc::new(CogDriverImpl {
            backend: Arc::new(CogBackend::new(source)),
        }))
    }
}

/// Decides local vs. trusted administrative remote from `raw`'s own scheme.
/// Public callers never reach this constructor: they pass a broker-created
/// [`RangeObject`] to [`CogDriverFactory::build_range_object`] instead.
fn parse_source(storage_id: &str, source_env: &str, raw: &str) -> CoreResult<CogSource> {
    if is_http_locator(raw) {
        RemoteCogSource::administrative_from_env(source_env)
            .map(CogSource::Remote)
            .map_err(|_| {
                CoreError::Config(format!("storage '{storage_id}': invalid remote source"))
            })
    } else {
        Ok(CogSource::Local(PathBuf::from(raw)))
    }
}

fn is_http_locator(raw: &str) -> bool {
    raw.get(.."http://".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || raw
            .get(.."https://".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

struct CogDriverImpl {
    backend: Arc<CogBackend>,
}

impl StorageDriver for CogDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn RasterSource>)
    }

    // `feature_source`/`tile_source`: default `None` — this driver never
    // implements either (see this module's own doc).
}

struct CogBackend {
    source: CogSource,
    meta: tokio::sync::OnceCell<CogMeta>,
}

impl CogBackend {
    fn new(source: CogSource) -> Self {
        Self {
            source,
            meta: tokio::sync::OnceCell::new(),
        }
    }

    async fn meta(&self) -> Result<&CogMeta, CogError> {
        self.meta
            .get_or_try_init(|| async {
                let source = self.source.clone();
                run_blocking_cog(move || reader::open(&source)).await
            })
            .await
    }

    async fn collections_inner(&self) -> Result<Vec<PhysicalCollection>, CogError> {
        let meta = self.meta().await?;
        Ok(vec![PhysicalCollection {
            name: meta.logical_name.clone(),
            // No table-shaped concept for either — this driver serves
            // decoded pixels, not queryable rows (mirrors PMTiles' own
            // `#20` reasoning for a tiles-only archive).
            geometry_column: None,
            primary_key: None,
            srid: meta.crs.epsg.map(|epsg| epsg as i32),
            geometry_type: None,
        }])
    }

    async fn extent_inner(&self) -> Result<Option<SpatialExtent>, CogError> {
        let meta = self.meta().await?;
        Ok(Some(SpatialExtent {
            bbox: meta.extent_crs84,
        }))
    }

    /// `#36` (STAC `projection` extension): every field here is read
    /// straight out of the GeoTIFF's own georeferencing tags, never
    /// invented — `epsg` from the GeoKey directory (`geokeys::parse_crs`;
    /// always genuinely present for a file this driver serves at all, since
    /// [`reader::open`] refuses anything that is not EPSG:4326), `transform`
    /// from `ModelPixelScaleTag`/`ModelTiepointTag` re-expressed in
    /// `proj:transform`'s row-major affine order (`[a, b, c, d, e, f]`, so
    /// `e` is the NEGATED pixel-scale-Y: a raster row advances southward
    /// while geographic Y grows northward), and `shape` as `[height, width]`
    /// (`proj:shape`'s own Y-first order) of the full-resolution level —
    /// `levels[0]`, which [`reader::open`] sorts finest-first regardless of
    /// file order.
    async fn projection_inner(&self) -> Result<Option<ProjectionFacts>, CogError> {
        let meta = self.meta().await?;
        let level0 = &meta.levels[0];
        Ok(Some(ProjectionFacts {
            epsg: meta.crs.epsg.map(|epsg| epsg as i32),
            transform: Some([
                meta.transform.pixel_scale_x,
                0.0,
                meta.transform.origin_x,
                0.0,
                -meta.transform.pixel_scale_y,
                meta.transform.origin_y,
            ]),
            shape: Some([u64::from(level0.height), u64::from(level0.width)]),
        }))
    }

    async fn raster_tile_inner(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
    ) -> Result<Option<RasterWindow>, CogError> {
        let meta = self.meta().await?;
        let colormap = resolve_colormap(collection, meta)?;

        let Some(read) = TileRead::plan(self.source.clone(), meta, coord, colormap) else {
            return Ok(None);
        };
        check_pixel_budget_total(read.source_pixels(), MAX_SOURCE_PIXELS)?;
        let rgba = read.run().await?;
        Ok(Some(RasterWindow {
            width: DEST_TILE_SIZE_PX,
            height: DEST_TILE_SIZE_PX,
            rgba,
        }))
    }
}

/// Which colormap (if any) applies to `collection` over `meta` — the `#92`
/// resolution rule, extracted so the `cog-mosaic` driver (`mosaic.rs`)
/// applies exactly the same one to every constituent COG rather than
/// forking it.
///
/// `#92`: a colormap is only meaningful over single-band (Gray) data —
/// refuse by name rather than silently ignore a configured colormap (or
/// render something the operator never asked for) for an RGB/RGBA source.
/// `CatalogSource::collections` (the earlier, boot-time refusal path every
/// other capability mismatch in this driver uses — striped layout,
/// unsupported CRS) has no per-collection config to compare against, so this
/// is the earliest point this driver CAN make the comparison.
/// `#37` categorical: a paletted GeoTIFF (`Bands::Palette`) already carries
/// its own embedded colormap (`meta.embedded_colormap`, resolved once at
/// `open()` from the file's own `ColorMap` tag) — an operator-configured one
/// on TOP of that has no meaning either (which one would win?), so it's
/// refused the same way a configured colormap over RGB/RGBA already is.
pub(crate) fn resolve_colormap(
    collection: &CollectionDecl,
    meta: &CogMeta,
) -> Result<Option<ResolvedColormap>, CogError> {
    match &collection.settings.colormap {
        Some(_) if meta.bands != Bands::Gray => Err(CogError::Unsupported(format!(
            "collection '{}' configures a colormap, but this GeoTIFF is not \
             single-band Grayscale (it may be paletted, which already carries its \
             own embedded colormap, or RGB/RGBA); colormaps only apply to \
             single-band Grayscale rasters",
            collection.id
        ))),
        Some(conf) => Ok(Some(ResolvedColormap::build(conf))),
        None => Ok(meta.embedded_colormap.clone()),
    }
}

/// One COG's own contribution to one WebMercatorQuad tile, planned but not
/// yet read: everything [`TileRead::run`] needs, owned, so the future it
/// returns is `'static` and can be driven from a `tokio::task::JoinSet` (the
/// bounded-concurrency gather in `mosaic.rs`) as readily as awaited inline.
///
/// The single-COG driver and the `cog-mosaic` driver share this type on
/// purpose: there is exactly ONE decode path in this crate, through
/// `tiling::plan_window` -> `reader::read_window` -> `tiling::
/// resample_to_tile`, and a mosaic composes its results rather than forking
/// them.
pub(crate) struct TileRead {
    source: CogSource,
    level: reader::Level,
    bands: Bands,
    window: reader::PixelWindow,
    plan: tiling::WindowPlan,
    coord: TileCoord,
    origin_y: f64,
    colormap: Option<ResolvedColormap>,
}

impl TileRead {
    /// `None` when `coord` does not intersect this COG's own extent at all
    /// — a legitimately empty contribution, not an error.
    pub(crate) fn plan(
        source: CogSource,
        meta: &CogMeta,
        coord: TileCoord,
        colormap: Option<ResolvedColormap>,
    ) -> Option<Self> {
        let bbox = tiling::tile_lonlat_bbox(coord);
        let plan = tiling::plan_window(
            &meta.levels,
            &meta.transform,
            meta.total_geo_width_deg,
            meta.total_geo_height_deg,
            bbox,
            DEST_TILE_SIZE_PX,
        )?;
        Some(Self {
            source,
            level: meta.levels[plan.level_index].clone(),
            bands: meta.bands,
            window: reader::PixelWindow {
                x0: plan.clamped_x0,
                y0: plan.clamped_y0,
                x1: plan.clamped_x1,
                y1: plan.clamped_y1,
            },
            plan,
            coord,
            origin_y: meta.transform.origin_y,
            colormap,
        })
    }

    /// How many SOURCE pixels this read will touch — what the per-request
    /// pixel budget is measured in (see [`check_pixel_budget_total`]).
    pub(crate) fn source_pixels(&self) -> u64 {
        u64::from(self.window.x1 - self.window.x0) * u64::from(self.window.y1 - self.window.y0)
    }

    /// Decodes and warps this contribution into a `DEST_TILE_SIZE_PX`-square
    /// straight-RGBA8 buffer. Pixels the source does not cover stay fully
    /// transparent — which is what makes a mosaic composable at all.
    pub(crate) async fn run(self) -> Result<Vec<u8>, CogError> {
        let window_w = self.window.x1 - self.window.x0;
        let window_h = self.window.y1 - self.window.y0;
        let Self {
            source,
            level,
            bands,
            window,
            plan,
            coord,
            origin_y,
            colormap,
        } = self;
        let window_rgba = run_blocking_cog(move || {
            reader::read_window(&source, &level, bands, window, colormap.as_ref())
        })
        .await?;

        Ok(tiling::resample_to_tile(
            &window_rgba,
            window_w,
            window_h,
            &plan,
            coord,
            DEST_TILE_SIZE_PX,
            origin_y,
        ))
    }
}

#[async_trait]
impl CatalogSource for CogBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        self.collections_inner().await.map_err(Into::into)
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        self.extent_inner().await.map_err(Into::into)
    }

    async fn projection(
        &self,
        _physical: &PhysicalCollection,
    ) -> CoreResult<Option<ProjectionFacts>> {
        self.projection_inner().await.map_err(Into::into)
    }
}

#[async_trait]
impl RasterSource for CogBackend {
    async fn raster_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
    ) -> CoreResult<Option<RasterWindow>> {
        self.raster_tile_inner(collection, coord)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use bytes::Bytes;
    use tellurion_http_source::{ContentIdentity, RangeObject, SourceError, SourceHandle};

    use super::*;

    struct FixtureRangeObject {
        handle: SourceHandle,
        identity: ContentIdentity,
        name: String,
        bytes: Bytes,
        requests: AtomicUsize,
    }

    impl FixtureRangeObject {
        fn new(name: &str, bytes: Vec<u8>) -> Self {
            let length = bytes.len() as u64;
            Self {
                handle: SourceHandle::new("fixture-source"),
                identity: ContentIdentity::StrongEtag {
                    source_key: [1; 32],
                    revision_key: [2; 32],
                    length,
                },
                name: name.to_string(),
                bytes: Bytes::from(bytes),
                requests: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl RangeObject for FixtureRangeObject {
        fn handle(&self) -> &SourceHandle {
            &self.handle
        }

        fn identity(&self) -> &ContentIdentity {
            &self.identity
        }

        fn length(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn display_name(&self) -> &str {
            &self.name
        }

        async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(self.bytes.slice(range.start as usize..range.end as usize))
        }
    }

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiled_rgb.tif")
    }

    fn striped_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/striped.tif")
    }

    fn decl() -> CollectionDecl {
        serde_yaml::from_str("id: tiled_rgb\ncatalog: default\nstorage: main\n").unwrap()
    }

    #[test]
    fn factory_name_matches_the_config_driver_key() {
        assert_eq!(CogDriverFactory::new().name(), "cog");
    }

    #[test]
    fn build_fails_fast_when_the_env_var_is_unset() {
        let factory = CogDriverFactory::new();
        let decl = StorageDecl {
            id: "main".to_string(),
            driver: "cog".to_string(),
            url_env: "TELLURION_COG_TEST_DOES_NOT_EXIST".to_string(),
            pool_size: None,
        };
        std::env::remove_var(&decl.url_env);
        assert!(matches!(factory.build(&decl), Err(CoreError::Config(_))));
    }

    #[tokio::test]
    async fn range_object_pixels_match_the_local_fixture() {
        let local = CogBackend::new(CogSource::Local(fixture_path()));
        let object = Arc::new(FixtureRangeObject::new(
            "shared-raster",
            std::fs::read(fixture_path()).unwrap(),
        ));
        let remote = CogDriverFactory::new().build_range_object(object.clone());

        let local = local
            .raster_tile(
                &decl(),
                TileCoord {
                    z: 10,
                    x: 513,
                    y: 513,
                },
            )
            .await
            .unwrap()
            .unwrap();
        let remote = remote
            .raster_source()
            .unwrap()
            .raster_tile(
                &decl(),
                TileCoord {
                    z: 10,
                    x: 513,
                    y: 513,
                },
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(remote.rgba, local.rgba);
        assert!(object.requests.load(Ordering::SeqCst) > 0);
    }

    #[tokio::test]
    async fn collections_reports_the_file_stem_and_the_geotiffs_own_epsg_code() {
        let backend = CogBackend::new(CogSource::Local(fixture_path()));
        let collections = backend.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "tiled_rgb");
        assert_eq!(collections[0].srid, Some(4326));
        assert_eq!(collections[0].geometry_column, None);
        assert_eq!(collections[0].primary_key, None);
    }

    #[tokio::test]
    async fn extent_comes_straight_from_the_geotiff_tags() {
        let backend = CogBackend::new(CogSource::Local(fixture_path()));
        let physical = &backend.collections().await.unwrap()[0];
        let extent = backend.extent(physical).await.unwrap().unwrap();
        for (actual, expected) in extent.bbox.iter().zip([-1.28, -1.28, 1.28, 1.28]) {
            assert!(
                (actual - expected).abs() < 1e-9,
                "extent {:?} did not match [-1.28, -1.28, 1.28, 1.28]",
                extent.bbox
            );
        }
    }

    /// `#36` (STAC projection extension): every projection fact comes
    /// straight from the fixture's own georeferencing — 256x256 pixels at
    /// 0.01 degrees/pixel from origin `(-1.28, 1.28)` (see
    /// `examples/gen_fixture.rs`) — with `proj:transform`'s `e` coefficient
    /// negated (rows advance southward) and `proj:shape` in the extension's
    /// own `[height, width]` order.
    #[tokio::test]
    async fn projection_reads_the_geotiffs_own_georeferencing() {
        let backend = CogBackend::new(CogSource::Local(fixture_path()));
        let physical = &backend.collections().await.unwrap()[0];
        let facts = backend.projection(physical).await.unwrap().unwrap();
        assert_eq!(facts.epsg, Some(4326));
        assert_eq!(facts.shape, Some([256, 256]));
        let transform = facts.transform.expect("a GeoTIFF always has a transform");
        for (actual, expected) in transform.iter().zip([0.01, 0.0, -1.28, 0.0, -0.01, 1.28]) {
            assert!(
                (actual - expected).abs() < 1e-12,
                "transform {transform:?} did not match the fixture's georeferencing"
            );
        }
    }

    /// `#37`: opening a striped GeoTIFF is refused at the same
    /// `CatalogSource` call `Router::validate_catalog` makes unconditionally
    /// for every registered storage at boot — the boot-time half of "an
    /// unsupported layout is `Error::Config`, never a panic."
    #[tokio::test]
    async fn boot_refuses_a_striped_geotiff() {
        let backend = CogBackend::new(CogSource::Local(striped_fixture_path()));
        match backend.collections().await {
            Err(CoreError::Config(message)) => {
                assert!(message.contains("striped"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    /// z10/x513/y513 sits entirely inside the fixture's solid-yellow
    /// quadrant (chunk (1, 1): lon [0, 1.28], lat [-1.28, 0]), away from
    /// every internal tile boundary and the raster's own edge — see
    /// `examples/gen_fixture.rs`'s own doc for the exact layout. At this
    /// zoom the finest (native) level is still coarser than requested, so
    /// this also proves the "fall back to the finest level and upsample"
    /// branch of overview selection.
    #[tokio::test]
    async fn raster_tile_is_solid_yellow_deep_inside_a_quadrant() {
        let backend = CogBackend::new(CogSource::Local(fixture_path()));
        let window = backend
            .raster_tile(
                &decl(),
                TileCoord {
                    z: 10,
                    x: 513,
                    y: 513,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!((window.width, window.height), (256, 256));
        assert!(
            window.rgba.chunks_exact(4).all(|p| p == [255, 255, 0, 255]),
            "every pixel deep inside the yellow quadrant should be solid opaque yellow"
        );
    }

    /// Each main-image quadrant is a distinct flat color (see
    /// `examples/gen_fixture.rs`) — reading deep inside each one proves the
    /// tile-edge window math lands in the right source tile, not just that
    /// "some" color comes back.
    #[tokio::test]
    async fn raster_tile_reads_the_right_quadrant_for_each_corner() {
        let backend = CogBackend::new(CogSource::Local(fixture_path()));
        // (z, x, y, expected color) — one tile deep inside each quadrant,
        // computed the same way as the yellow (south-east) case above.
        let cases: [(u8, u32, u32, [u8; 4]); 4] = [
            (10, 511, 511, [255, 0, 0, 255]),   // north-west: red
            (10, 513, 511, [0, 255, 0, 255]),   // north-east: green
            (10, 511, 513, [0, 0, 255, 255]),   // south-west: blue
            (10, 513, 513, [255, 255, 0, 255]), // south-east: yellow
        ];
        for (z, x, y, expected) in cases {
            let window = backend
                .raster_tile(&decl(), TileCoord { z, x, y })
                .await
                .unwrap()
                .unwrap();
            assert!(
                window.rgba.chunks_exact(4).all(|p| p == expected),
                "tile ({z},{x},{y}) expected solid {expected:?}"
            );
        }
    }

    /// A coarse zoom covering the raster's whole tiny extent (plus a huge
    /// margin of empty world around it) proves both world-bounds clamping
    /// (most of the destination tile stays transparent) and that the
    /// coarsest overview — flat gray, a color no main-image tile uses — was
    /// actually selected and read, not the native RGB image upsampled.
    #[tokio::test]
    async fn raster_tile_selects_the_overview_for_a_world_covering_zoom() {
        let backend = CogBackend::new(CogSource::Local(fixture_path()));
        let window = backend
            .raster_tile(&decl(), TileCoord { z: 0, x: 0, y: 0 })
            .await
            .unwrap()
            .unwrap();
        let pixels: Vec<[u8; 4]> = window
            .rgba
            .chunks_exact(4)
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect();
        assert!(
            pixels.contains(&[128, 128, 128, 255]),
            "some pixel should show the overview's flat gray"
        );
        assert!(
            pixels.iter().any(|p| p[3] == 0),
            "most of a world-covering tile should stay transparent outside the raster's tiny extent"
        );
        assert!(
            !pixels.contains(&[255, 0, 0, 255]) && !pixels.contains(&[0, 255, 0, 255]),
            "a coarse zoom should read the overview, never the main image's own colors"
        );
    }

    /// A tile on the opposite side of the globe from the fixture's tiny
    /// `[-1.28, -1.28, 1.28, 1.28]` extent never intersects it at all.
    #[tokio::test]
    async fn raster_tile_is_none_for_a_coordinate_the_raster_never_covers() {
        let backend = CogBackend::new(CogSource::Local(fixture_path()));
        let window = backend
            .raster_tile(&decl(), TileCoord { z: 2, x: 0, y: 0 })
            .await
            .unwrap();
        assert_eq!(window, None);
    }

    fn decl_with_colormap() -> CollectionDecl {
        serde_yaml::from_str(
            "id: tiled_rgb\ncatalog: default\nstorage: main\n\
             settings:\n  colormap: { kind: ramp, ramp: grayscale, min: 0.0, max: 255.0 }\n",
        )
        .unwrap()
    }

    /// `#92`: a colormap only ever applies to single-band (Gray) data — this
    /// fixture is RGB, so a collection that configures one refuses by name
    /// rather than silently ignore it (or render something misleading).
    #[tokio::test]
    async fn raster_tile_refuses_a_colormap_configured_over_a_non_gray_raster() {
        let backend = CogBackend::new(CogSource::Local(fixture_path()));
        match backend
            .raster_tile(
                &decl_with_colormap(),
                TileCoord {
                    z: 10,
                    x: 513,
                    y: 513,
                },
            )
            .await
        {
            Err(CoreError::Config(message)) => {
                assert!(
                    message.contains("colormap") && message.contains("single-band"),
                    "message should name the real reason: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    /// `#37` categorical: pins down `MAX_SOURCE_PIXELS`'s own doc claim
    /// rather than leaving it asserted only in prose — every band layout
    /// this crate reads (`Gray`/`Palette` 1 byte/pixel, `Rgb` 3, `Rgba` 4)
    /// widens to the SAME flat 4-byte RGBA8 destination pixel, so a
    /// paletted source's index -> RGBA expansion never exceeds the worst
    /// case (`Gray`/`Palette`'s own 1 -> 4 ratio) this budget already
    /// bounded before categorical authoring existed.
    #[test]
    fn decode_budget_bounds_the_worst_case_widen_ratio_gray_or_palette_to_rgba() {
        let max_source_bytes_gray_or_palette = MAX_SOURCE_PIXELS; // 1 byte/pixel
        let max_source_bytes_rgb = MAX_SOURCE_PIXELS * 3;
        let max_source_bytes_rgba = MAX_SOURCE_PIXELS * 4;
        let max_dest_bytes = MAX_SOURCE_PIXELS * 4; // every band layout widens to this
        assert_eq!(max_source_bytes_gray_or_palette, 4_000_000);
        assert_eq!(max_source_bytes_rgb, 12_000_000);
        assert_eq!(max_source_bytes_rgba, 16_000_000);
        assert_eq!(max_dest_bytes, 16_000_000);
        assert!(
            max_source_bytes_gray_or_palette <= max_dest_bytes
                && max_source_bytes_rgb <= max_dest_bytes
                && max_source_bytes_rgba <= max_dest_bytes,
            "no band layout's own source-side bytes ever exceed the destination widen, \
             including a paletted source's index bytes"
        );
    }

    #[test]
    fn check_pixel_budget_allows_a_window_within_budget() {
        assert!(check_pixel_budget_total(100 * 100, 500_000).is_ok());
    }

    #[test]
    fn check_pixel_budget_refuses_a_window_over_budget() {
        match check_pixel_budget_total(1_000 * 1_000, 500_000) {
            Err(CogError::PixelBudgetExceeded { requested, budget }) => {
                assert_eq!(requested, 1_000_000);
                assert_eq!(budget, 500_000);
            }
            other => panic!("expected PixelBudgetExceeded, got {other:?}"),
        }
    }

    /// The refusal surfaces to a caller as `Error::Invalid`, not `Config` or
    /// a generic storage error — a client-correctable 400, per this lane's
    /// contract (see `error.rs`'s own doc).
    #[test]
    fn pixel_budget_exceeded_maps_to_error_invalid() {
        let error: tellurion_core::Error = CogError::PixelBudgetExceeded {
            requested: 1_000_000,
            budget: 500_000,
        }
        .into();
        assert!(matches!(error, tellurion_core::Error::Invalid(_)));
    }

    #[test]
    fn parse_source_treats_a_bare_path_as_local() {
        let source = parse_source("main", "unused", "/data/tiled_rgb.tif").unwrap();
        assert!(
            matches!(source, CogSource::Local(path) if path.as_path() == std::path::Path::new("/data/tiled_rgb.tif"))
        );
    }

    #[test]
    fn parse_source_treats_an_http_url_as_remote() {
        let variable = "TELLURION_COG_ADMIN_HTTP_TEST";
        std::env::set_var(variable, "http://example.invalid/tiled_rgb.tif");
        let source =
            parse_source("main", variable, "http://example.invalid/tiled_rgb.tif").unwrap();
        std::env::remove_var(variable);
        assert!(matches!(source, CogSource::Remote(_)));
    }

    #[test]
    fn parse_source_treats_an_https_url_as_remote() {
        let variable = "TELLURION_COG_ADMIN_HTTPS_TEST";
        std::env::set_var(variable, "https://example.invalid/tiled_rgb.tif");
        let source =
            parse_source("main", variable, "https://example.invalid/tiled_rgb.tif").unwrap();
        std::env::remove_var(variable);
        assert!(matches!(source, CogSource::Remote(_)));
    }

    #[test]
    fn parse_source_treats_a_mixed_case_https_url_as_remote() {
        let variable = "TELLURION_COG_ADMIN_MIXED_CASE_HTTPS_TEST";
        let raw = "HTTPS://example.invalid/tiled_rgb.tif";
        std::env::set_var(variable, raw);
        let source = parse_source("main", variable, raw).unwrap();
        std::env::remove_var(variable);
        assert!(matches!(source, CogSource::Remote(_)));
    }

    #[test]
    fn parse_source_rejects_a_malformed_url() {
        assert!(matches!(
            parse_source("main", "unused", "http://"),
            Err(CoreError::Config(_))
        ));
    }

    struct ScopedEnvironmentVariable(&'static str);

    impl ScopedEnvironmentVariable {
        fn set(variable: &'static str, value: &str) -> Self {
            std::env::set_var(variable, value);
            Self(variable)
        }
    }

    impl Drop for ScopedEnvironmentVariable {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    fn remote_source(variable: &'static str, url: String) -> CogSource {
        let _environment = ScopedEnvironmentVariable::set(variable, &url);
        CogSource::Remote(RemoteCogSource::administrative_from_env(variable).unwrap())
    }

    #[test]
    fn administrative_test_sources_use_their_own_environment_variables() {
        let first = remote_source(
            "TELLURION_COG_ADMIN_REMOTE_FIRST_FIXTURE",
            "http://example.invalid/first.tif".to_string(),
        );
        let second = remote_source(
            "TELLURION_COG_ADMIN_REMOTE_SECOND_FIXTURE",
            "http://example.invalid/second.tif".to_string(),
        );
        assert!(matches!(&first, CogSource::Remote(source) if source.display_name() == "first"));
        assert!(matches!(&second, CogSource::Remote(source) if source.display_name() == "second"));
    }

    /// The same proof as `collections_reports_the_file_stem_and_the_geotiffs_own_epsg_code`,
    /// but reading through a loopback ranged-HTTP server instead of the
    /// local filesystem — `CatalogSource::collections` never touches a
    /// local file for a `CogSource::Remote` backend.
    #[tokio::test]
    async fn collections_and_extent_work_over_a_remote_range_backed_source() {
        let bytes = std::fs::read(fixture_path()).unwrap();
        let server = crate::test_support::MockServer::range_aware(bytes);
        let backend = CogBackend::new(remote_source(
            "TELLURION_COG_ADMIN_REMOTE_COLLECTIONS_FIXTURE",
            server.url("/tiled_rgb.tif"),
        ));

        let collections = backend.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "tiled_rgb");
        assert_eq!(collections[0].srid, Some(4326));

        let extent = backend.extent(&collections[0]).await.unwrap().unwrap();
        for (actual, expected) in extent.bbox.iter().zip([-1.28, -1.28, 1.28, 1.28]) {
            assert!((actual - expected).abs() < 1e-9, "extent {:?}", extent.bbox);
        }
    }

    /// The same proof as `raster_tile_is_solid_yellow_deep_inside_a_quadrant`,
    /// but reading through a loopback ranged-HTTP server — the destination
    /// pixels must match exactly, whether the source bytes came from a
    /// local file or a series of ranged GET requests.
    #[tokio::test]
    async fn raster_tile_reads_correctly_over_a_remote_range_backed_source() {
        let bytes = std::fs::read(fixture_path()).unwrap();
        let server = crate::test_support::MockServer::range_aware(bytes);
        let backend = CogBackend::new(remote_source(
            "TELLURION_COG_ADMIN_REMOTE_TILE_FIXTURE",
            server.url("/tiled_rgb.tif"),
        ));

        let window = backend
            .raster_tile(
                &decl(),
                TileCoord {
                    z: 10,
                    x: 513,
                    y: 513,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!((window.width, window.height), (256, 256));
        assert!(
            window.rgba.chunks_exact(4).all(|p| p == [255, 255, 0, 255]),
            "every pixel deep inside the yellow quadrant should be solid opaque yellow"
        );
    }

    /// Tokio cannot abort a started `spawn_blocking` task. Cancelling its
    /// awaiting future must therefore retain COG admission until the
    /// operation itself returns.
    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_waiters_keep_blocking_cog_admission_until_operations_finish() {
        let mut controls = Vec::new();
        for _ in 0..MAX_CONCURRENT_BLOCKING_COG_OPERATIONS {
            let (started, started_at) = tokio::sync::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            let task = tokio::spawn(async move {
                run_blocking_cog(move || {
                    let _ = started.send(());
                    released
                        .recv()
                        .map_err(|error| CogError::Decode(error.to_string()))?;
                    Ok(())
                })
                .await
            });
            controls.push((task, started_at, release));
        }

        for (_, started_at, _) in &mut controls {
            started_at.await.expect("the blocking operation starts");
        }
        for (task, _, _) in &controls {
            task.abort();
        }
        for (task, _, _) in &mut controls {
            assert!(task
                .await
                .expect_err("the waiter is cancelled")
                .is_cancelled());
        }
        // The outer awaiting futures have gone away, but their blocking
        // closures have not: every process-wide permit remains accounted for.
        assert_eq!(
            blocking_cog_available_permits(),
            0,
            "cancelling the awaiting futures must not release capacity while their blocking work continues"
        );

        for (_, _, release) in controls {
            release
                .send(())
                .expect("the blocking operation is still waiting");
        }
        while blocking_cog_available_permits() != MAX_CONCURRENT_BLOCKING_COG_OPERATIONS {
            tokio::task::yield_now().await;
        }
    }

    /// `#37` slice 2: a remote storage that ignores `Range` and answers
    /// `200 OK` with the whole body is refused at the same boot-time/
    /// first-touch `CatalogSource::collections` call a local unsupported
    /// GeoTIFF is (`boot_refuses_a_striped_geotiff`) — never silently
    /// downloaded whole.
    #[tokio::test]
    async fn collections_refuses_cleanly_when_the_remote_source_ignores_range_requests() {
        let bytes = std::fs::read(fixture_path()).unwrap();
        let server = crate::test_support::MockServer::ignoring_range(bytes);
        let backend = CogBackend::new(remote_source(
            "TELLURION_COG_ADMIN_REMOTE_RANGE_REFUSAL_FIXTURE",
            server.url("/tiled_rgb.tif"),
        ));

        match backend.collections().await {
            Err(CoreError::Config(message)) => {
                assert!(
                    message.contains("range"),
                    "message should name the real reason (no Range support): {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }
}
