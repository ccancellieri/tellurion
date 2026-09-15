# LinkedIn launch copy — ESA WorldCover STAC harvest

## Original Rome launch copy

I wanted to test a simple question: can a STAC record keep its meaning all the
way from discovery to a live, dynamically rendered map?

I harvested one bounded ESA WorldCover Item covering Rome. The preserved source
STAC metadata describes its Cloud Optimized GeoTIFF and the exact colors for the
eleven land-cover classes.

The demo keeps that source document and its provenance, validates the class
metadata into digest-linked colormap and legend evidence, routes the Item
footprint through GeoPackage and the raster data through the COG driver, then
exposes STAC and OGC API Features resources, raster TileSet metadata, and live
PNG tiles.

The WorldCover COG is paletted, so Tellurion uses its embedded authoritative
colors at runtime. The generated colormap and legend prove that the harvested
class meaning was preserved; they are not applied as a rendering override.

The important boundary: the COG is an asset; the dynamic APIs are links. The
harvest happens during release, not while serving requests, and unsupported
remote style instructions are never executed.

Live demo: https://ccancellieri.github.io/tellurion-demos/demos/stac/

Article: https://github.com/ccancellieri/tellurion-demos/blob/main/docs/articles/from-stac-discovery-to-a-live-map.md

Source and reproducibility: https://github.com/ccancellieri/tellurion-demos

#STAC #OGCAPI #CloudOptimizedGeoTIFF #Rust #Geospatial #OpenStandards

## Italy expansion follow-up

Italy is not a larger Rome tile.

The next Tellurion WorldCover release composes 17 source COGs, verified as ESA
WorldCover 2021 v200, behind one TileSet from a country view down to neighbourhood
windows. At release time, every Planetary Computer source object and its
official anonymous ESA mirror were streamed and compared: 889,726,110 bytes
per origin, with matching SHA-256 values.

The boundary is deliberate. The source selection is a build-time JSON POST
over the pinned GISCO Italy geometry. No request-time STAC federation.
The COGs remain source assets, while the live STAC, Features, and Tiles
resources are service links. Dynamic PNG tiles are not OGC API Maps conformance.

ESA WorldCover remains CC BY 4.0. The Italy boundary retains its GISCO and
EuroGeographics terms. The public release was verified at deployment time; it
is an evaluation path, not a throughput or availability claim.

Viewer: https://ccancellieri.github.io/tellurion-demos/demos/stac/
Article: https://github.com/ccancellieri/tellurion-demos/blob/main/docs/articles/from-one-cog-to-italy.md
Source: https://github.com/ccancellieri/tellurion-demos

#STAC #OGCAPI #CloudOptimizedGeoTIFF #Rust #Geospatial #OpenStandards
