//! The `cog-mosaic` `DriverFactory` (`#254`): one raster TileSet composed
//! from a **bounded** manifest of COG sources.
//!
//! ## Where the manifest comes from
//!
//! A `cog-mosaic` storage reuses `StorageDecl.url_env` exactly as `cog` does,
//! except that the named environment variable holds the path of a **manifest
//! sidecar** (`manifest.rs`) rather than of a GeoTIFF. `tellurion-ingest cog
//! mosaic` authors that sidecar by measuring every constituent COG; this
//! driver only ever validates it, refusing by name if it does not hold. It
//! never authors a manifest, never repairs one, and issues no DDL.
//!
//! ## The contract this driver keeps, in full
//!
//! * **1..=32 unique sources** ([`manifest::MAX_SOURCES`]), listed in
//!   ascending id order, each with a well-formed CRS84 bbox, a non-zero
//!   byte length and a 64-hex-character SHA-256. Distinct ids must resolve
//!   to distinct local files, so relative and symlink aliases are refused.
//!   Structural refusals happen at [`MosaicDriverFactory::build`], i.e. at
//!   boot / on config reload.
//! * **Measured, not declared.** Before a source's pixels are read for the
//!   first time, its bytes are hashed and its length taken, and its declared
//!   bbox is compared against the COG's OWN georeferencing tags. Any
//!   mismatch is a named refusal ([`CogError::MosaicSourceProvenance`]) —
//!   the source is never served "close enough".
//! * **Selection.** Only sources whose manifest bbox intersects the
//!   requested tile's own CRS84 bbox are read. A tile no source covers is
//!   `None` (the same empty-tile answer the single-COG driver gives), never
//!   a fabricated blank.
//! * **Bounded concurrency.** At most [`MAX_CONCURRENT_READS`] constituent
//!   COGs are read at a time, whatever the selection size — four is a
//!   maximum, not a target. See [`gather_bounded`].
//! * **Deterministic composition.** Selected sources are composed in
//!   **ascending source-id order**, a later id painting over an earlier one
//!   wherever its own pixel is not fully transparent. The result does not
//!   depend on which read finished first: [`gather_bounded`] returns results
//!   in request order, never completion order, and [`compose_in_source_id_order`]
//!   walks that order.
//! * **All-or-error.** If ANY selected source's read fails, the whole
//!   requested tile fails. There is no "compose what worked" path, because a
//!   partially composed tile is byte-indistinguishable from legitimate
//!   transparency — silent corruption dressed as a hole in the data.
//!
//! ## What it reuses rather than forks
//!
//! Everything below the manifest: `driver::TileRead` (the crate's single
//! decode path — `tiling::plan_window` -> `reader::read_window` ->
//! `tiling::resample_to_tile`), `driver::resolve_colormap`'s `#92` rule, and
//! `driver::MAX_SOURCE_PIXELS` — the same PER-REQUEST pixel budget, summed
//! ONCE across every selected source, so a 32-source mosaic may not quietly
//! read 32 times what a single COG is allowed to. There is no second cache
//! here either: a composed tile goes through this workspace's own
//! byte-budgeted tile cache at the response boundary, exactly like every
//! other raster tile.
//!
//! Like `cog`, this driver implements `CatalogSource` + `RasterSource` and
//! never `FeatureSource`/`TileSource` — asking a mosaic collection's tiles
//! lane for MVT is the router's ordinary by-name capability refusal, not a
//! stub and not an empty tile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use tellurion_core::{
    CatalogSource, CollectionDecl, DriverFactory, Error as CoreError, PhysicalCollection,
    RasterSource, RasterWindow, Result as CoreResult, SpatialExtent, StorageDecl, StorageDriver,
    TileCoord,
};

use crate::driver::{
    check_pixel_budget_total, resolve_colormap, run_blocking_cog, TileRead, DEST_TILE_SIZE_PX,
    MAX_SOURCE_PIXELS,
};
use crate::error::CogError;
use crate::manifest::{self, ManifestSource, MosaicManifest};
use crate::reader::{CogMeta, CogSource};
use crate::tiling;

/// The config `driver:` key this factory registers under.
pub const DRIVER_NAME: &str = "cog-mosaic";

/// The issue's own bound: "read no more than four constituent COGs
/// concurrently". A MAXIMUM, not a target — [`gather_bounded`] never has
/// more than this many reads in flight, whatever the selection size, and
/// `bounded_concurrency_never_exceeds_the_cap` observes the real peak rather
/// than reading this constant back.
pub const MAX_CONCURRENT_READS: usize = 4;

/// Registers the `cog-mosaic` driver.
#[derive(Default)]
pub struct MosaicDriverFactory;

impl MosaicDriverFactory {
    pub fn new() -> Self {
        Self
    }
}

impl DriverFactory for MosaicDriverFactory {
    fn name(&self) -> &str {
        DRIVER_NAME
    }

    /// Reads and structurally validates the manifest here, synchronously, so
    /// a malformed one fails `Router::build` — at boot, and on every config
    /// reload — rather than surfacing on some later request. That is the one
    /// place this driver differs from `cog`'s own no-I/O `build`: the
    /// manifest IS configuration, read from a file the way `AppConfig`
    /// itself is, and a configuration error belongs at configuration time.
    /// No source's PIXELS are touched here; provenance verification and the
    /// GeoTIFF parse still happen lazily, once per source (see
    /// [`MosaicBackend::source_meta`]).
    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let raw = std::env::var(&decl.url_env).map_err(|_| {
            CoreError::Config(format!(
                "storage '{}': environment variable '{}' is not set",
                decl.id, decl.url_env
            ))
        })?;
        let backend = MosaicBackend::load(Path::new(&raw))
            .map_err(|error| CoreError::Config(format!("storage '{}': {error}", decl.id)))?;
        Ok(Arc::new(MosaicDriverImpl {
            backend: Arc::new(backend),
        }))
    }
}

struct MosaicDriverImpl {
    backend: Arc<MosaicBackend>,
}

impl StorageDriver for MosaicDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn RasterSource>)
    }

    // `feature_source`/`tile_source`: default `None` — a mosaic serves
    // decoded pixels, never rows or MVT (see this module's own doc).
}

