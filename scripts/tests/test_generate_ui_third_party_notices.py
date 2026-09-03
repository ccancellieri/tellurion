#!/usr/bin/env python3
"""Behavioral tests for the UI third-party notice generator."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).parents[2]
SCRIPT = ROOT / "scripts" / "generate-ui-third-party-notices.py"


class UiThirdPartyNoticeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.lockfile = self.root / "package-lock.json"
        self.package_root = self.root / "node_modules"
        self.operator_bundle = self.root / "operator"
        self.public_demo_bundle = self.root / "public-demo"
        self.fallbacks = self.root / "fallbacks.json"
        self.output = self.root / "THIRD_PARTY_NOTICES.txt"
        self.package_root.mkdir()
        self.operator_bundle.mkdir()
        self.public_demo_bundle.mkdir()
        (self.operator_bundle / "index.html").write_text("operator bundle\n", encoding="utf-8")
        (self.public_demo_bundle / "index.html").write_text("public demo bundle\n", encoding="utf-8")
        self.lockfile.write_text(
            json.dumps(
                {
                    "lockfileVersion": 3,
                    "packages": {
                        "": {"name": "fixture-ui", "version": "0.0.0"},
                        "node_modules/fixture": {
                            "version": "1.0.0",
                            "license": "MIT",
                            "resolved": "https://registry.example.invalid/fixture-1.0.0.tgz",
                            "integrity": "sha512-fixture",
                        },
                        "node_modules/development-only": {
                            "version": "2.0.0",
                            "license": "MIT",
                            "dev": True,
                        },
                    },
                }
            ),
            encoding="utf-8",
        )
        fixture = self.package_root / "fixture"
        fixture.mkdir()
        (fixture / "package.json").write_text(
            json.dumps({"name": "fixture", "version": "1.0.0", "license": "MIT"}),
            encoding="utf-8",
        )

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def run_generator(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--lockfile",
                str(self.lockfile),
                "--package-root",
                str(self.package_root),
                "--operator-bundle",
                str(self.operator_bundle),
                "--public-demo-bundle",
                str(self.public_demo_bundle),
                "--fallbacks",
                str(self.fallbacks),
                "--output",
                str(self.output),
            ],
            text=True,
            capture_output=True,
        )

    def test_missing_reviewed_text_fails_closed(self) -> None:
        self.fallbacks.write_text('{"schema_version": 2, "fallbacks": []}', encoding="utf-8")

        completed = self.run_generator()

        self.assertEqual(2, completed.returncode)
        self.assertEqual("ui third-party notice generation unavailable\n", completed.stderr)
        self.assertFalse(self.output.exists())

    def test_version_pinned_fallback_records_lock_and_bundle_hashes(self) -> None:
        package_metadata = (self.package_root / "fixture" / "package.json").read_bytes()
        fallback_text = "Copyright 2026 Fixture\n\nMIT License\n\nPermission is granted.\n"
        fallback_file = self.root / "fixture-1.0.0.txt"
        fallback_file.write_text(fallback_text, encoding="utf-8")
        self.fallbacks.write_text(
            json.dumps(
                {
                    "schema_version": 2,
                    "fallbacks": [
                        {
                            "name": "fixture",
                            "version": "1.0.0",
                            "source": "https://registry.example.invalid/fixture-1.0.0.tgz",
                            "integrity": "sha512-fixture",
                            "package_json_sha256": hashlib.sha256(package_metadata).hexdigest(),
                            "notice_file": fallback_file.name,
                            "notice_sha256": hashlib.sha256(fallback_text.encode()).hexdigest(),
                        }
                    ],
                }
            ),
            encoding="utf-8",
        )

        first = self.run_generator()
        self.assertEqual(0, first.returncode, first.stderr)
        rendered = self.output.read_text(encoding="utf-8")
        self.assertIn(f"package-lock-sha256: {hashlib.sha256(self.lockfile.read_bytes()).hexdigest()}", rendered)
        self.assertIn("operator-bundle-sha256:", rendered)
        self.assertIn("public-demo-bundle-sha256:", rendered)
        self.assertIn("package: fixture@1.0.0", rendered)
        self.assertIn("source: https://registry.example.invalid/fixture-1.0.0.tgz", rendered)
        self.assertIn(fallback_text, rendered)
        self.assertNotIn('"name": "fixture"', rendered)
        self.assertNotIn("development-only", rendered)

        expected = self.output.read_bytes()
        second = self.run_generator()
        self.assertEqual(0, second.returncode, second.stderr)
        self.assertEqual(expected, self.output.read_bytes())

    def test_legacy_package_licenses_metadata_is_accepted_when_the_lock_omits_license(self) -> None:
        document = json.loads(self.lockfile.read_text(encoding="utf-8"))
        document["packages"]["node_modules/fixture"].pop("license")
        self.lockfile.write_text(json.dumps(document), encoding="utf-8")
        package = self.package_root / "fixture"
        (package / "package.json").write_text(
            json.dumps(
                {
                    "name": "fixture",
                    "version": "1.0.0",
                    "licenses": [{"type": "MIT"}],
                }
            ),
            encoding="utf-8",
        )
        (package / "LICENSE").write_text("Fixture MIT text\n", encoding="utf-8")
        self.fallbacks.write_text('{"schema_version": 2, "fallbacks": []}', encoding="utf-8")

        completed = self.run_generator()

        self.assertEqual(0, completed.returncode, completed.stderr)
        self.assertIn("license-expression: MIT", self.output.read_text(encoding="utf-8"))

    def test_direct_notice_with_contact_address_requires_reviewed_fallback(self) -> None:
        package = self.package_root / "fixture"
        address = "author" + "@example.invalid"
        (package / "LICENSE").write_text(
            f"Copyright Fixture <{address}>\n",
            encoding="utf-8",
        )
        self.fallbacks.write_text('{"schema_version": 2, "fallbacks": []}', encoding="utf-8")

        completed = self.run_generator()

        self.assertEqual(2, completed.returncode)
        self.assertEqual("ui third-party notice generation unavailable\n", completed.stderr)
        self.assertFalse(self.output.exists())

    def test_composite_and_license_requires_reviewed_fallback(self) -> None:
        document = json.loads(self.lockfile.read_text(encoding="utf-8"))
        document["packages"]["node_modules/fixture"]["license"] = "(MIT AND Zlib)"
        self.lockfile.write_text(json.dumps(document), encoding="utf-8")
        package = self.package_root / "fixture"
        (package / "package.json").write_text(
            json.dumps(
                {
                    "name": "fixture",
                    "version": "1.0.0",
                    "license": "(MIT AND Zlib)",
                }
            ),
            encoding="utf-8",
        )
        (package / "LICENSE").write_text("MIT terms only\n", encoding="utf-8")
        fallback_text = "MIT terms\n\nZlib terms\n"
        fallback_file = self.root / "fixture-1.0.0.txt"
        fallback_file.write_text(fallback_text, encoding="utf-8")
        self.fallbacks.write_text(
            json.dumps(
                {
                    "schema_version": 2,
                    "fallbacks": [
                        {
                            "name": "fixture",
                            "version": "1.0.0",
                            "source": "https://registry.example.invalid/fixture-1.0.0.tgz",
                            "integrity": "sha512-fixture",
                            "package_json_sha256": hashlib.sha256(
                                (package / "package.json").read_bytes()
                            ).hexdigest(),
                            "notice_file": fallback_file.name,
                            "notice_sha256": hashlib.sha256(fallback_text.encode()).hexdigest(),
                        }
                    ],
                }
            ),
            encoding="utf-8",
        )

        completed = self.run_generator()

        self.assertEqual(0, completed.returncode, completed.stderr)
        rendered = self.output.read_text(encoding="utf-8")
        self.assertIn("notice-origin: reviewed-fallback", rendered)
        self.assertIn("MIT terms\n\nZlib terms", rendered)
        self.assertNotIn("MIT terms only", rendered)

    def test_notice_text_preserves_upstream_whitespace_and_line_endings(self) -> None:
        package = self.package_root / "fixture"
        (package / "LICENSE").write_bytes(b"Fixture notice   \r\n")
        self.fallbacks.write_text('{"schema_version": 2, "fallbacks": []}', encoding="utf-8")

        completed = self.run_generator()

        self.assertEqual(0, completed.returncode, completed.stderr)
        self.assertIn(b"Fixture notice   \r\n", self.output.read_bytes())


if __name__ == "__main__":
    unittest.main()
