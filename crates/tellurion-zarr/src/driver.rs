//! The `zarr` `DriverFactory`, and the `CatalogSource` + `RasterSource`
//! implementation backing it. Read-only — same shape as `tellurion-cog`'s own
//! driver: `FeatureSource`/`TileSource` are never implemented, so a
//! collection routed to a `zarr` storage on the `features` lane, or asking
//! for MVT on the `tiles` lane, fails with the router's ordinary
//! missing-capability refusal, never a stub.
//!
//! ## Storage config
//!
//! A `zarr` storage reuses `StorageDecl.url_env` exactly as `cog`/`pmtiles`
//! do: the named environment variable holds a local filesystem path or an
//! `http(s)://` locator to one Zarr v2 array directory (containing
//! `.zarray`/`.zattrs` directly, plus its chunk files) — one storage backs
//! exactly one collection, the same "one storage, one physical source, one
//! collection" shape `tellurion-cog` uses for a single GeoTIFF file.
//! [`ZarrDriverFactory::build`] decides which by the locator's own scheme,
//! the same one place `tellurion-cog`'s own `parse_source` makes that
//! decision; nothing else in config names it. [`parse_source`] is
//! synchronous and does no I/O either way — for a remote locator, it only
//! validates the URL parses and builds the HTTP client (`store::
//! RemoteZarrSource`), it never probes the network. The first real request
//! this store answers (a `Router::validate_catalog` eager boot sweep, or
//! this collection's first touch under `registry.validation: lazy`) is
//! where a source that's unreachable, or doesn't serve a readable
//! `.zarray`/`.zattrs`, first surfaces — the same "first request pays"
//! choice `tellurion-cog::driver`'s own remote source makes (see that
//! crate's module doc: it defers its own range-support probe to first
//! metadata parse for exactly this reason).
//!
//! ## Georeferencing and colormap
//!
//! Extent/CRS and the fixed leading-dimension index come from the array's own
//! `.zattrs` (see `metadata`'s own doc), never from `config.yaml` —
//! `CatalogSource::extent` only ever receives a `PhysicalCollection`, never
//! the full `CollectionDecl`, so a YAML-declared extent would be invisible to
//! the `/collections` metadata endpoint; keeping the store self-describing
//! (mirroring how a GeoTIFF's own GeoKeys drive `tellurion-cog`) avoids that
//! mismatch entirely. A colormap, by contrast, DOES come from
//! `CollectionDecl.settings.colormap` (`RasterSource::raster_tile` receives
//! the full decl) and is mandatory here — see `colormap`'s own doc.
//!
//! Independent driver crates in this workspace never depend on one another
//! (drivers sit at the same layer below the protocol crates); this crate
//! duplicates `tellurion-cog`'s tiling/colormap math shape rather than
//! reaching into that crate for it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use tellurion_core::{
    CatalogSource, CollectionDecl, DriverFactory, Error as CoreError, PhysicalCollection,
    ProjectionFacts, RasterSource, RasterWindow, Result as CoreResult, SpatialExtent, StorageDecl,
    StorageDriver, TileCoord,
};

use crate::colormap;
use crate::error::ZarrError;
use crate::reader::{self, ZarrMeta};
use crate::store::{FsStore, RemoteZarrSource, ZarrStore};
use crate::tiling;

/// Per-request timeout for every `GET` a [`RemoteZarrSource`] makes — mirrors
/// `tellurion-cog::driver::REMOTE_REQUEST_TIMEOUT_S` exactly (same value,
/// same reasoning: bounds one slow/unreachable remote from tying up a
/// blocking-pool thread indefinitely, the same defensive role
/// `tellurion-core::auth`'s own OIDC HTTP client gives its requests). No new
/// knob invented for this driver's own remote source.
const REMOTE_REQUEST_TIMEOUT_S: u64 = 15;

/// Destination tile size, pixels — matches `tellurion-tiles`' own
/// `RENDER_TILE_SIZE_PX` (256, the OGC API Tiles `WebMercatorQuad` tile size
/// every PNG in this workspace is rendered at). Duplicated rather than
/// shared, the same reasoning `tellurion-cog::driver`'s own constant gives.
const DEST_TILE_SIZE_PX: u32 = 256;

