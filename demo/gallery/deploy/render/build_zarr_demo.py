#!/usr/bin/env python3
"""Build two deterministic, dependency-free Zarr v2 capability fixtures."""

from __future__ import annotations

import json
import math
import pathlib
import sys


WIDTH = 256
HEIGHT = 256
CHUNK = 64
EXTENT_CRS84 = [0.005, 0.005, 0.035, 0.025]


def sample(time_index: int, y: int, x: int) -> int:
    """Return a synthetic value; it is not an environmental observation."""
    u = (x - (WIDTH - 1) / 2) / ((WIDTH - 1) / 2)
    v = (y - (HEIGHT - 1) / 2) / ((HEIGHT - 1) / 2)
    center_x = -0.22 + 0.36 * time_index
    center_y = 0.08 - 0.18 * time_index
    hotspot = 150.0 * math.exp(
        -((u - center_x) ** 2 + (v - center_y) ** 2) / 0.08
    )
    ridge = 45.0 * (
        math.sin((u * 3.0 + v * 2.0 + time_index * 0.9) * math.pi) + 1.0
    ) / 2.0
    north_south = 55.0 * (1.0 - v) / 2.0
    return max(0, min(255, round(20.0 + north_south + ridge + hotspot)))


def write_json(path: pathlib.Path, value: object) -> None:
    path.write_text(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")


def write_store(root: pathlib.Path, selected_time: int) -> None:
    store = root / f"sample_zarr_t{selected_time}"
    store.mkdir(parents=True, exist_ok=False)
    write_json(
        store / ".zarray",
        {
            "zarr_format": 2,
            "shape": [2, 256, 256],
            "chunks": [1, 64, 64],
            "dtype": "|u1",
            "compressor": None,
            "fill_value": 0,
            "order": "C",
            "filters": None,
            "dimension_separator": ".",
        },
    )
    write_json(
        store / ".zattrs",
        {
            "_ARRAY_DIMENSIONS": ["time", "y", "x"],
            "tellurion:extent_crs84": EXTENT_CRS84,
            "tellurion:fixed_index": [selected_time],
            "demo:synthetic": True,
            "demo:time_values": ["T0", "T1"],
            "demo:notice": "Synthetic capability fixture; not observational data.",
        },
    )
    for time_index in range(2):
        for chunk_y in range(HEIGHT // CHUNK):
            for chunk_x in range(WIDTH // CHUNK):
                payload = bytearray()
                for local_y in range(CHUNK):
                    y = chunk_y * CHUNK + local_y
                    for local_x in range(CHUNK):
                        x = chunk_x * CHUNK + local_x
                        payload.append(sample(time_index, y, x))
                (store / f"{time_index}.{chunk_y}.{chunk_x}").write_bytes(payload)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: build_zarr_demo.py OUTPUT_DIRECTORY")
    output = pathlib.Path(sys.argv[1])
    output.mkdir(parents=True, exist_ok=True)
    write_store(output, selected_time=0)
    write_store(output, selected_time=1)


if __name__ == "__main__":
    main()
