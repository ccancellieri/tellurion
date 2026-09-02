# Changelog

All notable changes to Tellurion are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`[workspace.package] version` in the root `Cargo.toml` is the single source of
truth for the version; `scripts/release.sh` moves it and promotes the
`Unreleased` section below into a released one.

The fresh-root public repository launches without inherited tags or release
assets. Its first release will be published only after the gates and evidence
in `docs/publication-runbook.md` pass. Historical version entries below record
pre-publication milestones; they do not imply distribution from the new public
repository.

## [Unreleased]

### Changed

- Hosted CI runs for pull requests targeting `main`, pushes to `main`, and
  manual dispatches. A newer run for the same pull request or ref cancels its
  predecessor. Tag-triggered release artifacts remain the responsibility of
  the release workflow. The local mirror (`scripts/ci-local.sh`) remains the
  fastest way to verify the same contract before pushing.


### Added

- A specification deviation register at `docs/spec-deviations.md`:
  where this server knowingly does something other than a clause's letter,
  it is written in one place with its reasoning, auditable and
  distinguishable from a bug. Its first — and only — deviation entry is OGC
  20-058's own self-contradiction on a `bbox`-less `bbox-crs` (Requirement
  18 clause F's unconditional "SHALL be ignored" against §13.5's
  unconditional 400 for an unsupported CRS value): on `/map`, the
  parameter's *effect* is ignored — the response is byte-for-byte the one
  the same request without it produces, and the ignored parameter never
  reaches the render or the cache key — while its *value* is still
  validated, so an unsupported CRS is refused by name whether or not `bbox`
  is present. That was already the shipped behaviour; the register turns it from an
  accident of parse ordering into a recorded decision, with no
  configuration flag, locked by byte-identity, cache-key-fragmentation and
  named-refusal tests plus a demo smoke phase. A separate section records
  interpretations — spec questions resolved *without* the document
  contradicting itself, opening with the withholding of
  `basic-spatial-functions` on GeoPackage — precisely so the two categories
  cannot blur.
- The Iceberg driver reads table files from any store speaking the **S3 protocol**
  — AWS S3, MinIO, Ceph RGW, Cloudflare R2 — as well as the local filesystem.
  The `FileIO` layer is implemented over this workspace's own
  `ObjectStore` port and its hand-rolled SigV4 signer rather than a second S3
  client: no vendor SDK and no opendal chain enters the tree, and one S3
  implementation stays one. Parquet data files are read with ranged GETs, and a
  store that ignores a `Range` request is caught by name rather than silently
  feeding the reader bytes from the wrong offset. A local-filesystem table is
  unchanged: its locator declares no S3 settings and it reads through Iceberg's
  own local storage exactly as before.
- S3 connection settings travel in the storage's existing locator — the string
  held in the environment variable `url_env` names — as `s3_endpoint`,
  `s3_region`, `s3_access_key_env` and `s3_secret_key_env`. The last two
  are variable NAMES: credentials come from infrastructure environment
  variables and are never read from `config.yaml`, which gains no field to hold
  one. A table on S3 whose locator is missing one of the four is refused at load
  by naming the missing key, never completed with a guessed endpoint or region.
- `tellurion-core`'s object-store port gained a path-addressed READ capability
  (`PathAddressedObjectStore`), advertised through an `Option` accessor that
  defaults to `None`. Only the `s3` profile implements it; `fs`
  deliberately reports it absent, because resolving a nested caller-supplied
  path under that profile's root is exactly the traversal its `Uuid`-only key
  space exists to prevent. It is read-only by construction — there is no
  path-addressed write verb — so nothing on the serving path can write an object
  at a path it did not generate.
- A gated live test validates the Iceberg driver against a real, external REST
  catalog: `TELLURION_ICEBERG_LIVE_TEST_LOCATOR` points it at one, and it
  skips (as a pass) when unset — the same convention the live PostGIS tests use,
  so `cargo test` never needs a network service.

- Hierarchical, path-scoped administration policies are enforced at one
  middleware checkpoint covering the platform, tenant, catalog and collection
  administrative paths. A role binding grants downward through the
  resource hierarchy and never upward; an explicit deny beats an allow at every
  depth, in both directions; absence of an allow is a deny. The checkpoint
  decides a request only when a declared statement's own patterns mention that
  request's canonical path, so a deployment that declares no statements answers
  exactly as it did before, and a deployment that declares statements about one
  subtree is unchanged everywhere else. Statements and bindings are declarable
  in the boot envelope (`role_bindings`, `path_policies`) against a durable
  control store; declaring them against the legacy file backend is refused by
  name, because that backend's reload path would silently drop them.