/// Hard per-request cap on how many source pixels (at whichever level
/// `tiling::select_overview` picks) one tile request may read — bounds the
/// assembled `f64` sample buffer (`out_w * out_h * 8` bytes worst case)
/// before any chunk is even opened. Same order of magnitude as
/// `tellurion-cog::driver::MAX_SOURCE_PIXELS`. A `multiscales` pyramid keeps
/// a low zoom level's read small by picking a coarse level in the first
/// place (`tiling`'s own doc); a plain, non-pyramid store still reads at
/// native resolution regardless of zoom, so a low zoom level over a large
/// array is still the case most likely to trip this there — refusing rather
/// than reading an unbounded window, exactly the `#37` first-slice contract.
const MAX_WINDOW_ELEMENTS: u64 = 4_000_000;

fn check_window_budget(width: u32, height: u32, budget: u64) -> Result<(), ZarrError> {
    let requested = u64::from(width) * u64::from(height);
    if requested > budget {
        return Err(ZarrError::WindowBudgetExceeded {
            width: u64::from(width),
            height: u64::from(height),
            budget,
        });
    }
    Ok(())
}

/// Registers the `zarr` driver.
#[derive(Default)]
pub struct ZarrDriverFactory;

impl ZarrDriverFactory {
    pub fn new() -> Self {
        Self
    }
}

impl DriverFactory for ZarrDriverFactory {
    fn name(&self) -> &str {
        "zarr"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let raw = std::env::var(&decl.url_env).map_err(|_| {
            CoreError::Config(format!(
                "storage '{}': environment variable '{}' is not set",
                decl.id, decl.url_env
            ))
        })?;
        let store = parse_source(&decl.id, &raw)?;
        Ok(Arc::new(ZarrDriverImpl {
            backend: Arc::new(ZarrBackend::new(store)),
        }))
    }
}

/// Decides `FsStore` vs. `RemoteZarrSource` from `raw`'s own scheme — the
/// one place in this driver that branches on it, mirroring
/// `tellurion-cog::driver::parse_source`'s own split exactly. No I/O either
/// way: a remote locator only gets its URL parsed and its HTTP client built
/// (see [`REMOTE_REQUEST_TIMEOUT_S`]'s own doc) — reaching the store at all
/// waits for this collection's first metadata parse (`ZarrBackend::meta`),
/// same as a local directory's first open (see this module's own doc).
///
/// A remote locator is always treated as the array directory's own base —
/// `.zarray`, `.zattrs`, and every chunk key are fetched relative to it — so
/// a locator missing its trailing `/` gets one appended before parsing:
/// `Url::join`'s own relative-reference resolution (RFC 3986) would
/// otherwise treat the locator's last path segment as a filename to
/// replace, not a directory to read inside (`store::RemoteZarrSource`'s own
/// doc).
fn parse_source(storage_id: &str, raw: &str) -> CoreResult<Arc<dyn ZarrStore>> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        let normalized = if raw.ends_with('/') {
            raw.to_string()
        } else {
            format!("{raw}/")
        };
        let base_url = reqwest::Url::parse(&normalized).map_err(|error| {
            CoreError::Config(format!(
                "storage '{storage_id}': '{raw}' is not a valid URL: {error}"
            ))
        })?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(REMOTE_REQUEST_TIMEOUT_S))
            .build()
            .map_err(|error| {
                CoreError::Config(format!(
                    "storage '{storage_id}': failed to build the remote HTTP client: {error}"
                ))
            })?;
        Ok(Arc::new(RemoteZarrSource { client, base_url }))
    } else {
        Ok(Arc::new(FsStore::new(PathBuf::from(raw))))
    }
}

struct ZarrDriverImpl {
    backend: Arc<ZarrBackend>,
}

impl StorageDriver for ZarrDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn RasterSource>)
    }

    // `feature_source`/`tile_source`: default `None` — this driver never
    // implements either (see this module's own doc).
}

