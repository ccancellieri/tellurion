# Tellurion development guidelines

Use these requirements to evaluate a change, not to assert that a proposed feature already exists. Verify the current code and supported capabilities. Historical GeoID lessons are inputs to design, not a mandate to inherit its complexity.

## 1. Keep the serving engine focused

- Build the smallest independently testable vertical slice. Assign one owning crate and explicit dependencies. Keep optional drivers, control storage, protocol adaptation, rendering, and serving concerns separate.
- Use the current core contracts instead of adding special cases to every handler or moving driver internals into core. Backend-specific optimizations belong behind the owning driver boundary.
- Do not copy GeoID code, tests, fixtures, or concrete managers. Express the required behavior in independent terms and create synthetic fixtures. Review current contribution and licensing rules before introducing third-party material; do not assume that an open-source license permits every intended redistribution model.
- Keep the current deployment model and configuration conventions unless the requested feature requires a reviewed change. Do not add a cloud SDK, orchestration system, or mandatory service just because another project used it.

## 2. Driver parity is explicit, not assumed

- Build a capability matrix for the operations touched: reads, filters, projection, sorting, pagination, transactions, writes, tiles, and metadata. Refuse unsupported operations clearly; do not silently emulate them with unbounded scans.
- Compare supported drivers with the same synthetic contract fixtures: stable feature IDs, null/missing values, property types, geometry, CRS, temporal bounds, errors, and filter results.
- Keep identifier and cursor stability across paging, snapshot changes, and updates. Verify the current pagination contract and prefer keyset paging with an explicit stable tie-breaker; define behavior when source data changes.
- Separate server-controlled physical identifiers from user input. Parameterize values and validate identifiers at their actual trust boundary.
- Check remote-source access policies, redirects, bounded range reads, decompression, and malformed inputs. Do not let externally supplied URLs or archive paths bypass network/filesystem boundaries.

## 3. Preserve OGC/STAC wire contracts

- Select the relevant standard family, current version/status, conformance class, requirement IDs, and executable checks before advertising support.
- Test actual HTTP payloads, media types, links, pagination/count semantics, filters, and error responses. Internal row/driver structures are not public wire formats.
- Treat STAC assets, metadata, coordinate/temporal extents, and extension fields deliberately. Do not flatten away semantics merely to share a Records or Features handler.
- Check CRS and axis order, tile matrix dimensions/origins, boundary tiles, empty results, and unsupported styles/formats. Keep protocol-specific behavior at the protocol boundary.

## 4. Bound work without hiding incorrect results

- Make request duration, queue length, concurrency, memory, decoded bytes, output size, and background work bounded. Propagate cancellation into database, remote I/O, decode, and rendering work where supported.
- Use backpressure and explicit overload behavior. Streaming must not first materialize the whole dataset. Avoid blocking the async executor; isolate unavoidable blocking or CPU-heavy work with bounded execution.
- A low-zoom budget must preserve a documented spatial representation: generalization, aggregation, or representative selection. A global LIMIT can leave entire regions invisible. Exact queries need truthful pagination or an explicit limit/error contract, not silent sampling.
- Test dense and sparse regions, large geometries, boundaries, multiple zooms, and pathological requests. Explain the quality/performance tradeoff and measure it.

## 5. Tiles, styles, raster, and cache form one contract

- Cache identity must account for all result-affecting dimensions: source/version, collection, tile matrix and coordinate, CRS, format, filter, style/revision, and access scope where applicable. Prove omissions are safe rather than assuming they are.
- Invalidate affected representations after writes, deletes, schema/style changes, and source refresh. Geometry moves can affect both old and new footprints. Check vector and rendered output consistency.
- If the current architecture uses a vector-to-rendered pipeline, reuse it where appropriate rather than creating divergent geometry/filter paths for speed. Inspect the current rendering tests and any golden-comparison policy before changing determinism or tolerances.
- For raster/COG work, verify range-read behavior, overview choice, nodata, bounds, resampling, and styling. Do not equate a displayed map with efficient source access.
- Background tile warming must be bounded, cancellable, resumable, and revision-aware. It must not consume unbounded queues or republish obsolete tiles after a source change.

## 6. Durable jobs and lifecycle

- Bind retries and cleanup to immutable resource incarnations, not reused logical names. Protect destructive work from time-of-check/time-of-use races and stale ownership.
- Persist observable job outcomes and distinguish accepted work from completed effects. Keep retry/idempotency and partial-failure recovery explicit across control storage, ingestion, derived data, and caches.
- Test restart, cancellation, duplicate submission, late completion, and cleanup after partial failure. Optional components must fail clearly without breaking unrelated capabilities.

## 7. Evidence and review gates

For Rust changes, inspect the current toolchain and CI first. The currently inspected workflow provides these checks; reverify them on future revisions:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked -- --nocapture
```

Run focused affected-crate tests during development and relevant wider checks before claiming completion. Provide isolated test services when required. A successful exit with live tests skipped is not evidence that integration tests ran; inspect skip output and report missing prerequisites. Do not execute CI cleanup or deployment commands locally just because they appear in the workflow.

For benchmarks, record revision, build profile, hardware, dataset source/size, query/viewport, concurrency, warm/cold conditions, sample count, latency percentiles, throughput, memory, bytes, and correctness. Separate database time, transfer, encoding, and rendering where useful. Do not promote a single warm demo into a cold-cache or production-scale claim.

For each transfer or new feature, state acceptance criteria, tests, operational limits, documentation changes, and what remains deliberately out of scope. Update existing user-facing documentation without exposing local agent context or private history.
