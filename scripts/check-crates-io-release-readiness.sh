#!/usr/bin/env bash
# Deliberately refuses crates.io publication until Rust dependency license
# notices are generated and reviewed as complete release evidence.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

./scripts/audit-crates-io-policy.sh
./scripts/audit-artifacts.sh

if [ ! -s crates/tellurion-server/ui/THIRD_PARTY_NOTICES.txt ]; then
    echo 'BLOCKED: canonical UI third-party notices are missing' >&2
    exit 1
fi

cat >&2 <<'EOF'
BLOCKED: Rust third-party notice coverage is not yet complete.

The current dependency inventory is machine-readable evidence, not a complete
set of verbatim Rust dependency license, copyright, and NOTICE texts. Do not
run cargo publish until the runbook in docs/release/rust-third-party-notices.md
has been completed and this gate is replaced by a verified notice generator.
EOF
exit 1
