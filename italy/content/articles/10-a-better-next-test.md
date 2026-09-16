# The useful next step is a better reproducible test

**Scheduled: 2026-08-20**

This Rome series is an invitation to test, not a conclusion about Italian geospatial infrastructure. It provides a small on-prem workflow with inspectable OSM vectors, an ESA WorldCover raster window, Python/Rasterio analysis, a local Tellurion serving path, benchmark design documents, and the artifacts needed to question each claim.

The observed sample is useful precisely because it is bounded: 5,603 selected ways, 155,417 touched pixels, and a central-Rome AOI. It is not an Italy-scale dataset, a production load test, or a substitute for domain validation. The serving comparison now includes raw Tellurion and GeoServer records for one equivalent feature-page workload; Esri receives no invented performance result because no licensed runtime was exercised.

If you want to extend it, start by reproducing the smallest case using [README.md](../README.md). Then choose one variable at a time: another AOI, a buffered definition, a different WorldCover vintage, a controlled local raster mirror, a client-compatibility test, or the serving contract in [the benchmark design](../benchmark/BENCHMARK-DESIGN.md). Keep the command, environment, inputs, and outputs alongside the conclusion.

Tellurion, QGIS, GeoServer, and Esri can all be part of serious geospatial stacks. The more useful question is not who “wins” a generic contest, but which evidence supports a particular deployment, authoring, analysis, and serving decision.

What should this project measure or clarify next? Please share the scenario, constraints, and evidence you would want included. A reproducible disagreement would be an excellent next contribution.

#OpenSourceGIS #Reproducibility #Italy #Geospatial #CommunityTesting
