# Italy-wide ESA WorldCover Mosaic

## Status

Approved product design. Implementation and public claims remain contingent on
the verification gates in this document.

## Goal

Expand the live ESA WorldCover demonstration from one Rome source tile to real
2021 coverage across Italy. The opening map must show the country while the
same service continues to expose original 10 m categorical detail when a user
zooms below city scale.

The result must make three facts visible:

1. country scale is a composition of independently identified source COGs;
2. city detail is read from those sources, not from an enlarged overview image;
3. every public request is served from a bounded release snapshot, never from
   request-time STAC federation.

This is a coverage and serving demonstration. It is not an Italy-scale capacity
claim or a land-cover analysis.

## Current boundary

The deployed collection `esa_worldcover_2021_rome` routes one STAC Item and one
3° × 3° COG, `ESA_WorldCover_10m_2021_v200_N39E012`. Its map starts over Rome
at zoom 13.2. Zooming that collection out cannot truthfully represent national
coverage.

The existing Tellurion COG driver intentionally binds one storage declaration
to one local or remote GeoTIFF. Its per-request window planning, overview
selection, source-pixel budget, embedded palette handling, cache integration,
and PNG encoding are already the correct primitives for each national source.
The missing primitive is deterministic composition across a bounded manifest
of those sources.

## Source selection

The source remains Microsoft Planetary Computer's `esa-worldcover` STAC
Collection, with ESA and the ESA WorldCover Consortium retained as producer and
processor. Only 2021 v200 Items and their `map` assets are eligible.

The boundary input is the Italy feature (`CNTR_ID=IT`) extracted from the
Eurostat/GISCO Countries 2024 regions dataset at 1:1 million scale in
EPSG:4326. The accepted source file, extracted geometry, attribution, and
SHA-256 digest are committed as release evidence. The geometry is treated as
CRS84 longitude/latitude when sent to STAC.

A bounding-box search currently produces 22 candidate 2021 Items; the release
manifest records the exact subset whose Item footprints intersect that pinned
country geometry. This count is evidence, not a hard-coded protocol
assumption.

For every accepted Item, the harvester preserves:

- the complete source Item document and stable source Item id;
- the Item footprint and bounding box;
- the original `map` asset href and media type;
- the official anonymous ESA S3 mirror used at runtime;
- providers, licence, temporal metadata, and classification metadata;
- SHA-256 digests for the source documents, plus byte length and SHA-256 for
  each served COG;
- the harvest timestamp and transform version.

The source search, mirror verification, and manifest generation occur only at
build or release time. Refreshing them must create a finite, reviewable diff.

## Architecture

```text
Planetary Computer STAC + versioned Italy boundary
        |
        | bounded 2021 harvest, validation, mirror verification
        v
immutable Item snapshots + national mosaic manifest
        |                                |
        | footprints                     | official ESA COG locators
        v                                v
GeoPackage feature lane          Tellurion COG mosaic raster lane
        |                                |
        +----------- one collection ----+
                         |
                         v
         STAC / Features / raster Tiles / browser map
```

One logical collection, `esa_worldcover_2021_italy`, uses split routing:

- GeoPackage supplies one feature per harvested source Item to the Features
  and STAC lanes;
- a new COG mosaic source supplies raster windows to the Tiles lane;
- the existing Rome collection remains configured separately for backward
  compatibility.

The public runtime reads committed metadata and official ESA COGs by HTTP range
request. It never calls the upstream STAC API.

## COG mosaic driver

The reusable mosaic capability is added inside the existing `tellurion-cog`
crate and registered as driver `cog-mosaic`. Keeping it beside the single-COG
implementation lets it reuse the proven reader and tiling code without a
second TIFF stack.

Its storage environment variable resolves to a local JSON manifest with this
logical shape:

```json
{
  "id": "esa_worldcover_2021_italy",
  "sources": [
    {
      "id": "ESA_WorldCover_10m_2021_v200_N39E012",
      "href": "https://esa-worldcover.s3.eu-central-1.amazonaws.com/...tif",
      "bbox": [12.0, 39.0, 15.0, 42.0],
      "byte_length": 41236803,
      "sha256": "5d951afb19e5fdcb90773bac374b556d425f8945ba4b719114c7f7b03157464a"
    }
  ]
}
```

The manifest parser rejects an empty source list, duplicate ids or hrefs,
invalid CRS84 boxes, non-HTTPS remote locators, missing digests, and more than
32 sources. Sources are sorted by id to make composition deterministic.

For each WebMercator tile request the driver:

1. computes the request box in CRS84;
2. selects only manifest sources whose boxes intersect it;
3. lazily opens their metadata and delegates window planning, overview
   selection, pixel-budget enforcement, and palette decoding to the current COG
   implementation;
4. reads at most four sources concurrently;
5. composites non-transparent pixels in stable source-id order;
6. returns `None` when no source intersects.

A source failure fails the tile request rather than silently returning partial
national coverage. The existing rendered-tile cache stores the final composite,
so repeat requests do not repeat source reads. A linear scan of at most 32
bounding boxes is deliberate; a spatial index would add complexity without
material value at this scale.

WorldCover value `0` stays transparent. All other class colors come from each
paletted source COG. The mosaic does not apply a generated colormap override or
execute remote style instructions.

## STAC and Features projection

The national GeoPackage layer contains one polygon feature per accepted source
Item. Local integer ids are deterministic after sorting by source Item id. Each
feature retains `source_item_id`, acquisition time, source grid code, product
version, and the manifest digest that binds it to the raster lane.

