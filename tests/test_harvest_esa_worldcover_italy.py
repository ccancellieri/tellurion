import copy
import hashlib
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.harvest_esa_worldcover_italy import (
    ESA_S3_PREFIX,
    HarvestError,
    fetch_live_sources,
    main,
    validate_and_derive,
    verify_assets,
    write_artifacts,
)


FIXTURES = Path(__file__).parent / "fixtures" / "stac"


def load(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def hrefs(value: object) -> list[str]:
    if isinstance(value, dict):
        return ([value["href"]] if isinstance(value.get("href"), str) else []) + [
            href for child in value.values() for href in hrefs(child)
        ]
    if isinstance(value, list):
        return [href for child in value for href in hrefs(child)]
    return []


class ItalyHarvestContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.collection = load("collection.json")
        self.search = load("item-search-italy.json")
        self.boundary = load("italy-boundary.geojson")

    def test_derives_the_out_of_order_fixture_in_stable_source_order(self):
        result = validate_and_derive(self.collection, self.search, self.boundary)
        self.assertEqual(
            [item["id"] for item in result.items],
            sorted(item["id"] for item in self.search["features"]),
        )
        self.assertTrue(all("_2021_v200_" in item["id"] for item in result.items))
        self.assertEqual(
            [feature["id"] for feature in result.footprints["features"]],
            list(range(1, len(result.items) + 1)),
        )
        self.assertEqual(result.mosaic["id"], "esa_worldcover_2021_italy")
        self.assertEqual(
            [source["href"] for source in result.mosaic["sources"]],
            [
                ESA_S3_PREFIX + item["assets"]["map"]["href"].rsplit("/", 1)[-1]
                for item in result.items
            ],
        )

    def test_rejects_invalid_or_untrusted_source_items(self):
        cases = [
            ("wrong-year", lambda item: item.__setitem__("id", item["id"].replace("2021", "2020")), "2021 v200"),
            ("wrong-version", lambda item: item.__setitem__("id", item["id"].replace("v200", "v100")), "2021 v200"),
            ("missing-map", lambda item: item.__setitem__("assets", {}), "missing map"),
            ("query", lambda item: item["assets"]["map"].__setitem__("href", item["assets"]["map"]["href"] + "?sig=secret"), "query"),
            ("untrusted", lambda item: item["assets"]["map"].__setitem__("href", "https://example.test/map.tif"), "Planetary Computer"),
            ("bbox", lambda item: item.__setitem__("bbox", [12, 40, 11, 41]), "bbox"),
            ("palette", lambda item: item["assets"]["map"]["classification:classes"][0].__setitem__("color-hint", "112233"), "color-hint"),
        ]
        for name, change, message in cases:
            with self.subTest(name=name):
                search = copy.deepcopy(self.search)
                change(search["features"][0])
                with self.assertRaisesRegex(HarvestError, message):
                    validate_and_derive(self.collection, search, self.boundary)

    def test_rejects_duplicate_identity_too_many_items_and_geometry_disjoint_from_italy(self):
        duplicate_id = copy.deepcopy(self.search)
        duplicate_id["features"][1]["id"] = duplicate_id["features"][0]["id"]
        with self.assertRaisesRegex(HarvestError, "duplicate id"):
            validate_and_derive(self.collection, duplicate_id, self.boundary)

        duplicate_href = copy.deepcopy(self.search)
        duplicate_href["features"][1]["assets"]["map"]["href"] = duplicate_href["features"][0]["assets"]["map"]["href"]
        with self.assertRaisesRegex(HarvestError, "duplicate map href"):
            validate_and_derive(self.collection, duplicate_href, self.boundary)

        too_many = copy.deepcopy(self.search)
        too_many["features"] *= 11
        for index, item in enumerate(too_many["features"]):
            item["id"] = item["id"].replace("_Map", f"_{index}_Map")
            item["assets"]["map"]["href"] = item["assets"]["map"]["href"].replace(".tif", f"_{index}.tif")
        with self.assertRaisesRegex(HarvestError, "more than 32"):
            validate_and_derive(self.collection, too_many, self.boundary)

        nonintersecting = copy.deepcopy(self.search)
        nonintersecting["features"][0]["geometry"] = {
            "type": "Polygon",
            "coordinates": [[[80, 80], [81, 80], [81, 81], [80, 81], [80, 80]]],
        }
        with self.assertRaisesRegex(HarvestError, "geometry does not intersect"):
            validate_and_derive(self.collection, nonintersecting, self.boundary)

    def test_rejects_an_item_from_another_collection_or_with_a_different_map_filename_id(self):
        wrong_collection = copy.deepcopy(self.search)
        wrong_collection["features"][0]["collection"] = "unrelated-collection"
        with self.assertRaisesRegex(HarvestError, "collection"):
            validate_and_derive(self.collection, wrong_collection, self.boundary)

        wrong_id = copy.deepcopy(self.search)
        wrong_id["features"][0]["id"] = "ESA_WorldCover_10m_2021_v200_N99E099"
        with self.assertRaisesRegex(HarvestError, "filename"):
            validate_and_derive(self.collection, wrong_id, self.boundary)

    def test_posts_the_complete_pinned_geometry_and_refuses_pagination(self):
        calls = []

        def request_json(url, body):
            calls.append((url, body))
            return self.search

        with patch("scripts.harvest_esa_worldcover_italy.post_json", side_effect=request_json):
            result = fetch_live_sources(self.boundary)
        self.assertEqual(result, self.search)
        self.assertEqual(calls[0][1]["intersects"], self.boundary["geometry"])
        self.assertEqual(calls[0][1]["collections"], ["esa-worldcover"])
        self.assertEqual(calls[0][1]["limit"], 100)

        paged = copy.deepcopy(self.search)
        paged["links"] = [{"rel": "next", "href": "https://planetarycomputer.microsoft.com/next"}]
        with patch("scripts.harvest_esa_worldcover_italy.post_json", return_value=paged):
            with self.assertRaisesRegex(HarvestError, "pagination"):
                fetch_live_sources(self.boundary)

        overflow = copy.deepcopy(self.search)
        overflow["numberMatched"] = 33
        with patch("scripts.harvest_esa_worldcover_italy.post_json", return_value=overflow):
            with self.assertRaisesRegex(HarvestError, "more than 32"):
                fetch_live_sources(self.boundary)

    def test_rejects_nonexact_or_malformed_number_matched(self):
        for number_matched in (4, "3", True, -1):
            with self.subTest(number_matched=number_matched):
                incomplete = copy.deepcopy(self.search)
                incomplete["numberMatched"] = number_matched
                with patch("scripts.harvest_esa_worldcover_italy.post_json", return_value=incomplete):
                    with self.assertRaisesRegex(HarvestError, "numberMatched"):
                        fetch_live_sources(self.boundary)

    def test_full_asset_verification_streams_and_persists_only_stable_hrefs(self):
        payload = b"byte-identical-worldcover-cog"
        signed = "https://pc.example.test/asset.tif?sig=temporary"
        mirror = ESA_S3_PREFIX + "ESA_WorldCover_10m_2021_v200_N33E012_Map.tif"
        item = copy.deepcopy(self.search["features"][0])

        class Response(io.BytesIO):
            def __enter__(self):
                return self

            def __exit__(self, *args):
                return False

        with patch("scripts.harvest_esa_worldcover_italy.sign_asset", return_value=signed), patch(
            "scripts.harvest_esa_worldcover_italy.urlopen", side_effect=lambda url, timeout=30: Response(payload)
        ):
            verified = verify_assets([item])
        self.assertEqual(verified[item["id"]]["byte_length"], len(payload))
        self.assertEqual(verified[item["id"]]["sha256"], hashlib.sha256(payload).hexdigest())
        self.assertEqual(verified[item["id"]]["official_esa_href"], mirror)
        self.assertNotIn("?", json.dumps(verified))

    def test_full_asset_verification_rejects_a_mirror_digest_or_length_mismatch(self):
        item = copy.deepcopy(self.search["features"][0])

        class Response(io.BytesIO):
            def __enter__(self):
                return self

            def __exit__(self, *args):
                return False

        payloads = iter((b"planetary-computer", b"official-esa-mirror"))
        with patch("scripts.harvest_esa_worldcover_italy.sign_asset", return_value="https://pc.example.test/asset.tif?sig=temporary"), patch(
            "scripts.harvest_esa_worldcover_italy.urlopen", side_effect=lambda url, timeout=30: Response(next(payloads))
        ):
            with self.assertRaisesRegex(HarvestError, "not byte-identical"):
                verify_assets([item])

    def test_production_requires_matching_verified_manifest_and_writes_atomic_safe_artifacts(self):
        derived = validate_and_derive(self.collection, self.search, self.boundary)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "release"
            with self.assertRaisesRegex(HarvestError, "verified"):
                write_artifacts(derived, output, "2026-08-05T00:00:00Z")

            verified = {
                item["id"]: {
                    "source_href": item["assets"]["map"]["href"],
                    "official_esa_href": source["href"],
                    "byte_length": 1,
                    "sha256": "a" * 64,
                }
                for item, source in zip(derived.items, derived.mosaic["sources"])
            }
            mismatch = copy.deepcopy(verified)
            mismatch[derived.items[0]["id"]]["official_esa_href"] = "https://esa-worldcover.s3.eu-central-1.amazonaws.com/other.tif"
            with self.assertRaisesRegex(HarvestError, "does not match"):
                write_artifacts(derived, output, "2026-08-05T00:00:00Z", mismatch)
            write_artifacts(derived, output, "2026-08-05T00:00:00Z", verified)
            mosaic = json.loads((output / "mosaic.json").read_text())
            manifest = json.loads((output / "manifest.json").read_text())
            self.assertEqual(set(mosaic), {"id", "sources"})
            self.assertTrue(all("?" not in source["href"] for source in mosaic["sources"]))
            self.assertNotIn(
                "?",
                (output / f"items/{derived.items[0]['id']}.json").read_text(),
            )
            self.assertEqual(manifest["digests"]["mosaic"], hashlib.sha256((output / "mosaic.json").read_bytes()).hexdigest())

    def test_manifest_digests_verify_sanitized_snapshots_and_retain_raw_upstream_digests(self):
        collection = copy.deepcopy(self.collection)
        collection["links"].append({"rel": "preview", "href": "https://example.test/preview?sig=temporary"})
        search = copy.deepcopy(self.search)
        search["features"][0]["links"] = [{"rel": "self", "href": "relative/item?sig=temporary"}]
        derived = validate_and_derive(collection, search, self.boundary)

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "release"
            write_artifacts(derived, output, "2026-08-05T00:00:00Z", fixture_mode=True)
            manifest = json.loads((output / "manifest.json").read_text())

            self.assertEqual(
                manifest["digests"]["collection"],
                hashlib.sha256((output / "collection.json").read_bytes()).hexdigest(),
            )
            self.assertEqual(
                manifest["digests"]["upstream_collection"],
                hashlib.sha256((json.dumps(collection, indent=2, sort_keys=True) + "\n").encode("utf-8")).hexdigest(),
            )
            for item in search["features"]:
                with self.subTest(item_id=item["id"]):
                    self.assertEqual(
                        manifest["digests"]["items"][item["id"]],
                        hashlib.sha256((output / f"items/{item['id']}.json").read_bytes()).hexdigest(),
                    )
                    self.assertEqual(
                        manifest["digests"]["upstream_items"][item["id"]],
                        hashlib.sha256((json.dumps(item, indent=2, sort_keys=True) + "\n").encode("utf-8")).hexdigest(),
                    )

    def test_committed_release_item_digests_verify_all_snapshots_and_name_upstream_digests(self):
        release = Path(__file__).parents[1] / "data/stac/esa-worldcover-italy"
        manifest = json.loads((release / "manifest.json").read_text())
        item_ids = {path.stem for path in (release / "items").glob("*.json")}

        self.assertEqual(set(manifest["digests"]["items"]), item_ids)
        self.assertEqual(set(manifest["digests"]["upstream_items"]), item_ids)
        self.assertIn("upstream_collection", manifest["digests"])
        for item_id in sorted(item_ids):
            with self.subTest(item_id=item_id):
                self.assertEqual(
                    manifest["digests"]["items"][item_id],
                    hashlib.sha256((release / f"items/{item_id}.json").read_bytes()).hexdigest(),
                )

    def test_rejects_reuse_records_that_are_not_exact_positive_lowercase_sha256_records(self):
        derived = validate_and_derive(self.collection, self.search, self.boundary)
        verified = {
            item["id"]: {
                "source_href": item["assets"]["map"]["href"],
                "official_esa_href": source["href"],
                "byte_length": 1,
                "sha256": "a" * 64,
            }
            for item, source in zip(derived.items, derived.mosaic["sources"])
        }
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "release"
            extra = copy.deepcopy(verified)
            extra["unexpected-source"] = copy.deepcopy(next(iter(verified.values())))
            with self.assertRaisesRegex(HarvestError, "record ids"):
                write_artifacts(derived, output, "2026-08-05T00:00:00Z", extra)

            for digest in ("A" * 64, "z" * 64):
                with self.subTest(digest=digest[:1]):
                    invalid_digest = copy.deepcopy(verified)
                    invalid_digest[derived.items[0]["id"]]["sha256"] = digest
                    with self.assertRaisesRegex(HarvestError, "digest"):
                        write_artifacts(derived, output, "2026-08-05T00:00:00Z", invalid_digest)

            zero_length = copy.deepcopy(verified)
            zero_length[derived.items[0]["id"]]["byte_length"] = 0
            with self.assertRaisesRegex(HarvestError, "byte length"):
                write_artifacts(derived, output, "2026-08-05T00:00:00Z", zero_length)

    def test_omits_query_and_fragment_link_records_from_every_generated_artifact(self):
        collection = copy.deepcopy(self.collection)
        volatile_hrefs = (
            "https://example.test/collection?sig=secret#fragment",
            "relative/collection?sig=secret",
            "relative/collection#fragment",
            "?sig=secret",
            "#fragment",
            "#abc",
            "#006400",
            "#12345678",
            "relative/collection?sig=secret ",
        )
        collection["links"].extend({"rel": "self", "href": href} for href in volatile_hrefs)
        derived = validate_and_derive(collection, self.search, self.boundary)

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "release"
            write_artifacts(derived, output, "2026-08-05T00:00:00Z", fixture_mode=True)
            artifacts = [json.loads(path.read_text()) for path in output.rglob("*") if path.is_file()]
            persisted_hrefs = [href for artifact in artifacts for href in hrefs(artifact)]
            self.assertTrue(all(href not in volatile_hrefs for href in persisted_hrefs))
            persisted_collection = json.loads((output / "collection.json").read_text())
            self.assertFalse(any(link.get("href") in volatile_hrefs for link in persisted_collection["links"]))
            self.assertNotIn("https://example.test/collection", persisted_hrefs)

    def test_rejects_query_and_fragment_bearing_scalar_boundary_provenance_url(self):
        for source_url in (
            "https://example.test/boundary?sig=secret#fragment",
            "relative/collection?sig=secret",
            "relative/collection#fragment",
            "?sig=secret",
            "#fragment",
            "#abc",
            "#006400",
            "#12345678",
            "relative/collection?sig=secret ",
        ):
            with self.subTest(source_url=source_url):
                boundary = copy.deepcopy(self.boundary)
                boundary["properties"]["gisco:source_url"] = source_url
                with self.assertRaisesRegex(HarvestError, "source URL"):
                    validate_and_derive(self.collection, self.search, boundary)

    def test_requires_the_pinned_absolute_gisco_source_url(self):
        for source_url in (
            "http://gisco-services.ec.europa.eu/distribution/v2/countries/geojson/CNTR_RG_01M_2024_4326.geojson",
            "https://example.test/distribution/v2/countries/geojson/CNTR_RG_01M_2024_4326.geojson",
            "https://gisco-services.ec.europa.eu/distribution/v2/countries/geojson/other.geojson",
            "relative/italy-boundary.geojson",
        ):
            with self.subTest(source_url=source_url):
                boundary = copy.deepcopy(self.boundary)
                boundary["properties"]["gisco:source_url"] = source_url
                with self.assertRaisesRegex(HarvestError, "source URL"):
                    validate_and_derive(self.collection, self.search, boundary)

    def test_preserves_legend_colours_and_prose_bytes(self):
        boundary = copy.deepcopy(self.boundary)
        boundary["properties"]["gisco:terms"] = "Non-commercial terms? See #attribution."
        derived = validate_and_derive(self.collection, self.search, boundary)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "release"
            write_artifacts(derived, output, "2026-08-05T00:00:00Z", fixture_mode=True)
            self.assertEqual(
                (output / "boundary.geojson").read_bytes(),
                (json.dumps(boundary, indent=2, sort_keys=True) + "\n").encode("utf-8"),
            )
            self.assertEqual(
                (output / "legend.json").read_bytes(),
                (json.dumps(derived.legend, indent=2, sort_keys=True) + "\n").encode("utf-8"),
            )

    def test_fixture_cli_is_network_free_and_byte_stable(self):
        with tempfile.TemporaryDirectory() as directory:
            first = Path(directory) / "first"
            second = Path(directory) / "second"
            arguments = [
                "harvest_esa_worldcover_italy.py", str(first), "--fixture-dir", str(FIXTURES),
                "--retrieved-at", "2026-08-05T00:00:00Z",
            ]
            with patch("scripts.harvest_esa_worldcover_italy.urlopen", side_effect=AssertionError("fixture mode must be network-free")):
                with patch.object(sys, "argv", arguments):
                    main()
                arguments[1] = str(second)
                with patch.object(sys, "argv", arguments):
                    main()
            self.assertEqual(
                {path.relative_to(first): path.read_bytes() for path in first.rglob("*") if path.is_file()},
                {path.relative_to(second): path.read_bytes() for path in second.rglob("*") if path.is_file()},
            )

    def test_direct_script_execution_can_import_the_shared_palette_validator(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(
                [
                    sys.executable,
                    str(Path(__file__).parents[1] / "scripts/harvest_esa_worldcover_italy.py"),
                    str(Path(directory) / "output"),
                    "--fixture-dir",
                    str(FIXTURES),
                    "--retrieved-at",
                    "2026-08-05T00:00:00Z",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
