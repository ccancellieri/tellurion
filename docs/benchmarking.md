# Reproducible benchmarking

Tellurion includes a small public harness for measuring an evaluator's own build,
dataset, database, hardware, and cache state. It is a methodology, not a published
capacity claim. Results are meaningful only when the full environment and raw output
are retained alongside them.

## What the harness measures

`bench/scenarios.sh` exercises feature pages, individual items, vector tiles, rendered
PNG tiles, optional 3D tiles, and a mixed workload. `bench/load_shed.sh` and
`bench/mesh_limits.sh` verify bounded overload and mesh-cap behaviour at smoke scale.
`bench/summarize.sh` reports latency percentiles, requests per second, error rate, and
RSS when that metric is available.

The harness separates localized cache-path walks from uniformly distributed DB-path
walks. Warmups are discarded, measured scenarios default to three repetitions, and
the summary uses the median. A run should be rejected when unrelated host load,
different datasets, different database settings, or different cache states make the
comparison unfair.

## Run a local measurement

The tools require Bash, `awk`, `jq`, `curl`, and `oha`. Start a Tellurion instance and
then run, from the repository root:

```sh
BASE_URL=http://127.0.0.1:8080 COLLECTION=demo ./bench/scenarios.sh
./bench/summarize.sh
```

For a short smoke exercise rather than a capacity measurement:

```sh
ZMAX=3 DB_ZMAX=11 REPS=1 WARMUP=0 DURATION=3s CONCURRENCY=10 \
  ./bench/scenarios.sh
```

The open-data helpers in `bench/data/` prepare checksummed Geofabrik extracts and
record their retrieved size and attribution. They deliberately keep download,
ingestion, serving, and measurement as separate operations.

## Evidence required for a public number

Publish a performance statement only with the Tellurion commit or release, build
features and profile, CPU and memory, operating system, database and client versions,
dataset URL and checksum, table and index sizes, concurrency, duration, repetitions,
cache state, error rate, and raw result files. Compare like-for-like request shapes and
label smoke checks as smoke checks. A green demo or a single laptop run is not a
general capacity guarantee.

Known constraints: the mixed workload is concurrency-weighted rather than guaranteed
to produce an exact request ratio; a never-seen tile coordinate is not the same as a
cold process; RSS is currently available only where the process collector exposes it;
and missing cache-hit or truncation counters must not be inferred from latency alone.
