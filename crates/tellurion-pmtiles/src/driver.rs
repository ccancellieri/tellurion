//! The `pmtiles` `DriverFactory`, and the `CatalogSource` + `TileSource`
//! implementation backing it. Read-only: an archive is opened for reading
//! only, there is no write path, no DDL, nothing beyond what the driver
//! contract's mandatory `CatalogSource` plus the optional `TileSource`
//! capability require. `FeatureSource` is never implemented — a collection
//! routed to a `pmtiles` storage on the `features` lane fails at boot with
//! the router's ordinary missing-capability error, exactly like any other
//! driver that doesn't claim a capability.
//!
//! ## Storage config
//!
//! A `pmtiles` storage reuses `StorageDecl.url_env` exactly as `postgis`
//! does: the named environment variable holds the archive's location. Today
//! that's always a local filesystem path (`PathBuf::from` on the raw
//! string), but the shape is deliberately future-compatible — the same
//! field could later hold an `http(s)://` or `s3://` URL and dispatch to a
//! different `pmtiles` backend without any change to `StorageDecl` or
//! config shape. Implementing that dispatch is out of scope here (see the
//! driver-contract design doc's PMTiles section — `object_store`-backed is
//! future work); this driver only ever opens a local file.
//!
//! ## Opening the archive
//!
//! `DriverFactory::build` is synchronous, but opening a PMTiles archive
//! (reading its header and root directory) is real I/O and the `pmtiles`
//! crate's reader constructor is `async`. Rather than blocking the runtime
//! inside `build`, the archive path is captured eagerly (cheap, no I/O) and
//! the reader itself opens lazily on first use via a `tokio::sync::OnceCell`
//! — `Router::validate_catalog` already calls `collections()` once,
//! unconditionally, for every registered storage at boot (see its own doc
//! comment), so a broken or missing archive still surfaces as a boot
//! failure without `build` itself needing to be async.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use pmtiles::{AsyncPmTilesReader, Header, MmapBackend, NoCache, TileCoord as PmTileCoord};
use tokio::sync::OnceCell;

use tellurion_core::{
    CatalogSource, DriverFactory, Error as CoreError, PhysicalCollection, Result as CoreResult,
    SpatialExtent, StorageDecl, StorageDriver, TileCoord, TileSource,
};

use crate::error::{PmtilesDriverError, Result};

/// The tile grid's projection (Web Mercator / EPSG:3857, matching
/// `WebMercatorQuad` — the only tile matrix set this workspace serves).
/// Reported as `PhysicalCollection.srid` for introspection purposes; this
/// driver's own logic never uses it (unlike postgis, which needs it to
/// reproject a native geometry SRID, PMTiles tiles are already rendered
/// into this grid).
const WEB_MERCATOR_SRID: i32 = 3857;

/// Registers the `pmtiles` driver.
#[derive(Default)]
pub struct PmtilesDriverFactory;

impl PmtilesDriverFactory {
    pub fn new() -> Self {
        Self
    }
}

impl DriverFactory for PmtilesDriverFactory {
    fn name(&self) -> &str {
        "pmtiles"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let raw = std::env::var(&decl.url_env).map_err(|_| {
            CoreError::Config(format!(
                "storage '{}': environment variable '{}' is not set",
                decl.id, decl.url_env
            ))
        })?;
        Ok(Arc::new(PmtilesDriverImpl {
            backend: Arc::new(PmtilesBackend::new(PathBuf::from(raw))),
        }))
    }
}

struct PmtilesDriverImpl {
    backend: Arc<PmtilesBackend>,
}

impl StorageDriver for PmtilesDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }

    fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn TileSource>)
    }

    // `feature_source`: default `None` — this driver never implements it.
    // `capacity_hint`: default `None` — a single mmap'd file has no pool-like
    // concurrency ceiling worth reporting; see the trait's own doc comment.
    // `validate_collection`: default accepts everything — there is no
    // operator-declared physical identifier syntax for this driver to check
    // (unlike postgis's table/column identifiers).
}

struct PmtilesBackend {
    path: PathBuf,
    reader: OnceCell<AsyncPmTilesReader<MmapBackend, NoCache>>,
}

