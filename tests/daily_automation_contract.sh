#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WORKFLOWS="$ROOT/.github/workflows"
DAILY="$WORKFLOWS/daily.yml"
COMMIT_GATE="$WORKFLOWS/commit-gate.yml"

fail() {
  printf 'daily automation contract failed: %s\n' "$1" >&2
  exit 1
}

scheduled=$(grep -l '^[[:space:]]*schedule:' "$WORKFLOWS"/*.yml | wc -l | tr -d ' ')
test "$scheduled" = 1 || fail "expected exactly one scheduled workflow, found $scheduled"

test -f "$DAILY" || fail "missing .github/workflows/daily.yml"
test -f "$COMMIT_GATE" || fail "missing .github/workflows/commit-gate.yml"
test ! -f "$WORKFLOWS/smoke.yml" || fail "smoke.yml must be consolidated into daily.yml"
test ! -f "$WORKFLOWS/verify.yml" || fail "verify.yml must be consolidated into daily.yml"

if grep -Eq '^[[:space:]]+(push|pull_request):' "$DAILY"; then
  fail "daily.yml must not run automatically for pushes or pull requests"
fi

grep -Fq 'workflow_dispatch:' "$DAILY" || fail "daily.yml must retain manual dispatch"
grep -Fq 'cron: "17 6 * * *"' "$DAILY" || fail "daily.yml must run at 06:17 UTC"
grep -Fq 'group: daily-verification-and-deploy' "$DAILY" || fail "daily.yml must serialize daily verification"
grep -Fq 'cancel-in-progress: true' "$DAILY" || fail "daily.yml must cancel superseded attempts"

grep -Fq 'name: deployment and fixture contracts' "$DAILY" || fail "daily.yml is missing repository contracts"
grep -Fq 'name: public distribution manifest' "$DAILY" || fail "daily.yml is missing distribution verification"
grep -Fq 'name: ${{ matrix.demo }}' "$DAILY" || fail "daily.yml is missing live endpoint smoke checks"
grep -Fq 'sh tests/proof_brief_contract.sh' "$DAILY" || fail "daily.yml is missing the proof brief contract"
grep -Fq 'http://127.0.0.1:8080/proof/' "$DAILY" || fail "daily.yml does not load the proof brief"

grep -Fq 'pull_request:' "$COMMIT_GATE" || fail "commit gate must validate pull requests"
grep -Fq 'push:' "$COMMIT_GATE" || fail "commit gate must validate main commits"
grep -Fq 'main' "$COMMIT_GATE" || fail "commit gate must be limited to main"
if grep -Eq '^[[:space:]]*schedule:' "$COMMIT_GATE"; then
  fail "commit gate must not add live checks to the daily schedule"
fi
grep -Fq 'name: deterministic deployment contracts' "$COMMIT_GATE" || fail "commit gate is missing deterministic contracts"
grep -Fq 'sh tests/daily_automation_contract.sh' "$COMMIT_GATE" || fail "commit gate must verify automation policy"
grep -Fq 'sh tests/render_deployment_contract.sh' "$COMMIT_GATE" || fail "commit gate must verify Render deployment contracts"
grep -Fq 'python3 tests/static_site_links.py' "$COMMIT_GATE" || fail "commit gate must verify static site links"
if grep -Eq 'https?://(ccancellieri\.github\.io|tellurion-[a-z0-9-]+\.onrender\.com)' "$COMMIT_GATE"; then
  fail "commit gate must not probe public endpoints"
fi
grep -Fq 'timeout-minutes:' "$COMMIT_GATE" || fail "commit gate jobs require timeouts"

python3 - "$DAILY" <<'PY'
import sys

import yaml

with open(sys.argv[1], encoding="utf-8") as workflow_file:
    workflow = yaml.safe_load(workflow_file)

jobs = workflow["jobs"]
live_entries = jobs["live-endpoints"]["strategy"]["matrix"]["include"]
italy_entries = jobs["italy-endpoints"]["strategy"]["matrix"]["include"]


def require_entry(entries, demo, expected):
    matches = [entry for entry in entries if entry.get("demo") == demo]
    if len(matches) != 1:
        raise SystemExit(f"expected one {demo!r} smoke entry, found {len(matches)}")
    for key, value in expected.items():
        if matches[0].get(key) != value:
            raise SystemExit(
                f"{demo!r} must set {key}={value!r}, got {matches[0].get(key)!r}"
            )


vector_base = "https://tellurion-vector-demo.onrender.com"
stac_base = "https://tellurion-stac-harvest-demo.onrender.com"
require_entry(
    live_entries,
    "vector landing links",
    {
        "url": f"{vector_base}/public",
        "expected_base_url": vector_base,
        "link_contract": "landing",
    },
)
require_entry(
    live_entries,
    "vector features",
    {"expected_base_url": vector_base, "link_contract": "items"},
)
require_entry(
    live_entries,
    "STAC harvest landing links",
    {
        "url": f"{stac_base}/public",
        "expected_base_url": stac_base,
        "link_contract": "landing",
    },
)
require_entry(
    italy_entries,
    "Italy FeatureCollection",
    {"expected_base_url": stac_base, "link_contract": "items"},
)

for job_name in ("live-endpoints", "italy-endpoints"):
    run_blocks = "\n".join(
        step.get("run", "") for step in jobs[job_name]["steps"] if isinstance(step, dict)
    )
    if "python3 tests/canonical_links.py" not in run_blocks:
        raise SystemExit(f"{job_name} must enforce the canonical-link response contract")
PY

render_services=$(grep -c '^[[:space:]]*-[[:space:]]*type: web' "$ROOT/render.yaml")
checks_pass=$(grep -c 'autoDeployTrigger: checksPass' "$ROOT/render.yaml")
test "$render_services" = 5 || fail "expected five Render web services, found $render_services"
test "$checks_pass" = "$render_services" || fail "every Render service must require passing checks"

printf 'Daily automation contract verified.\n'
