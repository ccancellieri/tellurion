"""Export preserved WorldCover metadata as a portable static STAC subset.

Offline demo support only: no raster bytes are fetched, copied or converted.
"""

import argparse
import copy
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
from urllib.parse import urljoin, urlsplit


DEFAULT_SOURCE = Path(__file__).resolve().parents[1] / "data/stac/esa-worldcover-italy"
MAX_ITEMS = 17
MAX_METADATA_BYTES = 2_000_000
NAVIGATION = {"self", "root", "parent", "collection", "child", "item", "items", "next", "prev", "first", "last", "search"}


class ExportError(ValueError):
    """The local metadata cannot be exported safely and truthfully."""


def _read(path: Path) -> tuple[dict, str]:
    if path.is_symlink() or path.stat().st_size > MAX_METADATA_BYTES:
        raise ExportError("Source metadata must be a regular bounded file")
    raw = path.read_bytes()
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise ExportError("Source metadata must be a JSON object")
    return value, hashlib.sha256(raw).hexdigest()


def _self_href(document: dict) -> str:
    for link in document.get("links", []):
        if link.get("rel") == "self":
            href = link.get("href")
            parsed = urlsplit(href) if isinstance(href, str) else None
            if parsed and parsed.scheme in {"http", "https"} and parsed.netloc and not parsed.username and not parsed.password:
                return href
    raise ExportError("An absolute source self link is required for provenance and relative assets")


def _resolved_web_href(base: str, href: object) -> str:
    if not isinstance(href, str):
        raise ExportError("Retained links and assets require an href")
    resolved = urljoin(base, href)
    parsed = urlsplit(resolved)
    if parsed.scheme not in {"http", "https"} or not parsed.netloc or parsed.username or parsed.password:
        raise ExportError("Retained links and assets require an absolute HTTP(S) href without credentials")
    return resolved


def _portable_copy(document: dict, omit_item_index: bool = False) -> tuple[dict, str]:
    result = copy.deepcopy(document)
    base = _self_href(document)
    result["links"] = []
    for link in document.get("links", []):
        if link.get("rel") in NAVIGATION:
            continue
        retained = copy.deepcopy(link)
        retained["href"] = _resolved_web_href(base, retained.get("href"))
        result["links"].append(retained)
    if omit_item_index:
        result["assets"] = {
            key: asset for key, asset in result.get("assets", {}).items()
            if not (isinstance(asset, dict) and asset.get("roles") == ["stac-items"])
        }
    for asset in result.get("assets", {}).values():
        if not isinstance(asset, dict) or not isinstance(asset.get("href"), str):
            raise ExportError("Every asset requires an href")
        asset["href"] = _resolved_web_href(base, asset["href"])
    return result, base


def _timestamp(value: object) -> datetime:
    if not isinstance(value, str):
        raise ExportError("Selected items need a datetime or a complete temporal interval")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ExportError("Invalid item timestamp") from error
    if parsed.tzinfo is None:
        raise ExportError("Item timestamps require a timezone")
    return parsed.astimezone(timezone.utc)


def _extent(items: list[dict]) -> dict:
    boxes, starts, ends = [], [], []
    for item in items:
        bbox = item.get("bbox")
        if not isinstance(bbox, list) or len(bbox) != 4 or not all(
            isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) for value in bbox
        ):
            raise ExportError("Selected items require a finite two-dimensional bbox")
        west, south, east, north = bbox
        if not (-180 <= west <= east <= 180 and -90 <= south <= north <= 90):
            raise ExportError("Unsupported item bbox; wrapped or invalid extents cannot be exported")
        boxes.append(bbox)
        properties = item.get("properties", {})
        instant = properties.get("datetime")
        start = _timestamp(instant if instant is not None else properties.get("start_datetime"))
        end = _timestamp(instant if instant is not None else properties.get("end_datetime"))
        if end < start:
            raise ExportError("Item temporal interval is reversed")
        starts.append(start)
        ends.append(end)
    return {
        "spatial": {"bbox": [[min(box[0] for box in boxes), min(box[1] for box in boxes), max(box[2] for box in boxes), max(box[3] for box in boxes)]]},
        "temporal": {"interval": [[min(starts).isoformat().replace("+00:00", "Z"), max(ends).isoformat().replace("+00:00", "Z")]]},
    }


def _link(rel: str, href: str, media_type: str = "application/json") -> dict:
    return {"rel": rel, "href": href, "type": media_type}


