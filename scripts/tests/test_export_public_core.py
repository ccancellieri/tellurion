#!/usr/bin/env python3
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
EXPORTER_PATH = REPOSITORY_ROOT / "scripts" / "export-public-core.py"


def load_exporter():
    spec = importlib.util.spec_from_file_location("export_public_core", EXPORTER_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load public-core exporter")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class ExportPublicCoreTests(unittest.TestCase):
    def setUp(self):
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.tempdir = Path(self.temporary_directory.name)
        self.exporter = load_exporter()

    def tearDown(self):
        self.temporary_directory.cleanup()

    def policy(self):
        return self.exporter.ExportPolicy(
            root_files=frozenset({"Cargo.toml"}),
            include_directories=("scripts",),
            exclude_globs=("bench/**",),
        )

    def make_repo(self):
        source = self.tempdir / "source"
        source.mkdir()
        self.run_git(source, "init")
        self.run_git(source, "config", "user.name", "Test User")
        self.run_git(source, "config", "user.email", "test@example.invalid")
        self.write(source / "Cargo.toml", "[package]\nname = 'public-core'\n")
        self.write(source / "bench" / "private.md", "do not export\n")
        self.write(source / "scripts" / "runner.py", "#!/usr/bin/env python3\n")
        (source / "scripts" / "runner.py").chmod(0o755)
        self.write(source / "unlisted-root.txt", "do not export\n")
        self.write(source / "untracked-token.txt", "example-secret-value\n")
        self.run_git(source, "add", "Cargo.toml", "bench/private.md", "scripts/runner.py", "unlisted-root.txt")
        self.run_git(source, "commit", "-m", "fixture")
        return source

    def test_export_copies_only_tracked_allowlisted_files(self):
        source = self.make_repo()
        destination = self.tempdir / "candidate"

        manifest = self.exporter.export_repository(source, destination, self.policy())

        self.assertTrue((destination / "Cargo.toml").is_file())
        self.assertTrue((destination / "scripts" / "runner.py").is_file())
        self.assertFalse((destination / "bench/private.md").exists())
        self.assertFalse((destination / "untracked-token.txt").exists())
        self.assertFalse((destination / "unlisted-root.txt").exists())
        self.assertFalse((destination / ".git").exists())
        self.assertTrue(os.access(destination / "scripts" / "runner.py", os.X_OK))
        rendered = json.dumps(manifest.as_dict(), sort_keys=True)
        self.assertNotIn("example-secret-value", rendered)
        self.assertNotIn("Test User", rendered)
        self.assertNotIn("test@example.invalid", rendered)

    def test_export_refuses_nonempty_or_nested_destination(self):
        source = self.make_repo()
        nested_destination = source / "public"
        with self.assertRaisesRegex(ValueError, "empty and outside the source"):
            self.exporter.export_repository(source, nested_destination, self.policy())

        destination = self.tempdir / "nonempty"
        destination.mkdir()
        self.write(destination / "existing.txt", "keep\n")
        with self.assertRaisesRegex(ValueError, "empty and outside the source"):
            self.exporter.export_repository(source, destination, self.policy())

        regular_file_destination = self.tempdir / "regular-file"
        self.write(regular_file_destination, "keep\n")
        with self.assertRaisesRegex(ValueError, "empty and outside the source"):
            self.exporter.export_repository(
                source, regular_file_destination, self.policy()
            )

    def test_committed_policy_exports_governance_and_excludes_private_roots(self):
        policy = self.exporter.load_policy(
            REPOSITORY_ROOT / "distribution" / "public-core.toml"
        )
        destination = self.tempdir / "committed-policy-candidate"

        manifest = self.exporter.export_repository(
            REPOSITORY_ROOT, destination, policy, allow_dirty=True
        )

        mandatory_governance = (
            "LICENSE",
            "COPYRIGHT.md",
            "COMMERCIAL-LICENSE.md",
            "CLA.md",
            "CONTRIBUTING.md",
            "SECURITY.md",
            "SUPPORT.md",
            ".github/CODEOWNERS",
            ".github/dependabot.yml",
            ".github/ISSUE_TEMPLATE/bug.yml",
            ".github/ISSUE_TEMPLATE/evaluation.yml",
            "render.yaml",
            "demo/public-demo.yaml",
            "demo/sources/README.md",
            "demo/sources/public-examples.yaml",
            "bench/data/README.md",
            "bench/data/ingest-osm.sh",
            "bench/data/prepare.sh",
            "bench/data/profiles.env",
            "bench/lib/tilewalk.awk",
            "bench/load_shed.sh",
            "bench/mesh_limits.sh",
            "bench/scenarios.sh",
            "bench/summarize.sh",
            "distribution/public-core.toml",
            "docs/benchmarking.md",
            "docs/publication-runbook.md",
        )
        for relative_path in mandatory_governance:
            with self.subTest(relative_path=relative_path):
                self.assertTrue((destination / relative_path).is_file())
                self.assertIn(relative_path, manifest.copied_paths)
        self.assertFalse((destination / "docs" / "design").exists())
        self.assertFalse((destination / "bench" / "compare").exists())
        self.assertFalse((destination / "bench" / "spike-131-materialization").exists())
        if (REPOSITORY_ROOT / "docs" / "design").exists():
            self.assertTrue(
                any(path.startswith("docs/design/") for path in manifest.excluded_paths)
            )
        if (REPOSITORY_ROOT / "bench" / "README.md").exists():
            self.assertIn("bench/README.md", manifest.excluded_paths)
        self.assertEqual(manifest.bare_issue_references, 0)

    def test_committed_policy_candidate_passes_license_audit(self):
        policy = self.exporter.load_policy(
            REPOSITORY_ROOT / "distribution" / "public-core.toml"
        )
        destination = self.tempdir / "audited-policy-candidate"
        self.exporter.export_repository(
            REPOSITORY_ROOT, destination, policy, allow_dirty=True
        )
        self.run_git(destination, "init")
        self.run_git(destination, "add", ".")

        result = subprocess.run(
            ["bash", "scripts/audit-license-policy.sh"],
            cwd=destination,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )

        self.assertEqual(result.returncode, 0, result.stdout)

    def test_export_rejects_dirty_tracked_source_by_default(self):
        source = self.make_repo()
        self.write(source / "Cargo.toml", "dirty\n")

        with self.assertRaisesRegex(ValueError, "clean Git worktree"):
            self.exporter.export_repository(source, self.tempdir / "candidate", self.policy())

    def test_export_rejects_unsafe_or_untracked_policy_paths(self):
        source = self.make_repo()
        unsafe_policies = (
            self.exporter.ExportPolicy(frozenset({".git"}), (), ()),
            self.exporter.ExportPolicy(frozenset({"../Cargo.toml"}), (), ()),
            self.exporter.ExportPolicy(frozenset({"/Cargo.toml"}), (), ()),
            self.exporter.ExportPolicy(frozenset({"untracked-token.txt"}), (), ()),
            self.exporter.ExportPolicy(frozenset(), (".",), ()),
        )

        for policy in unsafe_policies:
            with self.subTest(policy=policy):
                with self.assertRaisesRegex(ValueError, "policy"):
                    self.exporter.export_repository(source, self.tempdir / "candidate", policy)

    def test_export_rejects_include_directory_without_tracked_descendant(self):
        source = self.make_repo()
        policy = self.exporter.ExportPolicy(
            root_files=frozenset({"Cargo.toml"}),
            include_directories=("missing",),
            exclude_globs=(),
        )

        with self.assertRaisesRegex(ValueError, "policy.*tracked"):
            self.exporter.export_repository(source, self.tempdir / "candidate", policy)

    def test_export_excludes_descendants_at_any_depth(self):
        source = self.make_repo()
        excluded_paths = (
            "docs/design/nested/evidence.md",
            "ui/node_modules/package/nested/private.js",
            "crates/example/target/debug/build/private.bin",
        )
        for relative_path in excluded_paths:
            self.write(source / relative_path, "private\n")
        self.write(source / "docs" / "public" / "guide.md", "public\n")
        self.run_git(source, "add", "docs", "ui", "crates")
        self.run_git(source, "commit", "-m", "add nested exclusions")
        policy = self.exporter.ExportPolicy(
            root_files=frozenset({"Cargo.toml"}),
            include_directories=("docs", "ui", "crates"),
            exclude_globs=(
                "docs/design/**",
                "**/node_modules/**",
                "**/target/**",
            ),
        )

        manifest = self.exporter.export_repository(
            source, self.tempdir / "candidate", policy
        )

        self.assertTrue((self.tempdir / "candidate/docs/public/guide.md").is_file())
        for relative_path in excluded_paths:
            with self.subTest(relative_path=relative_path):
                self.assertFalse((self.tempdir / "candidate" / relative_path).exists())
                self.assertIn(relative_path, manifest.excluded_paths)

    def test_export_rejects_tracked_symlink_escaping_source(self):
        source = self.make_repo()
        outside = self.tempdir / "outside.py"
        self.write(outside, "outside\n")
        (source / "scripts" / "outside.py").symlink_to(outside)
        self.run_git(source, "add", "scripts/outside.py")
        self.run_git(source, "commit", "-m", "add unsafe symlink")

        with self.assertRaisesRegex(ValueError, "symlink"):
            self.exporter.export_repository(source, self.tempdir / "candidate", self.policy())

    def test_manifest_counts_bare_issue_references_in_exported_markdown(self):
        source = self.make_repo()
        self.write(source / "README.md", "See #123 for setup.\n")
        self.run_git(source, "add", "README.md")
        self.run_git(source, "commit", "-m", "add readme")
        policy = self.exporter.ExportPolicy(
            root_files=frozenset({"Cargo.toml", "README.md"}),
            include_directories=(),
            exclude_globs=(),
        )

        manifest = self.exporter.export_repository(source, self.tempdir / "candidate", policy)

        self.assertEqual(manifest.bare_issue_references, 1)

    def test_cli_rejects_dirty_bypass_option(self):
        source = self.make_repo()

        result = subprocess.run(
            [
                sys.executable,
                str(EXPORTER_PATH),
                "--source",
                str(source),
                "--destination",
                str(self.tempdir / "candidate"),
                "--manifest",
                str(self.tempdir / "manifest.json"),
                "--allow-dirty",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

        self.assertEqual(result.returncode, 2)
        self.assertIn("unrecognized arguments: --allow-dirty", result.stderr)

    @staticmethod
    def write(path, contents):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(contents, encoding="utf-8")

    @staticmethod
    def run_git(source, *arguments):
        subprocess.run(
            ["git", *arguments],
            cwd=source,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
