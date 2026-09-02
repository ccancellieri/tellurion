# Video runbook: Tellurion Italy-to-Europe open-data demo

This is a reproducible recording script rather than a pre-recorded benchmark
claim. Record the Italy chapter now; record the Europe chapter only after an
external volume passes the 64 GiB preflight. Keep `© OpenStreetMap contributors`
visible in the title card and final frame.

## Shot list (6–8 minutes)

| Time | Screen | Narration/caption |
|---:|---|---|
| 0:00–0:25 | Title card and repository | “Tellurion open-data stress demo: seeded, Italy, then Europe on an external disk.” |
| 0:25–0:55 | `bench/data/prepare.sh --profile italy --check-only` | Show the destination filesystem, free GiB, threshold, source URL, and OSM attribution. |
| 0:55–1:35 | Italy download and `manifest.env` | Show the resumable download, checksum, bytes, and free space after download. |
| 1:35–2:15 | `ogrinfo` layer list and ingest helper | Highlight `lines` and `multipolygons`; explain why the small seeded demo remains the no-network default. |
| 2:15–3:00 | Tellurion map/UI and a QGIS layer connection | Compare visual QA with service responses; QGIS is the client reference, not the server benchmark. |
| 3:00–4:20 | Short benchmark summary | Show cold MVT, warm MVT, uniform DB-path, PNG, features, and load-shed rows. Keep “pending” rows if a lane was not run. |
| 4:20–5:00 | Comparison slide | GeoServer, Tellurion, QGIS, and GeoID/DynaStore pros/cons, with the same-host/frozen-extract rule. |
| 5:00–5:45 | External-disk preflight (record later) | Show Europe refusing on the internal disk, then passing on `/Volumes/TellurionData/tellurion`. |
| 5:45–6:30 | Europe profile (record later) | Show the explicit download command, manifest, and the same benchmark matrix—not a fabricated speed claim. |
| 6:30–7:00 | Closing card | Sources, ODbL attribution, commit/config IDs, host specs, and links to the blog/runbook. |

## Commands to show

```sh
./bench/data/prepare.sh --profile italy --check-only
./bench/data/prepare.sh --profile italy --download
DATABASE_URL='postgres://...' ./bench/data/ingest-osm.sh

ZMAX=5 DB_ZMIN=8 DB_ZMAX=10 REPS=1 WARMUP=0 DURATION=5s CONCURRENCY=20 \
  BASE_URL=http://localhost:8080 COLLECTION=italy_multipolygons \
  ./bench/scenarios.sh
./bench/summarize.sh
```

Later, with the external disk mounted:

```sh
export TELLURION_DEMO_DATA_ROOT=/Volumes/TellurionData/tellurion
export TELLURION_DB_DATA_ROOT=/Volumes/PostGISData
./bench/data/prepare.sh --profile europe --check-only
./bench/data/prepare.sh --profile europe --download
DATABASE_URL='postgres://...' ./bench/data/ingest-osm.sh --profile europe
```

`TELLURION_DB_DATA_ROOT` must be the host path or mounted volume backing
PostGIS, not a path that exists only inside a database container. The 64 GiB
database reserve is a conservative preflight guard, not a guarantee of the
final expanded-table/index capacity; record actual database and index sizes.

Use `config/example-europe-osm.yaml` for the Europe registry shape and
`COLLECTION=europe_multipolygons` for the corresponding benchmark lane.

## Recording recipe

Use any desktop recorder for the UI and terminal shots. For a silent, captioned
artifact assembled from screenshots, save PNGs as `scene-01.png`, … and run:

```sh
ffmpeg -framerate 1/4 -i scene-%02d.png -c:v libx264 -pix_fmt yuv420p \
  tellurion-open-data-demo.mp4
```

Add narration in the editor after the benchmark JSON has been generated. Do not
record passwords, connection strings, or unredacted local paths. The final frame
should include the source links, Tellurion commit, dataset manifest checksum,
and the statement “results are host/config specific.”
