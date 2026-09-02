#!/usr/bin/env python3
"""Render deterministic third-party dependency inventory evidence as JSON."""

from __future__ import annotations

import argparse
from collections.abc import Mapping
import json
from pathlib import Path
import sys
from typing import Any


PACKAGE_FIELDS = ("license_expression", "name", "source", "version")


def render_notice(inventory: Any) -> dict[str, Any]:
    if not isinstance(inventory, Mapping) or inventory.get("schema_version") != 1:
        raise ValueError("unsupported dependency inventory")
    packages = inventory.get("packages")
    if not isinstance(packages, list):
        raise ValueError("unsupported dependency inventory")

    normalized = []
    for package in packages:
        if not isinstance(package, Mapping):
            raise ValueError("unsupported dependency inventory")
        values = {field: package.get(field) for field in PACKAGE_FIELDS}
        if (
            not all(isinstance(values[field], str) and values[field].strip() for field in PACKAGE_FIELDS)
            or values["source"] not in {"cargo", "npm"}
        ):
            raise ValueError("unsupported dependency inventory")
        normalized.append({field: values[field].strip() for field in PACKAGE_FIELDS})
    normalized.sort(
        key=lambda package: (
            package["name"],
            package["version"],
            package["source"],
            package["license_expression"],
        )
    )
    return {"schema_version": 1, "packages": normalized}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        notice = render_notice(json.loads(arguments.inventory.read_text()))
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        arguments.output.write_text(
            json.dumps(notice, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    except (OSError, ValueError, json.JSONDecodeError):
        print("third-party notice generation unavailable", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
