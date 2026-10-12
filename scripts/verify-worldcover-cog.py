import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
import rasterio


parser = argparse.ArgumentParser(description="Compare five WorldCover map tiles with explicitly selected GDAL image directories.")
parser.add_argument("input_directory", type=Path)
parser.add_argument("--output", type=Path)
args = parser.parse_args()
EVIDENCE = args.input_directory.resolve()
SOURCE_WIDTH = 36_000
SOURCE_HEIGHT = 36_000
SOURCE_EXTENT = [12.0, 39.0, 15.0, 42.0]

# The GDAL extracts are deliberately explicit about the source overview.
# Their output dimensions equal that selected IFD's dimensions, so GDAL does
# not create another decimated raster before this independent sampler runs.
LEVELS = {
    36_000: {
        "path": "gdal-main-z14-window.tif",
        "gdal_overview": "NONE",
        "array_origin": [17_894, 17_834],
        "extract_command": (
            "gdal_translate -ovr NONE -srcwin 17894 17834 265 201 "
            "-outsize 265 201 -r nearest"
        ),
    },
    2_250: {
        "path": "gdal-overview-2250.tif",
        "gdal_overview": 3,
        "array_origin": [0, 0],
        "extract_command": (
            "gdal_translate -ovr 3 -srcwin 0 0 36000 36000 "
            "-outsize 2250 2250 -r nearest"
        ),
    },
    562: {
        "path": "gdal-overview-562.tif",
        "gdal_overview": 5,
        "array_origin": [0, 0],
        "extract_command": (
            "gdal_translate -ovr 5 -srcwin 0 0 36000 36000 "
            "-outsize 562 562 -r nearest"
        ),
    },
}

TILES = [
    (14, 8806, 6172, 36_000),
    (10, 550, 385, 2_250),
    (8, 137, 96, 562),
    (8, 136, 96, 562),
    (8, 137, 97, 562),
]


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def source_window(z: int, x: int, y: int, level_size: int) -> list[int]:
    n = 2**z
    west = x / n * 360.0 - 180.0
    east = (x + 1) / n * 360.0 - 180.0
    north = math.degrees(math.atan(math.sinh(math.pi * (1.0 - 2.0 * y / n))))
    south = math.degrees(
        math.atan(math.sinh(math.pi * (1.0 - 2.0 * (y + 1) / n)))
    )
    scale = 3.0 / level_size
    x0 = max(0, math.floor((west - SOURCE_EXTENT[0]) / scale))
    x1 = min(level_size, math.ceil((east - SOURCE_EXTENT[0]) / scale))
    y0 = max(0, math.floor((SOURCE_EXTENT[3] - north) / scale))
    y1 = min(level_size, math.ceil((SOURCE_EXTENT[3] - south) / scale))
    return [x0, y0, x1, y1]


def sample_centers(
    source_classes: np.ndarray,
    palette: np.ndarray,
    array_origin: tuple[int, int],
    z: int,
    x: int,
    y: int,
    level_size: int,
) -> np.ndarray:
    """Implement Tellurion's center-coordinate then floor mapping directly."""
    result = np.zeros((256, 256, 4), dtype=np.uint8)
    matrix_side = 2**z
    scale = 3.0 / level_size
    origin_x, origin_y = array_origin

    for destination_y in range(256):
        tile_row = y + (destination_y + 0.5) / 256.0
        latitude = math.degrees(
            math.atan(math.sinh(math.pi * (1.0 - 2.0 * tile_row / matrix_side)))
        )
        source_y = math.floor((SOURCE_EXTENT[3] - latitude) / scale)
        if source_y < 0 or source_y >= level_size:
            continue
        local_y = source_y - origin_y

        for destination_x in range(256):
            longitude = (
                (x + (destination_x + 0.5) / 256.0) / matrix_side * 360.0
                - 180.0
            )
            source_x = math.floor((longitude - SOURCE_EXTENT[0]) / scale)
            if source_x < 0 or source_x >= level_size:
                continue
            local_x = source_x - origin_x
            result[destination_y, destination_x] = palette[
                source_classes[local_y, local_x]
            ]

    return result


level_data = {}
level_evidence = []
for level_size, config in LEVELS.items():
    path = EVIDENCE / config["path"]
    with rasterio.open(path) as dataset:
        classes = dataset.read(1)
        palette = np.asarray(
            [dataset.colormap(1)[value] for value in range(256)], dtype=np.uint8
        )
        # Tellurion applies the supported scalar nodata value after the source
        # palette. GDAL reports the same nodata value, but its color table still
        # describes entry zero independently.
        palette[int(dataset.nodata), 3] = 0
        level_data[level_size] = (classes, palette, tuple(config["array_origin"]))
        level_evidence.append(
            {
                "level_shape": [level_size, level_size],
                "gdal_overview_ordinal": config["gdal_overview"],
                "extract_command_without_source_or_destination": config[
                    "extract_command"
                ],
                "extract_shape": list(classes.shape[::-1]),
                "array_origin_in_selected_level": config["array_origin"],
                "class_array_sha256": sha256(classes.tobytes()),
                "nodata": int(dataset.nodata),
            }
        )


observations = []
for z, x, y, level_size in TILES:
    classes, palette, array_origin = level_data[level_size]
    expected = sample_centers(
        classes, palette, array_origin, z, x, y, level_size
    )
    actual = np.fromfile(
        EVIDENCE / f"tellurion-z{z}-x{x}-y{y}.rgba", dtype=np.uint8
    ).reshape(256, 256, 4)
    exact = np.all(expected == actual, axis=2)
    alpha_exact = expected[:, :, 3] == actual[:, :, 3]
    observations.append(
        {
            "z": z,
            "x": x,
            "y": y,
            "selected_level_shape": [level_size, level_size],
            "gdal_overview_ordinal": LEVELS[level_size]["gdal_overview"],
            "tellurion_planned_source_window_xyxy": source_window(
                z, x, y, level_size
            ),
            "exact_rgba_pixels": int(exact.sum()),
            "rgba_mismatch_pixels": int((~exact).sum()),
            "alpha_mismatch_pixels": int((~alpha_exact).sum()),
            "oracle_nontransparent_pixels": int((expected[:, :, 3] != 0).sum()),
            "tellurion_nontransparent_pixels": int((actual[:, :, 3] != 0).sum()),
            "oracle_rgba_sha256": sha256(expected.tobytes()),
            "tellurion_rgba_sha256": sha256(actual.tobytes()),
        }
    )


result = {
    "oracle": (
        f"GDAL {rasterio.__gdal_version__} explicit overview extraction plus "
        "independent destination-pixel-center/source-floor sampling"
    ),
    "source_shape": [SOURCE_WIDTH, SOURCE_HEIGHT],
    "source_extent_crs84": SOURCE_EXTENT,
    "destination_shape": [256, 256],
    "selected_levels": level_evidence,
    "tiles": observations,
    "all_rgba_pixels_exact": all(
        observation["rgba_mismatch_pixels"] == 0 for observation in observations
    ),
    "all_alpha_pixels_exact": all(
        observation["alpha_mismatch_pixels"] == 0 for observation in observations
    ),
}

(args.output or EVIDENCE / "selected-overview-oracle.json").write_text(
    json.dumps(result, indent=2) + "\n"
)
print(json.dumps(result, indent=2))

raise SystemExit(0 if result["all_rgba_pixels_exact"] and result["all_alpha_pixels_exact"] else 1)
