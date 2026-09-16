#!/usr/bin/env python3
"""Overlay a Rome OSM road sample on ESA WorldCover and report corridor classes."""

from __future__ import annotations

import json
import math
import sys
import warnings
from collections import Counter
from pathlib import Path

import numpy as np
import rasterio
from PIL import Image, ImageDraw, ImageFont
from rasterio.features import rasterize
from shapely.geometry import LineString, mapping

warnings.filterwarnings(
    "ignore",
    message="Setting the shape on a NumPy array has been deprecated.*",
    category=DeprecationWarning,
)


ROOT = Path(__file__).resolve().parent
OSM_PATH = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "data/rome-osm-overpass.json"
RASTER_PATH = Path(sys.argv[2]) if len(sys.argv) > 2 else ROOT / "data/rome-worldcover.tif"
OUTPUT_DIR = ROOT / "output"

CLASSES = {
    10: ("Tree cover", "#006400"),
    20: ("Shrubland", "#ffbb22"),
    30: ("Grassland", "#ffff4c"),
    40: ("Cropland", "#f096ff"),
    50: ("Built-up", "#fa0000"),
    60: ("Bare / sparse", "#b4b4b4"),
    70: ("Snow and ice", "#f0f0f0"),
    80: ("Permanent water", "#0064c8"),
    90: ("Herbaceous wetland", "#0096a0"),
    95: ("Mangroves", "#00cf75"),
    100: ("Moss and lichen", "#fae6a0"),
}


def font(size: int, bold: bool = False) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    candidates = [
        Path("/System/Library/Fonts/Supplemental/Arial Bold.ttf" if bold else
             "/System/Library/Fonts/Supplemental/Arial.ttf"),
        Path("/System/Library/Fonts/SFNS.ttf"),
    ]
    for candidate in candidates:
        if candidate.exists():
            return ImageFont.truetype(str(candidate), size)
    return ImageFont.load_default()


