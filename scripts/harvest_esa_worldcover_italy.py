"""Harvest and verify the bounded 2021 ESA WorldCover Italy source set."""

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import re
import tempfile
from urllib.parse import urlencode, urlsplit
from urllib.request import Request, urlopen

try:
    from scripts.harvest_esa_worldcover import HarvestError, derive_legend
except ModuleNotFoundError:  # Direct execution puts scripts/, not the project root, on sys.path.
    from harvest_esa_worldcover import HarvestError, derive_legend


COLLECTION_ID = "esa-worldcover"
PRODUCT_TOKEN = "_2021_v200_"
MAX_ITEMS = 32
SEARCH_URL = "https://planetarycomputer.microsoft.com/api/stac/v1/search"
COLLECTION_URL = "https://planetarycomputer.microsoft.com/api/stac/v1/collections/esa-worldcover"
SIGN_URL = "https://planetarycomputer.microsoft.com/api/sas/v1/sign"
ESA_S3_PREFIX = "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/"
SOURCE_ASSET_HOST = "ai4edataeuwest.blob.core.windows.net"
GISCO_SOURCE_URL = "https://gisco-services.ec.europa.eu/distribution/v2/countries/geojson/CNTR_RG_01M_2024_4326.geojson"
TRANSFORM_VERSION = "1"
BOUNDARY_FIXTURE = Path(__file__).resolve().parents[1] / "tests/fixtures/stac/italy-boundary.geojson"


@dataclass(frozen=True)
class ItalyDerived:
    collection: dict
    items: list[dict]
    footprints: dict
    legend: list[dict]
    manifest: dict
    mosaic: dict


