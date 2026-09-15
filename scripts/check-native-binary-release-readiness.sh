#!/usr/bin/env bash
# Native archives need feature-resolved Rust dependency notice coverage.
set -euo pipefail

cat >&2 <<'EOF'
BLOCKED: prebuilt native binary archives are not ready for release.

The Rust dependency inventory is not yet a reviewed, feature-resolved set of
license, copyright, and NOTICE texts for the binaries being distributed. This
does not block crates.io source-crate publication; it blocks native archives.
See docs/release/rust-third-party-notices.md.
EOF
exit 1
