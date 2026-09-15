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

To unblock native binaries, the maintainer must define the archive feature
sets, generate and review the corresponding Rust notice text, package it in
each archive, and replace the native gate with a currentness and byte-identity
check. The project does not provide legal advice.
