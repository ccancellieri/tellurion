# Connecting QGIS to a running Tellurion server

This walks through connecting QGIS Desktop to the local server described in
[real-data-osm-geopackage.md](real-data-osm-geopackage.md) — real
OpenStreetMap vector tiles and PNG raster tiles served from a single
`.gpkg` file. The exact URL shapes below were read directly out of a real
running server's own responses (see that document's "What was actually
verified" section), not guessed. The QGIS menu steps and field names below
come from QGIS's own published documentation (linked at the end) rather
than a live click-through in this environment — flagged explicitly wherever
that distinction matters.

Every screenshot, saved project, or map you produce from this data must
carry **"© OpenStreetMap contributors"** — see the license note at the top
of `real-data-osm-geopackage.md`.

## What Tellurion actually exposes, and what that means for QGIS

QGIS does not have native client support for the OGC API — Tiles standard
as of the documentation checked while writing this (July 2026): see
<https://github.com/qgis/QGIS/issues/50296>, an open feature request, and
QGIS's own "Working with OGC / ISO protocols" documentation page, which
lists WMS/WMTS, WCS, WFS/OGC API — Features, SensorThings, STAC, and
ArcGIS REST, but no OGC API — Tiles client. There is a separate,
GDAL-backed **raster** path for OGC API Tiles/Maps/Coverages (QGIS ≥ 3.38
with GDAL ≥ 3.9), covered as an alternative for the PNG lane below — but
the vector (MVT) lane has no native OGC API Tiles client in QGIS at all.