struct ZarrBackend {
    store: Arc<dyn ZarrStore>,
    meta: tokio::sync::OnceCell<ZarrMeta>,
}

impl ZarrBackend {
    fn new(store: Arc<dyn ZarrStore>) -> Self {
        Self {
            store,
            meta: tokio::sync::OnceCell::new(),
        }
    }

    async fn meta(&self) -> Result<&ZarrMeta, ZarrError> {
        self.meta
            .get_or_try_init(|| async {
                let store = Arc::clone(&self.store);
                match tokio::task::spawn_blocking(move || reader::open(store.as_ref())).await {
                    Ok(result) => result,
                    Err(join_error) => Err(ZarrError::Decode(join_error.to_string())),
                }
            })
            .await
    }

    async fn collections_inner(&self) -> Result<Vec<PhysicalCollection>, ZarrError> {
        let meta = self.meta().await?;
        Ok(vec![PhysicalCollection {
            name: meta.logical_name.clone(),
            // No table-shaped concept — this driver serves decoded pixels,
            // not queryable rows, the same reasoning `tellurion-cog`'s own
            // `collections_inner` gives.
            geometry_column: None,
            primary_key: None,
            // Always CRS84/EPSG:4326-identity in this slice (see
            // `metadata`'s own doc) — the same restriction `tellurion-cog`
            // places on a GeoTIFF's CRS.
            srid: Some(4326),
            geometry_type: None,
        }])
    }

    async fn extent_inner(&self) -> Result<Option<SpatialExtent>, ZarrError> {
        let meta = self.meta().await?;
        Ok(Some(SpatialExtent {
            bbox: meta.extent_crs84,
        }))
    }

    /// `#36` (STAC `projection` extension): every field here comes straight
    /// from the store's own declared georeferencing, never invented —
    /// `epsg` is 4326 by construction ([`reader::open`] refuses a store
    /// with no `tellurion:extent_crs84` declaration, and that declaration
    /// is defined as CRS84/EPSG:4326 with an axis-aligned pixel transform —
    /// see `metadata`'s module doc), `transform` is the finest level's own
    /// pixel -> CRS84 affine (`ZarrMeta::transform`) re-expressed in
    /// `proj:transform`'s row-major order (`e` is the NEGATED
    /// pixel-scale-Y, since rows advance southward), and `shape` is
    /// `[height, width]` (`proj:shape`'s Y-first order) of the finest
    /// level, `levels[0]`.
    async fn projection_inner(&self) -> Result<Option<ProjectionFacts>, ZarrError> {
        let meta = self.meta().await?;
        let finest = &meta.levels[0];
        Ok(Some(ProjectionFacts {
            epsg: Some(4326),
            transform: Some([
                meta.transform.pixel_scale_x,
                0.0,
                meta.transform.origin_x,
                0.0,
                -meta.transform.pixel_scale_y,
                meta.transform.origin_y,
            ]),
            shape: Some([u64::from(finest.height()), u64::from(finest.width())]),
        }))
    }

