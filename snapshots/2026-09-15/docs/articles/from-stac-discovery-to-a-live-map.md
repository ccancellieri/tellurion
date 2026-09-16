# From STAC discovery to a live map

What should survive when a catalog record becomes a live service?

This demo follows one ESA WorldCover 2021 Item covering Rome from a preserved
STAC snapshot to a browser map. The visible result is not a screenshot of a
pre-rendered image: MapLibre requests PNG tiles that Tellurion composes from the
source Cloud Optimized GeoTIFF (COG).

The live Rome resources remain listed below. The gallery also presents a live
Italy explorer for the separately reviewed national release.

![Desktop view of the WorldCover source-to-map demo, including its live map,
provenance chain, class legend and resource checks](../../evidence/stac-harvest/desktop.jpg)

## One source Item, deliberately bounded

The experiment starts with Microsoft Planetary Computer's `esa-worldcover`
Collection and one Item:
`ESA_WorldCover_10m_2021_v200_N39E012`. Its tile covers the Rome viewer extent,
references a WorldCover 2021 v200 COG, and carries the classification metadata
needed to explain all eleven land-cover values.

Choosing one Item is a constraint, not a federation strategy. It makes the
lineage inspectable and gives every transformation a finite input. The public
service never contacts the upstream STAC API while handling a request.

## Preserve first, transform second

The harvester stores the source Collection and Item as immutable JSON
snapshots. A manifest records when they were retrieved, their identifiers,
providers, licence, source asset media type and SHA-256 digests. The committed
snapshot digests are:

- Collection: `381ac0bb927b0a3014134ac472c627ddf95a1b7c7ed01fbb9ede9f4916f92c49`
- Item: `8370f37c0ffadd33fff59473c633a827653c14f1d46c4175ed07b77efbf17aa5`

The snapshot is evidence; the serving configuration is a derived product. That
separation makes it possible to inspect the upstream document without treating
the transformed representation as if it were the original.

The Planetary Computer asset needs an expiring SAS token for data access. The
runtime therefore reads ESA's official anonymous S3 mirror. During validation,
the signed source object and official public object were downloaded and found
to be byte-identical: 41,236,803 bytes with SHA-256
`5d951afb19e5fdcb90773bac374b556d425f8945ba4b719114c7f7b03157464a`.

## A COG is an asset; an API is a link

The source COG remains the data asset. STAC, Features and Tiles resources are
service capabilities derived around it, so the generated Item does not pretend
that a dynamic API endpoint is another copy of the TIFF.

This distinction matters for clients and future contributors. An asset can be
downloaded, checksummed and read by range. A link advertises an operation or a
representation that a service can provide. Keeping those roles separate avoids
inventing asset semantics for endpoints whose availability and parameters are
different from the source object.

## Eleven classes, with their meaning intact

WorldCover describes eleven integer classes, from tree cover at value 10 to
moss and lichen at value 100. The harvest step translates the preserved class
metadata into a validated, digest-linked colormap and legend. These generated
files are provenance evidence: they prove that the values, labels and colors
survived the transform.

The COG itself is paletted and contains the authoritative WorldCover colors.
Tellurion uses that embedded palette while rendering. It does **not** apply the
generated colormap as a runtime override, and the demo never executes remote
style instructions supplied by an untrusted catalog.

## Split the footprint from the pixels

One storage abstraction does not need to do every job. The harvested Item
footprint is loaded into a small GeoPackage and exposed through the metadata and
feature lane. It receives local identifier `1`, while `source_item_id` preserves
`ESA_WorldCover_10m_2021_v200_N39E012`.

The raster lane points to the byte-identical public COG. Tellurion's COG driver
reads raster windows and composes PNG tiles. The browser then renders those
returned tiles. This split keeps feature filtering and raster access in storage
drivers suited to each representation while retaining one provenance chain.

## Inspect the live resources

The deployed result exposes five independently checkable resources:

- [STAC Collection](https://tellurion-stac-harvest-demo.onrender.com/public/stac/catalogs/default/collections/esa_worldcover_2021_rome)
- [STAC Item](https://tellurion-stac-harvest-demo.onrender.com/public/stac/catalogs/default/collections/esa_worldcover_2021_rome/items/1)
- [OGC API FeatureCollection](https://tellurion-stac-harvest-demo.onrender.com/public/features/catalogs/default/collections/esa_worldcover_2021_rome/items?limit=1)
- [WebMercatorQuad raster TileSet](https://tellurion-stac-harvest-demo.onrender.com/public/tiles/catalogs/default/collections/esa_worldcover_2021_rome/tiles/WebMercatorQuad)
- [Representative 256 × 256 PNG tile over Rome](https://tellurion-stac-harvest-demo.onrender.com/public/tiles/catalogs/default/collections/esa_worldcover_2021_rome/tiles/WebMercatorQuad/13/3043/4380.png)

Each resource can be inspected separately. A cold or unavailable endpoint does
not hide the others, and the verification workflow reports the state it
observes rather than turning hosted response time into benchmark evidence.

## What this result does not claim

This is a reproducible, one-Item release snapshot. It is not request-time STAC
federation, a production-scale load test, or proof that arbitrary upstream
styles are safe to execute. The PNG route demonstrates dynamic raster Tiles
behavior; it is not presented as an OGC API Maps conformance claim. An
Item-specific persisted asset projection is also intentionally left for the
general Tellurion contract rather than claimed by this demo.

The repository contains the source snapshots, transformation script, generated
artifacts, deployment configuration, contract tests and live endpoint evidence.
Start with the [reproducible source](https://github.com/ccancellieri/tellurion-demos),
then compare the manifest to the live Rome resources above. The
[Italy release page](https://ccancellieri.github.io/tellurion-demos/demos/stac/)
adds a live Italy explorer while keeping the released provenance visible.

Interoperability is not copying JSON; it is preserving meaning while each system adds only the capabilities it can prove.

## Italy expansion — 2026-08-05

The first release proved one bounded source path: one preserved Rome Item, one
official mirror, and one dynamic PNG route. The reviewed Italy release proves a
different bounded composition in code and committed artifacts: 17 selected
source Items, their footprints, a release manifest, and one mosaic definition.

The Italy routes were verified after deployment and are available as a bounded,
read-only public evaluation path. The Rome resources above remain a separate,
one-Item field note rather than a substitute for national coverage.
The [national design](../design/2026-08-05-italy-worldcover-mosaic-design.md)
and [manifest](../../data/stac/esa-worldcover-italy/manifest.json) document the
reviewed release boundary; the public v0.5.0-rc.1 release-candidate source is available.
[From one COG to Italy](from-one-cog-to-italy.md) explains
the live country-to-neighbourhood route and its verification scope.
