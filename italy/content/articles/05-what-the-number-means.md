# What the 80.10% built-up result does—and does not—mean

**Scheduled: 2026-08-04**

The Rome output reports that 80.10% of WorldCover pixels touched by the selected OSM line mask are classified as built-up; tree cover is 18.68%. These values are measured from the included inputs and are reproducible through [the metrics file](../output/metrics.json) and [runner instructions](../README.md).

They are not a statement that 80.10% of Rome’s roads, transport corridors, or land area is built-up. In this demo, “road corridor” means a pixel touched by a selected vector line at the analysis grid. It does not add a physical buffer, infer a right-of-way, or correct for directional duplicates in OSM. The sample is a central-Rome bounding box, not an administrative boundary.

There are ordinary data-quality reasons to be cautious too. WorldCover is a 2021 categorical product; an OSM refresh can reflect a different moment; and pixel resolution changes how a line interacts with classes. A plausible output still needs a definition, lineage, and a way to inspect it.

That is why the kit keeps the inputs and intermediate vector output close to the result: [Overpass JSON](../data/rome-osm-overpass.json), [Rome vectors](../output/rome-roads.geojson), [raster window](../data/rome-worldcover.tif), and [the analysis screenshot](../output/italy-osm-copernicus-analysis.png).

If this were used as an operational signal, what validation would you add first: a buffer sensitivity test, independent network data, an updated land-cover product, or a different AOI? Please challenge the interpretation.

#DataQuality #GIScience #LandCover #OpenData #Rome