/// One manifest entry, resolved against the manifest's own directory, with
/// its lazily verified metadata beside it.
struct MosaicSource {
    declared: ManifestSource,
    path: PathBuf,
    /// Verified-and-parsed exactly once per process, on this source's first
    /// use — see [`MosaicBackend::source_meta`]. Behind an `Arc` so a
    /// verification can be handed to [`gather_bounded`] as an owned,
    /// `'static` future and therefore run under the SAME bounded-concurrency
    /// cap as a pixel read, rather than one-at-a-time.
    meta: Arc<tokio::sync::OnceCell<CogMeta>>,
}

struct MosaicBackend {
    /// The manifest file's own stem — the physical collection name this
    /// driver reports, the same "no embedded logical name to prefer"
    /// fallback the single-COG driver uses for a GeoTIFF.
    logical_name: String,
    /// In ascending source-id order, which the manifest itself is required
    /// to be in. That order IS the composition order.
    sources: Vec<MosaicSource>,
}

impl MosaicBackend {
    fn load(manifest_path: &Path) -> Result<Self, CogError> {
        let manifest = MosaicManifest::load(manifest_path)?;
        let manifest_dir = manifest_path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        reject_duplicate_local_sources(&manifest, &manifest_dir, manifest_path)?;
        let logical_name = manifest_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "mosaic".to_string());
        Ok(Self::from_manifest(manifest, &manifest_dir, logical_name))
    }

    fn from_manifest(manifest: MosaicManifest, manifest_dir: &Path, logical_name: String) -> Self {
        let sources = manifest
            .sources
            .into_iter()
            .map(|declared| MosaicSource {
                path: declared.resolve_path(manifest_dir),
                declared,
                meta: Arc::new(tokio::sync::OnceCell::new()),
            })
            .collect();
        Self {
            logical_name,
            sources,
        }
    }

    /// Verifies `index`'s provenance against its real bytes and parses its
    /// georeferencing — once per process, whichever request gets there
    /// first. Both halves run on the blocking pool, the same way every other
    /// read in this crate does.
    async fn source_meta(&self, index: usize) -> Result<&CogMeta, CogError> {
        let source = &self.sources[index];
        source
            .meta
            .get_or_try_init(|| async {
                let declared = source.declared.clone();
                let path = source.path.clone();
                run_blocking_cog(move || manifest::verify_source(&declared, &path)).await
            })
            .await
    }

    /// Every source, verified. This is what `Router::validate_catalog`'s own
    /// boot sweep reaches through `CatalogSource::collections`, so a
    /// manifest whose recorded provenance no longer matches the objects on
    /// disk refuses the deployment at boot rather than at the first tile
    /// that happens to select the tampered source.
    async fn verify_all(&self) -> Result<(), CogError> {
        let indices: Vec<usize> = (0..self.sources.len()).collect();
        self.ensure_metas(&indices).await
    }

    /// Verifies (and therefore parses) the given sources, at most
    /// [`MAX_CONCURRENT_READS`] at a time — through the very same
    /// [`gather_bounded`] a pixel read goes through, because a verify is a
    /// full-file hash plus a header walk and is exactly the kind of work the
    /// cap exists to hold down. Errors are all-or-nothing and reported for
    /// the LOWEST failing source id, so the refusal a caller sees does not
    /// depend on scheduling.
    async fn ensure_metas(&self, indices: &[usize]) -> Result<(), CogError> {
        let pending: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&index| !self.sources[index].meta.initialized())
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let mut jobs: Vec<Option<VerifyJob>> = pending
            .iter()
            .map(|&index| {
                Some((
                    Arc::clone(&self.sources[index].meta),
                    self.sources[index].declared.clone(),
                    self.sources[index].path.clone(),
                ))
            })
            .collect();
        let count = jobs.len();
        gather_bounded(count, MAX_CONCURRENT_READS, move |slot| {
            let (cell, declared, path) = jobs[slot]
                .take()
                .expect("gather_bounded requests each slot exactly once");
            async move { verify_into(cell, declared, path).await }
        })
        .await
        .map_err(|(slot, error)| self.name_source_failure(pending[slot], error))?;
        Ok(())
    }

    /// Wraps a per-source failure so it names WHICH source failed — the
    /// difference between a diagnosable mosaic and "something in the mosaic
    /// broke". Provenance refusals already name themselves and are passed
    /// through unchanged.
    fn name_source_failure(&self, index: usize, error: CogError) -> CogError {
        match error {
            provenance @ CogError::MosaicSourceProvenance { .. } => provenance,
            other => CogError::MosaicSourceRead {
                id: self.sources[index].declared.id.clone(),
                message: other.to_string(),
            },
        }
    }

    async fn collections_inner(&self) -> Result<Vec<PhysicalCollection>, CogError> {
        self.verify_all().await?;
        Ok(vec![PhysicalCollection {
            name: self.logical_name.clone(),
            // No table-shaped concept for either — this driver serves
            // decoded pixels, not queryable rows (same as `cog`).
            geometry_column: None,
            primary_key: None,
            srid: self.sources[0]
                .meta
                .get()
                .and_then(|meta| meta.crs.epsg)
                .map(|epsg| epsg as i32),
            geometry_type: None,
        }])
    }

    async fn extent_inner(&self) -> Result<Option<SpatialExtent>, CogError> {
        self.verify_all().await?;
        Ok(Some(SpatialExtent {
            bbox: union_bbox(self.sources.iter().map(|source| source.declared.bbox)),
        }))
    }

    async fn raster_tile_inner(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
    ) -> Result<Option<RasterWindow>, CogError> {
        let tile_bbox = tiling::tile_lonlat_bbox(coord);
        let selected = select_sources(
            self.sources.iter().map(|source| source.declared.bbox),
            tile_bbox,
        );
        if selected.is_empty() {
            return Ok(None);
        }

        // Provenance first: a source is never decoded before its bytes have
        // been proven to be the bytes the manifest recorded.
        self.ensure_metas(&selected).await?;

        // Plan every selected read, then charge the WHOLE request against
        // the one pixel budget, once, before any of it runs.
        let mut reads: Vec<(usize, TileRead)> = Vec::with_capacity(selected.len());
        let mut total_source_pixels: u64 = 0;
        for &index in &selected {
            let meta = self
                .source_meta(index)
                .await
                .map_err(|error| self.name_source_failure(index, error))?;
            let colormap = resolve_colormap(collection, meta)?;
            let Some(read) = TileRead::plan(
                CogSource::Local(self.sources[index].path.clone()),
                meta,
                coord,
                colormap,
            ) else {
                continue;
            };
            total_source_pixels = total_source_pixels.saturating_add(read.source_pixels());
            reads.push((index, read));
        }
        if reads.is_empty() {
            return Ok(None);
        }
        check_pixel_budget_total(total_source_pixels, MAX_SOURCE_PIXELS)?;

        let indices: Vec<usize> = reads.iter().map(|(index, _)| *index).collect();
        let mut planned: Vec<Option<TileRead>> =
            reads.into_iter().map(|(_, read)| Some(read)).collect();
        let count = planned.len();
        let tiles = gather_bounded(count, MAX_CONCURRENT_READS, move |slot| {
            let read = planned[slot]
                .take()
                .expect("gather_bounded requests each slot exactly once");
            async move { read.run().await }
        })
        .await
        .map_err(|(slot, error)| self.name_source_failure(indices[slot], error))?;

        let composed = compose_in_source_id_order(&tiles, DEST_TILE_SIZE_PX);
        Ok(composed.map(|rgba| RasterWindow {
            width: DEST_TILE_SIZE_PX,
            height: DEST_TILE_SIZE_PX,
            rgba,
        }))
    }
}

