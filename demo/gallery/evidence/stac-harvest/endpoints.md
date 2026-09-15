# ESA WorldCover STAC harvest — live evidence

Verified at `2026-08-03T21:56:17Z` against:

- Tellurion service revision: `f928b23838bd26dd09cd7cb1731ebfafbf66de42`
- Viewer revision: `62b9a556470a31adfcec70f26f22f75451b7966d`
- Service: <https://tellurion-stac-harvest-demo.onrender.com>
- Viewer: <https://ccancellieri.github.io/tellurion-demos/demos/stac/>

Render reported the service `live`. Startup logs registered both `cog` and
`geopackage` storage drivers.

## Public resource matrix

| Resource | Result | Content type | Bytes | Semantic assertion |
| --- | ---: | --- | ---: | --- |
| [STAC Collection](https://tellurion-stac-harvest-demo.onrender.com/public/stac/catalogs/default/collections/esa_worldcover_2021_rome) | 200 | `application/json` | 1,874 | `id` is `esa_worldcover_2021_rome` |
| [STAC Item](https://tellurion-stac-harvest-demo.onrender.com/public/stac/catalogs/default/collections/esa_worldcover_2021_rome/items/1) | 200 | `application/geo+json` | 982 | local `id` is `1`; `source_item_id` preserves `ESA_WorldCover_10m_2021_v200_N39E012` |
| [OGC API FeatureCollection](https://tellurion-stac-harvest-demo.onrender.com/public/features/catalogs/default/collections/esa_worldcover_2021_rome/items?limit=1) | 200 | `application/geo+json` | 592 | one Feature; `source_item_id` preserves `ESA_WorldCover_10m_2021_v200_N39E012` |
| [raster TileSet](https://tellurion-stac-harvest-demo.onrender.com/public/tiles/catalogs/default/collections/esa_worldcover_2021_rome/tiles/WebMercatorQuad) | 200 | `application/json` | 1,952 | `tileMatrixSetId` is `WebMercatorQuad`; item template advertises `image/png` |
| [Rome PNG tile](https://tellurion-stac-harvest-demo.onrender.com/public/tiles/catalogs/default/collections/esa_worldcover_2021_rome/tiles/WebMercatorQuad/13/3043/4380.png) | 200 | `image/png` | 27,076 | valid 256 × 256, 8-bit RGBA PNG |

The representative tile is the WebMercator zoom-13 tile at the viewer center
`[12.49, 41.903]`: column `4380`, row `3043`. The route follows the advertised
`{tileMatrix}/{tileRow}/{tileCol}` ordering.

## Source and rendering integrity

- The Planetary Computer Collection and Item snapshots remain immutable source
  evidence with SHA-256 digests in the harvest manifest.
- Planetary Computer's Azure asset requires an expiring SAS token for data
  access. Runtime serving therefore uses ESA's official anonymous S3 mirror.
- The signed source object and the public ESA object were downloaded during
  validation and were byte-identical: 41,236,803 bytes, SHA-256
  `5d951afb19e5fdcb90773bac374b556d425f8945ba4b719114c7f7b03157464a`.
- The WorldCover COG is paletted and carries its authoritative class colors.
  Tellurion uses that embedded palette; the separately generated eleven-class
  legend and colormap remain digest-linked provenance artifacts.
- PNG tile composition is dynamic raster Tiles behavior. This evidence does
  not claim OGC API Maps conformance.

Item-specific persisted asset projection is intentionally not claimed here;
it remains tracked in `ccancellieri/tellurion#221`.

## Visual evidence

- [Desktop](desktop.jpg)
- [Mobile, 390 × 844](mobile.jpg)
