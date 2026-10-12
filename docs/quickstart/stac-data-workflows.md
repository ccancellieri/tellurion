# STAC discovery, rendering, harvesting and data export

STAC describes datasets and their assets. It does not replace the decoder,
renderer or writer for those assets. A browser can use STAC to discover a
granule, show its footprint, and follow a link to a Tellurion rendering service;
the service then reads the data through a capable driver.

## Current source-build capabilities

| Operation | Available behavior | Important boundary |
| --- | --- | --- |
| ZIP Shapefile discovery | STAC Collection and feature-derived Items; Features and vector tiles | These Items describe individual features, not automatically one archive asset per dataset |
| Zarr discovery | STAC Collection and PNG raster tiles for supported Zarr v2 stores | No native STAC Item source, arbitrary chunk navigation, or interactive dimension selection |
| Remote STAC harvest | Replay selected collections into predeclared PostGIS targets | Not a lossless STAC mirror: remote assets are counted, not persisted |
| GeoPackage writing | Provision a target, then load canonical GeoJSON features | No browser selection-export workflow or automatic source pagination |
| COG authoring | Convert a supported local GeoTIFF to a tiled COG with overviews | Not arbitrary raster cropping or Zarr conversion |
| Zarr writing | No production writer/export command | The synthetic demo generator is not a general writer |

These are implementation boundaries, not a conformance certification or a claim
that every hosted demo uses the latest source build.

## Navigate the appropriate objects

Use STAC Items for meaningful scenes or granules with spatial and temporal
identity. A Zarr storage chunk is an implementation detail, not automatically
such an Item. Exposing temporal or vertical slices requires a stable slice
identity and a renderer that applies the same selection.

Use Features to retrieve exact vector geometries and attributes. Rendered MVT
features may be clipped or generalized and carry only selected attributes;
they are not a lossless source for a GeoPackage export. A future map-selection
export must resolve stable IDs back to canonical features first.

## Harvest into PostGIS

The current CLI resolves targets from the relational registry. Provision and
publish the local collection first, including its schema and physical identity.
Then inspect the mapping without writing:

```sh
tellurion-ingest harvest stac https://example.org/stac \
  --tenant public --catalog default \
  --collections remote-id --map remote-id=local-id \
  --database-url-env DATABASE_URL --dry-run
```

Remove `--dry-run` to replay Items. Use `--max-items`, `--bookmark` and `--strict`
for a bounded, resumable run. The item limit is evaluated at page boundaries;
it is not a byte-download budget. Asset bytes are never downloaded by this
command. The current collection discovery reads one `/collections` response;
it does not traverse collection-list pagination. Item pagination supports
plain GET next links.

**Preservation warning:** the current mapping writes canonical features and
projects properties onto the target schema. It does not preserve original
STAC links, extension declarations or asset objects. Re-exposing the target
therefore produces locally generated STAC metadata, not an exact copy of the
source. [Issue #42](https://github.com/ccancellieri/tellurion/issues/42) tracks
preservation and recovery across feature, metadata and asset writes.

## Write actual vector data to a file

For a GeoJSON selection already retrieved completely, the existing writer can
create a GeoPackage. This example assumes a `name` attribute and top-level
feature IDs compatible with the driver's integer primary key:

```sh
tellurion-ingest geopackage create-tables \
  --path selected.gpkg --table selected \
  --srid 4326 --geometry-type GEOMETRY --columns name:TEXT
tellurion-ingest geopackage load \
  --path selected.gpkg --table selected \
  --chunk-items 100 --strict selected.geojson
```

Adapt the schema to the source. The loader does not follow Features next links:
one downloaded page is not a complete export. The browser inspector does not
yet orchestrate this workflow.

## Dynamic rendering enrichment: the target contract

Tellurion already contributes cross-protocol links and can generate vector
service asset templates. Completing this for granules and raster sources must
preserve these distinctions:

- **Original assets:** keep source keys, roles, media types, metadata and
  provenance. Do not overwrite them with generated service entries.
- **Concrete previews:** a dereferenceable image may be an asset with role
  `thumbnail` or `visual`. Its rendering must honor the exact Item or slice,
  requested style and access scope.
- **Services:** tile sets, map services and styles belong in typed navigation
  links. Do not present an unresolved tile template as a downloadable image.
- **Collection versus Item:** a whole-collection map is not automatically an
  individual Item preview. Omit unsupported item-scoped representations.
- **Access and cache:** authorize the underlying data and selected style;
  separate cached output by source revision, selection, style revision and
  access scope. Publishing metadata must not trigger bulk rendering or copying.

The current generated templates are collection-scoped, including when attached
to Items, and the legacy asset capability probe is vector-tile based. Therefore
automatic Zarr previews and exact granule-scoped rendering must not be inferred
from their presence.

## Export the preserved WorldCover metadata

The repository's bounded exporter creates a portable static STAC catalog from
the 17 preserved WorldCover records without downloading raster assets:

```sh
python3 demo/gallery/scripts/export_worldcover_stac.py \
  --output /path/to/new/worldcover-stac
```

The destination must not already exist. Omit `--item` to export all preserved
records, or repeat `--item ITEM_ID` to select a subset. The generated manifest
records source metadata hashes, any replaced item-index asset key and reason,
and confirmation that asset bytes were not copied;
retained asset links remain absolute remote HTTP(S) references. A collection
asset whose sole role is `stac-items` is replaced by the exported JSON Item
documents; assets with any other role remain subject to the HTTP(S) boundary.

## Authoritative examples

The existing [ESA WorldCover demo](../../demo/gallery/demos/stac/index.html)
uses preserved upstream records, an explicit source manifest and ESA COGs.
It separates footprint storage in GeoPackage from raster rendering. This is a
dataset-specific workflow, not proof of generic lossless harvesting.

The [EarthCODE collection directory](https://eoapi.workspace.earthcode.eox.at/stac/collections)
is another discovery source. On 2026-09-24 its `4dmed-2d-alt` Collection was
available under CC-BY-4.0 while its Items endpoint returned an empty list.
Always verify actual Items and asset access before choosing a demonstration.

See the [STAC overview](https://stacspec.org/en) and the
[STAC Item assets contract](https://github.com/radiantearth/stac-spec/blob/v1.1.0/item-spec/item-spec.md#assets)
for the distinction between discovery, footprints and downloadable or streamed
assets. An authoritative host is not a guarantee of permanent availability;
record provenance and retrieval evidence for reproducibility.
