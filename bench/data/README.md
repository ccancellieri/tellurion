# Open-data demo profiles

The benchmark uses three deliberately different data tiers:

| Profile | Source | Approx. source size | Minimum free space | Status |
|---|---|---:|---:|---|
| `small` | existing seeded GeoPackage | tiny | none | default, deterministic, no network |
| `italy` | Geofabrik Italy OSM PBF | 2.1 GB | 8 GiB PBF / 16 GiB PostGIS | first large demo to prepare |
| `europe` | Geofabrik Europe OSM PBF | 32.3 GB | 64 GiB PBF / 64 GiB PostGIS | external-disk-only, opt-in |

The source sizes are estimates; the downloaded byte count is recorded in each
profile's `manifest.env`. Free space is checked on the destination filesystem,
and the ingest helper separately checks the PostGIS volume before creating
tables and indexes. The script preserves a partial download so a later
invocation can resume it.

## Italy (current machine)

```sh
./bench/data/prepare.sh --profile italy --check-only
./bench/data/prepare.sh --profile italy --download
```

The default root is the ignored `demo-data/` directory in the Tellurion
repository. Override it when the repository is on a different volume:

```sh
TELLURION_DEMO_DATA_ROOT=/path/to/tellurion-data \
  ./bench/data/prepare.sh --profile italy --download
```

## Europe (external disk)

Europe is intentionally not downloaded by the Italy workflow. After mounting a
volume with at least 64 GiB free, run the preflight first and then download:

```sh
TELLURION_DEMO_DATA_ROOT=/Volumes/TellurionData/tellurion \
  ./bench/data/prepare.sh --profile europe --check-only

TELLURION_DEMO_DATA_ROOT=/Volumes/TellurionData/tellurion \
  ./bench/data/prepare.sh --profile europe --download
```

The files are OpenStreetMap data distributed by Geofabrik under the ODbL 1.0;
retain the attribution `© OpenStreetMap contributors` in screenshots, videos,
and any published benchmark report. The authoritative source and terms are
[Geofabrik's Europe extract page](https://download.geofabrik.de/europe.html) and
[OpenStreetMap's copyright page](https://www.openstreetmap.org/copyright).

After validation, set `DATABASE_URL` and use `bench/data/ingest-osm.sh` to load
the `lines` and `multipolygons` layers into PostGIS. The helper is intentionally
separate from download so a benchmark can be repeated against a frozen,
checksummed extract. For Italy, the database-volume check defaults to the data
root; set `TELLURION_DB_DATA_ROOT` when PostGIS uses another volume. Europe
requires that variable explicitly, because its PBF and database volumes are
expected to be different. The resulting registry shapes are in
[`config/example-italy-osm.yaml`](../../config/example-italy-osm.yaml) and
[`config/example-europe-osm.yaml`](../../config/example-europe-osm.yaml).

These free-space thresholds are conservative preflight guards, not guarantees
of the final expanded OSM table/index size. Record actual PostGIS and index
bytes during the benchmark.