- Administrative paths are canonicalized before policy evaluation by decoding
  them exactly as the router does, then replacing every external id with the
  internal one it resolves to within its parent. Encoded separators, dot
  segments, duplicate slashes and aliases therefore cannot produce a canonical
  path other than the one belonging to the resource actually served: agreement
  with the handler's own view, rather than a stricter decoder of its own, which
  would itself have been the divergence such a path could exploit.
- Every administrative mutation records the effective scope it was authorised
  at and the decision context that authorised it, alongside the principal and
  revision it already recorded.
- Golden-image coverage for the Zarr and COG drivers' own colormap
  classification, taken through `RasterSource::raster_tile` rather than around
  it, and for zoom-driven style expressions across the zoom range.

### Changed

- The PostGIS features lane builds a page's GeoJSON `properties` by naming the
  columns it keeps (`jsonb_build_object`) instead of rendering the whole row
  and deleting the columns it does not. `to_jsonb(t) - <geom> - <pk>`
  serialized the geometry column through its own output function -- hex WKB --
  purely so the adjacent `- <geom>` could throw it away, which on a page of
  large geometries costs more than everything the response keeps. The column
  list is the backend-derived attribute schema the collection descriptor
  already carries, so this adds no configuration surface: nothing here is
  operator-authored, and a collection whose descriptor was never derived (its
  `table`, `geometry` and `pk` all pinned) keeps the previous expression byte
  for byte, as does a column name this crate's identifier whitelist cannot
  quote. Output is unchanged on every path: both projections render each value
  through the same function, and `crs=` is honoured by the same geometry
  expression as before. Regression coverage verifies the generated query shape
  and byte-equivalent response contract.
