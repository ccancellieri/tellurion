#!/bin/sh

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
WORK=$(mktemp -d)

cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

mkdir -p "$WORK/bin"
cat >"$WORK/bin/cargo" <<'EOF'
#!/bin/sh
: >"$CARGO_MARKER"
exit 23
EOF
chmod +x "$WORK/bin/cargo"

cat >"$WORK/bin/timeout" <<'EOF'
#!/bin/sh
exit 125
EOF
chmod +x "$WORK/bin/timeout"

cat >"$WORK/bin/ps" <<'EOF'
#!/bin/sh
printf '62490 grep -F %s\n' "$FAKE_TELLURION"
EOF
chmod +x "$WORK/bin/ps"

mkdir -p "$WORK/repo/scripts"

for smoke in demo-smoke italy-contract-smoke; do
  sed 's/if \[ -r \/proc\/self\/exe \]; then/if false; then/' \
    "$ROOT/scripts/$smoke.sh" >"$WORK/repo/scripts/$smoke.sh"
  chmod +x "$WORK/repo/scripts/$smoke.sh"
  OUTPUT_FILE="$WORK/$smoke-output"
  CARGO_MARKER="$WORK/$smoke-cargo-called" \
    FAKE_TELLURION="$WORK/repo/target/debug/tellurion" \
    PATH="$WORK/bin:$PATH" \
    "$WORK/repo/scripts/$smoke.sh" >"$OUTPUT_FILE" 2>&1 || true

  if [ ! -f "$WORK/$smoke-cargo-called" ]; then
    cat "$OUTPUT_FILE" >&2
    printf 'FAIL: %s did not reach cargo after its process preflight\n' "$smoke" >&2
    exit 1
  fi

  if ! grep -Fq 'ok no tellurion process from this worktree is already running' "$OUTPUT_FILE"; then
    cat "$OUTPUT_FILE" >&2
    printf 'FAIL: %s process preflight reported its own detector\n' "$smoke" >&2
    exit 1
  fi
done

printf 'PASS: smoke process preflights ignore their own detectors\n'