def haversine_km(a: tuple[float, float], b: tuple[float, float]) -> float:
    lon1, lat1 = map(math.radians, a)
    lon2, lat2 = map(math.radians, b)
    dlon, dlat = lon2 - lon1, lat2 - lat1
    h = math.sin(dlat / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin(dlon / 2) ** 2
    return 6371.0088 * 2 * math.asin(math.sqrt(h))


def load_roads(path: Path) -> tuple[list[tuple[int, LineString, dict]], float]:
    payload = json.loads(path.read_text())
    roads: list[tuple[int, LineString, dict]] = []
    total_km = 0.0
    for element in payload.get("elements", []):
        coords = [(point["lon"], point["lat"]) for point in element.get("geometry", [])]
        if len(coords) < 2:
            continue
        line = LineString(coords)
        tags = element.get("tags", {})
        roads.append((int(element["id"]), line, tags))
        total_km += sum(haversine_km(a, b) for a, b in zip(coords, coords[1:]))
    return roads, total_km


def colorize(values: np.ndarray) -> np.ndarray:
    rgb = np.full((*values.shape, 3), 238, dtype=np.uint8)
    for code, (_, color) in CLASSES.items():
        rgb[values == code] = tuple(int(color[i:i + 2], 16) for i in (1, 3, 5))
    return rgb


def main() -> None:
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    roads, total_km = load_roads(OSM_PATH)
    if not roads:
        raise SystemExit("No OSM ways found")

    with rasterio.open(RASTER_PATH) as src:
        values = src.read(1)
        transform = src.transform
        crs = str(src.crs)
        bounds = tuple(round(value, 6) for value in src.bounds)

    features = {
        "type": "FeatureCollection",
        "features": [
            {
                "type": "Feature",
                "id": way_id,
                "geometry": mapping(line),
                "properties": {
                    key: tags.get(key)
                    for key in ("highway", "name", "railway")
                    if tags.get(key) is not None
                },
            }
            for way_id, line, tags in roads
        ],
    }
    (OUTPUT_DIR / "rome-roads.geojson").write_text(json.dumps(features) + "\n")

    shapes = [(mapping(line), 1) for _, line, _ in roads]
    road_mask = rasterize(
        shapes,
        out_shape=values.shape,
        transform=transform,
        fill=0,
        all_touched=True,
        dtype="uint8",
    ).astype(bool)

    road_counts = Counter(int(value) for value in values[road_mask] if int(value) in CLASSES)
    area_counts = Counter(int(value) for value in values.ravel() if int(value) in CLASSES)
    road_total = sum(road_counts.values())
    area_total = sum(area_counts.values())

    metrics = {
        "analysis": "ESA WorldCover classes intersected by OSM road pixels",
        "bbox_crs84": bounds,
        "raster_crs": crs,
        "osm_way_count": len(roads),
        "osm_approximate_length_km": round(total_km, 2),
        "road_pixels": road_total,
        "road_corridor_classes_percent": {
            CLASSES[code][0]: round(count * 100 / road_total, 2)
            for code, count in road_counts.most_common()
        },
        "aoi_classes_percent": {
            CLASSES[code][0]: round(count * 100 / area_total, 2)
            for code, count in area_counts.most_common()
        },
        "sources": {
            "vector": "OpenStreetMap contributors via Overpass API",
            "raster": "ESA WorldCover 2021 v200, Sentinel-1/Sentinel-2 derived COG",
        },
    }
    (OUTPUT_DIR / "metrics.json").write_text(json.dumps(metrics, indent=2) + "\n")

    map_image = Image.fromarray(colorize(values), mode="RGB")
    draw = ImageDraw.Draw(map_image, "RGBA")
    inverse = ~transform
    for _, line, tags in roads:
        pixels = [inverse * coordinate for coordinate in line.coords]
        width = 4 if tags.get("highway") in {"primary", "secondary"} else 2
        color = (255, 255, 255, 235) if "highway" in tags else (0, 225, 255, 235)
        draw.line(pixels, fill=color, width=width, joint="curve")

    canvas = Image.new("RGB", (1600, 1000), "#07131c")
    canvas.paste(map_image.resize((1180, 885), Image.Resampling.NEAREST), (20, 95))
    panel = ImageDraw.Draw(canvas)
    panel.text((25, 20), "ROME: OSM × COPERNICUS, ON PREMISE", fill="#f4f7f8", font=font(34, True))
    panel.text(
        (25, 61),
        "Road corridors over ESA WorldCover 10 m • reproducible with public data",
        fill="#9fc4d6",
        font=font(20),
    )

    x = 1230
    panel.rounded_rectangle((1215, 95, 1580, 980), radius=18, fill="#102530", outline="#2f596a", width=2)
    panel.text((x, 120), "ANALYSIS", fill="#7dd3fc", font=font(24, True))
    panel.text((x, 165), f"{len(roads):,} OSM ways", fill="white", font=font(22, True))
    panel.text((x, 198), f"{total_km:,.1f} km mapped", fill="#c8dce5", font=font(18))
    panel.text((x, 230), f"{road_total:,} raster pixels crossed", fill="#c8dce5", font=font(18))

    panel.text((x, 285), "ROAD-CORRIDOR CLASSES", fill="#7dd3fc", font=font(17, True))
    y = 325
    for code, count in road_counts.most_common(7):
        name, color = CLASSES[code]
        pct = count * 100 / road_total
        panel.rectangle((x, y + 2, x + 18, y + 20), fill=color)
        panel.text((x + 28, y), f"{name}", fill="#edf5f7", font=font(16))
        panel.text((1535, y), f"{pct:4.1f}%", anchor="ra", fill="#edf5f7", font=font(16, True))
        y += 36

    built_pct = road_counts.get(50, 0) * 100 / road_total
    green_pct = sum(road_counts.get(code, 0) for code in (10, 20, 30, 40, 90)) * 100 / road_total
    panel.text((x, 620), "QUICK SIGNAL", fill="#7dd3fc", font=font(17, True))
    panel.text((x, 662), f"{built_pct:.1f}% built-up", fill="#ff8d8d", font=font(25, True))
    panel.text((x, 700), f"{green_pct:.1f}% vegetated", fill="#8fe388", font=font(25, True))
    panel.multiline_text(
        (x, 755),
        "Interpretation:\nland-cover context directly\nunder mapped transport lines.\nIt is a demo signal, not a\npolicy or causal conclusion.",
        fill="#c8dce5",
        font=font(17),
        spacing=7,
    )
    panel.text((30, 934), "© OpenStreetMap contributors", fill="#e8f0f2", font=font(12))
    panel.text(
        (30, 950),
        "© ESA WorldCover project 2021 / Contains modified Copernicus Sentinel data (2021)",
        fill="#e8f0f2",
        font=font(11),
    )
    panel.text(
        (30, 965),
        "processed by ESA WorldCover consortium",
        fill="#e8f0f2",
        font=font(11),
    )
    panel.text((1170, 948), "Tellurion v0.2 • local-first demo", anchor="ra", fill="#e8f0f2", font=font(15))

    output = OUTPUT_DIR / "italy-osm-copernicus-analysis.png"
    canvas.save(output, optimize=True)
    print(json.dumps({"screenshot": str(output), **metrics}, indent=2))


if __name__ == "__main__":
    main()
