#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MODE="${1:-run}"
OSM_JSON="${ROOT}/data/rome-osm-overpass.json"
WORLD_COVER="${ROOT}/data/rome-worldcover.tif"
WORLD_COVER_URL="https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_N39E012_Map.tif"
TELLURION_VERSION="v0.3.0"
RELEASE_BASE="https://github.com/ccancellieri/tellurion-italy-demo/releases/download/tellurion-${TELLURION_VERSION}"
CHECKSUM_NAME="SHA256SUMS"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) EXECUTABLE_SUFFIX=".exe" ;;
  *) EXECUTABLE_SUFFIX="" ;;
esac

need() {
  command -v "$1" >/dev/null 2>&1 || {
    printf 'missing dependency: %s\n' "$1" >&2
    exit 1
  }
}

check_python() {
  python3 -c 'import numpy, rasterio, shapely, PIL' >/dev/null 2>&1 || {
    printf '%s\n' "Python packages missing. Install with:" >&2
    printf '%s\n' "  python3 -m pip install --user numpy rasterio shapely pillow" >&2
    exit 1
  }
}

refresh_data() {
  need curl
  need gdal_translate
  mkdir -p "${ROOT}/data"
  curl --fail --location --retry 2 --max-time 120 \
    --user-agent 'Tellurion reproducible demo/0.2' \
    --data-urlencode 'data=[out:json][timeout:60];(way["highway"~"^(primary|secondary|tertiary|residential|pedestrian)$"](41.885,12.46,41.925,12.515);way["railway"](41.885,12.46,41.925,12.515););out tags geom;' \
    --output "${OSM_JSON}" \
    https://overpass-api.de/api/interpreter

  gdal_translate -q \
    -projwin 12.46 41.925 12.515 41.885 \
    -outsize 1200 900 -r nearest \
    "/vsicurl/${WORLD_COVER_URL}" "${WORLD_COVER}"
}

install_tellurion() {
  need curl
  system="$(uname -s)"
  machine="$(uname -m)"
  case "${system}:${machine}" in
    Darwin:arm64)
      target="aarch64-apple-darwin"
      archive="tellurion-${TELLURION_VERSION}-${target}.tar.gz"
      package_dir="tellurion-${TELLURION_VERSION}-${target}"
      need tar
      ;;
    Linux:x86_64)
      target="x86_64-unknown-linux-musl"
      archive="tellurion-${TELLURION_VERSION}-${target}.tar.gz"
      package_dir="tellurion-${TELLURION_VERSION}-${target}"
      need tar
      ;;
    MINGW*:x86_64|MSYS*:x86_64|CYGWIN*:x86_64)
      target="x86_64-pc-windows-gnu"
      archive="tellurion-${TELLURION_VERSION}-${target}.zip"
      package_dir="tellurion-${TELLURION_VERSION}-${target}"
      need unzip
      ;;
    *)
      printf 'unsupported Tellurion binary target: %s %s\n' "${system}" "${machine}" >&2
      printf '%s\n' "Use the source archive or open a platform request:" >&2
      printf '%s\n' "  https://github.com/ccancellieri/tellurion-italy-demo/issues/new" >&2
      exit 1
      ;;
  esac

  mkdir -p "${ROOT}/downloads" "${ROOT}/bin"
  curl --fail --location --retry 3 \
    --output "${ROOT}/downloads/${archive}" "${RELEASE_BASE}/${archive}"
  curl --fail --location --retry 3 \
    --output "${ROOT}/downloads/${CHECKSUM_NAME}" "${RELEASE_BASE}/${CHECKSUM_NAME}"

  (
    cd "${ROOT}/downloads"
    expected="$(awk -v file="${archive}" '$2 == file {print $1; exit}' "${CHECKSUM_NAME}")"
    test -n "${expected}"
    if command -v shasum >/dev/null 2>&1; then
      actual="$(shasum -a 256 "${archive}" | awk '{print $1}')"
    elif command -v sha256sum >/dev/null 2>&1; then
      actual="$(sha256sum "${archive}" | awk '{print $1}')"
    else
      printf '%s\n' "missing SHA-256 tool (shasum or sha256sum)" >&2
      exit 1
    fi
    test "${actual}" = "${expected}"
    printf 'Tellurion archive verified: %s\n' "${actual}"
    if [[ "${archive}" == *.zip ]]; then
      rm -rf "${package_dir}"
      unzip -q "${archive}"
    else
      tar -xzf "${archive}"
    fi
  )
  install -m 0755 \
    "${ROOT}/downloads/${package_dir}/tellurion${EXECUTABLE_SUFFIX}" \
    "${ROOT}/downloads/${package_dir}/tellurion-ingest${EXECUTABLE_SUFFIX}" \
    "${ROOT}/bin/"
  "${ROOT}/bin/tellurion${EXECUTABLE_SUFFIX}" --help >/dev/null
  "${ROOT}/bin/tellurion-ingest${EXECUTABLE_SUFFIX}" --help >/dev/null
  printf 'Tellurion %s installed for %s in %s/bin\n' \
    "${TELLURION_VERSION}" "${target}" "${ROOT}"
}