    async fn raster_tile_inner(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
    ) -> Result<Option<RasterWindow>, ZarrError> {
        let meta = self.meta().await?;

        // A Zarr sample has no inherent visual meaning (unlike an 8-bit COG
        // image); serving PNG tiles from one requires an explicit colormap
        // rather than a guessed default scaling — see `colormap`'s own doc.
        let Some(colormap_conf) = &collection.settings.colormap else {
            return Err(ZarrError::Unsupported(format!(
                "collection '{}' has no colormap configured; a Zarr raster's raw sample has no visual meaning of its own, so PNG tile serving requires an explicit colormap",
                collection.id
            )));
        };

        let bbox = tiling::tile_lonlat_bbox(coord);
        let Some(plan) = tiling::plan_window(
            &meta.levels,
            &meta.transform,
            meta.total_geo_width_deg,
            meta.total_geo_height_deg,
            bbox,
            DEST_TILE_SIZE_PX,
        ) else {
            return Ok(None);
        };

        let window_w = plan.clamped_x1 - plan.clamped_x0;
        let window_h = plan.clamped_y1 - plan.clamped_y0;
        check_window_budget(window_w, window_h, MAX_WINDOW_ELEMENTS)?;

        let store = Arc::clone(&self.store);
        let level = meta.levels[plan.level_index].clone();
        let fixed_index = meta.fixed_index.clone();
        let window = reader::PixelWindow {
            x0: plan.clamped_x0,
            y0: plan.clamped_y0,
            x1: plan.clamped_x1,
            y1: plan.clamped_y1,
        };
        let samples = tokio::task::spawn_blocking(move || {
            reader::read_window(store.as_ref(), &level, &fixed_index, window)
        })
        .await
        .map_err(|join_error| ZarrError::Decode(join_error.to_string()))??;

        let resampled = tiling::resample_to_tile(
            &samples,
            window_w,
            window_h,
            &plan,
            coord,
            DEST_TILE_SIZE_PX,
            meta.transform.origin_y,
        );
        let rgba: Vec<u8> = resampled
            .into_iter()
            .flat_map(|sample| match sample {
                Some(value) => colormap::apply(colormap_conf, value),
                None => [0, 0, 0, 0],
            })
            .collect();

        Ok(Some(RasterWindow {
            width: DEST_TILE_SIZE_PX,
            height: DEST_TILE_SIZE_PX,
            rgba,
        }))
    }
}

#[async_trait]
impl CatalogSource for ZarrBackend {
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
impl RasterSource for ZarrBackend {
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
    use super::*;
    use crate::test_support::{test_client, FixtureStore, MockDirServer};

    fn remote_store(server: &MockDirServer) -> Arc<dyn ZarrStore> {
        Arc::new(RemoteZarrSource {
            client: test_client(),
            base_url: server.base_url(),
        })
    }

    fn decl() -> CollectionDecl {
        serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap()
    }

    fn decl_with_colormap() -> CollectionDecl {
        serde_yaml::from_str(
            "id: demo\ncatalog: default\nstorage: main\n\
             settings:\n  colormap: { kind: ramp, ramp: grayscale, min: 0.0, max: 255.0 }\n",
        )
        .unwrap()
    }

    #[test]
    fn factory_name_matches_the_config_driver_key() {
        assert_eq!(ZarrDriverFactory::new().name(), "zarr");
    }

    #[test]
    fn build_fails_fast_when_the_env_var_is_unset() {
        let factory = ZarrDriverFactory::new();
        let decl = StorageDecl {
            id: "main".to_string(),
            driver: "zarr".to_string(),
            url_env: "TELLURION_ZARR_TEST_DOES_NOT_EXIST".to_string(),
            pool_size: None,
        };
        std::env::remove_var(&decl.url_env);
        assert!(matches!(factory.build(&decl), Err(CoreError::Config(_))));
    }

    #[test]
    fn parse_source_treats_a_bare_path_as_local() {
        let store = parse_source("main", "/data/demo").unwrap();
        assert_eq!(store.describe(), "/data/demo");
    }

    #[test]
    fn parse_source_treats_an_http_url_as_remote() {
        let store = parse_source("main", "http://example.invalid/demo").unwrap();
        // Normalized with a trailing `/` -- see `parse_source`'s own doc for
        // why an array-directory locator always needs one.
        assert_eq!(store.describe(), "http://example.invalid/demo/");
    }

    #[test]
    fn parse_source_treats_an_https_url_as_remote() {
        let store = parse_source("main", "https://example.invalid/demo").unwrap();
        assert_eq!(store.describe(), "https://example.invalid/demo/");
    }

    #[test]
    fn parse_source_rejects_a_malformed_url() {
        assert!(matches!(
            parse_source("main", "http://"),
            Err(CoreError::Config(_))
        ));
    }

