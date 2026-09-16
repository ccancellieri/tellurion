# Rust third-party notice release gates

Tellurion source crates and prebuilt native archives have different notice
boundaries.

`./scripts/check-crates-io-release-readiness.sh` checks that the generated UI
third-party notice is present, contains no contact address, and is included in
the `tellurion` source crate. It does not claim that the workspace's Rust
dependency inventory is complete legal notice material.

`./scripts/check-native-binary-release-readiness.sh` fails closed without
target-specific evidence. With `--evidence DIR --target TARGET --workspace DIR`,
it verifies that evidence before permitting an internal native archive build.
A binary release needs the actual Cargo registry license, copyright, and NOTICE
texts for its selected dependencies; a package/version JSON inventory alone is
not sufficient.

The native workflow selects the server's default features plus `ui` and the
ingestion executable's default features. This embeds the operator interface,
not the `public-demo` interface. It now collects target-specific Cargo evidence
and Rust toolchain license files before the native gate, then uploads that
directory even when collection fails. The verified UI text is packaged as
`UI_THIRD_PARTY_NOTICES.txt` and compared with the server's served notice. It is
not a substitute for the Rust and bundled native dependency text.
Any feature-profile change must be reflected in that dependency review before
native archives are unblocked.

The gate reruns locked, offline Cargo resolution for the declared target and
profile, regenerates both notice files, and compares their exact contents. It
also checks the pinned Rust release and commit, copied runtime license files,
and musl source hashes. Windows requires the static CRT build flag. Supported
native targets are Linux x86_64 musl and Windows x86_64 MSVC.

This admits an internal build, not public promotion: extracted archives must
still pass byte-identity and installation smoke checks. Maintainers remain
responsible for reviewing dependency and runtime redistribution obligations.
The project does not provide legal advice.

## Offline Rust source-text evidence

For each native target, capture Cargo 1.97.1 metadata with the archive feature
selection and target filter. From the repository root, for example:

```sh
cargo +1.97.1 metadata --locked --offline --format-version 1 \
  --filter-platform x86_64-unknown-linux-musl --features tellurion/ui \
  > /path/to/cargo-metadata.json
python3 scripts/generate-native-third-party-notices.py \
  --metadata /path/to/cargo-metadata.json --target x86_64-unknown-linux-musl \
  --feature-profile 'server=default,ui;ingest=default' \
  --fallbacks distribution/native-notices/fallbacks.json \
  --workspace "$PWD" \
  --manifest /path/to/rust-notice-evidence.json \
  --text /path/to/RUST_THIRD_PARTY_NOTICES.txt
```

The generator traverses resolved normal and build dependency edges from the
`tellurion` server and `tellurion-ingest` executable roots, excluding dev-only
edges. It copies source-tree license, copying, copyright, and NOTICE files,
including a crate's declared `license_file` even when unusually named;
for SQLite, ring, aws-lc, and zstd packages it searches nested bundled-native
trees too. For `libsqlite3-sys`, it also copies the complete copyright-disclaimer
comment from the first 64 KiB of `sqlite3/sqlite3.c`, without copying the C
implementation. Missing or changed disclaimer structure blocks collection;
the manifest identifies this excerpt with `#copyright-disclaimer` and hashes
its exact bytes. The deterministic JSON records source identity, relative file names,
SHA-256 per source file, a digest of the input Cargo metadata, the reviewer-
declared target and feature profile, and a digest of the combined text. The
declared fields label the supplied metadata; they cannot independently prove
that Cargo resolved it for that target or profile.
Missing or empty license
text, unresolved graph entries, invalid UTF-8 text, or an escaping notice path
block generation. Missing source-text errors are reported together for review.

This is collection evidence, not a legal conclusion. A maintainer must review
the exact target/feature graph and contents, including packages whose crates
do not ship a license file, plus any native/system libraries linked outside
Cargo. A workspace-wide Cargo resolution may unify features from unrelated
workspace members; use target-specific metadata and review the reachable
dependency set before treating the result as archive-specific. The native
release gate does not treat `status=collected` as sufficient evidence: it
independently repeats resolution and notice generation. Archive packaging and
installation remain separate checks.

## Runtime license evidence outside Cargo

The Cargo graph does not describe the Rust standard library or the C runtime
selected by a target. The native workflow therefore captures the pinned Rust
toolchain's `share/doc/rust/COPYRIGHT.html`, `COPYRIGHT-library.html`, and
complete `licenses/` directory under `licenses/rust-stdlib/`. Each archive also
contains `runtime-provenance.json`, which binds the Rust revision and the musl
source text hash. The Linux musl package carries the pinned musl 1.2.5 source
archive and its checked-in `COPYRIGHT` file as
`licenses/musl/musl-1.2.5.tar.gz` and `licenses/musl/COPYRIGHT.txt`; the
workflow downloads the archive, verifies its SHA-256 against provenance before
collection, and verifies both files again after packaging. The source archive
preserves the per-file terms referenced by `COPYRIGHT`. Hash checks establish
source identity and packaging integrity, not a legal conclusion.

The Windows MSVC build requests `+crt-static`, so the archive does not rely on a
separately installed Visual C++ runtime. This setting still requires a clean
machine execution check before public promotion. Windows system libraries and
the Linux kernel/libc interfaces remain operating-system responsibilities, not
Cargo registry packages.

## Pinned upstream texts missing from crate archives

`distribution/native-notices/fallbacks.json` supplies the upstream MIT and
Apache texts for `geo` 0.29.3, `geo-traits` 0.3.0, `geo-types` 0.7.19,
`geozero` 0.15.1, and `rstar` 0.12.2. Their registry archives do not contain
these files. Each entry binds the crate name, version, registry, license
expression, repository, Cargo VCS commit, upstream URL, and SHA-256. Shared
local texts are deduplicated only when their upstream bytes are identical.
The generator reads these checked-in texts offline; it does not download a
moving branch or substitute a generic license based on an SPDX identifier.
Packaged copyright and NOTICE files remain in the output.

The current Linux musl and Windows MSVC dependency graphs produce source-text
evidence with these fallbacks. That is not an archive installation test or a
claim of complete native-library review. macOS still requires attention to
`objc2-core-foundation` and `objc2-system-configuration` 0.3.2. Their pinned
[upstream licensing note](https://github.com/madsmtm/objc2/blob/7b1abfd750a2cacaea71d6a56ecfb83cb7de560b/LICENSE.md)
links to license terms and discusses Apple SDK-derived material; it is not
silently accepted as the missing complete license text. The native release
gate rejects macOS targets while that review is unresolved; macOS remains a
documented source-build option.
