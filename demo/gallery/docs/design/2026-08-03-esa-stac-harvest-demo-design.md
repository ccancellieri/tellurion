# ESA STAC Harvest and Dynamic Rendering Demo

## Status

Approved design for a bounded public demonstration. Implementation and public
claims remain contingent on the verification gates in this document.

## Goal

Demonstrate a reproducible path from a public ESA dataset described through
STAC to a Tellurion-served catalog, item footprint, source COG asset, and
dynamically rendered map. Preserve upstream metadata and provenance while
adding Tellurion links only for capabilities that the deployed service has
verified it can serve.

The result must be understandable in three views:

1. a visual map for a non-specialist;
2. inspectable STAC and OGC API resources for an engineer;
3. a reproducible harvest manifest and contract tests for an operator.

## Demonstration dataset

The first slice uses the ESA WorldCover collection exposed by the Microsoft
Planetary Computer STAC API. The data producer remains ESA and the catalog host
is identified separately as Microsoft.

- Source collection: `esa-worldcover`
- Source item: `ESA_WorldCover_10m_2021_v200_N39E012`
- Search bounds: `12.45,41.87,12.55,41.95` (a bounded Rome viewport)
- Time: calendar year 2021
- Primary asset: `map`
- Asset format: Cloud Optimized GeoTIFF
- License: `CC-BY-4.0`
- Rendering source: the `classification:classes` value, description, and
  `color-hint` entries attached to the `map` asset

This item is selected because one upstream object contains the complete story:
a footprint, a public COG, explicit producer/host attribution, and eleven
categorical class colors. It avoids inventing a style and avoids dependence on
the Pilot STAC Rendering extension for the first public claim.

## Architectural boundary

The harvest is a build/release operation, never part of a public request.

```text
Planetary Computer STAC API
        |
        | bounded search and snapshot
        v
source Collection + source Item + provenance manifest
        |
        | deterministic validation and normalization
        v
GeoPackage footprint + COG locator + Tellurion colormap
        |
        | configured per-lane routing
        v
Tellurion STAC / Features / Tiles resources
        |
        | browser follows advertised links
        v
interactive map + metadata and provenance panels
```

The upstream documents remain immutable evidence. Derived configuration and
fixtures are generated separately and record the input digest and transform
version. Refreshing the harvest must produce a reviewable diff.

## Resource model

### Source assets

The upstream `map` COG remains a STAC asset. Its original `href`, media type,
roles, title, description, raster metadata, projection metadata, and
classification metadata are preserved in the snapshot.

The upstream `input_quality`, `tilejson`, and `rendered_preview` assets are
preserved as source metadata but are not automatically presented as Tellurion
capabilities. Their presence does not prove that Tellurion can reproduce or
serve them.

### Derived service links

Tellurion resources are links, not harvested assets:

- the Tellurion STAC Collection and Item;
- the OGC API Features representation of the harvested footprint;
- the raster TileSet metadata resource;
- the templated PNG tile resource advertised by that TileSet;
- a bounded map-image request when the raster Maps lane supports it;
- the provenance manifest and original source documents.

The first deploy may expose Tiles without claiming a raster OGC API Maps
resource. The viewer can compose PNG tiles into a map while clearly labelling
the protocol actually used. A Maps link is added only after a live contract
test proves the endpoint.

### Item and collection projection

The demo uses one logical collection with split serving lanes:

- a generated GeoPackage contains the harvested Item footprint, deterministic
  local integer id `1`, and the upstream id in `source_item_id` alongside
  selected searchable properties;
- the COG driver supplies raster windows for the Tiles lane;
- STAC projects the GeoPackage feature back into an Item and attaches the
  source asset metadata;
- collection metadata comes from the harvested Collection after normalization.

The single-item scope makes collection-declared source assets equivalent to
the one Item's source assets for this slice. General multi-item, item-specific
asset persistence is explicitly deferred and tracked as an engine issue.

## Harvest contract

The harvester is a small Python standard-library program with no credentials
and no provider SDK. It accepts an output directory and optional source base
URL, then:

1. retrieves the named Collection;
2. searches the fixed bounds, time, and collection with `limit=1`;
3. requires the expected Item id;
4. validates STAC core fields needed by the demo;
5. requires one `map` asset whose media type declares a COG;
6. requires exactly the expected eleven WorldCover class values, with unique
   values and six-digit hexadecimal color hints;
7. records source URLs, retrieval time, SHA-256 digests, transform version,
   collection/item ids, asset href, media type, license, and providers;
