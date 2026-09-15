#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

fail() {
  printf 'proof brief contract failed: %s\n' "$1" >&2
  exit 1
}

require_file() {
  test -f "$1" || fail "missing required file: $1"
}

require_text() {
  grep -Fq "$2" "$1" || fail "missing required text in $1: $2"
}

reject_text() {
  if grep -Fq "$2" "$1"; then
    fail "forbidden text in $1: $2"
  fi
}

require_file proof/index.html
require_file proof/proof.css

require_text index.html 'href="proof/"'
require_text README.md 'actions/workflows/daily.yml'
reject_text README.md 'actions/workflows/smoke.yml'

require_text proof/index.html 'For recruiters'
require_text proof/index.html 'For self-hosted evaluation'
require_text proof/index.html 'self-hosted Community software'
require_text proof/index.html 'Five read-only service definitions'
require_text proof/index.html 'Five active backends'
require_text proof/index.html 'Public Tellurion v0.5.0-rc.1 release-candidate source'
require_text proof/index.html 'Italy explorer verified at deployment time'
require_text proof/index.html 'https://github.com/ccancellieri/tellurion-demos/releases/tag/tellurion-v0.3.0'
require_text proof/index.html 'https://github.com/ccancellieri/tellurion'
require_text proof/index.html 'https://github.com/ccancellieri/tellurion#quickstart'
require_text proof/index.html 'https://github.com/ccancellieri/tellurion/blob/main/docs/quickstart/install.md'
require_text proof/index.html 'No Tellurion Cloud, SLA or support service'
require_text proof/index.html 'https://github.com/ccancellieri'
require_text proof/index.html 'https://www.linkedin.com/in/ccancellieri/'
require_text proof/index.html 'data-contact="github"'
require_text proof/index.html 'data-contact="linkedin"'
require_text proof/index.html 'free on standard public runners, but usage is not unlimited'

reject_text proof/index.html 'mailto:'
reject_text proof/index.html 'tel:'
reject_text proof/index.html '#OGC'
reject_text proof/index.html '#ISO'
reject_text proof/index.html 'production-ready'
reject_text proof/index.html 'requests per second'
reject_text proof/index.html 'four read-only services'
reject_text proof/index.html 'Public engine source launch pending'

printf 'proof brief contract: ok\n'
