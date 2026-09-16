# On-prem geospatial is still a choice worth preserving

**Scheduled: 2026-07-23**

Some geospatial work has a perfectly good cloud home. Some does not: data-residency obligations, disconnected operations, cost predictability, or simply the need to inspect every layer of the stack can make local deployment the practical choice.

This series starts with a deliberately small, reproducible Rome workload. It uses a snapshot of 5,603 OpenStreetMap road and rail ways and a bundled window from ESA WorldCover 2021. The vector/raster calculation runs locally. Tellurion is used separately as the local vector-serving demonstration; it is not the raster-analysis engine.

Measured facts and limits are kept together: the sample covers only a central-Rome bounding box, not a city boundary or Italy-scale workload. The published land-cover percentages describe pixels touched by selected linework; they do not establish a policy, causal, or network-wide conclusion. The complete evidence is in [the result summary](../RESULTS.md), [metrics JSON](../output/metrics.json), and [reproduction instructions](../README.md).

I am not arguing that one deployment model or product is universal. QGIS is a capable desktop and Processing environment; GeoServer is a mature service platform; Esri provides broad commercial desktop and enterprise capabilities; Tellurion is the open, local serving component being exercised here. The useful question is which fit is best for a defined workload and operating context.

If you run geospatial infrastructure on-prem, where does it create real value—and where does it add avoidable friction? I would welcome concrete counterexamples.

#geospatial #onprem #opensource #Rome #reproducibility