8. writes source snapshots, a GeoJSON footprint with deterministic local id
   `1` and the upstream id in `source_item_id`, a Tellurion colormap fragment,
   and a browser legend JSON.

Failures are closed and named. Missing license, ambiguous assets, malformed
colors, an unexpected Item, or a changed classification table stops the build
instead of silently substituting defaults.

## Styling and safety

WorldCover class colors are data semantics, not an arbitrary executable style.
The transform converts each integer class value and hexadecimal `color-hint`
to a Tellurion explicit-stop colormap. Value `0` remains transparent/no-data.

The harvester never executes expressions, JavaScript, remote MapLibre styles,
SLD documents, or URLs found in metadata. Future support for STAC `renders`
must be a separate allowlisted translator with a versioned input schema and
must preserve unsupported fields without executing them.

## Deployment

A fifth read-only Render service is added to the demo Blueprint. Its image:

- pins the same verified Tellurion source distribution policy as the other
  services;
- runs the bounded harvest during the data-builder stage;
- downloads or range-validates the selected public COG;
- creates the one-feature GeoPackage fixture;
- configures GeoPackage for the Features lane and COG for the Tiles lane;
- installs the generated explicit-stop colormap;
- runs as UID/GID `10001` with no write route;
- exposes no credentials in the image or browser.

If full remote-COG serving is reliable within Render's request and cold-start
budgets, the runtime points directly at the public source COG. Otherwise the
build crops a bounded Rome COG and records that it is a derived local serving
copy. The public page must state which mode was verified; it must not blur the
two.

## Viewer

The existing STAC projection page becomes an evidence-led harvest viewer. It
shows:

- the live Tellurion-rendered WorldCover map;
- the eleven-class legend generated from harvested metadata;
- source producer, host, license, collection id, Item id, and harvest digest;
- side-by-side links to the upstream snapshot and Tellurion projection;
- a resource graph distinguishing source asset from derived API links;
- live status for STAC Collection, Item, TileSet, and one representative PNG
  tile.

The page does not embed upstream provider APIs at runtime. It reads committed
metadata and the deployed Tellurion service, so upstream catalog downtime does
not break the demonstration.

## Verification and evidence

Repository contract tests must verify:

- the source URLs, ids, license, and attribution are present;
- the harvester rejects malformed or incomplete fixtures without network
  access;
- the generated colormap contains all eleven exact values and colors;
- the GeoPackage fixture contains exactly one feature whose `source_item_id`
  equals the upstream Item id;
- the deployment is read-only and non-root;
- no secret, signed URL, or expiring token is committed;
- the viewer has accessible status, fallback, attribution, and legend content;
- daily smoke checks cover the STAC Collection, Item, TileSet, and PNG tile;
- the STAC snapshot validates with a pinned validator;
- the COG passes a structural COG check before publication.

Visual QA must capture desktop and mobile screenshots after the live service is
deployed. Performance numbers are excluded unless a separate, repeatable
measurement run is recorded.

## Issue structure

Work is tracked as one public epic with independently verifiable child issues:

1. demo epic and evidence boundary;
2. deterministic ESA WorldCover STAC harvester and snapshots;
3. source-to-Tellurion colormap translator;
4. split GeoPackage/COG fixture and deployment;
5. typed STAC and OGC link enrichment cleanup in Tellurion;
6. bounded raster Maps support for COG/Zarr capabilities;
7. item-specific harvested asset persistence for multi-item collections;
8. harvest viewer, legend, and provenance graph;
9. validation, smoke tests, and deployment evidence;
10. article and portfolio launch updates.

The two engine issues are not blockers for the bounded one-item demo unless a
verified endpoint would otherwise make a false or non-interoperable claim.

## Article proposal

Working title: **From STAC discovery to a live map: preserving meaning across
the geospatial delivery chain**.

The article leads with the visible result, then follows one WorldCover Item
through discovery, provenance, COG routing, class-color translation, STAC
projection, and dynamic PNG rendering. It explicitly distinguishes what was
harvested, what Tellurion derived, and what was measured. Screenshots and exact
endpoint links are inserted only after the deployment passes the evidence
gates.

Suggested closing message: interoperability is not copying JSON; it is
preserving meaning while each system adds only the capabilities it can prove.

## Non-goals

- a generic federated STAC proxy;
- harvesting an entire global collection;
- runtime dependence on Planetary Computer;
- executing arbitrary upstream styles;
- supporting every STAC extension;
- claiming OGC API Maps support for raster drivers before it exists;
- benchmarking hosted cold-start performance;
- solving multi-item asset persistence inside the demo repository.
