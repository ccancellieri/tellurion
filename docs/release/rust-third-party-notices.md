# Rust third-party notice release gate

Tellurion's UI archive carries a generated, hash-recorded third-party notice
file. The Rust dependency inventory is different: at the current locked
workspace it contains hundreds of registry packages across the publishable
crate family and native binary feature sets. The existing JSON inventory is
useful evidence, but it is not a substitute for the license, copyright, and
NOTICE text for each shipped Rust dependency.

Until that evidence is generated and reviewed, crates.io publication is
blocked by `./scripts/check-crates-io-release-readiness.sh`. This is an
intentional release gate, not a claim that the existing JSON inventory is a
complete legal notice.

To remove the gate, the maintainer must:

1. Define the exact binary and crate feature sets being published.
2. Generate a deterministic, lock-hash-recorded union of the corresponding
   Cargo registry license, copyright, and NOTICE files.
3. Fail closed for absent or non-text files, allowing only version-pinned,
   source-linked reviewed fallbacks.
4. Put the resulting text in every affected crate archive and native release
   archive, and verify byte identity in CI.
5. Replace the blocking script with checks that prove the generated Rust
   notice file is current, packaged, and byte-identical.

The publisher records the review decision and any exception with the release
candidate before running `cargo publish`. This project does not provide legal
advice.
