#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_ROOT="${TELLURION_DEMO_DATA_ROOT:-${SCRIPT_DIR}/../../demo-data}"
DB_DATA_ROOT="${TELLURION_DB_DATA_ROOT:-$DATA_ROOT}"
PROFILE="italy"
INGEST_BIN="${TELLURION_INGEST_BIN:-cargo run -q -p tellurion-ingest --}"

usage() {
  cat <<'EOF'
Usage: ingest-osm.sh [--profile italy|europe] [--root PATH]

DATABASE_URL must point at the PostGIS database. The helper loads the OSM
`lines` and `multipolygons` layers into italy_* or europe_* tables and prints
the configuration path to use afterwards.

Set TELLURION_INGEST_BIN to an installed tellurion-ingest executable when
available. The default uses cargo run from the Tellurion checkout.
EOF
}

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while (($#)); do
  case "$1" in
    --profile) (($# >= 2)) || die "--profile needs italy or europe"; PROFILE="$2"; shift 2 ;;
    --root) (($# >= 2)) || die "--root needs a path"; DATA_ROOT="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

[[ "$PROFILE" == italy || "$PROFILE" == europe ]] || die "unknown profile: $PROFILE"
[[ -n "${DATABASE_URL:-}" ]] || die "DATABASE_URL is required"
command -v ogrinfo >/dev/null 2>&1 || die "GDAL ogrinfo is required to inspect the PBF"

if [[ "$PROFILE" == europe && -z "${TELLURION_DB_DATA_ROOT:-}" ]]; then
  die "TELLURION_DB_DATA_ROOT is required for Europe so the PostGIS volume is checked separately from the PBF volume"
fi

# The PBF volume and the PostGIS volume are often different (especially when
# Europe is staged on an external disk). Guard the database volume before OGR
# creates tables and indexes there.
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/profiles.env"
if [[ "$PROFILE" == italy ]]; then
  DB_MIN_FREE_GIB="$ITALY_DB_MIN_FREE_GIB"
else
  DB_MIN_FREE_GIB="$EUROPE_DB_MIN_FREE_GIB"
fi
DB_DF_TARGET="$DB_DATA_ROOT"
while [[ ! -e "$DB_DF_TARGET" && "$DB_DF_TARGET" != "/" ]]; do
  DB_DF_TARGET="$(dirname "$DB_DF_TARGET")"
done
DB_FREE_KIB="$(df -Pk "$DB_DF_TARGET" 2>/dev/null | awk 'NR==2 {print $4}')"
[[ "$DB_FREE_KIB" =~ ^[0-9]+$ ]] || die "cannot determine free space for PostGIS volume $DB_DATA_ROOT"
DB_MIN_FREE_KIB=$((DB_MIN_FREE_GIB * 1024 * 1024))
if (( DB_FREE_KIB < DB_MIN_FREE_KIB )); then
  die "PostGIS volume needs at least ${DB_MIN_FREE_GIB} GiB free before loading $PROFILE; ${DB_FREE_KIB} KiB is available at $DB_DATA_ROOT"
fi
printf 'PostGIS volume=%s free_gib=%.2f minimum_free_gib=%s\n' \
  "$DB_DATA_ROOT" "$(awk -v kib="$DB_FREE_KIB" 'BEGIN {printf "%.2f", kib/1024/1024}')" "$DB_MIN_FREE_GIB"

"${SCRIPT_DIR}/prepare.sh" --profile "$PROFILE" --check-only --root "$DATA_ROOT" >/dev/null
if [[ "$PROFILE" == italy ]]; then
  ID_PREFIX="italy"; ID="italy-osm"
else
  ID_PREFIX="europe"; ID="europe-osm"
fi
PBF="$(find "$DATA_ROOT/$ID" -maxdepth 1 -name '*.osm.pbf' -type f -print -quit 2>/dev/null || true)"
[[ -f "$PBF" ]] || die "validated $PROFILE data is missing; run prepare.sh --download first"

for layer in lines multipolygons; do
  printf 'Inspecting OSM layer %s in %s\n' "$layer" "$PBF"
  ogrinfo -so "$PBF" "$layer" >/dev/null || die "GDAL layer '$layer' is not available in $PBF"
  collection="${ID_PREFIX}_${layer}"
  printf 'Loading %s as collection %s\n' "$layer" "$collection"
  # Intentional word splitting: TELLURION_INGEST_BIN may include arguments.
  # shellcheck disable=SC2086
  $INGEST_BIN load "$PBF" --collection "$collection" --layer "$layer"
done

if [[ "$PROFILE" == italy ]]; then
  CONFIG_PATH="config/example-italy-osm.yaml"
else
  CONFIG_PATH="config/example-europe-osm.yaml"
fi
printf '\nLoaded OSM profile %s. Use %s as the registry shape.\n' "$PROFILE" "$CONFIG_PATH"
printf 'Benchmark example: COLLECTION=%s ./bench/scenarios.sh\n' "${ID_PREFIX}_multipolygons"
