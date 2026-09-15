"""Validate and derive the bounded ESA WorldCover STAC harvest contract."""

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import tempfile
from typing import Mapping
from urllib.parse import urlencode, urlsplit
from urllib.request import urlopen


COLLECTION_ID = "esa-worldcover"
ITEM_ID = "ESA_WorldCover_10m_2021_v200_N39E012"
EXPECTED_VALUES = [10, 20, 30, 40, 50, 60, 70, 80, 90, 95, 100]
EXPECTED_CLASS_METADATA = {
    10: ("006400", "Tree cover"),
    20: ("FFBB22", "Shrubland"),
    30: ("FFFF4C", "Grassland"),
    40: ("F096FF", "Cropland"),
    50: ("FA0000", "Built-up"),
    60: ("B4B4B4", "Bare / sparse vegetation"),
    70: ("F0F0F0", "Snow and ice"),
    80: ("0064C8", "Permanent water bodies"),
    90: ("0096A0", "Herbaceous wetland"),
    95: ("00CF75", "Mangroves"),
    100: ("FAE6A0", "Moss and lichen"),
}
COLLECTION_URL = "https://planetarycomputer.microsoft.com/api/stac/v1/collections/esa-worldcover"
SEARCH_URL = "https://planetarycomputer.microsoft.com/api/stac/v1/search"
TRANSFORM_VERSION = "2"
SERVED_ASSET = {
    "byte_length": 41236803,
    "href": "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_N39E012_Map.tif",
    "relationship": "byte-identical-official-mirror",
    "sha256": "5d951afb19e5fdcb90773bac374b556d425f8945ba4b719114c7f7b03157464a",
}


@dataclass(frozen=True)
class Derived:
    collection: dict
    item: dict
    footprint: dict
    legend: list[dict]
    colormap: dict
    manifest_facts: dict


class HarvestError(ValueError):
    pass


def derive_legend(asset: dict) -> list[dict]:
    classes = asset.get("classification:classes") or []
    values = [entry.get("value") for entry in classes]
    if values != EXPECTED_VALUES:
        if len(values) != len(set(values)):
            raise HarvestError("duplicate class value")
        raise HarvestError("unexpected classification values")
    legend = []
    for entry in classes:
        color = entry.get("color-hint", "")
        label = entry.get("description", "")
        if not re.fullmatch(r"[0-9A-Fa-f]{6}", color):
            raise HarvestError("invalid color-hint")
        expected_color, expected_label = EXPECTED_CLASS_METADATA[entry["value"]]
        if color != expected_color:
            raise HarvestError("unexpected color-hint")
        if not label:
            raise HarvestError("classification description is required")
        if label != expected_label:
            raise HarvestError("unexpected classification description")
        legend.append(
            {"value": entry["value"], "label": label, "color": f"#{color.upper()}"}
        )
    return legend


def validate_and_derive(collection: dict, search: dict) -> Derived:
    if collection.get("id") != COLLECTION_ID:
        raise HarvestError("unexpected Collection")
    if collection.get("license") != "CC-BY-4.0":
        raise HarvestError("expected CC-BY-4.0 license")
    providers = collection.get("providers") or []
    names = {provider.get("name") for provider in providers}
    if "ESA" not in names or "Microsoft" not in names:
        raise HarvestError("expected ESA producer and Microsoft host")

    features = search.get("features") or []
    if len(features) != 1 or features[0].get("id") != ITEM_ID:
        raise HarvestError("unexpected Item")
    item = features[0]
    asset = (item.get("assets") or {}).get("map")
    if not isinstance(asset, dict):
        raise HarvestError("missing map asset")
    media_type = asset.get("type", "")
    if "profile=cloud-optimized" not in media_type:
        raise HarvestError("map asset is not a Cloud Optimized GeoTIFF")
    href = asset.get("href", "")
    parsed = urlsplit(href)
    if parsed.scheme != "https" or parsed.query:
        raise HarvestError("map asset must be an unsigned public HTTPS URL")

    legend = derive_legend(asset)

    properties = item.get("properties") or {}
    footprint = {
        "type": "FeatureCollection",
        "features": [
            {
                "type": "Feature",
                "id": 1,
                "geometry": item.get("geometry"),
                "properties": {
                    "id": 1,
                    "source_item_id": ITEM_ID,
                    "start_datetime": properties.get("start_datetime"),
                    "end_datetime": properties.get("end_datetime"),
                    "product_version": properties.get("esa_worldcover:product_version"),
                    "grid_code": properties.get("grid:code"),
                },
            }
        ],
    }
    stops = [{"value": 0.0, "rgba": [0, 0, 0, 0]}]
    for entry in legend:
        color = entry["color"]
        stops.append(
            {
                "value": float(entry["value"]),
                "rgba": [
                    int(color[1:3], 16),
                    int(color[3:5], 16),
                    int(color[5:7], 16),
                    255,
                ],
            }
        )
    return Derived(
        collection=collection,
        item=item,
        footprint=footprint,
        legend=legend,
        colormap={"kind": "stops", "stops": stops},
        manifest_facts={
            "collection_id": COLLECTION_ID,
            "item_id": ITEM_ID,
            "asset_href": href,
        },
    )