/// Refuses aliases that would make one physical COG enter the composition
/// more than once under distinct source ids. This runs only after the
/// manifest's structural validation has accepted every local path. It keeps
/// the declared path for later provenance verification rather than replacing
/// it with the canonical path, so source replacement and integrity checks
/// retain their existing behavior.
///
/// A path that cannot yet be canonicalized is deliberately left to the
/// existing source verification path, which owns its named I/O refusal. It
/// cannot be a duplicate local file at this point, and making it an eager
/// boot error would silently change the driver's lazy-verification contract.
fn reject_duplicate_local_sources(
    manifest: &MosaicManifest,
    manifest_dir: &Path,
    manifest_path: &Path,
) -> Result<(), CogError> {
    let mut source_ids_by_file = BTreeMap::<PathBuf, String>::new();

    for source in &manifest.sources {
        let resolved = source.resolve_path(manifest_dir);
        let Ok(canonical) = std::fs::canonicalize(&resolved) else {
            continue;
        };

        if let Some(first_id) = source_ids_by_file.insert(canonical.clone(), source.id.clone()) {
            return Err(CogError::MosaicDuplicateLocalSource {
                manifest_path: manifest_path.display().to_string(),
                first_id,
                duplicate_id: source.id.clone(),
                canonical_path: canonical.display().to_string(),
            });
        }
    }

    Ok(())
}

/// Everything one provenance verification needs, owned: the shared
/// `OnceCell` it fills, the manifest entry it checks against, and where that
/// entry's bytes are. Named rather than written inline so
/// [`MosaicBackend::ensure_metas`]'s job list stays readable.
type VerifyJob = (Arc<tokio::sync::OnceCell<CogMeta>>, ManifestSource, PathBuf);

/// Verifies one source's provenance and parses its georeferencing into
/// `cell`, exactly once. A free function taking owned values (rather than a
/// `&self` method) so the future it returns is `'static` and can be driven
/// by [`gather_bounded`]'s `JoinSet` under the concurrency cap.
async fn verify_into(
    cell: Arc<tokio::sync::OnceCell<CogMeta>>,
    declared: ManifestSource,
    path: PathBuf,
) -> Result<(), CogError> {
    cell.get_or_try_init(|| async move {
        run_blocking_cog(move || manifest::verify_source(&declared, &path)).await
    })
    .await
    .map(|_| ())
}

/// The union of every source's own bbox, CRS84 — the mosaic's extent.
fn union_bbox(boxes: impl Iterator<Item = [f64; 4]>) -> [f64; 4] {
    boxes.fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |acc, bbox| {
            [
                acc[0].min(bbox[0]),
                acc[1].min(bbox[1]),
                acc[2].max(bbox[2]),
                acc[3].max(bbox[3]),
            ]
        },
    )
}

/// The indices of every source whose manifest bbox intersects `tile` —
/// **in the input's own (ascending source-id) order**, which is the order
/// they will be composed in.
///
/// Touching edges do not count as intersecting: a source whose eastern edge
/// is exactly the tile's western edge contributes no pixel to it, and
/// selecting it would cost a whole decode for a fully transparent result.
pub(crate) fn select_sources(
    boxes: impl Iterator<Item = [f64; 4]>,
    tile: tiling::LonLatBbox,
) -> Vec<usize> {
    boxes
        .enumerate()
        .filter(|(_, bbox)| {
            bbox[0] < tile.max_lon
                && bbox[2] > tile.min_lon
                && bbox[1] < tile.max_lat
                && bbox[3] > tile.min_lat
        })
        .map(|(index, _)| index)
        .collect()
}

/// Composes `tiles` — one straight-RGBA8 `size`-square buffer per selected
/// source, **in ascending source-id order** — into a single tile.
///
/// The rule, and it is the whole contract: walking that order, a source's
/// pixel replaces the accumulated one wherever the source's own alpha is not
/// zero. A later (higher) source id therefore paints OVER an earlier one;
/// where a source is transparent, whatever an earlier source put there shows
/// through. Nothing about the result depends on which read finished first.
///
/// `None` when every contribution is fully transparent everywhere — the same
/// empty-tile answer the single-COG driver gives for a coordinate it does
/// not cover, rather than a fabricated blank PNG.
pub(crate) fn compose_in_source_id_order(tiles: &[Vec<u8>], size: u32) -> Option<Vec<u8>> {
    let pixels = size as usize * size as usize;
    let mut dest = vec![0u8; pixels * 4];
    let mut any = false;
    for tile in tiles {
        debug_assert_eq!(
            tile.len(),
            dest.len(),
            "every contribution is the same size"
        );
        for (out, src) in dest.chunks_exact_mut(4).zip(tile.chunks_exact(4)) {
            if src[3] != 0 {
                out.copy_from_slice(src);
                any = true;
            }
        }
    }
    any.then_some(dest)
}

