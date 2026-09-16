# Standards interfaces, locally controlled deployment

**Scheduled: 2026-08-16**

Interoperability should describe what a client can ask for, not force where an operator runs the service. The Rome demo loads generated OSM vectors into a local GeoPackage and documents Tellurion requests for OGC API Features-style items and a Mapbox Vector Tile response.

The measured demonstration facts are deliberately modest: the packaged Tellurion v0.2.0 binary returned GeoJSON from the local feature endpoint, and the documented Rome tile request returned an 85,996-byte MVT. These facts are recorded in [RESULTS.md](../RESULTS.md). They do not prove a full conformance class, broad client compatibility, or a performance characteristic.

Readers can repeat the local serving path from [the reproducibility guide](../../#reproduce): install a public Tellurion 0.3.0 binary, load the Rome data, start the server, and issue the listed requests. Binaries are available for macOS Apple Silicon, Linux x86_64 and Windows x86_64. Keep the product boundary clear: this is vector serving; the land-cover analysis remains Python/Rasterio, and the published performance table remains the historical 0.2.0 run until 0.3.0 is measured separately.

GeoServer is also built around interoperable geospatial services and has a long-established ecosystem. QGIS can consume and author data through many GIS formats and services. Esri offers extensive standards-facing and proprietary integration options depending on product and deployment. Which is appropriate is an operational decision, not a badge that a single endpoint can award.

If you try the endpoints, please report the client, request, media type, response, and any incompatibility. Negative interoperability results, when reproducible, are often the most actionable.

#OGCAPI #Interoperability #VectorTiles #OnPrem #GIS
