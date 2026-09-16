# Two open layers, one inspectable Rome question

**Scheduled: 2026-07-26**

The Rome demo combines two sources that answer different questions. OpenStreetMap supplies the selected road and rail geometries. ESA WorldCover 2021 supplies categorical land cover on a 10 m grid. Joining them asks a narrow descriptive question: which WorldCover classes occur in pixels touched by those lines?

The bundled snapshot contains 5,603 ways and approximately 763.69 km of mapped linework. The mask touches 155,417 WorldCover pixels. In this sample, 80.10% of those pixels are classified built-up and 18.68% tree cover. Those are measured outputs from the packaged inputs, available in [the metrics](../output/metrics.json), not claims about every road in Rome or Italy.

The boundaries matter. OSM and WorldCover have different vintages and collection methods; overlapping or opposite carriageways can affect approximate line length; and a line-touched pixel is not a legal road corridor. The exact CRS84 bounding box and class breakdown are also recorded in the metrics file.

The default run is local and deterministic after download. Start with [the README](../README.md), then inspect [the Overpass snapshot](../data/rome-osm-overpass.json), [generated vectors](../output/rome-roads.geojson), and [analysis code](../analyze.py). A refresh mode intentionally changes the upstream-data condition.

What would make this question useful in your context: another Italian city, a buffered corridor, or a different feature class? Feedback on the definition is more valuable than agreement with the percentage.

#OpenStreetMap #ESAWorldCover #OpenData #GIS #Rome
