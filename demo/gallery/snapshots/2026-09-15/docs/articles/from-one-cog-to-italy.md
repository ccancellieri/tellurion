# From one COG to Italy

Italy first, 10 m underneath: that is the useful distinction in this release.
The opening view is national, but the categorical pixels remain read from the
source Cloud Optimized GeoTIFFs when the view moves to a city or neighbourhood.

## Why Rome cannot stand in for Italy

The earlier Rome field note deliberately served one ESA WorldCover COG. Zooming
that source out would enlarge its footprint on a national map without creating
coverage beyond it. That would be visually misleading. Italy needs a finite
source set whose coverage can be inspected independently of the browser view.

## Build-time selection and provenance

At build time, a JSON POST containing the complete pinned GISCO Italy geometry
selected 17 ESA WorldCover 2021 v200 source Items. Selection uses true geometry
intersection, not a bounding-box shortcut. The accepted identifiers are sorted,
and the release rejects more than 32 sources.

The harvester commits query-free, sanitized Collection and Item snapshots with
their footprint, metadata, and stable source identity. The public Collection,
Item, and mosaic digests verify those serialized snapshots; separately named
upstream Collection and Item digests retain the raw source-document hashes
seen before sanitization. It then streams every
temporary signed Planetary Computer object and its anonymous official ESA S3
mirror. Both origins matched 889,726,110 bytes and their SHA-256 values, for
about 1.78 GB transferred during verification. The release tree has 23 files:
the Collection, boundary, footprints, legend, manifest, mosaic, and 17 Item
snapshots.

The source Collection digest is
`381ac0bb927b0a3014134ac472c627ddf95a1b7c7ed01fbb9ede9f4916f92c49`.
The runtime mosaic digest is
`7b1499635f5463b8e2a510b8d6f4f6a3a6ae0b494dc0e15eed48bcd02fcdbedf`.
The complete GISCO source digest is
`5c0019d82d9c54dae8e6b6c1b5a97198c6c67e66fa110b1b55a2ed4b527c5c9e`.

This is build-time provenance, not request-time STAC federation.

## One bounded TileSet

The reviewed mosaic reads one bounded manifest of 17 official ESA COG mirrors.
For a requested tile, it selects only the intersecting sources, reads their
windows, and composites non-transparent pixels in stable source-id order. The
browser receives one dynamic PNG route through one Tellurion TileSet; it does
not consult an upstream catalog while serving the request.

At country scale, the service can use source overviews. At city and
neighbourhood scale, it reads the relevant source windows instead of presenting
an enlarged national overview. This is a composition boundary, not a capacity
or performance statement.

## Assets, links, and deployment shape

The ESA COGs remain source assets. The service resources around them are links:
STAC and Features describe the bounded release, and Tiles produces the PNG
representation. The source footprints are carried through the GeoPackage lane;
the raster lane reads the mosaic manifest. This demo does not invent a
Collection-level asset projection for the individual source COGs.

## Reproducibility and live resources

### Live evaluation — verified resources

The national routes below returned their expected representations during
post-deployment verification. They are a public, read-only evaluation path, not
an uptime, capacity, or performance commitment.

- [STAC Collection](https://tellurion-stac-harvest-demo.onrender.com/public/stac/catalogs/default/collections/esa_worldcover_2021_italy)
- [FeatureCollection](https://tellurion-stac-harvest-demo.onrender.com/public/features/catalogs/default/collections/esa_worldcover_2021_italy/items?limit=1)
- [raster TileSet](https://tellurion-stac-harvest-demo.onrender.com/public/tiles/catalogs/default/collections/esa_worldcover_2021_italy/tiles/WebMercatorQuad)
- [country PNG tile](https://tellurion-stac-harvest-demo.onrender.com/public/tiles/catalogs/default/collections/esa_worldcover_2021_italy/tiles/WebMercatorQuad/5/11/17.png)

The committed [Italy manifest](../../data/stac/esa-worldcover-italy/manifest.json),
[mosaic definition](../../data/stac/esa-worldcover-italy/mosaic.json), and
[national design](../design/2026-08-05-italy-worldcover-mosaic-design.md) remain
the reviewable provenance. Public v0.5.0-rc.1 release-candidate source is available;
the public evidence also includes the committed release data, demo contracts,
and live evaluation links.

## Claims deliberately not made

- This is not request-time STAC federation.
- Dynamic PNG tiles are not an OGC API Maps conformance claim.
- The release does not publish a benchmark, throughput, latency, capacity, or
  Italy-scale performance claim.
- It does not claim exact raster clipping to Italy's political boundary or an
  administrative or environmental analysis.

## Licence boundaries

ESA WorldCover is available under [ESA WorldCover CC BY 4.0](https://esa-worldcover.org/en/data-access) and is attributed as © ESA WorldCover project 2021 / Contains modified Copernicus Sentinel data (2021) processed by ESA WorldCover consortium.
The pinned GISCO boundary has separate terms: Non-commercial use; © EuroGeographics for the administrative boundaries.

The bounded composition is specific: 17 reviewed 2021 v200 source COGs,
identified before deployment, compose one live Italy TileSet without a
request-time catalog dependency.
