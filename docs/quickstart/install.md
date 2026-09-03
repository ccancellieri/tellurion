# Installing and running Tellurion on-premise

This covers the embedded, self-contained deployment: the `tellurion` binary
plus a single `.gpkg` file, no database service, no container runtime. See
the top-level README's "Scaling up: PostGIS" section for the database-backed
path once a single GeoPackage's one-writer-many-readers ceiling stops
fitting — that path is out of scope here.

## Version 0.5.0-rc.1 release archives

Archives are available only after an approved v0.5.0-rc.1 public release. They describe the
intended release assets; no archive or binary is added to this Git repository. Until
an approved release provides the matching asset, build from source using the
instructions below.

| Platform | Intended archive |
|---|---|
| macOS Apple Silicon | `tellurion-v0.5.0-rc.1-aarch64-apple-darwin.tar.gz` |
| Linux x86_64 musl | `tellurion-v0.5.0-rc.1-x86_64-unknown-linux-musl.tar.gz` |
| Windows x86_64 MSVC | `tellurion-v0.5.0-rc.1-x86_64-pc-windows-msvc.zip` |

After downloading the archive for your platform from the approved release, verify its
published SHA-256 checksum before extracting it. Download `SHA256SUMS` into the same
directory as the candidate files, then run:

```sh
shasum -a 256 -c SHA256SUMS
```

For a public release, also verify the GitHub artifact attestation against this
repository. For example, the macOS archive is verified with:

```sh
gh attestation verify tellurion-v0.5.0-rc.1-aarch64-apple-darwin.tar.gz \
  --repo ccancellieri/tellurion
```

If no attestation exists, the artifact is an internal candidate, not an approved public binary.
The macOS and Linux archives contain the `tellurion` and
`tellurion-ingest` executables; extract them into a directory on your `PATH`. On
Windows, extract the ZIP and add its directory to `PATH` before using `tellurion.exe`
or `tellurion-ingest.exe`.

These assets are intended for Tellurion 0.5.0-rc.1 under `AGPL-3.0-only`. Review the
[licensing guide](../licensing.md) before deployment or redistribution.

## Install the release candidate with Cargo

After the approved crates.io publication, install the server and ingestion CLI from
the locked 0.5 release-candidate line with:

```sh
cargo +1.97.1 install tellurion --version '=0.5.0-rc.1' --locked
cargo +1.97.1 install tellurion-ingest --version '=0.5.0-rc.1' --locked
```

To embed the anonymous remote-source demo and its web interface in the server binary:

```sh
cargo +1.97.1 install tellurion --version '=0.5.0-rc.1' --locked \
  --features public-demo,ui
```

Cargo does not select a pre-release implicitly, so evaluators must request
`0.5.0-rc.1` explicitly. Ordinary installation can move to `0.5.0` after that stable
version is approved and published. Each install above compiles from the published
crate sources; use the signed platform archives when a prebuilt binary is preferred.

## Prerequisites

- Rust 1.97.1. `rust-toolchain.toml` pins this exact compiler with the
  `clippy` and `rustfmt` components. With `rustup` installed, use
  `cargo +1.97.1` for release builds.
- `cmake` and `pkg-config` on the build machine — the CI workflow installs
  these explicitly before building on Linux, because the `geopackage`
  feature (default-on) pulls in `rusqlite`'s `bundled` feature, which
  compiles SQLite from source, and other dependencies in the workspace's
  dependency graph reach for `cmake`-based native builds too.
- Optionally, GDAL's `ogr2ogr`/`ogrinfo`/`gdal_translate` command-line
  tools on your `PATH`, if you intend to load a real (non-synthetic)
  vector dataset — see
  [real-data-osm-geopackage.md](real-data-osm-geopackage.md). The embedded
  GeoPackage driver itself needs no GDAL at runtime; GDAL is only useful at
  data-preparation time, as an external tool you run yourself.

### macOS

Install Rust via [rustup](https://rustup.rs/) if you don't already have it:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

`cmake` and `pkg-config` are commonly already present via Homebrew's
build tooling; if not:

```sh
brew install cmake pkg-config
```

Then, from the repository root:

```sh
cargo +1.97.1 build --release -p tellurion -p tellurion-ingest
```

### Linux

Install Rust via [rustup](https://rustup.rs/), and the native build
dependencies CI itself uses (Debian/Ubuntu shown; adjust for your
distribution's package manager):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
sudo apt-get update && sudo apt-get install -y cmake pkg-config
cargo +1.97.1 build --release -p tellurion -p tellurion-ingest
```

### Windows

Install Rust via [rustup-init.exe](https://rustup.rs/), which on Windows
also prompts to install the Microsoft C++ Build Tools (required — several
dependencies in this workspace, including the bundled SQLite build behind
the `geopackage` feature, need a working C toolchain). Install `cmake`
separately (e.g. via the [official installer](https://cmake.org/download/)
or `winget install Kitware.CMake`) and make sure it's on `PATH`. Then, from
a shell with Rust on `PATH` (PowerShell or `cmd.exe`), from the repository
root:

```powershell
cargo +1.97.1 build --release -p tellurion -p tellurion-ingest
```

This path (native Windows build via the MSVC toolchain) has not been
exercised as part of producing this documentation — the steps above follow
directly from what the crate graph needs (a C toolchain and `cmake`, the
same as the Linux/macOS legs), but treat it as unverified until you've
actually run it.

## Running it

Once built, the fastest path — provisions a fresh `.gpkg`, seeds it with
~500 synthetic demo features, and serves it, with no network connection
string anywhere — is:

```sh
target/release/tellurion-ingest demo
```

then, in another terminal:

```sh
curl http://localhost:8080/public/features/catalogs/default/collections/demo/items?limit=10
```

See the top-level README's own "Quickstart" section for the step-by-step
provision/seed/serve breakdown, and
[real-data-osm-geopackage.md](real-data-osm-geopackage.md) in this
directory for loading real OpenStreetMap data instead of the synthetic
demo grid.