The STAC Collection spatial extent is the union of the accepted source Item
footprints; its temporal extent remains the preserved 2021 product interval.
Collection links advertise only live, verified Tellurion capabilities.

Current Tellurion supports config-declared Collection assets and PostGIS-backed
per-Item asset records, plus a separate PostGIS STAC metadata sidecar. The
embedded GeoPackage driver used by this deployment advertises neither sidecar,
and the STAC harvester does not yet project harvested asset maps into those
records. Therefore the source Item snapshots and national manifest remain this
demo's authoritative asset evidence. The demo must not invent Collection-level
asset semantics for a multi-Item source set or present the PostGIS-only
capability as if it were available through GeoPackage.

## Viewer

The live map becomes the opening thesis: one country view backed by a single
Tellurion TileSet URL, with source COG composition occurring behind that
stable protocol surface.

The initial camera fits Italy with a small responsive margin. Five keyboard-
accessible place controls move to Milan, Venice, Rome, Naples, and Palermo at
city zoom; normal pan and zoom continue to sub-city detail through zoom 14. A
“Show Italy” control always restores the country view.

A scale rail labels the current narrative level—country, region, city, or
neighbourhood—and reports the current zoom and approximate ground resolution.
At country and regional zoom, a quiet optional footprint overlay exposes which
harvested source Items form the mosaic. The layer disappears at city zoom so
it does not obscure categorical pixels.

The existing dark, evidence-led visual system, WorldCover legend, provenance
graph, attribution, endpoint checks, responsive behavior, keyboard focus, and
reduced-motion behavior remain. Copy changes from “WorldCover over Rome” to an
Italy-wide statement and explicitly distinguishes national coverage from
performance or analytical claims.

## Backward compatibility

The original `esa_worldcover_2021_rome` collection, Item, TileSet, and
representative PNG route remain live. Existing field-note, LinkedIn, portfolio,
and evidence links must continue returning their prior resources.

The new collection uses additive URLs under
`esa_worldcover_2021_italy`. The viewer may make it the primary experience,
but historical evidence remains labelled as the one-Item first release.

## Deployment

The Render image includes the national snapshots, GeoPackage layer, and mosaic
manifest. The `cog-mosaic` driver ships under the existing `cog` build feature,
so the deployment does not need another native raster dependency.

The runtime is read-only and non-root. It receives only a local manifest path;
all remote source locations are public HTTPS ESA URLs recorded in that
manifest. No signed URL, credential, upstream catalog token, or expiring query
parameter may enter the image or browser.

## Failure behavior

- Invalid or changed source metadata stops the harvest.
- A missing or non-byte-identical public mirror stops the release.
- An invalid mosaic manifest stops service startup with a named configuration
  error.
- A source that fails during composition fails that PNG request; the service
  does not present a partial tile as complete.
- Viewer endpoint failures remain independent and visible, including cold-start
  guidance.
- The country outline and committed evidence remain readable when the live
  service is temporarily unavailable.

## Verification

Automated tests must prove:

- only 2021 v200 Items intersecting the versioned Italy boundary are accepted;
- source ids, hrefs, boxes, licence, providers, lengths, and digests survive the
  transform;
- the manifest rejects every invalid condition named above;
- source selection returns zero, one, and multiple intersecting COGs for known
  tile coordinates;
- composition is deterministic, transparent outside source coverage, palette-
  preserving, and seamless at a fixture boundary;
- a failed constituent source cannot yield a successful partial tile;
- the per-source pixel budget and four-read concurrency bound remain enforced;
- the national STAC Collection and FeatureCollection expose the expected item
  count and stable `source_item_id` values;
- all legacy Rome endpoint contracts continue to pass;
- HTML contracts cover Italy bounds, every place control, scale status,
  attribution, fallbacks, and resource links;
- no secret or expiring URL is committed.

Live evidence must include:

- the national STAC Collection and at least two source Items;
- the national OGC API FeatureCollection;
- the national raster TileSet metadata;
- one country-scale PNG tile;
- city-scale PNG tiles for northern, central, southern, and island locations;
- desktop country and city screenshots;
- mobile country and city screenshots;
- a public endpoint matrix recording time, status, media type, size, and image
  dimensions.

The country tile must complete inside the deployed service request timeout. A
city tile must select no more than two source COGs. No throughput, latency, or
Italy-capacity claim is published without a separate reproducible benchmark.

## Issue structure

Track the work as one Tellurion engine issue for reusable COG mosaic support and
one demo epic with child issues for:

1. exact national STAC harvest and provenance manifest;
2. national GeoPackage/STAC projection;
3. COG mosaic configuration and deployment;
4. country-to-neighbourhood viewer interaction;
5. backward-compatibility contracts;
6. live endpoint and visual evidence;
7. field-note and portfolio updates.

The current Item-asset work is linked as prior engine capability. If its tracked
scope does not already cover embedded storage and harvest projection, open a
precise follow-up for GeoPackage-backed Item assets instead of treating this
demo manifest as a general solution.

## Non-goals

- request-time STAC federation;
- a global or pan-European mosaic;
- administrative or environmental analysis;
- exact clipping of raster pixels to Italy's political boundary;
- a hosted performance or capacity claim;
- arbitrary TIFF formats beyond the existing COG reader contract;
- execution of upstream style documents;
- OGC API Maps conformance;
- adding GeoPackage Item-asset or STAC-metadata sidecars;
- projecting harvested assets into Tellurion's PostGIS asset-record store.
