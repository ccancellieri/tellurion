# Inspect a remote vector source

This walkthrough exercises the next-release feature inspector. It is not part of
the v0.5.0 release. The independently deployed [public preview](https://tellurion-public-demo.onrender.com/ui/)
supports this walkthrough only when its map displays **Inspect map center**.
If that control is absent, the deployment has not yet received the feature.
Do not interpret a working preview as evidence of a particular release revision.

## Monaco buildings: from file to attributes

1. Choose **Google and Microsoft Open Buildings — Monaco GeoParquet** from the
   source examples. The inventory records its publisher, ODbL 1.0 attribution,
   object size and revision validator.
2. Inspect the source. The qualified example reports 868 polygon features in
   EPSG:4326, with fields including `bf_source`, `confidence`, `area_in_meters`
   and `country_iso`. These are source fields, not values calculated by Tellurion.
3. Select **Configure map**, keep **Survey ink**, then **Open temporary map**.
4. Click an individual building. **Tile feature details** displays the selected
   rendered feature ID, when present, and its available properties.
5. For keyboard operation, pan the map to place a feature at its center, then
   tab to **Inspect map center** and activate it. An empty location reports that
   no feature was found; it must not silently retain the previous selection.
6. Select **Close details** to clear the inspection, or **Remove temporary layer**
   to release this test source.

The example URL is:

```text
https://data.source.coop/vida/google-microsoft-open-buildings/geoparquet/by_country/country_iso=MCO/MCO.parquet
```

The [source inventory](../../demo/sources/public-examples.yaml) is the authoritative
record for this example. Dataset attribution: Google Open Buildings and Microsoft
GlobalMLBuildingFootprints, distributed by Source Cooperative under
[ODbL 1.0](https://opendatacommons.org/licenses/odbl/1-0/).

## Shapefile variation

Repeat the workflow with **Natural Earth 1:110m coastline — Shapefile archive**.
Choose **Coastline signal** and click a coastline segment. Its fields include
`scalerank`, `featurecla` and `min_zoom`. This generalized dataset demonstrates
line inspection, not detailed shoreline accuracy. Natural Earth data are public
domain; attribution is retained in the viewer.

GeoParquet uses bounded range reads. The Shapefile ZIP uses a bounded temporary
archive spool. The inspector introduces no additional source fetch: it reads
the vector tile already rendered in the browser. Ordinary map pan/zoom may still
request more tiles. Including attributes increases tile payloads and can require
reading additional source columns when the tile is first produced; this is not
a zero-cost transfer or performance claim.

## What the inspector does not establish

- A tile can contain clipped or simplified geometry and only the attributes
  included by its producer. Inspection is not a full-resolution source query.
- The public preview selects at most 16 supported scalar attributes, excluding
  the reserved `id` field. Its conservative selection omits binary, nested,
  date/time and `real` metadata types; `real` does not distinguish all Arrow
  floating-point variants supported by the tile encoder. The source schema can
  therefore list more fields than appear in a tile. The tile identifier is
  provided separately by the encoder.
- Numeric integers outside JavaScript's safe range are marked approximate.
  Use the original source for exact large numeric identifiers. Unsupported or
  non-finite source values can be refused by the existing tile encoder rather
  than silently converted to misleading values.
- Overlapping features can yield one selected rendered feature; this is not an
  exhaustive search of the source or an exact feature count.
- Long values and large property lists are bounded in the UI. A truncation notice
  means the panel does not display the complete content.
- Values are displayed as text, never interpreted as HTML or executable links.
- Selection is cleared on layer replacement, removal and session expiry. Nothing
  is saved to a tenant or catalog.
- Raster layers do not expose this vector inspector. Pixel-value inspection and
  raster legends are separate capabilities.

## Demo acceptance

Before advertising a deployment, record its exact build revision and verify the
Monaco polygon and Natural Earth line workflows in the browser. Exercise click,
keyboard activation, empty space, close, source removal and expiry. Check a narrow
mobile viewport and a long value; the panel must remain readable without pushing
the map sideways. Keep the historical demo routes and release snapshots unchanged.

This is a small-data functional demonstration, not a performance benchmark or an
OGC conformance claim.
