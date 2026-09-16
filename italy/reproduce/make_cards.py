#!/usr/bin/env python3
"""Create one 1200×627 social card per article from the verified analysis image."""

from pathlib import Path

from PIL import Image, ImageDraw, ImageEnhance, ImageFont


ROOT = Path(__file__).resolve().parent
SOURCE = ROOT / "output/italy-osm-copernicus-analysis.png"
CARDS = ROOT / "output/cards"

ARTICLES = [
    ("01", "On-prem is a choice\nworth preserving", "Public data, local control, inspectable evidence."),
    ("02", "Two open layers,\none Rome question", "OSM vectors meet ESA WorldCover at 10 m."),
    ("03", "Remote COG,\nlocal computation", "Read the window—not the whole raster."),
    ("04", "Name the\nanalysis boundary", "Python/Rasterio analysis; Tellurion vector serving."),
    ("05", "What 80.1%\nreally means", "A reproducible observation, not a policy conclusion."),
    ("06", "Reproduce before\nyou believe", "Sources, bounds, code and metrics ship together."),
    ("07", "Different tools,\nfair questions", "Tellurion, QGIS, GeoServer and Esri have different jobs."),
    ("08", "A benchmark is\na contract", "Same inputs, validated outputs, named limits."),
    ("09", "Standards with\nlocal control", "OGC-oriented interfaces over operator-controlled storage."),
    ("10", "Design a better\nnext test", "Run the kit, challenge the assumptions, share evidence."),
]


def font(size: int, bold: bool = False):
    path = Path(
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf"
        if bold
        else "/System/Library/Fonts/Supplemental/Arial.ttf"
    )
    return ImageFont.truetype(str(path), size) if path.exists() else ImageFont.load_default()


def main() -> None:
    CARDS.mkdir(parents=True, exist_ok=True)
    base = Image.open(SOURCE).convert("RGB")
    base = base.resize((1200, 750), Image.Resampling.LANCZOS).crop((0, 61, 1200, 688))
    base = ImageEnhance.Brightness(base).enhance(0.58)

    for number, title, subtitle in ARTICLES:
        card = base.copy()
        draw = ImageDraw.Draw(card, "RGBA")
        draw.rectangle((0, 0, 1200, 627), fill=(2, 15, 23, 65))
        draw.rounded_rectangle((52, 62, 1148, 540), radius=28, fill=(4, 20, 30, 205), outline=(96, 192, 224, 160), width=2)
        draw.text((84, 92), f"{number}/10  TELLURION • ITALY DEMO", fill="#7dd3fc", font=font(24, True))
        draw.multiline_text((84, 175), title, fill="white", font=font(52, True), spacing=10)
        draw.multiline_text((84, 365), subtitle, fill="#c2dce8", font=font(27), spacing=8)
        draw.text((84, 490), "OSM × COPERNICUS • ON PREMISE • REPRODUCIBLE", fill="#86efac", font=font(20, True))
        draw.text((84, 558), "© OpenStreetMap contributors", fill="#d8e5ea", font=font(13))
        draw.text(
            (84, 577),
            "© ESA WorldCover project 2021 / Contains modified Copernicus Sentinel data (2021)",
            fill="#d8e5ea",
            font=font(12),
        )
        draw.text(
            (84, 594),
            "processed by ESA WorldCover consortium",
            fill="#d8e5ea",
            font=font(12),
        )
        card.save(CARDS / f"{number}-tellurion-italy.png", optimize=True)


if __name__ == "__main__":
    main()
