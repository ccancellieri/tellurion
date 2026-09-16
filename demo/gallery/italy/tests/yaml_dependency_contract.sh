#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
REAL_PYTHON=$(command -v python3)

if output=$(
  REAL_PYTHON="$REAL_PYTHON" \
  PATH="$ROOT/tests/fixtures/python-no-site:$PATH" \
  SKIP_YAML_DEPENDENCY_CONTRACT=1 \
  sh "$ROOT/tests/render_deployment_contract.sh" 2>&1
); then
  printf 'the deployment contract unexpectedly passed without test dependencies\n' >&2
  exit 1
fi

case "$output" in
  *'missing test dependency: PyYAML'*) ;;
  *)
    printf 'the deployment contract did not identify the missing PyYAML dependency\n' >&2
    exit 1
    ;;
esac

case "$output" in
  *'python3 -m pip install -r tests/requirements.txt'*) ;;
  *)
    printf 'the deployment contract did not provide the test dependency install command\n' >&2
    exit 1
    ;;
esac
