#!/usr/bin/env python3
"""Behavioral tests for dependency and legal-surface publication gates."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).parents[2]
SCRIPT = ROOT / "scripts" / "audit-dependency-licenses.py"
NOTICE_SCRIPT = ROOT / "scripts" / "generate-third-party-notices.py"
SPEC = importlib.util.spec_from_file_location("audit_dependency_licenses", SCRIPT)
assert SPEC and SPEC.loader
audit_dependency_licenses = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = audit_dependency_licenses
SPEC.loader.exec_module(audit_dependency_licenses)


class DependencyLicenseAuditTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    @staticmethod
    def metadata(packages: list[tuple[str, str, str | None, bool]]) -> dict[str, object]:
        rendered_packages = []
        workspace_members = []
        for index, (name, version, license_expression, first_party) in enumerate(packages):
            package_id = f"{name} {version} (path+file:///fixture/{index})"
            rendered_packages.append(
                {
                    "id": package_id,
                    "name": name,
                    "version": version,
                    "license": license_expression,
                }
            )
            if first_party:
                workspace_members.append(package_id)
        return {"packages": rendered_packages, "workspace_members": workspace_members}

    def write_fixture(self, name: str, value: object) -> Path:
        path = self.root / name
        path.write_text(json.dumps(value))
        return path

    def test_missing_and_unavoidable_forbidden_licenses_are_blockers(self) -> None:
        cargo = self.metadata(
            [
                ("permissive", "1.0.0", "MIT OR Apache-2.0", False),
                ("missing", "1.0.0", None, False),
                ("forbidden", "1.0.0", "AGPL-3.0-only", False),
                ("forbidden-and", "1.0.0", "MIT AND AGPL-3.0-only", False),
                ("forbidden-or", "1.0.0", "(Apache-2.0 OR AGPL-3.0-only)", False),
                ("forbidden-as-exception", "1.0.0", "MIT WITH GPL-3.0-only", False),
                (
                    "forbidden-either-way",
                    "1.0.0",
                    "(GPL-3.0-only OR AGPL-3.0-only)",
                    False,
                ),
                ("tellurion-core", "0.4.0", "AGPL-3.0-only", True),
            ]
        )

        report = audit_dependency_licenses.audit_cargo_metadata(cargo)

        self.assertEqual(
            {
                "missing",
                "forbidden",
                "forbidden-and",
                "forbidden-as-exception",
                "forbidden-either-way",
            },
            {item.name for item in report.blockers},
        )
        self.assertEqual(
            {
                "MIT OR Apache-2.0",
                "MIT AND AGPL-3.0-only",
                "(Apache-2.0 OR AGPL-3.0-only)",
                "MIT WITH GPL-3.0-only",
                "(GPL-3.0-only OR AGPL-3.0-only)",
            },
            set(report.compound_expressions),
        )
        self.assertNotIn("tellurion-core", {item.name for item in report.blockers})

    def test_reviewed_override_fills_only_exact_missing_npm_license(self) -> None:
        overrides = audit_dependency_licenses.load_license_overrides(
            ROOT / "scripts" / "dependency-license-overrides.json"
        )

        reviewed = audit_dependency_licenses.audit_npm_sbom(
            {"components": [{"name": "json-bignum", "version": "0.0.3"}]},
            overrides,
        )
        unreviewed = audit_dependency_licenses.audit_npm_sbom(
            {"components": [{"name": "json-bignum", "version": "0.0.4"}]},
            overrides,
        )

        self.assertEqual([], reviewed.blockers)
        self.assertEqual("MIT", reviewed.packages[0].license_expression)
        self.assertEqual("reviewed-override", reviewed.packages[0].license_origin)
        self.assertIn("json-bignum/blob/", reviewed.packages[0].license_evidence or "")
        self.assertIn("CycloneDX", reviewed.packages[0].license_reason or "")
        self.assertEqual(["missing-license"], [item.rule_id for item in unreviewed.blockers])
        self.assertEqual("missing", unreviewed.packages[0].license_origin)
        self.assertIsNone(unreviewed.packages[0].license_evidence)
        self.assertIsNone(unreviewed.packages[0].license_reason)

    def test_reviewed_override_schema_is_exact_and_fail_closed(self) -> None:
        valid = {
            "source": "npm",
            "name": "fixture",
            "version": "1.0.0",
            "license": "MIT",
            "evidence": "https://example.invalid/fixture/LICENSE",
            "reason": "The source SBOM omits the declared licence.",
        }
        invalid_entries = (
            {**valid, "license": "MIT OR"},
            {**valid, "license": "MIT WITH GPL-3.0-only"},
            {**valid, "license": "not-a-real-SPDX-license"},
            {**valid, "license": "GPL-3.0-only"},
            {**valid, "evidence": "http://example.invalid/fixture/LICENSE"},
            {**valid, "evidence": "https://"},
            {**valid, "unexpected": "field"},
        )

        for index, entry in enumerate(invalid_entries):
            with self.subTest(index=index):
                path = self.write_fixture(
                    f"invalid-license-override-{index}.json",
                    {"schema_version": 1, "overrides": [entry]},
                )
                with self.assertRaises(RuntimeError):
                    audit_dependency_licenses.load_license_overrides(path)

        duplicate_path = self.write_fixture(
            "duplicate-license-overrides.json",
            {"schema_version": 1, "overrides": [valid, valid]},
        )
        with self.assertRaises(RuntimeError):
            audit_dependency_licenses.load_license_overrides(duplicate_path)

        invalid_documents = (
            {"schema_version": True, "overrides": []},
            {"schema_version": 1, "overrides": [], "unexpected": "field"},
        )
        for index, document in enumerate(invalid_documents):
            with self.subTest(document=index):
                path = self.write_fixture(f"invalid-override-document-{index}.json", document)
                with self.assertRaises(RuntimeError):
                    audit_dependency_licenses.load_license_overrides(path)

    def test_cli_uses_locked_fixed_commands_when_fixtures_are_not_provided(self) -> None:
        def completed(command: list[str], **_kwargs: object) -> subprocess.CompletedProcess[str]:
            output = {
                tuple(audit_dependency_licenses.CARGO_METADATA_COMMAND): "{}",
                tuple(audit_dependency_licenses.NPM_SBOM_COMMAND): "{}",
                ("cargo", "--version"): "cargo 1.97.1 (fixture)\n",
                ("npm", "--version"): "11.6.0\n",
            }[tuple(command)]
            return subprocess.CompletedProcess(command, 0, stdout=output, stderr="")

        with mock.patch.object(
            audit_dependency_licenses.subprocess, "run", side_effect=completed
        ) as run:
            audit_dependency_licenses.load_source_json(None, None)

        self.assertEqual(
            [
                ["cargo", "metadata", "--locked", "--format-version", "1"],
                [
                    "npm",
                    "--prefix",
                    "ui",
                    "sbom",
                    "--package-lock-only",
                    "--sbom-format",
                    "cyclonedx",
                ],
                ["cargo", "--version"],
                ["npm", "--version"],
            ],
            [call.args[0] for call in run.call_args_list],
        )

    def test_cli_records_actual_sanitized_tool_versions(self) -> None:
        cargo = self.metadata([("safe", "1.0.0", "MIT", False)])
        npm = {"components": [{"name": "frontend", "version": "1.0.0", "license": "MIT"}]}

        def completed(command: list[str], **_kwargs: object) -> subprocess.CompletedProcess[str]:
            outputs = {
                tuple(audit_dependency_licenses.CARGO_METADATA_COMMAND): json.dumps(cargo),
                tuple(audit_dependency_licenses.NPM_SBOM_COMMAND): json.dumps(npm),
                ("cargo", "--version"): "cargo 1.97.1 (fixture 2026-08-27)\n",
                ("npm", "--version"): "11.6.0\n",
            }
            return subprocess.CompletedProcess(command, 0, stdout=outputs[tuple(command)], stderr="")

        with mock.patch.object(
            audit_dependency_licenses.subprocess, "run", side_effect=completed
        ):
            _cargo, _npm, tool_versions = audit_dependency_licenses.load_source_json(None, None)

        self.assertEqual(
            {"cargo": "cargo 1.97.1 (fixture 2026-08-27)", "npm": "11.6.0"},
            tool_versions,
        )
        self.assertNotIn("--locked", json.dumps(tool_versions))
        self.assertNotIn("--package-lock-only", json.dumps(tool_versions))

    def test_cli_splits_redacted_summary_from_optional_private_inventory(self) -> None:
        cargo_fixture = self.write_fixture(
            "cargo-private.json",
            self.metadata([("private-cargo-name", "9.8.7", "MIT OR Apache-2.0", False)]),
        )
        npm_fixture = self.write_fixture(
            "npm-private.json",
            {
                "components": [
                    {
                        "name": "private-npm-name",
                        "version": "6.5.4",
                        "license": "BSD-3-Clause",
                    }
                ]
            },
        )
        summary_output = self.root / "dependency-summary.json"
        inventory_output = self.root / "dependency-inventory.json"

        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--cargo-metadata-fixture",
                str(cargo_fixture),
                "--npm-sbom-fixture",
                str(npm_fixture),
                "--output",
                str(summary_output),
                "--inventory-output",
                str(inventory_output),
            ],
            text=True,
            capture_output=True,
        )

        self.assertEqual(0, completed.returncode, completed.stdout + completed.stderr)
        summary = json.loads(summary_output.read_text())
        inventory = json.loads(inventory_output.read_text())
        rendered_summary = json.dumps(summary, sort_keys=True)
        for private_value in (
            "private-cargo-name",
            "private-npm-name",
            "9.8.7",
            "6.5.4",
            "MIT OR Apache-2.0",
            "BSD-3-Clause",
        ):
            self.assertNotIn(private_value, rendered_summary)
        self.assertEqual("ready", summary["status"])
        self.assertEqual({"cargo": 1, "npm": 1, "total": 2}, summary["dependency_counts"])
        self.assertEqual({"compound-license-expression": 1}, summary["category_counts"])
        self.assertEqual([], summary["rule_ids"])
        self.assertRegex(summary["tool_versions"]["cargo"], r"^cargo [^\n]+$")
        self.assertRegex(summary["tool_versions"]["npm"], r"^[^\n]+$")
        self.assertEqual(
            ["private-cargo-name", "private-npm-name"],
            [package["name"] for package in inventory["packages"]],
        )
        self.assertEqual(1, inventory["schema_version"])

    def test_cli_accepts_json_fixtures_and_writes_redacted_blocker_summary(self) -> None:
        cargo_fixture = self.write_fixture(
            "cargo.json",
            self.metadata([("private-missing-package", "1.0.0", None, False)]),
        )
        npm_fixture = self.write_fixture(
            "npm.json", {"components": [{"name": "frontend", "version": "1.0.0", "licenses": [{"license": {"id": "MIT"}}]}]}
        )
        output = self.root / "dependency-report.json"

        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--cargo-metadata-fixture",
                str(cargo_fixture),
                "--npm-sbom-fixture",
                str(npm_fixture),
                "--output",
                str(output),
            ],
            text=True,
            capture_output=True,
        )

        self.assertEqual(1, completed.returncode)
        self.assertIn("blocked", completed.stdout)
        report = json.loads(output.read_text())
        self.assertEqual("blocked", report["status"])
        self.assertEqual({"missing-license": 1}, report["finding_counts"])
        self.assertEqual(["missing-license"], report["rule_ids"])
        self.assertNotIn("private-missing-package", json.dumps(report))

    def test_cli_rejects_repo_output_when_started_outside_repository(self) -> None:
        cargo_fixture = self.write_fixture(
            "cargo-safe.json", self.metadata([("safe", "1.0.0", "MIT", False)])
        )
        npm_fixture = self.write_fixture(
            "npm-safe.json",
            {"components": [{"name": "frontend", "version": "1.0.0", "license": "MIT"}]},
        )
        output = ROOT / "dependency-report-inside-fixture.json"
        self.addCleanup(output.unlink, missing_ok=True)

        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--cargo-metadata-fixture",
                str(cargo_fixture),
                "--npm-sbom-fixture",
                str(npm_fixture),
                "--output",
                str(output),
            ],
            cwd=self.root,
            text=True,
            capture_output=True,
        )

        self.assertEqual(2, completed.returncode)
        self.assertFalse(output.exists())
        self.assertNotIn("Traceback", completed.stderr)

    def test_malformed_dependency_json_and_schema_return_redacted_exit_two(self) -> None:
        safe_cargo = self.metadata([("safe", "1.0.0", "MIT", False)])
        safe_npm = {"components": [{"name": "frontend", "version": "1.0.0", "license": "MIT"}]}
        cases = (
            ([], safe_npm),
            ({"packages": {}, "workspace_members": []}, safe_npm),
            ({"packages": [{"id": "bad", "name": [], "version": "1.0.0", "license": "MIT"}], "workspace_members": []}, safe_npm),
            (safe_cargo, []),
            (safe_cargo, {"components": {}}),
            (safe_cargo, {"components": [{"name": "bad", "version": "1.0.0", "licenses": {}}]}),
        )
        for index, (cargo_value, npm_value) in enumerate(cases):
            with self.subTest(index=index):
                cargo_fixture = self.write_fixture(f"cargo-malformed-{index}.json", cargo_value)
                npm_fixture = self.write_fixture(f"npm-malformed-{index}.json", npm_value)
                output = self.root / f"malformed-report-{index}.json"
                completed = subprocess.run(
                    [
                        sys.executable,
                        str(SCRIPT),
                        "--cargo-metadata-fixture",
                        str(cargo_fixture),
                        "--npm-sbom-fixture",
                        str(npm_fixture),
                        "--output",
                        str(output),
                    ],
                    text=True,
                    capture_output=True,
                )

                self.assertEqual(2, completed.returncode)
                self.assertNotIn("Traceback", completed.stderr)
                self.assertEqual("dependency audit unavailable\n", completed.stderr)
                self.assertFalse(output.exists())

        invalid_json = self.root / "invalid-sensitive.json"
        invalid_json.write_text('{"attacker@example.invalid": leaked-token}')
        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--cargo-metadata-fixture",
                str(invalid_json),
                "--npm-sbom-fixture",
                str(self.write_fixture("npm-valid.json", safe_npm)),
                "--output",
                str(self.root / "invalid-json-report.json"),
            ],
            text=True,
            capture_output=True,
        )
        self.assertEqual(2, completed.returncode)
        self.assertEqual("dependency audit unavailable\n", completed.stderr)
        self.assertNotIn("attacker@example.invalid", completed.stdout + completed.stderr)
        self.assertNotIn("leaked-token", completed.stdout + completed.stderr)
        self.assertNotIn("Traceback", completed.stderr)

    def test_cargo_schema_requires_member_and_package_identity_strings(self) -> None:
        valid_package = {
            "id": "safe 1.0.0 (registry+fixture)",
            "name": "safe",
            "version": "1.0.0",
            "license": "MIT",
        }
        cargo_cases = (
            {"packages": [valid_package], "workspace_members": [{"secret": "leaked-token"}]},
            {"packages": [{**valid_package, "id": ""}], "workspace_members": []},
            {"packages": [{key: value for key, value in valid_package.items() if key != "id"}], "workspace_members": []},
            {"packages": [{**valid_package, "name": ""}], "workspace_members": []},
            {"packages": [{**valid_package, "version": ""}], "workspace_members": []},
        )
        npm_fixture = self.write_fixture(
            "npm-schema-valid.json", {"components": []}
        )
        for index, cargo_value in enumerate(cargo_cases):
            with self.subTest(index=index):
                output = self.root / f"cargo-schema-report-{index}.json"
                completed = subprocess.run(
                    [
                        sys.executable,
                        str(SCRIPT),
                        "--cargo-metadata-fixture",
                        str(self.write_fixture(f"cargo-schema-{index}.json", cargo_value)),
                        "--npm-sbom-fixture",
                        str(npm_fixture),
                        "--output",
                        str(output),
                    ],
                    text=True,
                    capture_output=True,
                )

                self.assertEqual(2, completed.returncode)
                self.assertEqual("dependency audit unavailable\n", completed.stderr)
                self.assertNotIn("Traceback", completed.stderr)
                self.assertNotIn("leaked-token", completed.stdout + completed.stderr)
                self.assertFalse(output.exists())

    def test_npm_schema_requires_explicit_collection_and_package_identity(self) -> None:
        npm_cases = (
            {},
            {"components": [{"name": "", "version": "1.0.0", "license": "MIT"}]},
            {"components": [{"name": "frontend", "version": "", "license": "MIT"}]},
        )
        cargo_fixture = self.write_fixture(
            "cargo-npm-schema-valid.json", self.metadata([("safe", "1.0.0", "MIT", False)])
        )
        for index, npm_value in enumerate(npm_cases):
            with self.subTest(index=index):
                output = self.root / f"npm-schema-report-{index}.json"
                completed = subprocess.run(
                    [
                        sys.executable,
                        str(SCRIPT),
                        "--cargo-metadata-fixture",
                        str(cargo_fixture),
                        "--npm-sbom-fixture",
                        str(self.write_fixture(f"npm-schema-{index}.json", npm_value)),
                        "--output",
                        str(output),
                    ],
                    text=True,
                    capture_output=True,
                )

                self.assertEqual(2, completed.returncode)
                self.assertEqual("dependency audit unavailable\n", completed.stderr)
                self.assertNotIn("Traceback", completed.stderr)
                self.assertFalse(output.exists())

        valid_empty_output = self.root / "npm-explicit-empty-report.json"
        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--cargo-metadata-fixture",
                str(cargo_fixture),
                "--npm-sbom-fixture",
                str(self.write_fixture("npm-explicit-empty.json", {"components": []})),
                "--output",
                str(valid_empty_output),
            ],
            text=True,
            capture_output=True,
        )
        self.assertEqual(0, completed.returncode)
        self.assertEqual("ready", json.loads(valid_empty_output.read_text())["status"])

    def test_dependency_report_write_error_is_redacted_exit_two(self) -> None:
        cargo_fixture = self.write_fixture(
            "cargo-write.json", self.metadata([("safe", "1.0.0", "MIT", False)])
        )
        npm_fixture = self.write_fixture(
            "npm-write.json",
            {"components": [{"name": "frontend", "version": "1.0.0", "license": "MIT"}]},
        )
        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--cargo-metadata-fixture",
                str(cargo_fixture),
                "--npm-sbom-fixture",
                str(npm_fixture),
                "--output",
                str(self.root),
            ],
            text=True,
            capture_output=True,
        )

        self.assertEqual(2, completed.returncode)
        self.assertEqual("dependency audit unavailable\n", completed.stderr)
        self.assertNotIn("Traceback", completed.stderr)

    def test_notice_generation_is_deterministic_inventory_evidence_only(self) -> None:
        inventory = self.write_fixture(
            "notice-inventory.json",
            {
                "schema_version": 1,
                "packages": [
                    {
                        "name": "zeta-package",
                        "version": "2.0.0",
                        "license_expression": "MIT",
                        "source": "npm",
                    },
                    {
                        "name": "alpha-package",
                        "version": "1.0.0",
                        "license_expression": "Apache-2.0",
                        "source": "cargo",
                    },
                ],
                "tool_versions": {"npm": "11.6.0", "cargo": "cargo 1.97.1"},
            },
        )
        first = self.root / "first" / "THIRD_PARTY_NOTICES.json"
        second = self.root / "second" / "THIRD_PARTY_NOTICES.json"

        for output in (first, second):
            completed = subprocess.run(
                [
                    sys.executable,
                    str(NOTICE_SCRIPT),
                    "--inventory",
                    str(inventory),
                    "--output",
                    str(output),
                ],
                text=True,
                capture_output=True,
            )
            self.assertEqual(0, completed.returncode, completed.stdout + completed.stderr)

        self.assertEqual(first.read_bytes(), second.read_bytes())
        notice = json.loads(first.read_text())
        self.assertEqual(1, notice["schema_version"])
        self.assertEqual(
            ["alpha-package", "zeta-package"],
            [package["name"] for package in notice["packages"]],
        )
        self.assertEqual(
            {"license_expression", "name", "source", "version"},
            set(notice["packages"][0]),
        )
        rendered = json.dumps(notice, sort_keys=True).lower()
        for unsupported_claim in ("approved", "compatible", "counsel", "cleared"):
            self.assertNotIn(unsupported_claim, rendered)


class PublicationLicenseAuditTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        for relative in (
            "Cargo.toml",
            "LICENSE",
            "README.md",
            "docs/licensing.md",
            "docs/maturity.md",
            "docs/quickstart/install.md",
            "COMMERCIAL-LICENSE.md",
            "CLA.md",
        ):
            target = self.root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(ROOT / relative, target)

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def test_consistent_copied_legal_surfaces_pass(self) -> None:
        completed = subprocess.run(
            ["bash", str(ROOT / "scripts" / "audit-publication-license.sh")],
            env={**dict(), "AUDIT_ROOT": str(self.root)},
            text=True,
            capture_output=True,
        )

        self.assertEqual(0, completed.returncode, completed.stdout + completed.stderr)

    def test_mismatched_version_is_a_blocker(self) -> None:
        path = self.root / "COMMERCIAL-LICENSE.md"
        path.write_text(path.read_text().replace("Tellurion 0.4.0", "Tellurion 0.3.0"))

        completed = subprocess.run(
            ["bash", str(ROOT / "scripts" / "audit-publication-license.sh")],
            env={**dict(), "AUDIT_ROOT": str(self.root)},
            text=True,
            capture_output=True,
        )

        self.assertEqual(1, completed.returncode)
        self.assertIn("COMMERCIAL-LICENSE.md", completed.stdout)

    def test_obsolete_business_source_terms_are_a_blocker(self) -> None:
        path = self.root / "README.md"
        path.write_text(path.read_text() + "\nBUSL-1.1\n")

        completed = subprocess.run(
            ["bash", str(ROOT / "scripts" / "audit-publication-license.sh")],
            env={**dict(), "AUDIT_ROOT": str(self.root)},
            text=True,
            capture_output=True,
        )

        self.assertEqual(1, completed.returncode)
        self.assertIn("README.md", completed.stdout)

    def test_truncated_license_text_is_a_blocker(self) -> None:
        path = self.root / "LICENSE"
        path.write_text(
            "GNU AFFERO GENERAL PUBLIC LICENSE\n"
            "Version 3, 19 November 2007\n"
        )

        completed = subprocess.run(
            ["bash", str(ROOT / "scripts" / "audit-publication-license.sh")],
            env={**dict(), "AUDIT_ROOT": str(self.root)},
            text=True,
            capture_output=True,
        )

        self.assertEqual(1, completed.returncode)
        self.assertIn("LICENSE", completed.stdout)


if __name__ == "__main__":
    unittest.main()