download_full_italy() {
  need curl
  mkdir -p "${ROOT}/data/italy-osm"
  printf '%s\n' "Downloading the current Geofabrik Italy extract (~2.2 GB)."
  curl --fail --location --retry 3 --continue-at - \
    --output "${ROOT}/data/italy-osm/italy-latest.osm.pbf" \
    https://download.geofabrik.de/europe/italy-latest.osm.pbf
  curl --fail --location --retry 3 \
    --output "${ROOT}/data/italy-osm/italy-latest.osm.pbf.md5" \
    https://download.geofabrik.de/europe/italy-latest.osm.pbf.md5
  expected="$(awk 'NF {print $1; exit}' "${ROOT}/data/italy-osm/italy-latest.osm.pbf.md5")"
  actual="$(md5 -q "${ROOT}/data/italy-osm/italy-latest.osm.pbf" 2>/dev/null || md5sum "${ROOT}/data/italy-osm/italy-latest.osm.pbf" | awk '{print $1}')"
  test "${actual}" = "${expected}"
  printf 'Italy extract verified: %s\n' "${actual}"
}

case "${MODE}" in
  run)
    check_python
    python3 "${ROOT}/analyze.py"
    python3 "${ROOT}/make_cards.py"
    ;;
  --refresh)
    refresh_data
    check_python
    python3 "${ROOT}/analyze.py"
    python3 "${ROOT}/make_cards.py"
    ;;
  --install)
    install_tellurion
    ;;
  --serve-demo)
    test -x "${ROOT}/bin/tellurion-ingest${EXECUTABLE_SUFFIX}" || install_tellurion
    exec "${ROOT}/bin/tellurion-ingest${EXECUTABLE_SUFFIX}" demo --path "${ROOT}/data/tellurion-demo.gpkg" --port 8080
    ;;
  --load-rome)
    test -x "${ROOT}/bin/tellurion-ingest${EXECUTABLE_SUFFIX}" || install_tellurion
    check_python
    python3 "${ROOT}/analyze.py" >/dev/null
    "${ROOT}/bin/tellurion-ingest${EXECUTABLE_SUFFIX}" geopackage create-tables \
      --path "${ROOT}/data/rome-osm.gpkg" \
      --table rome_roads --geometry geom --srid 4326 \
      --geometry-type LINESTRING \
      --columns highway:TEXT,name:TEXT,railway:TEXT
    "${ROOT}/bin/tellurion-ingest${EXECUTABLE_SUFFIX}" geopackage load \
      --path "${ROOT}/data/rome-osm.gpkg" \
      --table rome_roads "${ROOT}/output/rome-roads.geojson"
    ;;
  --serve-rome)
    test -f "${ROOT}/data/rome-osm.gpkg" || "$0" --load-rome
    exec env \
      TELLURION_GEOPACKAGE_PATH="${ROOT}/data/rome-osm.gpkg" \
      TELLURION_CONFIG="${ROOT}/config-rome-osm.yaml" \
      PORT=8080 \
      "${ROOT}/bin/tellurion${EXECUTABLE_SUFFIX}"
    ;;
  --full-italy)
    download_full_italy
    ;;
  *)
    printf 'Usage: %s [run|--refresh|--install|--serve-demo|--load-rome|--serve-rome|--full-italy]\n' "$0" >&2
    exit 2
    ;;
esac