- An omitted `bbox-crs` on `GET .../collections/{cid}/map` is now read as
  CRS84, not as the tile matrix set's own CRS. OGC 20-058 Requirement
  18 (`/req/spatial-subsetting/bbox-crs`) clause C: "If the bbox-crs is not
  indicated https://www.opengis.net/def/crs/OGC/1.3/CRS84 SHALL be assumed."
  This is a wire-visible change of contract on both halves of the maps lane,
  vector and raster, which share one parameter parse. It is paired with a
  guard, because reading a client's metres as degrees would otherwise return a
  wildly different window under a `200` with nothing saying so: a `bbox`
  supplied WITHOUT a `bbox-crs` whose coordinates fall outside the CRS84
  ranges (±180 longitude, ±90 latitude) is refused by name in problem+json
  (`"BboxCrsRequired"`), naming the parameter to add and the value to give it,
  rather than interpreted. §13.5 of the same standard provides for exactly
  that 400 ("...or the parameter value is out-of-range, the status code of the
  response will be 400"). A client that already declares `bbox-crs` is
  unaffected in either CRS; a client that sends WebMercatorQuad metres without
  declaring one must now add
  `bbox-crs=http://www.opengis.net/def/crs/EPSG/0/3857`. The OUTPUT `crs`
  parameter's own default is deliberately unchanged — Requirement 35 NOTE 2
  gives the two parameters different defaults, and the map's is the native
  (storage) CRS. Detection is reliable except for a metres `bbox` lying
  entirely within a 360 m × 180 m patch of ocean at 0°N 0°E; inside it the two
  readings differ by the same 111319.49× factor as everywhere else, so what
  bounds that residual case is its improbability, not any agreement between
  the readings. No conformance class is added or removed:
  `conf/spatial-subsetting` still requires `subset`/`subset-crs`/`center`/
  `center-crs`, none of which this lane implements.
- A configuration reload whose document hashes identically to the one already
  serving is no longer activated. The config watch is on the file's
  directory and cannot filter by filename, so any sibling file's write — a log
  file beside `config.yaml`, an editor swap file, a ConfigMap restaged with
  unchanged content — arrived as a full activation, and every activation resets
  the readiness probe generation, so `/readyz` answered 503 until the next
  probe. A config file placed beside a churning log file measured five
  activations and 190 of 200 readiness probes non-200 with nothing about the
  configuration having changed. The comparison reuses the SHA-256
  `ConfigVersion` the versioned read already computes; it is unconditional,
  with no configuration knob. `touch config.yaml` therefore no longer forces a
  recycle: change the document to reload it, restart the process to recycle it.
  A declined activation logs at INFO naming the path and version and increments
  the new `tellurion_config_reload_skipped_unchanged_total` counter, so a skip
  is observable rather than a silence. The guard applies on the default
  `registry.backend: file`, where the document is the whole input to a reload;
  under `relational` the catalog, collection and tenant tables live outside the
  document and a reload against an unchanged file is how an operator forces
  them to be re-read, so that backend still activates on every trigger.
- A style's zoom-driven `step`/`interpolate` paint expressions are evaluated at
  the zoom being rendered. They were previously resolved to their first stop
  regardless of zoom, so every zoom level drew the widest-scale end of each
  ramp — a `line-width` ramping 1px to 9px across zoom 4 to 12 drew 1px at
  every zoom. `step` now selects the class the zoom falls in, `interpolate`
  interpolates (`linear` and `exponential`, clamped rather than extrapolated
  outside its stop range), and an expression driven by a feature property
  rather than by zoom is refused by name instead of contributing its first
  stop.

## [0.4.0] - 2026-08-05

### Added

- STAC `/search` honours a CRS84 `filter-crs` and refuses every other value by
  name.
- A STAC Collection links to its own items resource.
- `Content-Crs` names the CRS a response is actually in.
- The `Allow` header is derived from live write capability rather than declared
  statically.
- Ingest can optionally provision a modified-column touch trigger.
- Every write records its own CRS84 extent instead of the extent being guessed.
- A bearer principal can name a `token_env` environment variable instead of
  carrying its token value inline in the configuration document.

### Changed

- A `filter-crs`-less filter is processed in CRS84 on projected storage.
- A `bbox-crs`-less bbox is processed in CRS84 on projected storage.
- A tile envelope is compared against the CRS the geometry is actually stored
  in.
- The OGC API Part 4 create/replace/delete conformance class is folded per
  deployment rather than declared unconditionally.
- `basic-spatial-functions` is withheld where `S_INTERSECTS` cannot compose.
- Write preconditions are evaluated inside the write transaction.

### Fixed

- `scripts/audit-license-policy.sh` prunes `ui/node_modules` from its
  stray-`LICENSE` sweep. The sweep's subject is first-party and vendored
  license copies inside the repository; a third-party npm package's own LICENSE
  is supposed to differ from this project's. CI never saw this — its artifact
  audit runs on a fresh checkout with no `npm ci` — but a full local
  `scripts/ci-local.sh` run always ended red on it, because that script's own
  `ui` feature-matrix leg installs `ui/node_modules` a phase earlier.
- The `iceberg` feature has a CI feature-matrix leg, in
  `.github/workflows/ci.yml` and in its mirror `scripts/ci-local.sh`. It
  was the only driver without one, so the only job that ever compiled it was
  `--all-features` — where the default-on PostGIS driver supplies half the
  dependency graph. The README's claim that every driver feature has its own
  matrix job and a serves-without-the-database proof was therefore not true of
  this one; now it is.

- A configured L2 cache that is unavailable is reported by name instead of
  silently bypassed.
- Live-test fixture DDL is serialised, and a collision is named when it happens.
- Each smoke phase's config is isolated from its log, and the harness names its
  own preconditions.
- The items vertex budget is applied before a page's properties are rendered,
  not after. The PostGIS page plan built `to_jsonb(t)` for every candidate row
  ahead of the budget, which rendered the geometry column to hex WKB purely so
  the adjacent `- <geom>` could discard it, leaving the budget guarding only
  the smaller half of a page's response-side cost. The response remains
  byte-equivalent while the vertex budget now bounds work before properties are
  rendered.
- An inline bearer token in the configuration document is reported by name at
  every boot and reload instead of passing unremarked.

## [0.3.0]

An internal pre-publication milestone whose historical tag is not imported
into the fresh-root public repository. Its proposed licensing direction was
superseded before the first public release from that repository; the current
tree uses AGPL-3.0-only. Its development history predates this changelog.