    /// `#37` remote-store follow-up: an `http(s)://` locator is now accepted
    /// at `build()` -- and, per this driver's own "first request pays"
    /// contract (this module's own doc), `build()` itself never reaches the
    /// network, so this returns immediately even against a domain reserved
    /// by RFC 2606 to never resolve.
    #[test]
    fn build_accepts_a_remote_url_locator_without_probing_the_network() {
        let factory = ZarrDriverFactory::new();
        let env_var = "TELLURION_ZARR_TEST_BUILD_REMOTE_URL";
        std::env::set_var(env_var, "https://example.invalid/array");
        let decl = StorageDecl {
            id: "main".to_string(),
            driver: "zarr".to_string(),
            url_env: env_var.to_string(),
            pool_size: None,
        };
        assert!(factory.build(&decl).is_ok());
        std::env::remove_var(env_var);
    }

    #[tokio::test]
    async fn collections_reports_the_directory_name_and_crs84() {
        let store = FixtureStore::plain_2d();
        let backend = ZarrBackend::new(Arc::new(FsStore::new(store.path().to_path_buf())));
        let collections = backend.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].srid, Some(4326));
        assert_eq!(collections[0].geometry_column, None);
    }

    #[tokio::test]
    async fn extent_comes_from_the_zattrs_declaration() {
        let store = FixtureStore::plain_2d();
        let backend = ZarrBackend::new(Arc::new(FsStore::new(store.path().to_path_buf())));
        let physical = &backend.collections().await.unwrap()[0];
        let extent = backend.extent(physical).await.unwrap().unwrap();
        assert_eq!(extent.bbox, [-2.0, -2.0, 2.0, 2.0]);
    }

    /// `#36` (STAC projection extension): every projection fact comes
    /// straight from the store's own declared georeferencing — the
    /// `plain_2d` fixture is an 8x8 array over `[-2, -2, 2, 2]` CRS84, so
    /// 0.5 degrees/pixel — with `proj:transform`'s `e` coefficient negated
    /// (rows advance southward) and `proj:shape` in `[height, width]`
    /// order.
    #[tokio::test]
    async fn projection_reads_the_stores_own_georeferencing() {
        let store = FixtureStore::plain_2d();
        let backend = ZarrBackend::new(Arc::new(FsStore::new(store.path().to_path_buf())));
        let physical = &backend.collections().await.unwrap()[0];
        let facts = backend.projection(physical).await.unwrap().unwrap();
        assert_eq!(facts.epsg, Some(4326));
        assert_eq!(facts.shape, Some([8, 8]));
        assert_eq!(facts.transform, Some([0.5, 0.0, -2.0, 0.0, -0.5, 2.0]));
    }

    #[tokio::test]
    async fn boot_refuses_a_store_missing_zattrs_georeferencing() {
        let store = FixtureStore::missing_georef();
        let backend = ZarrBackend::new(Arc::new(FsStore::new(store.path().to_path_buf())));
        match backend.collections().await {
            Err(CoreError::Config(message)) => assert!(message.contains("extent_crs84")),
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    #[tokio::test]
    async fn raster_tile_refuses_without_a_configured_colormap() {
        let store = FixtureStore::plain_2d();
        let backend = ZarrBackend::new(Arc::new(FsStore::new(store.path().to_path_buf())));
        match backend
            .raster_tile(&decl(), TileCoord { z: 0, x: 0, y: 0 })
            .await
        {
            Err(CoreError::Config(message)) => assert!(message.contains("colormap")),
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    /// The z0/x0/y0 tile covers the whole world, so it fully contains the
    /// fixture's tiny `[-2,-2,2,2]` extent -- every sample in this fixture is
    /// the constant `100.0`, so every pixel touched by real data must
    /// resolve to the same mid-gray color under the grayscale ramp
    /// `[0, 255]`, and most of the tile (everywhere else in the world)
    /// stays transparent. `FixtureStore::plain_2d` has no `multiscales`
    /// pyramid at all (`#37` overview/pyramid follow-up) -- this is also the
    /// "degrade honestly" proof that a plain, non-pyramid store keeps
    /// reading at native resolution exactly as it always did, unaffected by
    /// `open`/`read_window` now also supporting a pyramid it doesn't have.
    #[tokio::test]
    async fn raster_tile_applies_the_configured_colormap_to_a_constant_array() {
        let store = FixtureStore::plain_2d();
        let backend = ZarrBackend::new(Arc::new(FsStore::new(store.path().to_path_buf())));
        let window = backend
            .raster_tile(&decl_with_colormap(), TileCoord { z: 0, x: 0, y: 0 })
            .await
            .unwrap()
            .unwrap();
        assert_eq!((window.width, window.height), (256, 256));
        let expected = colormap::apply(
            &tellurion_core::config::ColormapConf::Ramp {
                ramp: tellurion_core::config::ColorRamp::Grayscale,
                min: 0.0,
                max: 255.0,
            },
            100.0,
        );
        assert!(
            window.rgba.chunks_exact(4).any(|pixel| pixel == expected),
            "at least one pixel should show the array's own constant value colored by the ramp"
        );
        assert!(
            window.rgba.chunks_exact(4).any(|pixel| pixel[3] == 0),
            "a tile only partially covered by the array should have transparent pixels outside it"
        );
    }

    #[tokio::test]
    async fn raster_tile_is_none_for_a_coordinate_the_array_never_covers() {
        let store = FixtureStore::plain_2d();
        let backend = ZarrBackend::new(Arc::new(FsStore::new(store.path().to_path_buf())));
        let window = backend
            .raster_tile(&decl_with_colormap(), TileCoord { z: 4, x: 15, y: 0 })
            .await
            .unwrap();
        assert_eq!(window, None);
    }

    #[tokio::test]
    async fn raster_tile_selects_the_fixed_leading_dimension_slice() {
        let store = FixtureStore::with_leading_time_dimension();
        let backend = ZarrBackend::new(Arc::new(FsStore::new(store.path().to_path_buf())));
        let window = backend
            .raster_tile(&decl_with_colormap(), TileCoord { z: 0, x: 0, y: 0 })
            .await
            .unwrap()
            .unwrap();
        // The fixture's `tellurion:fixed_index` selects time step 1, whose
        // constant value is 200.0, never time step 0's 50.0.
        let expected = colormap::apply(
            &tellurion_core::config::ColormapConf::Ramp {
                ramp: tellurion_core::config::ColorRamp::Grayscale,
                min: 0.0,
                max: 255.0,
            },
            200.0,
        );
        assert!(window.rgba.chunks_exact(4).any(|pixel| pixel == expected));
        let wrong_slice = colormap::apply(
            &tellurion_core::config::ColormapConf::Ramp {
                ramp: tellurion_core::config::ColorRamp::Grayscale,
                min: 0.0,
                max: 255.0,
            },
            50.0,
        );
        assert!(!window
            .rgba
            .chunks_exact(4)
            .any(|pixel| pixel == wrong_slice));
    }

    // -- multiscale pyramid serving (`#37` overview/pyramid follow-up) -----

    fn grayscale_ramp_conf() -> tellurion_core::config::ColormapConf {
        tellurion_core::config::ColormapConf::Ramp {
            ramp: tellurion_core::config::ColorRamp::Grayscale,
            min: 0.0,
            max: 255.0,
        }
    }

    /// `RecordingStore::log()` mixes two kinds of entry: metadata document
    /// names (`.zgroup`, `.zattrs`, and every level's own `.zarray` --
    /// `reader::open_pyramid` legitimately reads ALL of these once, up
    /// front, to learn every level's own shape before `tiling::
    /// select_overview` can pick one) and chunk keys (read only for
    /// whichever level actually got selected). The level-selection proof
    /// only means something against the second kind -- this filters the
    /// first back out.
    fn chunk_reads(log: &[String]) -> Vec<&String> {
        log.iter()
            .filter(|path| {
                !path.ends_with(".zarray") && !path.ends_with(".zattrs") && *path != ".zgroup"
            })
            .collect()
    }

    /// The discriminating proof for level selection. `FixtureStore::
    /// pyramid_2d` declares two levels holding two DIFFERENT constants (`10`
    /// at the finest, `200` at the coarsest) rather than one being a real
    /// downsample of the other, so a served tile's color already tells the
    /// two apart -- but the stronger assertion is on `RecordingStore::
    /// log()`, filtered to just the chunk reads (`chunk_reads`'s own doc):
    /// it proves the coarse level's own chunk file (`"1/0.0"`) was the one
    /// this driver actually opened for pixel data, and the fine level's own
    /// chunk files (`"0/..."`) never were, not merely that a
    /// plausible-looking pixel value came back. A z0/x0/y0 tile covers the
    /// whole world, far coarser than either level's own native resolution,
    /// so `tiling::select_overview` must pick the coarsest level that still
    /// satisfies it -- level `"1"`.
    #[tokio::test]
    async fn raster_tile_reads_the_coarse_pyramid_level_for_a_world_covering_zoom() {
        let store = FixtureStore::pyramid_2d();
        let recording = Arc::new(crate::test_support::RecordingStore::wrap(Arc::new(
            FsStore::new(store.path().to_path_buf()),
        )));
        let backend = ZarrBackend::new(recording.clone() as Arc<dyn ZarrStore>);

        let window = backend
            .raster_tile(&decl_with_colormap(), TileCoord { z: 0, x: 0, y: 0 })
            .await
            .unwrap()
            .unwrap();

        let conf = grayscale_ramp_conf();
        let coarse_color = colormap::apply(&conf, 200.0);
        let fine_color = colormap::apply(&conf, 10.0);
        assert!(
            window
                .rgba
                .chunks_exact(4)
                .any(|pixel| pixel == coarse_color),
            "the coarse pyramid level's own constant value should have been served"
        );
        assert!(
            !window.rgba.chunks_exact(4).any(|pixel| pixel == fine_color),
            "the finer level's own constant value must never appear once a coarser level \
             satisfies the requested resolution"
        );

        let log = recording.log();
        let reads = chunk_reads(&log);
        assert!(
            reads.iter().any(|path| path.starts_with("1/")),
            "the coarse level's own chunk file should have been read; chunk reads were {reads:?}"
        );
        assert!(
            !reads.iter().any(|path| path.starts_with("0/")),
            "the finer level's own chunk files must never be read once a coarser level was \
             selected; chunk reads were {reads:?}"
        );
    }

    /// The other direction of the same proof: a zoom whose desired
    /// resolution is finer than the coarse level's own 4x4 pixels must read
    /// the finest level (`"0"`) instead -- proving `select_overview` doesn't
    /// just always pick the last (coarsest) level.
    #[tokio::test]
    async fn raster_tile_reads_the_finest_pyramid_level_when_full_resolution_is_needed() {
        let store = FixtureStore::pyramid_2d();
        let recording = Arc::new(crate::test_support::RecordingStore::wrap(Arc::new(
            FsStore::new(store.path().to_path_buf()),
        )));
        let backend = ZarrBackend::new(recording.clone() as Arc<dyn ZarrStore>);

        // z=10 over the fixture's tiny [-2,-2,2,2] extent asks for a much
        // finer resolution than either level offers, so the finest (index 0,
        // dataset "0") must be picked.
        let window = backend
            .raster_tile(
                &decl_with_colormap(),
                TileCoord {
                    z: 10,
                    x: 511,
                    y: 511,
                },
            )
            .await
            .unwrap()
            .unwrap();

        let conf = grayscale_ramp_conf();
        let fine_color = colormap::apply(&conf, 10.0);
        assert!(
            window.rgba.chunks_exact(4).any(|pixel| pixel == fine_color),
            "the finest level's own constant value should have been served"
        );

        let log = recording.log();
        let reads = chunk_reads(&log);
        assert!(
            reads.iter().any(|path| path.starts_with("0/")),
            "the finest level's own chunk files should have been read; chunk reads were {reads:?}"
        );
        assert!(
            !reads.iter().any(|path| path.starts_with("1/")),
            "the coarse level's own chunk file must never be read when full resolution is \
             needed; chunk reads were {reads:?}"
        );
    }

    /// A `.zgroup` whose `.zattrs` declares no `multiscales` pyramid is
    /// still refused, exactly as a bare hierarchical group always was --
    /// supporting `multiscales` doesn't loosen this driver into serving an
    /// arbitrary group.
    #[tokio::test]
    async fn boot_refuses_a_zarr_group_that_declares_no_multiscales_pyramid() {
        // Build a directory shaped like a Zarr group (`.zgroup` present, no
        // `.zarray`) whose `.zattrs` declares georeferencing but no pyramid.
        let dir = std::env::temp_dir().join(format!(
            "tellurion-zarr-driver-test-no-multiscales-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".zgroup"), r#"{"zarr_format":2}"#).unwrap();
        std::fs::write(
            dir.join(".zattrs"),
            r#"{"tellurion:extent_crs84":[-2.0,-2.0,2.0,2.0]}"#,
        )
        .unwrap();

        let backend = ZarrBackend::new(Arc::new(FsStore::new(dir.clone())));
        match backend.collections().await {
            Err(CoreError::Config(message)) => {
                assert!(message.contains("multiscales"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- remote (`http(s)`) store, driven through a loopback `MockDirServer`
    // -- see `store.rs`'s own doc for the seam this exercises end to end,
    // and `reader.rs`'s own test module for lower-level chunk-fetch proofs
    // (missing chunk = fill, non-2xx = named error, the bomb guard).

    /// The same proof as `collections_reports_the_directory_name_and_crs84`/
    /// `extent_comes_from_the_zattrs_declaration`, but reading through a
    /// loopback HTTP server instead of the local filesystem —
    /// `CatalogSource::collections`/`extent` never touch a local file for a
    /// remote-backed `ZarrBackend`.
    #[tokio::test]
    async fn collections_and_extent_work_over_a_remote_store() {
        let store = FixtureStore::plain_2d();
        let server = MockDirServer::serve(store.path().to_path_buf(), vec![]);
        let backend = ZarrBackend::new(remote_store(&server));

        let collections = backend.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].srid, Some(4326));
        assert_eq!(collections[0].geometry_column, None);

        let extent = backend.extent(&collections[0]).await.unwrap().unwrap();
        assert_eq!(extent.bbox, [-2.0, -2.0, 2.0, 2.0]);
    }

    /// The same proof as
    /// `raster_tile_applies_the_configured_colormap_to_a_constant_array`,
    /// but reading through a loopback HTTP server — the destination pixels
    /// must match exactly, whether the store's bytes came from a local
    /// directory or a series of whole-object `GET` requests.
    #[tokio::test]
    async fn raster_tile_reads_correctly_over_a_remote_store() {
        let store = FixtureStore::plain_2d();
        let server = MockDirServer::serve(store.path().to_path_buf(), vec![]);
        let backend = ZarrBackend::new(remote_store(&server));

        let window = backend
            .raster_tile(&decl_with_colormap(), TileCoord { z: 0, x: 0, y: 0 })
            .await
            .unwrap()
            .unwrap();
        assert_eq!((window.width, window.height), (256, 256));
        let expected = colormap::apply(
            &tellurion_core::config::ColormapConf::Ramp {
                ramp: tellurion_core::config::ColorRamp::Grayscale,
                min: 0.0,
                max: 255.0,
            },
            100.0,
        );
        assert!(
            window.rgba.chunks_exact(4).any(|pixel| pixel == expected),
            "at least one pixel should show the array's own constant value colored by the ramp"
        );
    }

    /// A remote store whose `.zarray` fetch answers `500` refuses cleanly at
    /// the same boot-time/first-touch `CatalogSource::collections` call a
    /// local unreadable store is (`boot_refuses_a_store_missing_zattrs_
    /// georeferencing`) — a named `Error::Config`, not a panic and not a
    /// silently empty catalog.
    #[tokio::test]
    async fn collections_refuses_cleanly_when_the_remote_store_answers_a_server_error() {
        let store = FixtureStore::plain_2d();
        let server = MockDirServer::serve(store.path().to_path_buf(), vec![".zarray".to_string()]);
        let backend = ZarrBackend::new(remote_store(&server));

        match backend.collections().await {
            Err(CoreError::Config(message)) => assert!(
                message.contains("500"),
                "message should name the real reason (the remote status): {message}"
            ),
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }
}
