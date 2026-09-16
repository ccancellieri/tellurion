# Tellurion, QGIS, GeoServer, and Esri: different jobs, fair questions

**Scheduled: 2026-08-10**

Product comparisons become less useful when they erase the job being compared. In this project, the raster-mask lane is a Python/Rasterio workflow with an equivalent QGIS Processing comparison. QGIS is a strong fit when desktop inspection, interactive editing, and an established processing toolbox matter.

The serving lane is intentionally separate: Tellurion and GeoServer served the same read-only Rome GeoPackage and returned the same set of 100 geometries and properties. GeoServer is a mature, widely deployed server with a broad standards and extension ecosystem. Tellurion is the smaller local-serving implementation being evaluated for its own interfaces and operational shape. The measured contract and limitations are in [the benchmark report](../benchmark/REPORT.md).

Esri belongs in an honest conversation too. ArcGIS Pro and ArcGIS Enterprise/Image Server offer broad commercial desktop and enterprise workflows, support, and licensing choices. No licensed ArcGIS runtime was available for this host, so this project has no Esri performance row. It would be misleading to infer one from product architecture or from another tool’s numbers.

The decision should follow the workload: who authors data, where analysis runs, which protocols clients need, what support model applies, and which constraints are non-negotiable. A small Rome workload can reveal integration friction; it cannot crown a universal winner.

What selection criteria do you wish more GIS comparisons disclosed before giving a verdict? I am especially interested in constraints that do not appear in a throughput chart.

#QGIS #GeoServer #Esri #Tellurion #GeospatialArchitecture