def fetch_json(url: str, params: Mapping[str, str] | None = None) -> dict:
    if params:
        url = f"{url}?{urlencode(params)}"
    with urlopen(url, timeout=30) as response:
        return json.load(response)


def _json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def _yaml_colormap(colormap: dict) -> bytes:
    lines = ["kind: stops", "stops:"]
    for stop in colormap["stops"]:
        rgba = ", ".join(str(component) for component in stop["rgba"])
        lines.append(f"  - {{ value: {stop['value']:.1f}, rgba: [{rgba}] }}")
    return ("\n".join(lines) + "\n").encode("utf-8")


def _write_atomic(path: Path, contents: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as temporary:
        temporary.write(contents)
        temporary_path = Path(temporary.name)
    temporary_path.replace(path)


def write_artifacts(derived: Derived, output: Path, retrieved_at: str) -> None:
    collection_bytes = _json_bytes(derived.collection)
    item_bytes = _json_bytes(derived.item)
    manifest = {
        "digests": {
            "collection": hashlib.sha256(collection_bytes).hexdigest(),
            "item": hashlib.sha256(item_bytes).hexdigest(),
        },
        "retrieved_at": retrieved_at,
        "source": {
            "asset_href": derived.manifest_facts["asset_href"],
            "asset_media_type": derived.item["assets"]["map"]["type"],
            "collection_id": derived.manifest_facts["collection_id"],
            "item_id": derived.manifest_facts["item_id"],
            "license": derived.collection["license"],
            "providers": derived.collection["providers"],
        },
        "served_asset": SERVED_ASSET,
        "transform_version": TRANSFORM_VERSION,
    }
    artifacts = {
        "collection.json": collection_bytes,
        "item.json": item_bytes,
        "manifest.json": _json_bytes(manifest),
        "footprint.geojson": _json_bytes(derived.footprint),
        "legend.json": _json_bytes(derived.legend),
        "colormap.yaml": _yaml_colormap(derived.colormap),
    }
    for name, contents in artifacts.items():
        _write_atomic(output / name, contents)


def _read_fixture(fixture_dir: Path) -> tuple[dict, dict]:
    collection = json.loads((fixture_dir / "collection.json").read_text(encoding="utf-8"))
    search = json.loads((fixture_dir / "item-search.json").read_text(encoding="utf-8"))
    return collection, search


def _retrieved_at_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--fixture-dir", type=Path)
    parser.add_argument("--retrieved-at")
    arguments = parser.parse_args()

    if arguments.fixture_dir:
        collection, search = _read_fixture(arguments.fixture_dir)
    else:
        collection = fetch_json(COLLECTION_URL)
        search = fetch_json(
            SEARCH_URL,
            {
                "collections": COLLECTION_ID,
                "bbox": "12.45,41.87,12.55,41.95",
                "datetime": "2021-01-01T00:00:00Z/2021-12-31T23:59:59Z",
                "limit": "1",
            },
        )
    write_artifacts(
        validate_and_derive(collection, search),
        arguments.output,
        arguments.retrieved_at or _retrieved_at_now(),
    )


if __name__ == "__main__":
    main()