impl PmtilesBackend {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            reader: OnceCell::new(),
        }
    }

    async fn reader(&self) -> Result<&AsyncPmTilesReader<MmapBackend, NoCache>> {
        self.reader
            .get_or_try_init(|| async {
                AsyncPmTilesReader::new_with_path(&self.path)
                    .await
                    .map_err(PmtilesDriverError::from)
            })
            .await
    }

    /// The collection name this archive reports: the `name` key in its
    /// TileJSON-style metadata (the conventional way a PMTiles archive
    /// self-describes — set by generators like tippecanoe's `--name`, and by
    /// this crate's own test fixture, see `examples/gen_fixture.rs`), or the
    /// file stem when metadata carries none. Matched against
    /// `CollectionDecl::table`/`id` by `Router::validate_catalog`, exactly
    /// as postgis's reported table name is.
    async fn collection_name(&self) -> Result<String> {
        let reader = self.reader().await?;
        if let Ok(metadata) = reader.get_metadata().await {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&metadata) {
                if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                    return Ok(name.to_string());
                }
            }
        }
        Ok(self
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("archive")
            .to_string())
    }

    /// This archive's real MVT source-layer name(s) (`#49`), read straight
    /// from its own metadata rather than assumed from `collection_name` (the
    /// public collection id) — an archive's `vector_layers` array (the same
    /// TileJSON-convention field tippecanoe and other real generators write)
    /// can carry layer names with nothing in common with the collection id a
    /// deployment happens to configure, so this is the only honest source
    /// for what a client must actually put in a style's `source-layer`.
    /// `Ok(None)` when the metadata carries no such array at all (or isn't
    /// JSON) — the caller then falls back to `collection.external_id()`,
    /// same as any driver with no better answer.
    async fn vector_layers_inner(&self) -> Result<Option<Vec<String>>> {
        let reader = self.reader().await?;
        let Ok(metadata) = reader.get_metadata().await else {
            return Ok(None);
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&metadata) else {
            return Ok(None);
        };
        let names: Vec<String> = value
            .get("vector_layers")
            .and_then(|layers| layers.as_array())
            .map(|layers| {
                layers
                    .iter()
                    .filter_map(|layer| layer.get("id").and_then(|id| id.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Ok((!names.is_empty()).then_some(names))
    }

    async fn catalog_inner(&self) -> Result<Vec<PhysicalCollection>> {
        let name = self.collection_name().await?;
        Ok(vec![PhysicalCollection {
            name,
            // No table-shaped concept for either — this archive serves
            // pre-rendered tiles, not queryable rows (`#20`).
            geometry_column: None,
            primary_key: None,
            srid: Some(WEB_MERCATOR_SRID),
            geometry_type: None,
        }])
    }

    fn extent_from_header(header: &Header) -> Option<SpatialExtent> {
        // Header bounds are already CRS84 (lon/lat, WGS84) per the v3 spec —
        // no reprojection needed, unlike postgis's ST_Transform path.
        Some(SpatialExtent {
            bbox: [
                header.min_longitude,
                header.min_latitude,
                header.max_longitude,
                header.max_latitude,
            ],
        })
    }

    async fn extent_inner(&self) -> Result<Option<SpatialExtent>> {
        let reader = self.reader().await?;
        Ok(Self::extent_from_header(reader.get_header()))
    }

    async fn mvt_tile_inner(&self, coord: TileCoord) -> Result<Option<Bytes>> {
        let reader = self.reader().await?;
        let header = reader.get_header();
        // Out-of-range for this archive's own zoom pyramid: the collection's
        // configured `tiles.minzoom`/`maxzoom` gate already covers the
        // common case (`tellurion-tiles`' `parse_tile_coord`), but a
        // misconfigured collection with a wider range than the archive
        // itself must still come back empty rather than erroring.
        if coord.z < header.min_zoom || coord.z > header.max_zoom {
            return Ok(None);
        }

        // `x`/`y` are already bounds-checked against `2^z` by
        // `tellurion-tiles` before this is ever called; a `TileCoord::new`
        // failure here would mean an address this archive's pyramid cannot
        // represent — treated the same as "not found", not a real failure.
        let Ok(pm_coord) = PmTileCoord::new(coord.z, coord.x, coord.y) else {
            return Ok(None);
        };

        let tile = reader
            .get_tile_decompressed(pm_coord)
            .await
            .map_err(PmtilesDriverError::from)?;
        // A zero-length payload is not a real tile, same convention
        // postgis's `mvt_tile_inner` applies to an empty `ST_AsMVT` result.
        Ok(tile.filter(|bytes| !bytes.is_empty()))
    }
}

#[async_trait]
impl CatalogSource for PmtilesBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        self.catalog_inner().await.map_err(Into::into)
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        self.extent_inner().await.map_err(Into::into)
    }
}

#[async_trait]
impl TileSource for PmtilesBackend {
    // `filter` is always `None` here in practice: this driver leaves
    // `filter_capable` at the trait default (`false`), so `#34`'s policy
    // checkpoint never hands it a grant filter to begin with — a pre-baked
    // archive has no query to narrow, only whole tiles (see the trait's own
    // doc). The parameter still has to exist to satisfy `TileSource`.
    async fn mvt_tile(
        &self,
        _collection: &tellurion_core::CollectionDecl,
        coord: TileCoord,
        _filter: Option<&tellurion_core::Filter>,
    ) -> CoreResult<Option<Bytes>> {
        self.mvt_tile_inner(coord).await.map_err(Into::into)
    }

    async fn vector_layers(
        &self,
        _collection: &tellurion_core::CollectionDecl,
    ) -> CoreResult<Option<Vec<String>>> {
        self.vector_layers_inner().await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geozero::mvt::{tile, Message, Tile};

    /// Reproduces exactly what `examples/gen_fixture.rs` encoded into the
    /// committed archive (same layer-name/geometry convention as
    /// tellurion-tiles' own `valid_mvt_bytes_named` test helper) — lets
    /// `mvt_tile_returns_the_real_tile_bytes_for_an_addressed_coordinate`
    /// assert the served bytes are byte-for-byte the same real vector tile
    /// that was written, round-tripped through gzip decompression, not just
    /// "some non-empty bytes".
    fn expected_mvt_tile(layer_name: &str) -> Bytes {
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

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.pmtiles")
    }

    fn backend() -> PmtilesBackend {
        PmtilesBackend::new(fixture_path())
    }

    #[test]
    fn factory_name_matches_the_config_driver_key() {
        assert_eq!(PmtilesDriverFactory::new().name(), "pmtiles");
    }

    #[test]
    fn build_fails_fast_when_the_env_var_is_unset() {
        let factory = PmtilesDriverFactory::new();
        let decl = StorageDecl {
            id: "main".to_string(),
            driver: "pmtiles".to_string(),
            url_env: "TELLURION_PMTILES_TEST_DOES_NOT_EXIST".to_string(),
            pool_size: None,
        };
        // Defensive: make sure a leftover value from another test/run in the
        // same process can't make this flaky.
        std::env::remove_var(&decl.url_env);
        assert!(matches!(factory.build(&decl), Err(CoreError::Config(_))));
    }

    #[tokio::test]
    async fn collections_reports_the_archive_metadata_name() {
        let backend = backend();
        let collections = backend.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].name, "demo");
        assert_eq!(collections[0].geometry_column, None);
        assert_eq!(collections[0].primary_key, None);
    }

    /// `#49`: the fixture's `vector_layers` metadata names three layers
    /// ("world"/"quadrant"/"leaf") that have nothing to do with the
    /// archive's own collection name ("demo") or any collection id a
    /// deployment might configure — proving this reads the archive's own
    /// metadata, not a guess derived from the collection.
    #[tokio::test]
    async fn vector_layers_reports_the_archives_real_layer_names_from_its_own_metadata() {
        let backend = backend();
        let decl: tellurion_core::CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        let layers = backend.vector_layers(&decl).await.unwrap();
        assert_eq!(
            layers,
            Some(vec![
                "world".to_string(),
                "quadrant".to_string(),
                "leaf".to_string(),
            ])
        );
    }

    #[tokio::test]
    async fn extent_comes_straight_from_the_header_bounds() {
        let backend = backend();
        let physical = &backend.collections().await.unwrap()[0];
        let extent = backend.extent(physical).await.unwrap().unwrap();
        assert_eq!(extent.bbox, [-5.0, 45.0, 5.0, 55.0]);
    }

    #[tokio::test]
    async fn mvt_tile_returns_the_real_tile_bytes_for_an_addressed_coordinate() {
        let backend = backend();
        let decl: tellurion_core::CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        let tile = backend
            .mvt_tile(&decl, TileCoord { z: 0, x: 0, y: 0 }, None)
            .await
            .unwrap();
        assert_eq!(
            tile,
            Some(expected_mvt_tile("world")),
            "served bytes must be the exact, gzip-decompressed MVT tile the fixture wrote"
        );
    }

    #[tokio::test]
    async fn mvt_tile_is_none_for_a_coordinate_the_archive_never_addressed() {
        let backend = backend();
        let decl: tellurion_core::CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        // z1/0/0 was never written by the fixture generator (only 1/1/0 was).
        let tile = backend
            .mvt_tile(&decl, TileCoord { z: 1, x: 0, y: 0 }, None)
            .await
            .unwrap();
        assert_eq!(tile, None);
    }

    #[tokio::test]
    async fn mvt_tile_is_none_above_the_archive_max_zoom() {
        let backend = backend();
        let decl: tellurion_core::CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        let tile = backend
            .mvt_tile(&decl, TileCoord { z: 10, x: 0, y: 0 }, None)
            .await
            .unwrap();
        assert_eq!(tile, None);
    }
}