def export_subset(source: Path, output: Path, selected_ids: list[str] | None = None) -> dict:
    """Export up to 17 local items; refuse existing destinations before writing."""
    if output.exists() or output.is_symlink():
        raise ExportError("Output already exists; choose a new directory")
    collection, collection_hash = _read(source / "collection.json")
    if collection.get("type") != "Collection" or not isinstance(collection.get("id"), str):
        raise ExportError("Source must be a STAC Collection")
    files = []
    for path in (source / "items").glob("*.json"):
        files.append(path)
        if len(files) > MAX_ITEMS:
            raise ExportError("Source exceeds the 17-item demo bound")
    items = {}
    for path in sorted(files):
        item, digest = _read(path)
        item_id = item.get("id")
        if item.get("type") != "Feature" or not isinstance(item_id, str) or not item_id or item_id in items:
            raise ExportError("Source items require unique nonempty string IDs")
        if item.get("collection") != collection["id"]:
            raise ExportError("Source item belongs to another collection")
        items[item_id] = (item, path.relative_to(source).as_posix(), digest)
    selection = sorted(items) if selected_ids is None else sorted(selected_ids)
    if not selection or len(selection) != len(set(selection)) or any(item_id not in items for item_id in selection):
        raise ExportError("Selection must contain known, unique item IDs and must not be empty")
    selected_items = [items[item_id][0] for item_id in selection]
    omitted_collection_assets = [
        {"key": key, "reason": "replaced_by_exported_json_items"}
        for key, asset in sorted(collection.get("assets", {}).items())
        if isinstance(asset, dict) and asset.get("roles") == ["stac-items"]
    ]
    # The exported JSON Item documents replace an upstream collection-level
    # item-index asset (for example an internal GeoParquet catalog snapshot).
    exported_collection, collection_source = _portable_copy(collection, omit_item_index=True)
    exported_collection["extent"] = _extent(selected_items)
    exported_collection["links"].extend([
        _link("root", "catalog.json"), _link("parent", "catalog.json"),
        _link("derived_from", collection_source),
    ])
    catalog = {
        "type": "Catalog", "stac_version": collection["stac_version"],
        "id": f"{collection['id']}-local-subset",
        "description": "Portable metadata-only subset of preserved WorldCover records. Raster assets remain remote; no asset bytes were downloaded.",
        "links": [_link("root", "catalog.json"), _link("child", "collection.json")],
    }
    documents = {"catalog.json": catalog, "collection.json": exported_collection}
    hashes = {"collection.json": collection_hash}
    item_files = {}
    for item_id in selection:
        original, source_file, digest = items[item_id]
        item, original_href = _portable_copy(original)
        filename = "items/" + hashlib.sha256(item_id.encode("utf-8")).hexdigest() + ".json"
        item["links"].extend([
            _link("root", "../catalog.json"), _link("parent", "../collection.json"),
            _link("collection", "../collection.json"), _link("derived_from", original_href, "application/geo+json"),
        ])
        exported_collection["links"].append(_link("item", filename, "application/geo+json"))
        documents[filename] = item
        hashes[source_file] = digest
        item_files[item_id] = filename
    manifest = {
        "format_version": 1, "metadata_only": True, "asset_bytes_downloaded": False,
        "source_collection_id": collection["id"], "source_collection_href": collection_source,
        "selected_item_ids": selection, "item_files": item_files, "source_sha256": hashes,
        "omitted_collection_assets": omitted_collection_assets,
    }
    documents["manifest.json"] = manifest
    # Prepare all payloads before creating the destination; invalid metadata
    # cannot leave a partially validated catalog behind.
    payloads = {name: (json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + "\n").encode("utf-8") for name, value in documents.items()}
    try:
        output.mkdir()
    except FileExistsError as error:
        raise ExportError("Output already exists; choose a new directory") from error
    (output / "items").mkdir()
    for name, payload in payloads.items():
        (output / name).write_bytes(payload)
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=DEFAULT_SOURCE, help="Preserved local metadata directory")
    parser.add_argument("--output", type=Path, required=True, help="New destination directory (parent must exist)")
    parser.add_argument("--item", action="append", dest="items", help="Select an item ID; repeat for a subset, omit for all preserved items")
    args = parser.parse_args(argv)
    try:
        result = export_subset(args.source, args.output, args.items)
    except (ExportError, OSError, ValueError) as error:
        parser.exit(2, f"Export refused: {error}\n")
    print(f"Exported {len(result['selected_item_ids'])} STAC metadata records. Asset bytes were not downloaded.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