def _json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def _write_atomic(path: Path, contents: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as temporary:
        temporary.write(contents)
        temporary_path = Path(temporary.name)
    temporary_path.replace(path)


def _has_query_or_fragment(url: object) -> bool:
    if not isinstance(url, str):
        return False
    parsed = urlsplit(url)
    return bool(parsed.query or parsed.fragment)


def _has_volatile_href(value: object) -> bool:
    return isinstance(value, dict) and _has_query_or_fragment(value.get("href"))


def _is_url_field(key: object) -> bool:
    if not isinstance(key, str):
        return False
    normalized = key.lower()
    return normalized in {"url", "href"} or normalized.endswith(("_url", ":url", "_href"))


def _without_query_urls(value: object, key: object = None) -> object:
    if isinstance(value, dict):
        return {
            child_key: _without_query_urls(child, child_key)
            for child_key, child in value.items()
            if not _has_volatile_href(child)
        }
    if isinstance(value, list):
        return [_without_query_urls(child) for child in value if not _has_volatile_href(child)]
    if _is_url_field(key) and _has_query_or_fragment(value):
        raise HarvestError("generated artifact contains a query-bearing scalar URL")
    return value


def _source_href(asset: dict) -> str:
    href = asset.get("href")
    if not isinstance(href, str):
        raise HarvestError("map asset href is required")
    parsed = urlsplit(href)
    if parsed.scheme != "https" or parsed.query or parsed.fragment:
        raise HarvestError("map asset href must not contain a query or fragment")
    if parsed.hostname != SOURCE_ASSET_HOST:
        raise HarvestError("map asset must use the trusted Planetary Computer source host")
    return href


def _official_esa_href(source_href: str) -> str:
    filename = urlsplit(source_href).path.rsplit("/", 1)[-1]
    if not filename.endswith("_Map.tif") or PRODUCT_TOKEN not in filename:
        raise HarvestError("map asset filename is not a 2021 v200 WorldCover map")
    return ESA_S3_PREFIX + filename


def _valid_bbox(bbox: object) -> list[float]:
    if not isinstance(bbox, list) or len(bbox) != 4:
        raise HarvestError("invalid bbox")
    if not all(isinstance(value, (int, float)) and math.isfinite(value) for value in bbox):
        raise HarvestError("invalid bbox")
    west, south, east, north = bbox
    if not (-180 <= west < east <= 180 and -90 <= south < north <= 90):
        raise HarvestError("invalid bbox")
    return [float(value) for value in bbox]


def _bbox_intersects(left: list[float], right: list[float]) -> bool:
    return left[0] <= right[2] and left[2] >= right[0] and left[1] <= right[3] and left[3] >= right[1]


def _ring(coordinates: object, label: str) -> list[tuple[float, float]]:
    if not isinstance(coordinates, list) or len(coordinates) < 4:
        raise HarvestError(f"{label} has an invalid linear ring")
    ring = []
    for position in coordinates:
        if (
            not isinstance(position, list)
            or len(position) < 2
            or not all(isinstance(value, (int, float)) and math.isfinite(value) for value in position[:2])
        ):
            raise HarvestError(f"{label} has an invalid coordinate")
        ring.append((float(position[0]), float(position[1])))
    if ring[0] != ring[-1]:
        raise HarvestError(f"{label} has an unclosed linear ring")
    return ring


def _geometry_polygons(geometry: object, label: str) -> list[list[list[tuple[float, float]]]]:
    if not isinstance(geometry, dict) or geometry.get("type") not in {"Polygon", "MultiPolygon"}:
        raise HarvestError(f"{label} must contain a Polygon or MultiPolygon")
    coordinates = geometry.get("coordinates")
    polygons = coordinates if geometry["type"] == "MultiPolygon" else [coordinates]
    if not isinstance(polygons, list) or not polygons:
        raise HarvestError(f"{label} has no coordinates")
    result = []
    for polygon in polygons:
        if not isinstance(polygon, list) or not polygon:
            raise HarvestError(f"{label} has an invalid Polygon")
        result.append([_ring(ring, label) for ring in polygon])
    return result


def _polygon_bbox(polygon: list[list[tuple[float, float]]]) -> list[float]:
    coordinates = [point for ring in polygon for point in ring]
    return _valid_bbox(
        [
            min(point[0] for point in coordinates),
            min(point[1] for point in coordinates),
            max(point[0] for point in coordinates),
            max(point[1] for point in coordinates),
        ]
    )


def _cross_product(start: tuple[float, float], end: tuple[float, float], point: tuple[float, float]) -> float:
    return (end[0] - start[0]) * (point[1] - start[1]) - (end[1] - start[1]) * (point[0] - start[0])


def _point_on_segment(point: tuple[float, float], start: tuple[float, float], end: tuple[float, float]) -> bool:
    return (
        abs(_cross_product(start, end, point)) <= 1e-12
        and min(start[0], end[0]) <= point[0] <= max(start[0], end[0])
        and min(start[1], end[1]) <= point[1] <= max(start[1], end[1])
    )


def _segments_intersect(
    first_start: tuple[float, float],
    first_end: tuple[float, float],
    second_start: tuple[float, float],
    second_end: tuple[float, float],
) -> bool:
    first_crosses = (_cross_product(first_start, first_end, second_start), _cross_product(first_start, first_end, second_end))
    second_crosses = (_cross_product(second_start, second_end, first_start), _cross_product(second_start, second_end, first_end))
    if ((first_crosses[0] > 0 > first_crosses[1]) or (first_crosses[0] < 0 < first_crosses[1])) and (
        (second_crosses[0] > 0 > second_crosses[1]) or (second_crosses[0] < 0 < second_crosses[1])
    ):
        return True
    return any(
        (
            _point_on_segment(second_start, first_start, first_end),
            _point_on_segment(second_end, first_start, first_end),
            _point_on_segment(first_start, second_start, second_end),
            _point_on_segment(first_end, second_start, second_end),
        )
    )


def _point_in_ring(point: tuple[float, float], ring: list[tuple[float, float]]) -> bool:
    inside = False
    for start, end in zip(ring, ring[1:]):
        if _point_on_segment(point, start, end):
            return True
        if (start[1] > point[1]) != (end[1] > point[1]):
            crossing = (end[0] - start[0]) * (point[1] - start[1]) / (end[1] - start[1]) + start[0]
            if point[0] < crossing:
                inside = not inside
    return inside


def _point_in_polygon(point: tuple[float, float], polygon: list[list[tuple[float, float]]]) -> bool:
    if not _point_in_ring(point, polygon[0]):
        return False
    return not any(_point_in_ring(point, hole) for hole in polygon[1:])


def _polygon_edges(polygon: list[list[tuple[float, float]]]):
    for ring in polygon:
        yield from zip(ring, ring[1:])


def _polygons_intersect(left: list[list[tuple[float, float]]], right: list[list[tuple[float, float]]]) -> bool:
    if not _bbox_intersects(_polygon_bbox(left), _polygon_bbox(right)):
        return False
    if any(_segments_intersect(*left_edge, *right_edge) for left_edge in _polygon_edges(left) for right_edge in _polygon_edges(right)):
        return True
    return _point_in_polygon(left[0][0], right) or _point_in_polygon(right[0][0], left)


def _geometries_intersect(
    left: list[list[list[tuple[float, float]]]], right: list[list[list[tuple[float, float]]]]
) -> bool:
    return any(_polygons_intersect(left_polygon, right_polygon) for left_polygon in left for right_polygon in right)


def _validate_collection(collection: dict) -> None:
    if collection.get("id") != COLLECTION_ID:
        raise HarvestError("unexpected Collection")
    if collection.get("license") != "CC-BY-4.0":
        raise HarvestError("expected CC-BY-4.0 license")
    names = {provider.get("name") for provider in collection.get("providers") or []}
    if not {"ESA", "ESA WorldCover Consortium", "Microsoft"}.issubset(names):
        raise HarvestError("expected ESA, ESA WorldCover Consortium, and Microsoft providers")


def _validate_boundary(boundary: dict) -> dict:
    if boundary.get("type") != "Feature" or boundary.get("properties", {}).get("CNTR_ID") != "IT":
        raise HarvestError("expected pinned GISCO Italy boundary")
    properties = boundary["properties"]
    if not all(properties.get(key) for key in ("gisco:source_url", "gisco:source_sha256", "gisco:terms")):
        raise HarvestError("Italy boundary provenance is incomplete")
    if properties["gisco:source_url"] != GISCO_SOURCE_URL:
        raise HarvestError("Italy boundary source URL must be the pinned queryless HTTPS GISCO URL")
    geometry = boundary.get("geometry")
    _geometry_polygons(geometry, "Italy boundary")
    return geometry


def validate_and_derive(collection: dict, search: dict, italy_boundary: dict) -> ItalyDerived:
    _validate_collection(collection)
    boundary_geometry = _validate_boundary(italy_boundary)
    boundary_polygons = _geometry_polygons(boundary_geometry, "Italy boundary")
    features = search.get("features")
    if not isinstance(features, list) or not features:
        raise HarvestError("search returned no Items")
    if len(features) > MAX_ITEMS:
        raise HarvestError("search returned more than 32 Items")

    items = sorted(features, key=lambda item: item.get("id", ""))
    ids: set[str] = set()
    source_hrefs: set[str] = set()
    footprints = []
    legend: list[dict] | None = None
    sources = []
    upstream_item_digests = {}

    for index, item in enumerate(items, start=1):
        item_id = item.get("id")
        if not isinstance(item_id, str) or PRODUCT_TOKEN not in item_id:
            raise HarvestError("Item id must identify a 2021 v200 product")
        if item_id in ids:
            raise HarvestError("duplicate id")
        ids.add(item_id)
        if item.get("collection") != COLLECTION_ID:
            raise HarvestError("Item must belong to the esa-worldcover collection")
        asset = (item.get("assets") or {}).get("map")
        if not isinstance(asset, dict):
            raise HarvestError("missing map asset")
        if "profile=cloud-optimized" not in asset.get("type", ""):
            raise HarvestError("map asset is not a Cloud Optimized GeoTIFF")
        source_href = _source_href(asset)
        if source_href in source_hrefs:
            raise HarvestError("duplicate map href")
        source_hrefs.add(source_href)
        bbox = _valid_bbox(item.get("bbox"))
        item_geometry = _geometry_polygons(item.get("geometry"), f"Item {item_id} geometry")
        if not _geometries_intersect(item_geometry, boundary_polygons):
            raise HarvestError("Item geometry does not intersect the Italy boundary")
        item_legend = derive_legend(asset)
        if legend is None:
            legend = item_legend
        elif item_legend != legend:
            raise HarvestError("classification palette differs between Items")
        properties = item.get("properties") or {}
        official_esa_href = _official_esa_href(source_href)
        filename = urlsplit(source_href).path.rsplit("/", 1)[-1]
        if item_id != filename.removesuffix("_Map.tif"):
            raise HarvestError("Item id must exactly match the map asset filename")
        footprints.append(
            {
                "type": "Feature",
                "id": index,
                "geometry": item.get("geometry"),
                "properties": {
                    "id": index,
                    "source_item_id": item_id,
                    "start_datetime": properties.get("start_datetime"),
                    "end_datetime": properties.get("end_datetime"),
                    "product_version": properties.get("esa_worldcover:product_version"),
                    "grid_code": properties.get("grid:code"),
                },
            }
        )
        sources.append({"id": item_id, "href": official_esa_href, "bbox": bbox})
        upstream_item_digests[item_id] = hashlib.sha256(_json_bytes(item)).hexdigest()

    footprints_document = {"type": "FeatureCollection", "features": footprints}
    mosaic = {"id": "esa_worldcover_2021_italy", "sources": sources}
    manifest = {
        "boundary": {
            "digest": hashlib.sha256(_json_bytes(italy_boundary)).hexdigest(),
            "source_sha256": italy_boundary["properties"]["gisco:source_sha256"],
            "source_url": italy_boundary["properties"]["gisco:source_url"],
            "terms": italy_boundary["properties"]["gisco:terms"],
        },
        "boundary_document": italy_boundary,
        "digests": {
            "upstream_collection": hashlib.sha256(_json_bytes(collection)).hexdigest(),
            "upstream_items": upstream_item_digests,
        },
        "source": {
            "collection_id": COLLECTION_ID,
            "license": collection["license"],
            "providers": collection["providers"],
        },
        "transform_version": TRANSFORM_VERSION,
    }
    return ItalyDerived(collection, items, footprints_document, legend or [], manifest, mosaic)


def fetch_json(url: str) -> dict:
    with urlopen(url, timeout=30) as response:
        return json.load(response)


def post_json(url: str, body: dict) -> dict:
    request = Request(url, _json_bytes(body), {"Content-Type": "application/json"}, method="POST")
    with urlopen(request, timeout=30) as response:
        return json.load(response)


def fetch_live_sources(italy_boundary: dict) -> dict:
    geometry = _validate_boundary(italy_boundary)
    search = post_json(
        SEARCH_URL,
        {
            "collections": [COLLECTION_ID],
            "intersects": geometry,
            "datetime": "2021-01-01T00:00:00Z/2021-12-31T23:59:59Z",
            "limit": 100,
        },
    )
    if not isinstance(search, dict):
        raise HarvestError("source search returned an invalid response")
    number_matched = search.get("numberMatched")
    if number_matched is not None:
        if isinstance(number_matched, bool) or not isinstance(number_matched, int) or number_matched < 0:
            raise HarvestError("numberMatched must be a non-negative integer")
        if number_matched > MAX_ITEMS:
            raise HarvestError("pagination refused: more than 32 source Items match")
        features = search.get("features")
        if not isinstance(features, list) or number_matched != len(features):
            raise HarvestError("numberMatched must equal the returned feature count")
    if any(link.get("rel") == "next" for link in search.get("links") or []):
        raise HarvestError("pagination refused: source search supplied a next link")
    return search


def sign_asset(source_href: str) -> str:
    signed = fetch_json(f"{SIGN_URL}?{urlencode({'href': source_href})}")
    href = signed.get("href")
    if not isinstance(href, str) or not urlsplit(href).query:
        raise HarvestError("Planetary Computer signing endpoint returned no signed href")
    return href


def _stream_digest(url: str) -> tuple[int, str]:
    digest = hashlib.sha256()
    length = 0
    with urlopen(url, timeout=30) as response:
        while chunk := response.read(1024 * 1024):
            length += len(chunk)
            digest.update(chunk)
    return length, digest.hexdigest()


def verify_assets(items: list[dict]) -> dict:
    verified = {}
    for item in items:
        source_href = _source_href(item["assets"]["map"])
        signed_href = sign_asset(source_href)
        official_esa_href = _official_esa_href(source_href)
        source_length, source_digest = _stream_digest(signed_href)
        mirror_length, mirror_digest = _stream_digest(official_esa_href)
        if (source_length, source_digest) != (mirror_length, mirror_digest):
            raise HarvestError(f"official ESA mirror is not byte-identical for {item['id']}")
        verified[item["id"]] = {
            "source_href": source_href,
            "official_esa_href": official_esa_href,
            "byte_length": source_length,
            "sha256": source_digest,
        }
    return verified


def _verified_sources(derived: ItalyDerived, verified: dict | None, fixture_mode: bool) -> list[dict]:
    if verified is None:
        if not fixture_mode:
            raise HarvestError("a production release requires verified asset digests")
        return derived.mosaic["sources"]
    if not isinstance(verified, dict):
        raise HarvestError("verified records must be an object")
    expected_ids = {item["id"] for item in derived.items}
    if set(verified) != expected_ids:
        raise HarvestError("verified record ids must exactly match the harvested source ids")
    sources = []
    for item, source in zip(derived.items, derived.mosaic["sources"]):
        record = verified.get(item["id"])
        if not isinstance(record, dict):
            raise HarvestError(f"verified record missing for {item['id']}")
        if record.get("source_href") != item["assets"]["map"]["href"] or record.get("official_esa_href") != source["href"]:
            raise HarvestError(f"verified record does not match {item['id']}")
        if not isinstance(record.get("byte_length"), int) or record["byte_length"] <= 0:
            raise HarvestError(f"verified byte length is invalid for {item['id']}")
        if not isinstance(record.get("sha256"), str) or not re.fullmatch(r"[0-9a-f]{64}", record["sha256"]):
            raise HarvestError(f"verified digest is invalid for {item['id']}")
        sources.append({**source, "byte_length": record["byte_length"], "sha256": record["sha256"]})
    return sources


def write_artifacts(
    derived: ItalyDerived,
    output: Path,
    retrieved_at: str,
    verified: dict | None = None,
    fixture_mode: bool = False,
) -> None:
    mosaic = {**derived.mosaic, "sources": _verified_sources(derived, verified, fixture_mode)}
    collection = _without_query_urls(derived.collection)
    boundary = _without_query_urls(derived.manifest["boundary_document"])
    footprints = _without_query_urls(derived.footprints)
    legend = _without_query_urls(derived.legend)
    sanitized_items = {item["id"]: _without_query_urls(item) for item in derived.items}
    mosaic = _without_query_urls(mosaic)
    collection_bytes = _json_bytes(collection)
    boundary_bytes = _json_bytes(boundary)
    footprints_bytes = _json_bytes(footprints)
    legend_bytes = _json_bytes(legend)
    item_bytes = {item_id: _json_bytes(item) for item_id, item in sanitized_items.items()}
    mosaic_bytes = _json_bytes(mosaic)
    snapshot_digests = {
        "collection": hashlib.sha256(collection_bytes).hexdigest(),
        "items": {item_id: hashlib.sha256(contents).hexdigest() for item_id, contents in item_bytes.items()},
        "mosaic": hashlib.sha256(mosaic_bytes).hexdigest(),
    }
    manifest = {
        **{key: value for key, value in derived.manifest.items() if key != "boundary_document"},
        "boundary": {**derived.manifest["boundary"], "digest": hashlib.sha256(boundary_bytes).hexdigest()},
        "digests": {**snapshot_digests, **derived.manifest["digests"]},
        "retrieved_at": retrieved_at,
        "verified_assets": verified or {},
    }
    artifacts = {
        "collection.json": collection_bytes,
        "boundary.geojson": boundary_bytes,
        "footprints.geojson": footprints_bytes,
        "legend.json": legend_bytes,
        "manifest.json": _json_bytes(_without_query_urls(manifest)),
        "mosaic.json": mosaic_bytes,
    }
    artifacts.update({f"items/{item_id}.json": contents for item_id, contents in item_bytes.items()})
    for name, contents in artifacts.items():
        _write_atomic(output / name, contents)


def _read_fixture(fixture_dir: Path) -> tuple[dict, dict, dict]:
    return (
        json.loads((fixture_dir / "collection.json").read_text(encoding="utf-8")),
        json.loads((fixture_dir / "item-search-italy.json").read_text(encoding="utf-8")),
        json.loads((fixture_dir / "italy-boundary.geojson").read_text(encoding="utf-8")),
    )


def _read_verified_manifest(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8")).get("verified_assets") or {}


def _retrieved_at_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--fixture-dir", type=Path)
    parser.add_argument("--retrieved-at")
    parser.add_argument("--verify-assets", action="store_true")
    parser.add_argument("--reuse-verified-manifest", type=Path)
    arguments = parser.parse_args()

    if arguments.fixture_dir:
        collection, search, boundary = _read_fixture(arguments.fixture_dir)
        verified = None
    else:
        collection = fetch_json(COLLECTION_URL)
        boundary = json.loads(BOUNDARY_FIXTURE.read_text(encoding="utf-8"))
        search = fetch_live_sources(boundary)
        verified = verify_assets(validate_and_derive(collection, search, boundary).items) if arguments.verify_assets else None
        if verified is None and arguments.reuse_verified_manifest:
            verified = _read_verified_manifest(arguments.reuse_verified_manifest)
    derived = validate_and_derive(collection, search, boundary)
    write_artifacts(
        derived,
        arguments.output,
        arguments.retrieved_at or _retrieved_at_now(),
        verified,
        fixture_mode=bool(arguments.fixture_dir),
    )


if __name__ == "__main__":
    main()
