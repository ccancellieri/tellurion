#!/usr/bin/env python3
"""Create deterministic synthetic footprints for the 3D capability demo."""

from __future__ import annotations

import json
import pathlib
import sys


def footprint(lon: float, lat: float, width: float, depth: float) -> list[list[float]]:
    return [
        [lon, lat],
        [lon + width, lat],
        [lon + width, lat + depth],
        [lon, lat + depth],
        [lon, lat],
    ]


def build_fixture() -> dict[str, object]:
    features: list[dict[str, object]] = []
    for row in range(6):
        for column in range(5):
            index = row * 5 + column
            lon = 0.0100 + column * 0.0030 + (row % 2) * 0.00035
            lat = 0.0070 + row * 0.00235
            width = 0.00165 + (index % 3) * 0.00012
            depth = 0.00125 + (index % 4) * 0.00008
            height = 12.0 + float((index * 7) % 44)
            min_height = 4.0 if index % 7 == 0 else 0.0
            features.append(
                {
                    "type": "Feature",
                    "id": index + 1,
                    "properties": {
                        "name": f"Synthetic block {index + 1:02d}",
                        "height": height,
                        "min_height": min_height,
                    },
                    "geometry": {
                        "type": "Polygon",
                        "coordinates": [footprint(lon, lat, width, depth)],
                    },
                }
            )
    return {
        "type": "FeatureCollection",
        "demo:synthetic": True,
        "demo:notice": "Synthetic capability fixture; not observational data.",
        "features": features,
    }


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: build_3d_demo.py OUTPUT_GEOJSON")
    destination = pathlib.Path(sys.argv[1])
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(
        json.dumps(build_fixture(), sort_keys=True, separators=(",", ":")) + "\n"
    )


if __name__ == "__main__":
    main()
