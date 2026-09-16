import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).parents[1] / "deploy" / "render" / "build_stac_italy.py"
SPEC = importlib.util.spec_from_file_location("build_stac_italy", SCRIPT)
build_stac_italy = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(build_stac_italy)


class BuildStacItalyTests(unittest.TestCase):
    def source(self, payload=b"cog-bytes", **overrides):
        source = {
            "id": "ESA_WorldCover_10m_2021_v200_N39E012",
            "href": "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_N39E012_Map.tif",
            "bbox": [12.0, 39.0, 15.0, 42.0],
            "byte_length": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
        }
        source.update(overrides)
        return source

    def write_plan(self, directory, sources):
        path = Path(directory) / "mosaic.json"
        path.write_text(json.dumps({"id": "esa_worldcover_2021_italy", "sources": sources}))
        return path

    def test_downloads_and_verifies_an_exact_official_source(self):
        payload = b"verified-cog"
        with tempfile.TemporaryDirectory() as directory:
            plan = self.write_plan(directory, [self.source(payload)])
            requested = []

            def opener(request, timeout):
                requested.append((request.full_url, timeout))
                return io.BytesIO(payload)

            paths = build_stac_italy.download_sources(
                plan, Path(directory) / "cogs", opener=opener
            )

            self.assertEqual(len(paths), 1)
            self.assertEqual(paths[0].read_bytes(), payload)
            self.assertEqual(requested[0][0], self.source(payload)["href"])

    def test_digest_mismatch_leaves_no_partial_or_final_file(self):
        payload = b"tampered"
        with tempfile.TemporaryDirectory() as directory:
            source = self.source(b"expected")
            plan = self.write_plan(directory, [source])
            output = Path(directory) / "cogs"

            with self.assertRaisesRegex(ValueError, "sha256"):
                build_stac_italy.download_sources(
                    plan, output, opener=lambda request, timeout: io.BytesIO(payload)
                )

            self.assertEqual(list(output.iterdir()), [])

    def test_rejects_non_official_or_signed_source_urls(self):
        bad_urls = [
            "http://esa-worldcover.s3.eu-central-1.amazonaws.com/a.tif",
            "https://example.test/a.tif",
            "https://esa-worldcover.s3.eu-central-1.amazonaws.com/a.tif?sig=secret",
            "https://esa-worldcover.s3.eu-central-1.amazonaws.com/../a.tif",
        ]
        for href in bad_urls:
            with self.subTest(href=href), tempfile.TemporaryDirectory() as directory:
                plan = self.write_plan(directory, [self.source(href=href)])
                with self.assertRaises(ValueError):
                    build_stac_italy.download_sources(plan, Path(directory) / "cogs")

    def test_authors_the_manifest_with_one_source_flag_per_sorted_cog(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sources = [root / "b.tif", root / "a.tif"]
            for source in sources:
                source.write_bytes(b"fixture")
            calls = []

            def runner(command, check):
                calls.append((command, check))

            output = root / "mosaic.yaml"
            build_stac_italy.author_mosaic(
                sources,
                output,
                Path("/usr/local/bin/tellurion-ingest"),
                runner=runner,
            )

            command, check = calls[0]
            self.assertTrue(check)
            self.assertEqual(command[:3], ["/usr/local/bin/tellurion-ingest", "cog", "mosaic"])
            self.assertEqual(
                command[3:7],
                ["--source", str(root / "a.tif"), "--source", str(root / "b.tif")],
            )
            self.assertIn(str(output), command)


if __name__ == "__main__":
    unittest.main()
