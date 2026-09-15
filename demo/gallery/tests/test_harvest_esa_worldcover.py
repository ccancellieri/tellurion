import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.harvest_esa_worldcover import (
    HarvestError,
    fetch_json,
    main,
    validate_and_derive,
    write_artifacts,
)


FIXTURES = Path(__file__).parent / "fixtures" / "stac"


def load(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


class HarvestContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.collection = load("collection.json")
        self.search = load("item-search.json")

    def test_derives_exact_worldcover_palette_and_source_identity(self):
        result = validate_and_derive(self.collection, self.search)
        self.assertEqual(result.item["id"], "ESA_WorldCover_10m_2021_v200_N39E012")
        self.assertEqual(result.footprint["features"][0]["id"], 1)
        self.assertEqual(
            result.footprint["features"][0]["properties"]["source_item_id"],
            result.item["id"],
        )
        self.assertEqual(
            [entry["value"] for entry in result.legend],
            [10, 20, 30, 40, 50, 60, 70, 80, 90, 95, 100],
        )
        self.assertEqual(result.legend[0]["color"], "#006400")

    def test_writes_digest_linked_deterministic_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            derived = validate_and_derive(self.collection, self.search)
            write_artifacts(derived, output, "2026-08-03T00:00:00Z")
            manifest = json.loads((output / "manifest.json").read_text())
            legend = json.loads((output / "legend.json").read_text())
            self.assertEqual(manifest["source"]["item_id"], derived.item["id"])
            self.assertEqual(
                manifest["source"]["asset_media_type"],
                derived.item["assets"]["map"]["type"],
            )
            self.assertEqual(
                manifest["source"]["asset_href"],
                derived.item["assets"]["map"]["href"],
            )
            self.assertRegex(manifest["digests"]["item"], r"^[0-9a-f]{64}$")
            self.assertEqual(
                manifest["served_asset"],
                {
                    "byte_length": 41236803,
                    "href": "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_N39E012_Map.tif",
                    "relationship": "byte-identical-official-mirror",
                    "sha256": "5d951afb19e5fdcb90773bac374b556d425f8945ba4b719114c7f7b03157464a",
                },
            )
            self.assertEqual(
                legend[4],
                {"value": 50, "label": "Built-up", "color": "#FA0000"},
            )
            self.assertTrue((output / "colormap.yaml").read_text().endswith("\n"))

    def test_fixture_cli_is_network_free_and_byte_stable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "first"
            second = root / "second"
            command = [
                "harvest_esa_worldcover.py",
                "--fixture-dir",
                str(FIXTURES),
                "--retrieved-at",
                "2026-08-03T00:00:00Z",
            ]
            with patch(
                "scripts.harvest_esa_worldcover.urlopen",
                side_effect=AssertionError("fixture mode must not call the network"),
            ) as urlopen:
                with patch.object(sys, "argv", [command[0], str(first), *command[1:]]):
                    main()
                with patch.object(sys, "argv", [command[0], str(second), *command[1:]]):
                    main()
            urlopen.assert_not_called()
            self.assertEqual(
                {
                    path.relative_to(first): path.read_bytes()
                    for path in first.rglob("*")
                    if path.is_file()
                },
                {
                    path.relative_to(second): path.read_bytes()
                    for path in second.rglob("*")
                    if path.is_file()
                },
            )

    def test_fetch_json_encodes_query_parameters_with_timeout(self):
        calls = []

        def fake_urlopen(url, timeout):
            calls.append((url, timeout))
            return io.BytesIO(b'{"status": "ok"}')

        with patch("scripts.harvest_esa_worldcover.urlopen", side_effect=fake_urlopen):
            result = fetch_json(
                "https://example.test/search",
                {
                    "bbox": "12.45,41.87,12.55,41.95",
                    "datetime": "2021-01-01T00:00:00Z/2021-12-31T23:59:59Z",
                },
            )

        self.assertEqual(result, {"status": "ok"})
        self.assertEqual(
            calls,
            [
                (
                    "https://example.test/search?bbox=12.45%2C41.87%2C12.55%2C41.95"
                    "&datetime=2021-01-01T00%3A00%3A00Z%2F2021-12-31T23%3A59%3A59Z",
                    30,
                )
            ],
        )

    def test_rejects_an_unexpected_item(self):
        self.search["features"][0]["id"] = "another-item"
        with self.assertRaisesRegex(HarvestError, "unexpected Item"):
            validate_and_derive(self.collection, self.search)

    def test_rejects_a_duplicate_class_value(self):
        classes = self.search["features"][0]["assets"]["map"]["classification:classes"]
        classes[1]["value"] = classes[0]["value"]
        with self.assertRaisesRegex(HarvestError, "duplicate class value"):
            validate_and_derive(self.collection, self.search)

    def test_rejects_a_malformed_color(self):
        classes = self.search["features"][0]["assets"]["map"]["classification:classes"]
        classes[0]["color-hint"] = "green"
        with self.assertRaisesRegex(HarvestError, "color-hint"):
            validate_and_derive(self.collection, self.search)

    def test_rejects_a_valid_format_but_wrong_color(self):
        classes = self.search["features"][0]["assets"]["map"]["classification:classes"]
        classes[0]["color-hint"] = "112233"
        with self.assertRaisesRegex(HarvestError, "unexpected color-hint"):
            validate_and_derive(self.collection, self.search)

    def test_rejects_an_altered_non_empty_description(self):
        classes = self.search["features"][0]["assets"]["map"]["classification:classes"]
        classes[0]["description"] = "Trees"
        with self.assertRaisesRegex(HarvestError, "unexpected classification description"):
            validate_and_derive(self.collection, self.search)

    def test_rejects_a_non_cog_map_asset(self):
        self.search["features"][0]["assets"]["map"]["type"] = "image/tiff"
        with self.assertRaisesRegex(HarvestError, "Cloud Optimized GeoTIFF"):
            validate_and_derive(self.collection, self.search)