/// Drives `count` independent reads with **at most `limit` in flight at any
/// moment**, and returns their results **in slot order** — never in
/// completion order.
///
/// Both halves matter and both are proven by tests rather than asserted
/// here: `deterministic_order_survives_reversed_completion` makes the last
/// slot finish first and still expects slot order back;
/// `bounded_concurrency_never_exceeds_the_cap` observes the real peak
/// in-flight count from inside the read itself.
///
/// All-or-error: if any read fails, this returns the failure belonging to
/// the LOWEST slot that failed — so the error a caller sees is as
/// deterministic as the success path — and no partial result escapes.
/// The slot index rides along so the caller can name the source.
pub(crate) async fn gather_bounded<T, F, Fut>(
    count: usize,
    limit: usize,
    mut read: F,
) -> Result<Vec<T>, (usize, CogError)>
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = Result<T, CogError>> + Send + 'static,
    T: Send + 'static,
{
    let semaphore = Arc::new(tokio::sync::Semaphore::new(limit.max(1)));
    let mut set = tokio::task::JoinSet::new();
    for slot in 0..count {
        // Building the future does no work — Rust futures are lazy, so the
        // read only begins once the spawned task has a permit in hand. That
        // is what makes the cap a cap on real concurrent reads and not just
        // on task count.
        let future = read(slot);
        let semaphore = Arc::clone(&semaphore);
        set.spawn(async move {
            let permit = semaphore.acquire_owned().await;
            let outcome = match permit {
                Ok(_permit) => future.await,
                Err(error) => Err(CogError::Decode(error.to_string())),
            };
            (slot, outcome)
        });
    }

    let mut slots: Vec<Option<Result<T, CogError>>> = (0..count).map(|_| None).collect();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((slot, outcome)) => slots[slot] = Some(outcome),
            Err(join_error) => {
                return Err((0, CogError::Decode(join_error.to_string())));
            }
        }
    }

    let mut out = Vec::with_capacity(count);
    for (slot, outcome) in slots.into_iter().enumerate() {
        match outcome.expect("every spawned slot reports exactly once") {
            Ok(value) => out.push(value),
            Err(error) => return Err((slot, error)),
        }
    }
    Ok(out)
}

#[async_trait]
impl CatalogSource for MosaicBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        self.collections_inner().await.map_err(Into::into)
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        self.extent_inner().await.map_err(Into::into)
    }
}

#[async_trait]
impl RasterSource for MosaicBackend {
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use crate::manifest::author_mosaic_manifest;
    use crate::tiff_write::{encode_ifd, ifd_encoded_size, tiff_header, TagEntry, Value};

    const A_WEST: &str = "mosaic_a_west.tif";
    const B_EAST: &str = "mosaic_b_east.tif";
    const C_OVERLAP: &str = "mosaic_c_overlap.tif";

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// A real mosaic on disk: the named constituents copied into a fresh
    /// directory with a manifest authored beside them by the very same
    /// [`author_mosaic_manifest`] `tellurion-ingest cog mosaic` calls. The
    /// serving tests below therefore read exactly what `ingest` writes — if
    /// the two halves ever disagreed about the schema, every one of them
    /// would fail rather than a hand-written fixture papering over it.
    fn mosaic(names: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("creates a temp dir");
        let mut inputs = Vec::new();
        for name in names {
            let dest = dir.path().join(name);
            std::fs::copy(fixture(name), &dest).expect("copies the fixture");
            inputs.push(dest);
        }
        let manifest_path = dir.path().join("smoke_mosaic.yaml");
        author_mosaic_manifest(&inputs, &manifest_path).expect("authors the manifest");
        (dir, manifest_path)
    }

    fn backend(manifest_path: &Path) -> MosaicBackend {
        MosaicBackend::load(manifest_path).expect("loads the authored manifest")
    }

    fn assert_duplicate_local_source(error: CogError) {
        match error {
            CogError::MosaicDuplicateLocalSource {
                first_id,
                duplicate_id,
                canonical_path,
                ..
            } => {
                assert_eq!(first_id, "mosaic_a_west");
                assert_eq!(duplicate_id, "mosaic_b_alias");
                assert!(
                    canonical_path.ends_with(A_WEST),
                    "the error must name the shared canonical file: {canonical_path}"
                );
            }
            other => panic!("expected a duplicate-local-source refusal, got {other:?}"),
        }
    }

