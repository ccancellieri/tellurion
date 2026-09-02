# Run the stateless remote-source preview

Status: **Preview** on the development line. This is an evaluation surface,
not a released download or a persistent catalog.

The public preview lets a browser submit one public HTTPS object and inspect it
through a short-lived, server-side capability. Tellurion currently recognizes
three demonstrated direct-read paths:

| Format | Remote access | Preview output |
|---|---|---|
| Cloud-Optimized GeoTIFF | bounded HTTPS byte ranges | PNG tiles |
| GeoParquet | bounded HTTPS byte ranges | Features, MVT, and PNG |
| zipped Shapefile | bounded temporary spool, then local reads | Features, MVT, and PNG |

The verified gallery entries, publisher pages, licence terms, attribution,
object lengths, strong ETags, extents, and tested views live in
[`demo/sources/public-examples.yaml`](../../demo/sources/public-examples.yaml).
Candidate formats shown in the interface are not executable.

## What this mode does not do

- It creates no tenant, catalog, collection configuration, control record, or
  durable ingest job.
- It accepts no credentials, redirects, query strings, fragments, private
  address targets, writes, indexing, reprojection, or user-supplied styles.
- A browser session lasts at most 15 minutes, holds at most three sources, and
  allows two concurrent source operations. A later registration receives only
  the lifetime remaining in that shared session.
- Rendered responses are private and `no-store`. This preview does not expose a
  configurable tile cache.

GeoParquet remains range-native. A Shapefile archive is necessarily copied to
a private temporary directory first; compressed size, expanded size, member
count, expansion ratio, concurrent spools, aggregate spool bytes, and
materialization time are all bounded. The spool is removed on deletion or
expiry.

## Build and run from source

Prerequisites are Rust 1.97.1, Node.js 22, `cmake`, and `pkg-config`. From the
repository root:

```sh
cd ui
npm ci
npm test
npm run build:public-demo
cd ..

CARGO_BUILD_JOBS=1 cargo build --release --locked -p tellurion \
  --no-default-features --features public-demo,ui

PORT=8080 target/release/tellurion \
  --config demo/public-demo.yaml \
  --public-demo-only
```

Open <http://localhost:8080/ui/>. Readiness is available at
<http://localhost:8080/readyz>. Loopback HTTP uses a short-lived, host-bound
cookie without the `Secure` attribute so the local workflow works consistently
across browsers. Non-loopback HTTP is refused; an HTTPS deployment always uses
a `Secure`, `__Host-` cookie. The public-demo-only route table deliberately
does not expose ordinary tenant, configuration, metrics, or ingest endpoints.

Run the focused release gates before sharing an endpoint:

```sh
cargo test -p tellurion-http-source --test public_demo_inventory
CARGO_BUILD_JOBS=1 cargo test -p tellurion --locked \
  --no-default-features --features public-demo,ui \
  --test public_vector_demo
cargo fmt --all -- --check
./scripts/check-ci-workflows.sh
./scripts/validate-deploy-manifests.sh
```

The inventory's network verifier is opt-in because it reaches independent
publishers. Run it only when deliberately rechecking the live gallery identity:

```sh
cargo test -p tellurion-http-source --test public_demo_inventory -- --ignored
```

An identity mismatch is a release stop. Confirm the upstream change, licence,
attribution, extent, and rendering before updating the recorded facts.

## Deploy with the Render Blueprint

The repository-root [`render.yaml`](../../render.yaml) owns one service,
`tellurion-public-demo`. It uses
[`docker/Dockerfile.public-demo`](../../docker/Dockerfile.public-demo), checks
`/readyz`, and waits for repository checks to pass before an automatic deploy.
It declares no database, disk, secret, environment group, or pre-deploy hook.
The final image contains only the Tellurion server, stateless configuration,
licence text, CA certificates, and the embedded public UI.

In Render:

1. Choose **New → Blueprint**, connect this repository, and select the root
   `render.yaml`.
2. Review the compute plan before applying. The Blueprint intentionally does
   not choose or upgrade a paid plan.
3. After the merge commit passes the main-branch or manually dispatched CI workflow,
   sync the Blueprint and wait for `/readyz` to become healthy.
4. Open the service's `/ui/` URL in a clean browser profile. Exercise both
   curated vector examples, verify publisher attribution and the expected map
   extent, delete each source, and confirm the Shapefile spool returns to its
   baseline.
5. Record cold start, peak resident memory, peak temporary disk use, and cleanup
   on the selected plan. If the platform headroom is insufficient, reduce the
   application spool ceilings before public use; do not add persistent storage
   to an anonymous session.

Render uses the port supplied in its `PORT` environment variable. No manual
port setting or Docker command override is required.

## Safe verification order

Use one heavy build at a time on a development laptop. Build the dedicated UI
before any Rust command that enables `ui`, build and start the final container
only once, record the observations, then stop Docker immediately. Do not publish
screenshots, timings, or social claims until the deployed `/ui/` flow and both
publisher objects have been reverified.
