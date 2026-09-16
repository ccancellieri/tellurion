# Rust third-party notice release gates

Tellurion source crates and prebuilt native archives have different notice
boundaries.

`./scripts/check-crates-io-release-readiness.sh` checks that the generated UI
third-party notice is present, contains no contact address, and is included in
the `tellurion` source crate. It does not claim that the workspace's Rust
dependency inventory is complete legal notice material.

`./scripts/check-native-binary-release-readiness.sh` intentionally blocks
prebuilt native archives. A binary release needs a deterministic,
feature-resolved union of Cargo registry license, copyright, and NOTICE text
for the exact binaries it ships. The existing JSON inventory is useful review
evidence, but it is not that union.

The native workflow currently selects the server's default features plus `ui`
and the ingestion executable's default features. This embeds the operator
interface, not the `public-demo` interface. The verified UI text is packaged as
`UI_THIRD_PARTY_NOTICES.txt` and compared with the server's served notice. It is
not a substitute for the still-missing Rust and bundled native dependency text.
Any feature-profile change must be reflected in that dependency review before
native archives are unblocked.

To unblock native binaries, the maintainer must define the archive feature
sets, generate and review the corresponding Rust notice text, package it in
each archive, and replace the native gate with a currentness and byte-identity
check. The project does not provide legal advice.

## Offline Rust source-text evidence

For each native target, capture Cargo 1.97.1 metadata with the archive feature
selection and target filter. From the repository root, for example:

```sh
cargo +1.97.1 metadata --locked --offline --format-version 1 \
  --filter-platform aarch64-apple-darwin --features tellurion/ui \
  > /path/to/cargo-metadata.json
python3 scripts/generate-native-third-party-notices.py \
  --metadata /path/to/cargo-metadata.json --target aarch64-apple-darwin \
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
for SQLite, ring, and aws-lc packages it searches nested bundled-native trees
too. The deterministic JSON records source identity, relative file names,
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
release gate stays blocked until reviewed evidence is packaged and verified
byte-for-byte for every archive.

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
gate remains in place while this and the archive review are unresolved.
