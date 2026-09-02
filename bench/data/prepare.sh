#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_ROOT="${TELLURION_DEMO_DATA_ROOT:-${SCRIPT_DIR}/../../demo-data}"
PROFILE=""
ACTION=""

usage() {
  cat <<'EOF'
Usage:
  prepare.sh --profile italy|europe --check-only [--root PATH]
  prepare.sh --profile italy|europe --download   [--root PATH]

The Europe profile is never downloaded implicitly. DATA_ROOT can also be set
with TELLURION_DEMO_DATA_ROOT, for example /Volumes/TellurionData/tellurion.
EOF
}

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while (($#)); do
  case "$1" in
    --profile) (($# >= 2)) || die "--profile needs italy or europe"; PROFILE="$2"; shift 2 ;;
    --check-only) ACTION="check"; shift ;;
    --download) ACTION="download"; shift ;;
    --root) (($# >= 2)) || die "--root needs a path"; DATA_ROOT="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

[[ -n "$PROFILE" ]] || { usage >&2; die "--profile is required"; }
[[ -n "$ACTION" ]] || { usage >&2; die "choose --check-only or --download"; }
case "$PROFILE" in italy|europe) ;; *) die "unknown profile: $PROFILE" ;; esac

# shellcheck disable=SC1091
source "${SCRIPT_DIR}/profiles.env"
if [[ "$PROFILE" == italy ]]; then
  ID="$ITALY_ID"; URL="$ITALY_URL"; CHECKSUM_URL="$ITALY_CHECKSUM_URL"
  EXPECTED_BYTES="$ITALY_EXPECTED_SOURCE_BYTES"; MIN_FREE_GIB="$ITALY_MIN_FREE_GIB"
  LICENSE="$ITALY_LICENSE"; ATTRIBUTION="$ITALY_ATTRIBUTION"; LAYERS="$ITALY_LAYERS"
else
  ID="$EUROPE_ID"; URL="$EUROPE_URL"; CHECKSUM_URL="$EUROPE_CHECKSUM_URL"
  EXPECTED_BYTES="$EUROPE_EXPECTED_SOURCE_BYTES"; MIN_FREE_GIB="$EUROPE_MIN_FREE_GIB"
  LICENSE="$EUROPE_LICENSE"; ATTRIBUTION="$EUROPE_ATTRIBUTION"; LAYERS="$EUROPE_LAYERS"
fi

if [[ -d "$DATA_ROOT" ]]; then
  ROOT_ABS="$(cd "$DATA_ROOT" && pwd)"
else
  ROOT_ABS="$(cd "$(dirname "$DATA_ROOT")" && pwd)/$(basename "$DATA_ROOT")"
fi
DF_TARGET="$ROOT_ABS"
while [[ ! -e "$DF_TARGET" && "$DF_TARGET" != "/" ]]; do
  DF_TARGET="$(dirname "$DF_TARGET")"
done
FREE_KIB="$(df -Pk "$DF_TARGET" 2>/dev/null | awk 'NR==2 {print $4}')"
[[ "$FREE_KIB" =~ ^[0-9]+$ ]] || die "cannot determine free space for $ROOT_ABS"
MIN_FREE_KIB=$((MIN_FREE_GIB * 1024 * 1024))
if (( FREE_KIB < MIN_FREE_KIB )); then
  die "$PROFILE needs at least ${MIN_FREE_GIB} GiB free on the destination volume; ${FREE_KIB} KiB is available at $ROOT_ABS"
fi

if [[ "$ACTION" == check ]]; then
  printf 'profile=%s\nroot=%s\nfree_gib=%.2f\nminimum_free_gib=%s\nsource_url=%s\nexpected_source_bytes=%s\nlayers=%s\n' \
    "$ID" "$ROOT_ABS" "$(awk -v kib="$FREE_KIB" 'BEGIN {printf "%.2f", kib/1024/1024}')" "$MIN_FREE_GIB" "$URL" "$EXPECTED_BYTES" "$LAYERS"
  exit 0
fi

command -v curl >/dev/null 2>&1 || die "curl is required for --download"
mkdir -p "$ROOT_ABS/$ID"
DEST="$ROOT_ABS/$ID/$(basename "$URL")"
CHECKSUM_FILE="${DEST}.md5"
MANIFEST="${ROOT_ABS}/${ID}/manifest.env"

printf 'Downloading %s to %s\n' "$URL" "$DEST"
curl --fail --location --retry 3 --retry-delay 2 --continue-at - --output "$DEST" "$URL"
curl --fail --location --retry 3 --retry-delay 2 --output "$CHECKSUM_FILE" "$CHECKSUM_URL"

EXPECTED_MD5="$(awk 'NF {print $1; exit}' "$CHECKSUM_FILE")"
[[ "$EXPECTED_MD5" =~ ^[[:xdigit:]]{32}$ ]] || die "could not read an MD5 checksum from $CHECKSUM_FILE"
if command -v md5sum >/dev/null 2>&1; then
  ACTUAL_MD5="$(md5sum "$DEST" | awk '{print $1}')"
elif command -v md5 >/dev/null 2>&1; then
  ACTUAL_MD5="$(md5 -q "$DEST")"
else
  die "md5sum or md5 is required to verify the extract"
fi
[[ "$ACTUAL_MD5" == "$EXPECTED_MD5" ]] || die "checksum mismatch: expected $EXPECTED_MD5, got $ACTUAL_MD5"

SOURCE_BYTES="$(wc -c < "$DEST" | tr -d '[:space:]')"
RETRIEVED_AT="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
{
  printf 'profile=%q\n' "$ID"
  printf 'source_url=%q\nchecksum_url=%q\n' "$URL" "$CHECKSUM_URL"
  printf 'retrieved_at=%q\nsource_bytes=%q\nmd5=%q\n' "$RETRIEVED_AT" "$SOURCE_BYTES" "$ACTUAL_MD5"
  printf 'license=%q\nattribution=%q\nlayers=%q\n' "$LICENSE" "$ATTRIBUTION" "$LAYERS"
  printf 'destination=%q\nfree_kib_after_download=%q\n' "$DEST" "$(df -Pk "$ROOT_ABS" | awk 'NR==2 {print $4}')"
} > "$MANIFEST"

printf 'validated profile=%s bytes=%s md5=%s\nmanifest=%s\n' "$ID" "$SOURCE_BYTES" "$ACTUAL_MD5" "$MANIFEST"