    /// Writes the smallest tiled GeoTIFF this mosaic test needs: one 256px
    /// tile, one IFD, the supplied band layout and exact bounds for one web
    /// map tile. Keeping it here (rather than committing a binary fixture)
    /// makes the palette and alpha values under test visible beside their
    /// assertions.
    struct SingleTileGeoTiff<'a> {
        bits_per_sample: Vec<u16>,
        photometric: u16,
        samples_per_pixel: u16,
        extra_samples: Option<u16>,
        colormap: Option<Vec<u16>>,
        pixels: &'a [u8],
        bbox: tiling::LonLatBbox,
    }

    fn write_single_tile_geotiff(path: &Path, spec: SingleTileGeoTiff<'_>) {
        const SIZE: u32 = DEST_TILE_SIZE_PX;
        let expected_len = SIZE as usize * SIZE as usize * spec.samples_per_pixel as usize;
        assert_eq!(spec.pixels.len(), expected_len);

        let build_tags = |tile_offset: u32| {
            let mut tags: Vec<TagEntry> = vec![
                (256, Value::Long(vec![SIZE])),
                (257, Value::Long(vec![SIZE])),
                (258, Value::Short(spec.bits_per_sample.clone())),
                (259, Value::Short(vec![1])), // uncompressed
                (262, Value::Short(vec![spec.photometric])),
                (277, Value::Short(vec![spec.samples_per_pixel])),
                (284, Value::Short(vec![1])), // chunky samples
            ];
            if let Some(colormap) = &spec.colormap {
                tags.push((320, Value::Short(colormap.clone())));
            }
            tags.extend([
                (322, Value::Long(vec![SIZE])),
                (323, Value::Long(vec![SIZE])),
                (324, Value::Long(vec![tile_offset])),
                (325, Value::Long(vec![spec.pixels.len() as u32])),
            ]);
            if let Some(extra_sample) = spec.extra_samples {
                tags.push((338, Value::Short(vec![extra_sample])));
            }
            tags.extend([
                (
                    33550,
                    Value::Double(vec![
                        (spec.bbox.max_lon - spec.bbox.min_lon) / f64::from(SIZE),
                        (spec.bbox.max_lat - spec.bbox.min_lat) / f64::from(SIZE),
                        0.0,
                    ]),
                ),
                (
                    33922,
                    Value::Double(vec![
                        0.0,
                        0.0,
                        0.0,
                        spec.bbox.min_lon,
                        spec.bbox.max_lat,
                        0.0,
                    ]),
                ),
                (
                    34735,
                    Value::Short(vec![1, 1, 0, 2, 1024, 0, 1, 2, 2048, 0, 1, 4326]),
                ),
            ]);
            tags
        };

        const HEADER_LEN: u32 = 8;
        let ifd_size = ifd_encoded_size(&build_tags(0));
        let tile_offset = HEADER_LEN + ifd_size;
        let mut bytes = tiff_header(HEADER_LEN).to_vec();
        bytes.extend_from_slice(&encode_ifd(&build_tags(tile_offset), 0, HEADER_LEN));
        bytes.extend_from_slice(spec.pixels);
        std::fs::write(path, bytes).expect("writes the synthetic tiled GeoTIFF");
    }

    fn decl() -> CollectionDecl {
        serde_yaml::from_str("id: mosaic\ncatalog: default\nstorage: main\n").unwrap()
    }

    async fn tile_of(backend: &MosaicBackend, x: u32, y: u32) -> Option<RasterWindow> {
        backend
            .raster_tile(&decl(), TileCoord { z: 10, x, y })
            .await
            .expect("the tile request succeeds")
    }

    fn solid_color(window: &RasterWindow) -> [u8; 4] {
        let first: [u8; 4] = window.rgba[0..4].try_into().unwrap();
        assert!(
            window.rgba.chunks_exact(4).all(|p| p == first),
            "expected a flat tile, but it was not uniform"
        );
        first
    }

    // -- selection ----------------------------------------------------------

    fn bbox_of(min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> [f64; 4] {
        [min_lon, min_lat, max_lon, max_lat]
    }

    fn tile_bbox(min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> tiling::LonLatBbox {
        tiling::LonLatBbox {
            min_lon,
            min_lat,
            max_lon,
            max_lat,
        }
    }

    #[test]
    fn select_sources_returns_only_intersecting_sources_in_ascending_order() {
        let boxes = [
            bbox_of(-1.28, -0.64, 0.0, 0.64),  // 0: west
            bbox_of(0.0, -0.64, 1.28, 0.64),   // 1: east
            bbox_of(-0.64, -0.64, 0.64, 0.64), // 2: overlap
        ];
        assert_eq!(
            select_sources(boxes.into_iter(), tile_bbox(-1.05, 0.0, -0.70, 0.35)),
            vec![0],
            "a tile only the west source covers selects only it"
        );
        assert_eq!(
            select_sources(boxes.into_iter(), tile_bbox(0.70, 0.0, 1.05, 0.35)),
            vec![1],
            "a tile only the east source covers selects only it"
        );
        assert_eq!(
            select_sources(boxes.into_iter(), tile_bbox(-0.35, 0.0, 0.0, 0.35)),
            vec![0, 2],
            "an overlapping tile selects both, in ascending source-id order"
        );
    }

    #[test]
    fn select_sources_excludes_a_source_that_only_touches_the_tile_edge() {
        let boxes = [bbox_of(0.0, -0.64, 1.28, 0.64)];
        assert!(
            select_sources(boxes.into_iter(), tile_bbox(-0.35, 0.0, 0.0, 0.35)).is_empty(),
            "a source whose western edge IS the tile's eastern edge contributes no pixel; \
             selecting it would buy a whole decode for a fully transparent result"
        );
    }

    #[test]
    fn select_sources_returns_nothing_for_a_tile_no_source_covers() {
        let boxes = [bbox_of(-1.28, -0.64, 0.0, 0.64)];
        assert!(select_sources(boxes.into_iter(), tile_bbox(100.0, 40.0, 101.0, 41.0)).is_empty());
    }

    // -- composition --------------------------------------------------------

    fn flat_tile(size: u32, rgba: [u8; 4]) -> Vec<u8> {
        rgba.iter()
            .copied()
            .cycle()
            .take(size as usize * size as usize * 4)
            .collect()
    }

    #[test]
    fn compose_paints_a_later_source_id_over_an_earlier_one() {
        let composed =
            compose_in_source_id_order(&[flat_tile(2, RED), flat_tile(2, BLUE)], 2).unwrap();
        assert!(
            composed.chunks_exact(4).all(|p| p == BLUE),
            "the later source id must win where both are opaque"
        );
    }

    /// The other direction of the same rule — the pair above proves nothing
    /// on its own if composition happened to be order-insensitive.
    #[test]
    fn compose_is_order_sensitive_so_the_reverse_input_gives_the_reverse_answer() {
        let composed =
            compose_in_source_id_order(&[flat_tile(2, BLUE), flat_tile(2, RED)], 2).unwrap();
        assert!(composed.chunks_exact(4).all(|p| p == RED));
    }

    #[test]
    fn compose_lets_an_earlier_source_show_through_a_later_transparent_pixel() {
        let mut later = flat_tile(2, BLUE);
        later[0..4].copy_from_slice(&[0, 0, 0, 0]); // top-left fully transparent
        let composed = compose_in_source_id_order(&[flat_tile(2, RED), later], 2).unwrap();
        assert_eq!(&composed[0..4], &RED, "the earlier source shows through");
        assert!(
            composed[4..].chunks_exact(4).all(|p| p == BLUE),
            "every other pixel is still painted over"
        );
    }

    #[test]
    fn compose_returns_none_when_every_contribution_is_fully_transparent() {
        let empty = vec![0u8; 2 * 2 * 4];
        assert_eq!(
            compose_in_source_id_order(&[empty.clone(), empty], 2),
            None,
            "an all-transparent composition is the same empty-tile answer a single COG gives, \
             not a fabricated blank"
        );
    }

    // -- bounded, deterministic gather --------------------------------------

    /// The property that makes composition order meaningful at all: with
    /// several reads in flight, results come back in SLOT order, never in
    /// completion order. The harness deliberately inverts completion order
    /// and asserts that it really did — a test that could not tell the two
    /// apart would prove nothing.
    #[tokio::test]
    async fn gather_bounded_returns_slot_order_not_completion_order() {
        let completion = Arc::new(Mutex::new(Vec::new()));
        let results = gather_bounded(4, 4, |slot| {
            let completion = Arc::clone(&completion);
            async move {
                tokio::time::sleep(Duration::from_millis(40 * (4 - slot as u64))).await;
                completion.lock().unwrap().push(slot);
                Ok(slot)
            }
        })
        .await
        .expect("every slot succeeds");

        assert_eq!(
            *completion.lock().unwrap(),
            vec![3, 2, 1, 0],
            "the harness must genuinely finish in reverse order, or this test is vacuous"
        );
        assert_eq!(
            results,
            vec![0, 1, 2, 3],
            "results must be in slot order regardless of which read finished first"
        );
    }

    /// The concurrency cap, observed from inside the reads themselves rather
    /// than read back off the constant. Eight jobs against a cap of four: the
    /// peak in-flight count must be exactly four — no more (the cap holds)
    /// and no less (the reads really do overlap, so the cap is a cap and not
    /// an accident of serialization).
    #[tokio::test]
    async fn gather_bounded_never_exceeds_the_concurrency_cap() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let jobs = 8;

        let results = gather_bounded(jobs, MAX_CONCURRENT_READS, |slot| {
            let in_flight = Arc::clone(&in_flight);
            let peak = Arc::clone(&peak);
            async move {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(80)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(slot)
            }
        })
        .await
        .expect("every slot succeeds");

        assert_eq!(results, (0..jobs).collect::<Vec<_>>());
        assert_eq!(
            peak.load(Ordering::SeqCst),
            MAX_CONCURRENT_READS,
            "eight reads against a cap of {MAX_CONCURRENT_READS} must peak at exactly \
             {MAX_CONCURRENT_READS} in flight"
        );
    }

    /// All-or-error, at the gather: one failing slot fails the whole batch,
    /// and the reported failure is the LOWEST failing slot so the refusal is
    /// as deterministic as the success path.
    #[tokio::test]
    async fn gather_bounded_fails_the_whole_batch_and_names_the_lowest_failing_slot() {
        let (slot, error) = gather_bounded(4, MAX_CONCURRENT_READS, |slot| async move {
            if slot == 1 || slot == 3 {
                Err(CogError::Decode(format!("slot {slot} broke")))
            } else {
                Ok(slot)
            }
        })
        .await
        .expect_err("a failing slot fails the batch");
        assert_eq!(slot, 1);
        assert!(error.to_string().contains("slot 1 broke"), "{error}");
    }

    // -- the driver, over a real authored mosaic ----------------------------

    #[test]
    fn factory_name_matches_the_config_driver_key() {
        assert_eq!(MosaicDriverFactory::new().name(), "cog-mosaic");
    }

    fn storage_decl(id: &str, url_env: &str) -> StorageDecl {
        StorageDecl {
            id: id.to_string(),
            driver: DRIVER_NAME.to_string(),
            url_env: url_env.to_string(),
            pool_size: None,
        }
    }

    #[test]
    fn build_fails_fast_when_the_env_var_is_unset() {
        let decl = storage_decl("main", "TELLURION_COG_MOSAIC_TEST_DOES_NOT_EXIST");
        std::env::remove_var(&decl.url_env);
        assert!(matches!(
            MosaicDriverFactory::new().build(&decl),
            Err(CoreError::Config(_))
        ));
    }

    #[test]
    fn build_refuses_a_manifest_that_is_not_there_naming_the_file() {
        let env = "TELLURION_COG_MOSAIC_TEST_MISSING";
        std::env::set_var(env, "/nonexistent/mosaic.yaml");
        match MosaicDriverFactory::new().build(&storage_decl("main", env)) {
            Err(CoreError::Config(message)) => {
                assert!(
                    message.contains("mosaic manifest") && message.contains("/nonexistent"),
                    "{message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
        std::env::remove_var(env);
    }

    /// The source-count bound, refused where an operator meets it: at
    /// `Router::build`, through the real factory, naming the count and the
    /// bound.
    #[test]
    fn build_refuses_a_manifest_over_the_thirty_two_source_bound_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("too_many.yaml");
        let mut yaml = String::from("version: 1\nsources:\n");
        for index in 0..(crate::manifest::MAX_SOURCES + 1) {
            yaml.push_str(&format!(
                "  - id: s{index:03}\n    path: s{index:03}.tif\n    \
                 bbox: [-1.0, -1.0, 1.0, 1.0]\n    byte_length: 10\n    sha256: {}\n",
                "a".repeat(64)
            ));
        }
        std::fs::write(&manifest_path, yaml).unwrap();

        let env = "TELLURION_COG_MOSAIC_TEST_TOO_MANY";
        std::env::set_var(env, &manifest_path);
        match MosaicDriverFactory::new().build(&storage_decl("main", env)) {
            Err(CoreError::Config(message)) => {
                assert!(
                    message.contains("33 sources") && message.contains("bound of 32"),
                    "the refusal must name the count and the bound: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
        std::env::remove_var(env);
    }

    #[test]
    fn load_refuses_distinct_source_ids_with_relative_aliases_of_the_same_file() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST]);
        tamper(&manifest_path, |manifest| {
            let original = manifest.sources[0].clone();
            let alias = &mut manifest.sources[1];
            alias.id = "mosaic_b_alias".to_string();
            alias.path = format!("./{A_WEST}");
            alias.bbox = original.bbox;
            alias.byte_length = original.byte_length;
            alias.sha256 = original.sha256;
        });

        match MosaicBackend::load(&manifest_path) {
            Err(error) => assert_duplicate_local_source(error),
            Ok(_) => panic!(
                "distinct source ids must not let one local file appear twice through relative aliases"
            ),
        }
    }

    #[cfg(unix)]
    #[test]
    fn load_refuses_distinct_source_ids_with_symlink_aliases_of_the_same_file() {
        let (dir, manifest_path) = mosaic(&[A_WEST, B_EAST]);
        let alias_path = dir.path().join("mosaic_b_alias.tif");
        std::os::unix::fs::symlink(dir.path().join(A_WEST), &alias_path)
            .expect("creates a symlink alias to the first source");
        tamper(&manifest_path, |manifest| {
            let original = manifest.sources[0].clone();
            let alias = &mut manifest.sources[1];
            alias.id = "mosaic_b_alias".to_string();
            alias.path = "mosaic_b_alias.tif".to_string();
            alias.bbox = original.bbox;
            alias.byte_length = original.byte_length;
            alias.sha256 = original.sha256;
        });

        match MosaicBackend::load(&manifest_path) {
            Err(error) => assert_duplicate_local_source(error),
            Ok(_) => panic!(
                "distinct source ids must not let one local file appear twice through a symlink"
            ),
        }
    }

    /// Capability honesty: a mosaic advertises `RasterSource` and nothing
    /// else, so the tiles lane's own MVT refusal (and the features lane's
    /// 404) come from the router, not from a stub here.
    #[test]
    fn the_driver_advertises_raster_only_never_features_or_vector_tiles() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST, C_OVERLAP]);
        let env = "TELLURION_COG_MOSAIC_TEST_CAPS";
        std::env::set_var(env, &manifest_path);
        let driver = MosaicDriverFactory::new()
            .build(&storage_decl("main", env))
            .expect("builds over a real authored manifest");
        assert!(driver.raster_source().is_some());
        assert!(driver.feature_source().is_none());
        assert!(driver.tile_source().is_none());
        std::env::remove_var(env);
    }

    #[tokio::test]
    async fn collections_reports_the_manifest_stem_and_the_sources_own_epsg() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST, C_OVERLAP]);
        let backend = backend(&manifest_path);
        let collections = backend.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "smoke_mosaic");
        assert_eq!(collections[0].srid, Some(4326));
        assert_eq!(collections[0].geometry_column, None);
    }

    #[tokio::test]
    async fn extent_is_the_union_of_every_sources_own_measured_bbox() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST, C_OVERLAP]);
        let backend = backend(&manifest_path);
        let physical = &backend.collections().await.unwrap()[0];
        let extent = backend.extent(physical).await.unwrap().unwrap();
        for (actual, expected) in extent.bbox.iter().zip([-1.28, -0.64, 1.28, 0.64]) {
            assert!(
                (actual - expected).abs() < 1e-9,
                "extent {:?} should be the union of the three constituents",
                extent.bbox
            );
        }
    }

    /// Selection, observed in pixels: z10/x509 sits entirely inside the WEST
    /// constituent and outside the overlapping one, so a red tile proves
    /// only that source was read; z10/x514 is the same statement for EAST.
    #[tokio::test]
    async fn a_tile_inside_exactly_one_source_shows_that_sources_own_color() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST, C_OVERLAP]);
        let backend = backend(&manifest_path);

        let west = tile_of(&backend, 509, 511).await.expect("west covers it");
        assert_eq!((west.width, west.height), (256, 256));
        assert_eq!(solid_color(&west), RED);

        let east = tile_of(&backend, 514, 511).await.expect("east covers it");
        assert_eq!(solid_color(&east), GREEN);
    }

    /// Composition order, observed in pixels: z10/x511 is covered by BOTH
    /// `mosaic_a_west` and `mosaic_c_overlap`. `c` sorts last, so `c` paints
    /// over `a` and the tile is blue. z10/x512 is the mirror image on the
    /// east side.
    #[tokio::test]
    async fn where_two_sources_overlap_the_higher_source_id_paints_over_the_lower() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST, C_OVERLAP]);
        let backend = backend(&manifest_path);

        let west_seam = tile_of(&backend, 511, 511).await.expect("covered");
        assert_eq!(
            solid_color(&west_seam),
            BLUE,
            "'mosaic_c_overlap' sorts after 'mosaic_a_west', so it paints over it"
        );

        let east_seam = tile_of(&backend, 512, 511).await.expect("covered");
        assert_eq!(solid_color(&east_seam), BLUE);
    }

    /// A real palette COG under a real RGBA COG with transparent nodata.
    /// Index 1 in the lower source is red only through its TIFF ColorMap;
    /// the higher source uses alpha zero for its left half, where that red
    /// must show through, and opaque blue for its right half, where it wins.
    /// This is deliberately end-to-end through manifest authoring, palette
    /// decoding, tile warping, and mosaic composition.
    #[tokio::test]
    async fn a_paletted_source_shows_through_an_overlays_transparent_nodata() {
        const SIZE: usize = DEST_TILE_SIZE_PX as usize;
        let coord = TileCoord {
            z: 10,
            x: 512,
            y: 512,
        };
        let bbox = tiling::tile_lonlat_bbox(coord);
        let dir = tempfile::tempdir().expect("creates a temp dir");

        let mut palette = vec![0u16; 768];
        palette[1] = u16::MAX; // index 1 -> red
        let palette_path = dir.path().join("a_palette.tif");
        let palette_pixels = vec![1; SIZE * SIZE];
        write_single_tile_geotiff(
            &palette_path,
            SingleTileGeoTiff {
                bits_per_sample: vec![8],
                photometric: 3, // RGBPalette
                samples_per_pixel: 1,
                extra_samples: None,
                colormap: Some(palette),
                pixels: &palette_pixels,
                bbox,
            },
        );

        let mut overlay = vec![0u8; SIZE * SIZE * 4];
        for row in overlay.chunks_exact_mut(SIZE * 4) {
            for pixel in row.chunks_exact_mut(4).skip(SIZE / 2) {
                pixel.copy_from_slice(&BLUE);
            }
        }
        let overlay_path = dir.path().join("b_nodata_overlay.tif");
        write_single_tile_geotiff(
            &overlay_path,
            SingleTileGeoTiff {
                bits_per_sample: vec![8, 8, 8, 8],
                photometric: 2, // RGB
                samples_per_pixel: 4,
                extra_samples: Some(2), // unassociated alpha
                colormap: None,
                pixels: &overlay,
                bbox,
            },
        );

        let manifest_path = dir.path().join("palette_nodata.yaml");
        author_mosaic_manifest(&[palette_path, overlay_path], &manifest_path)
            .expect("authors the real sidecar");
        let window = MosaicBackend::load(&manifest_path)
            .expect("loads the real sidecar")
            .raster_tile(&decl(), coord)
            .await
            .expect("serves the mosaic")
            .expect("both sources cover the requested tile");

        let pixel = |x: usize, y: usize| -> [u8; 4] {
            window.rgba[(y * SIZE + x) * 4..(y * SIZE + x + 1) * 4]
                .try_into()
                .unwrap()
        };
        assert_eq!(
            pixel(64, 128),
            RED,
            "transparent nodata must reveal the palette's decoded red, not its raw index"
        );
        assert_eq!(
            pixel(192, 128),
            BLUE,
            "an opaque later source still paints over the palette"
        );
    }

    /// The same overlapping tile, requested repeatedly: byte-identical every
    /// time. With more than one read in flight this is exactly the property
    /// that a completion-ordered composition would violate intermittently
    /// rather than reliably.
    #[tokio::test]
    async fn an_overlapping_tile_is_byte_identical_across_repeated_requests() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST, C_OVERLAP]);
        let backend = backend(&manifest_path);
        let first = tile_of(&backend, 511, 511).await.unwrap();
        for _ in 0..8 {
            assert_eq!(
                tile_of(&backend, 511, 511).await.unwrap().rgba,
                first.rgba,
                "a composed tile's bytes must not depend on which read finished first"
            );
        }
    }

    #[tokio::test]
    async fn a_tile_no_source_covers_is_none_rather_than_a_blank() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST, C_OVERLAP]);
        let backend = backend(&manifest_path);
        assert_eq!(
            backend
                .raster_tile(&decl(), TileCoord { z: 2, x: 0, y: 0 })
                .await
                .unwrap(),
            None
        );
    }

    // -- provenance: measured, not declared ---------------------------------

    /// Rewrites the authored manifest with `mutate` applied to it — the
    /// hand-edit the provenance checks exist to catch.
    fn tamper(manifest_path: &Path, mutate: impl FnOnce(&mut MosaicManifest)) {
        let mut manifest = MosaicManifest::load(manifest_path).unwrap();
        mutate(&mut manifest);
        std::fs::write(manifest_path, serde_yaml::to_string(&manifest).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn a_sha256_that_does_not_match_the_bytes_is_refused_by_name() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST]);
        tamper(&manifest_path, |manifest| {
            manifest.sources[1].sha256 = "b".repeat(64);
        });
        let backend = backend(&manifest_path);
        match backend.collections().await {
            Err(CoreError::Config(message)) => {
                assert!(
                    message.contains("mosaic_b_east")
                        && message.contains("sha256")
                        && message.contains("hashes to"),
                    "the refusal must name the source and the mismatch: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    #[tokio::test]
    async fn a_byte_length_that_does_not_match_the_object_is_refused_by_name() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST]);
        tamper(&manifest_path, |manifest| {
            manifest.sources[0].byte_length += 1;
        });
        let backend = backend(&manifest_path);
        match backend.collections().await {
            Err(CoreError::Config(message)) => {
                assert!(
                    message.contains("mosaic_a_west") && message.contains("byte_length"),
                    "{message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    /// A bbox typed in by hand rather than measured — structurally valid,
    /// but not what the COG's own georeferencing tags say.
    #[tokio::test]
    async fn a_bbox_that_disagrees_with_the_geotiffs_own_tags_is_refused_by_name() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST]);
        tamper(&manifest_path, |manifest| {
            manifest.sources[0].bbox = [-2.0, -0.64, 0.0, 0.64];
        });
        let backend = backend(&manifest_path);
        match backend.collections().await {
            Err(CoreError::Config(message)) => {
                assert!(
                    message.contains("mosaic_a_west")
                        && message.contains("bbox")
                        && message.contains("MEASURED"),
                    "{message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    // -- all-or-error -------------------------------------------------------

    /// The policy that cannot be softened: once a source has been selected
    /// for a tile, a failure reading it fails the WHOLE tile. The alternative
    /// — composing what succeeded — would return a red tile here, which is
    /// byte-indistinguishable from a legitimate mosaic in which nothing blue
    /// covers this coordinate. That is silent corruption, so this asserts an
    /// `Err`, and asserts specifically that no tile came back.
    #[tokio::test]
    async fn a_failed_read_of_one_selected_source_fails_the_whole_tile() {
        let (dir, manifest_path) = mosaic(&[A_WEST, B_EAST, C_OVERLAP]);
        let backend = backend(&manifest_path);

        // First request succeeds and caches both selected sources' metadata,
        // so what follows is a READ failure, not a verification one.
        assert_eq!(
            solid_color(&tile_of(&backend, 511, 511).await.unwrap()),
            BLUE
        );

        std::fs::remove_file(dir.path().join(C_OVERLAP)).expect("removes one constituent");

        match backend
            .raster_tile(
                &decl(),
                TileCoord {
                    z: 10,
                    x: 511,
                    y: 511,
                },
            )
            .await
        {
            Err(CoreError::Storage(source)) => {
                let message = source.to_string();
                assert!(
                    message.contains("mosaic_c_overlap") && message.contains("whole tile fails"),
                    "the refusal must name the source that failed: {message}"
                );
            }
            Ok(Some(window)) => panic!(
                "a partially composed tile escaped: {:?} — indistinguishable from legitimate \
                 transparency",
                solid_color(&window)
            ),
            Ok(None) => panic!("an empty tile escaped instead of a named failure"),
            Err(other) => panic!("expected Err(Storage(_)), got {other:?}"),
        }
    }

    // -- reuse, not a fork --------------------------------------------------

    /// The mosaic charges the SAME per-request pixel budget the single-COG
    /// driver does, summed once over every selected source — not one budget
    /// per source. Exercised through the shared helper at the exact value
    /// the mosaic passes it, so the assertion is about the budget's meaning
    /// rather than about a fixture large enough to trip the real ceiling.
    #[test]
    fn the_pixel_budget_is_charged_once_per_request_across_every_selected_source() {
        let per_source = MAX_SOURCE_PIXELS / 3 + 1;
        assert!(
            check_pixel_budget_total(per_source, MAX_SOURCE_PIXELS).is_ok(),
            "one source well inside the budget is fine"
        );
        match check_pixel_budget_total(per_source * 3, MAX_SOURCE_PIXELS) {
            Err(CogError::PixelBudgetExceeded { requested, budget }) => {
                assert_eq!(requested, per_source * 3);
                assert_eq!(budget, MAX_SOURCE_PIXELS);
            }
            other => panic!(
                "three such sources together must exceed the ONE request budget, got {other:?}"
            ),
        }
    }

    /// The authored document itself, as text: the header comment, and one
    /// `- id:` line per source in ascending order. Asserted on the FILE
    /// rather than on the parsed struct because `scripts/demo-smoke.sh`
    /// phase 23 greps exactly these lines to prove the composition order is
    /// readable straight off the sidecar — a layout change that broke that
    /// grep would otherwise only surface in the smoke run.
    #[test]
    fn the_authored_document_lists_its_source_ids_one_per_line_in_ascending_order() {
        let (_dir, manifest_path) = mosaic(&[A_WEST, B_EAST, C_OVERLAP]);
        let text = std::fs::read_to_string(&manifest_path).unwrap();
        assert!(
            text.contains("MEASURED from the source object"),
            "the authored document must say, in the file, that its provenance was measured"
        );
        let ids: Vec<&str> = text
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("- id: "))
            .collect();
        assert_eq!(
            ids,
            vec!["mosaic_a_west", "mosaic_b_east", "mosaic_c_overlap"],
            "authored document was:\n{text}"
        );
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "ascending id order IS the composition order");
    }

    #[tokio::test]
    async fn a_single_source_mosaic_serves_exactly_what_that_source_serves() {
        let (_dir, manifest_path) = mosaic(&[A_WEST]);
        let backend = backend(&manifest_path);
        assert_eq!(
            solid_color(&tile_of(&backend, 509, 511).await.unwrap()),
            RED,
            "the smallest legal mosaic (one source) is just that source"
        );
    }
}