Because of that, both connections below use QGIS's **generic XYZ tile
connection** mechanism instead — the same "New Generic Connection"
approach QGIS's own documentation and community guides recommend for any
XYZ-shaped tile service QGIS doesn't have a dedicated client for. Tellurion's
tile URL path order (`{tileMatrix}/{tileRow}/{tileCol}`, i.e. zoom/row/column
— the OGC API Tiles standard's own ordering) maps directly onto QGIS's
`{z}/{y}/{x}` placeholder convention; the placeholders just need to land in
the right position in the URL template, which the templates below already do.

## Before you start

Have the server running against the real OSM data (see
`real-data-osm-geopackage.md`), and confirm the base URL works in a plain
browser or `curl` first:

```sh
curl -s http://localhost:8080/public/features/catalogs/default/collections
```

Everything below assumes `http://localhost:8080` — adjust the host/port if
you started the server differently (the `PORT` environment variable or
`server.port` in the config controls this).

## 1. Vector tiles (MVT) — "Add Vector Tile Layer"

The tile URL template, with QGIS's `{x}`/`{y}`/`{z}` placeholders dropped
into Tellurion's own `{tileMatrix}/{tileRow}/{tileCol}` path positions:

```
http://localhost:8080/public/tiles/catalogs/default/collections/italy_isole_multipolygons/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt
```

(swap the collection id for `italy_isole_lines` for the roads/waterways
layer, or your own collection's id)

Steps (menu path and field names per QGIS's official "Vector Tiles" and
"OGC / ISO protocols" documentation,
<https://docs.qgis.org/3.44/en/docs/user_manual/working_with_vector_tiles/vector_tiles.html>):

1. **Layer → Add Layer → Add Vector Tile Layer…** (or open the Data Source
   Manager and select the "Vector Tile" tab).
2. Click **New**, then **New Generic Connection…**.
3. **Name**: anything recognizable, e.g. `tellurion-italy-isole-multipolygons`.
4. **URL**: the template above.
5. **Min./Max. Zoom Level**: `0` and `14` — this collection's configured
   tile zoom range, read directly from the server's own tileset response
   (`tileMatrixSetLimits` ran from `tileMatrix: "0"` to `tileMatrix: "14"`
   when checked against the running server; `TilesConf`'s own default in
   `crates/tellurion-core/src/config.rs` is `minzoom: 0, maxzoom: 14`, and
   this config doesn't override it).
6. **Style URL** (optional): QGIS's vector tile connection dialog can take
   a MapLibre/Mapbox GL style document URL directly and apply it as
   client-side vector symbology —
   `http://localhost:8080/public/styles/catalogs/default/styles/osm-basic`
   is Tellurion's own registered style for this dataset. This is a
   different mechanism from Tellurion's own server-side styled-PNG lane
   (section 3 below): here, QGIS itself parses the style document and
   draws the vector geometry; the server never rasterizes anything. Whether
   QGIS's MapLibre style parser handles every paint property Tellurion's
   own style document uses was not verified in this environment — if the
   styling doesn't come through as expected, build your own QGIS
   symbology instead (skip this field and see QGIS's own "Symbology"
   section in the docs above).
7. **OK**, then select the new connection in the list and click **Add**
   (or double-click it).

The `id` field on each MVT feature is a plain string of the row's `fid`
(Tellurion's own MVT encoder always tags feature id `0` as `"id"`, per
`crates/tellurion-geopackage/src/driver.rs`'s `encode_mvt_feature`); the
allowlisted attribute columns configured under this collection's
`settings.tile_properties` — `name`, `building`, `landuse`, `natural`,
`leisure` for the multipolygons layer — come through as the vector tile's
own feature properties, visible in QGIS's attribute table and usable in
expressions/labeling.

## 2. Raster PNG tiles — "XYZ Tiles" connection

The unstyled PNG lane (per-collection default fill/stroke, no MapLibre
style involved):

```
http://localhost:8080/public/tiles/catalogs/default/collections/italy_isole_multipolygons/tiles/WebMercatorQuad/{z}/{y}/{x}.png
```

Or the styled PNG lane (server-side rasterized through the registered
`osm-basic` MapLibre style):

```
http://localhost:8080/public/tiles/catalogs/default/collections/italy_isole_multipolygons/styles/osm-basic/map/tiles/WebMercatorQuad/{z}/{y}/{x}
```

(no `.png` suffix needed on the styled endpoint — it's raster-only, PNG is
the only format it ever negotiates to)

Steps (per QGIS's own Browser panel documentation and community XYZ
guides):

1. Open the **Browser panel** (View → Panels → Browser Panel, if it isn't
   already visible).
2. Find **XYZ Tiles** in the panel tree, right-click it, and choose
   **New Connection…**.
3. **Name**: e.g. `tellurion-italy-isole-png` (unstyled) or
   `tellurion-italy-isole-styled` (styled).
4. **URL**: one of the two templates above.
5. Set **Max Zoom Level** to `14` (same collection zoom range as the
   vector lane).
6. **OK**, then double-click the new entry under XYZ Tiles (or right-click
   → Add Layer to Project) to load it.

## 3. Alternative for PNG: QGIS's native "OGC API" raster source (QGIS ≥ 3.38)

QGIS 3.38+ (with GDAL ≥ 3.9) has a GDAL-backed raster source type
specifically for OGC API Tiles/Maps/Coverages — **Layer → Add Layer → Add
Raster Layer…**, then set **Source Type** to **Protocol: HTTP(S), cloud,
etc.** and, per GDAL's own `OGCAPI` driver documentation
(<https://gdal.org/en/stable/drivers/raster/ogcapi.html>), give it either
the landing page URL or a specific collection's URL, prefixed with
`OGCAPI:`. In practice for this server, that would be one of:

```
OGCAPI:http://localhost:8080/public/tiles/catalogs/default
OGCAPI:http://localhost:8080/public/tiles/catalogs/default/collections/italy_isole_multipolygons
```

Two things to know before relying on this path: GDAL's `OGCAPI` driver
defaults to the `WorldCRS84Quad` tile matrix set, but Tellurion only ever
advertises `WebMercatorQuad` (`crates/tellurion-tiles/src/tilematrixset.rs`
— this server has no `WorldCRS84Quad` support at all), so the driver's
`TILEMATRIXSET=WebMercatorQuad` open option (`-oo TILEMATRIXSET=WebMercatorQuad`
on the GDAL command line; QGIS's Raster Layer dialog exposes GDAL open
options too, though the exact field layout for entering one was not
checked in this environment) is very likely required for this to resolve
tiles at all, not just an optional tuning knob. Whether QGIS's raster
dialog surfaces that option cleanly, and whether GDAL's `OGCAPI` driver
correctly parses this server's tileset JSON shape end-to-end, was **not
verified in this environment** — no live QGIS session was available to
click through it. The XYZ connection in section 2 above is the verified,
known-working path; treat this section as a documented alternative to try,
not a confirmed one.

## Sources

- [QGIS: Working with Vector Tiles](https://docs.qgis.org/3.44/en/docs/user_manual/working_with_vector_tiles/vector_tiles.html)
- [QGIS: Working with OGC / ISO protocols](https://docs.qgis.org/3.44/en/docs/user_manual/working_with_ogc/ogc_client_support.html)
- [QGIS issue 50296 — OGC API Tiles support](https://github.com/qgis/QGIS/issues/50296)
- [GDAL OGCAPI driver documentation](https://gdal.org/en/stable/drivers/raster/ogcapi.html)
