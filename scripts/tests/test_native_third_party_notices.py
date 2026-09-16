import importlib.util
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "generate-native-third-party-notices.py"
spec = importlib.util.spec_from_file_location("native_notices", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class NativeNoticesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.registry = self.root / "registry" / "src" / "example"
        self.registry.mkdir(parents=True)

    def package(self, name, package_id, *, workspace=False):
        path = self.root / name if workspace else self.registry / name
        path.mkdir()
        (path / "Cargo.toml").write_text("[package]\nname='x'\nversion='1'\n")
        if not workspace:
            (path / "LICENSE").write_text(f"License for {name}\n")
        return {"id": package_id, "name": name, "version": "1.0.0", "source": None if workspace else "registry+https://example.test", "manifest_path": str(path / "Cargo.toml"), "license": "MIT"}

    def metadata(self):
        server = self.package("tellurion", "server", workspace=True)
        ingest = self.package("tellurion-ingest", "ingest", workspace=True)
        runtime = self.package("runtime", "runtime")
        build = self.package("builddep", "build")
        dev = self.package("devdep", "dev")
        return {"packages": [server, ingest, runtime, build, dev], "resolve": {"nodes": [
            {"id": "server", "deps": [{"pkg": "runtime", "dep_kinds": [{"kind": None, "target": None}]}, {"pkg": "dev", "dep_kinds": [{"kind": "dev", "target": None}]}]},
            {"id": "ingest", "deps": [{"pkg": "build", "dep_kinds": [{"kind": "build", "target": None}]}]},
        ] + [{"id": x, "deps": []} for x in ("runtime", "build", "dev")]}}

    def fallback(self, metadata, **changes):
        package = metadata["packages"][2]
        package["repository"] = "https://github.com/example/runtime"
        (self.registry / "runtime" / ".cargo_vcs_info.json").write_text(json.dumps({"git": {"sha1": "a" * 40}}))
        (self.root / "upstream-license.txt").write_text("Reviewed upstream license\n")
        entry = {"name": package["name"], "version": package["version"], "source": package["source"],
                 "repository": package["repository"], "revision": "a" * 40,
                 "license_expression": package["license"], "files": [{"path": "upstream-license.txt",
                 "sha256": hashlib.sha256(b"Reviewed upstream license\n").hexdigest(),
                 "url": f"https://raw.githubusercontent.com/example/runtime/{'a' * 40}/LICENSE"}]}
        entry.update(changes)
        path = self.root / "fallbacks.json"
        path.write_text(json.dumps({"schema_version": 1, "packages": [entry]}))
        return path

    def test_pinned_fallback_supplies_missing_license_and_records_provenance(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").unlink()
        path = self.fallback(metadata)
        manifest, text = module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)
        self.assertIn("Reviewed upstream license", text)
        files = manifest["packages"][1]["files"]
        self.assertEqual("fallback", files[0]["provenance"])
        self.assertEqual("upstream-license.txt", files[0]["path"])
        self.assertTrue(files[0]["url"].startswith("https://raw.githubusercontent.com/example/runtime/"))

    def test_fallback_preserves_packaged_notice_without_license(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").unlink()
        (self.registry / "runtime" / "NOTICE").write_text("Packaged notice\n")
        path = self.fallback(metadata)
        manifest, text = module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)
        self.assertIn("Packaged notice", text)
        self.assertIn("NOTICE", [f["path"] for f in manifest["packages"][1]["files"]])

    def test_fallback_rejects_wrong_identity_and_revision(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").unlink()
        for change in ({"version": "2.0.0"}, {"source": "registry+wrong"},
                       {"license_expression": "Apache-2.0"}, {"repository": "https://github.com/other/repo"},
                       {"revision": "b" * 40}):
            with self.subTest(change=change):
                path = self.fallback(metadata, **change)
                with self.assertRaisesRegex(ValueError, "fallback package identity mismatch|fallback revision mismatch"):
                    module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)

    def test_fallback_rejects_hash_path_url_and_missing_vcs(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").unlink()
        for file_change in ({"sha256": "0" * 64}, {"path": "../upstream-license.txt"},
                            {"url": "https://raw.githubusercontent.com/other/repo/" + "a" * 40 + "/LICENSE"}):
            with self.subTest(change=file_change):
                path = self.fallback(metadata)
                data = json.loads(path.read_text())
                data["packages"][0]["files"][0].update(file_change)
                path.write_text(json.dumps(data))
                with self.assertRaisesRegex(ValueError, "fallback digest mismatch|unsafe fallback path|invalid fallback URL"):
                    module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)
        path = self.fallback(metadata)
        (self.registry / "runtime" / ".cargo_vcs_info.json").unlink()
        with self.assertRaisesRegex(ValueError, "missing safe Cargo VCS info"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)

    def test_fallback_does_not_hide_empty_packaged_license(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").write_text(" \n")
        path = self.fallback(metadata)
        with self.assertRaisesRegex(ValueError, "empty license text"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)

    def test_fallback_rejects_symlink(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").unlink()
        path = self.fallback(metadata)
        (self.root / "upstream-license.txt").unlink()
        outside = self.registry / "runtime" / "other.txt"
        outside.write_text("not reviewed")
        (self.root / "upstream-license.txt").symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "unsafe"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)

    def test_fallback_rejects_noncanonical_upstream_url(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").unlink()
        path = self.fallback(metadata)
        prefix = f"https://raw.githubusercontent.com/example/runtime/{'a' * 40}/"
        for suffix in ("../LICENSE", "./LICENSE", "%2e%2e/LICENSE", "dir\\LICENSE", "dir//LICENSE", "LICENSE%2Fextra"):
            with self.subTest(suffix=suffix):
                data = json.loads(path.read_text())
                data["packages"][0]["files"][0]["url"] = prefix + suffix
                path.write_text(json.dumps(data))
                with self.assertRaisesRegex(ValueError, "invalid fallback URL"):
                    module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)
                path = self.fallback(metadata)

    def test_fallback_rejects_duplicate_package_and_file(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").unlink()
        path = self.fallback(metadata)
        data = json.loads(path.read_text())
        data["packages"].append(data["packages"][0].copy())
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "duplicate fallback package"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)
        path = self.fallback(metadata)
        data = json.loads(path.read_text())
        data["packages"][0]["files"].append(data["packages"][0]["files"][0].copy())
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "duplicate fallback file"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)

    def test_fallback_rejects_bad_manifest_shape_with_value_error(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").unlink()
        path = self.fallback(metadata)
        for data in ([], {"schema_version": 1, "packages": [None]},
                     {"schema_version": 1, "packages": [{"name": "runtime", "files": None}]}):
            with self.subTest(data=data):
                path.write_text(json.dumps(data))
                with self.assertRaisesRegex(ValueError, "invalid fallback manifest"):
                    module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"), fallbacks_path=path)

    def test_runtime_and_build_included_dev_excluded_deterministically(self):
        metadata = self.metadata()
        manifest, text = module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))
        self.assertEqual([p["name"] for p in manifest["packages"]], ["builddep", "runtime"])
        self.assertIn("License for runtime", text)
        self.assertIn("License for builddep", text)
        self.assertNotIn("License for devdep", text)
        self.assertEqual((manifest, text), module.generate(metadata, self.root, ("tellurion", "tellurion-ingest")))
        self.assertEqual(64, len(manifest["packages"][0]["files"][0]["sha256"]))

    def test_missing_license_fails(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").unlink()
        with self.assertRaisesRegex(ValueError, "license text missing"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))

    def test_empty_license_fails(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").write_text(" \n")
        with self.assertRaisesRegex(ValueError, "empty license"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))

    def test_symlink_escape_fails(self):
        metadata = self.metadata()
        license_file = self.registry / "runtime" / "LICENSE"
        license_file.unlink()
        outside = self.root / "outside"
        outside.write_text("not part of crate")
        license_file.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "unsafe"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))

    def test_unresolved_package_fails(self):
        metadata = self.metadata()
        metadata["packages"] = [p for p in metadata["packages"] if p["id"] != "runtime"]
        with self.assertRaisesRegex(ValueError, "unresolved"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))

    def test_nested_bundled_native_notices_included(self):
        metadata = self.metadata()
        metadata["packages"][2]["name"] = "ring"
        nested = self.registry / "runtime" / "sqlite3" / "vendor"
        nested.mkdir(parents=True)
        (nested / "NOTICE.txt").write_text("Bundled native notice\n")
        manifest, text = module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))
        self.assertIn("Bundled native notice", text)
        self.assertIn("sqlite3/vendor/NOTICE.txt", [f["path"] for f in manifest["packages"][1]["files"]])

    def test_declared_license_file_with_nonstandard_name(self):
        metadata = self.metadata()
        package = metadata["packages"][2]
        package["license_file"] = "terms.txt"
        (self.registry / "runtime" / "LICENSE").unlink()
        (self.registry / "runtime" / "terms.txt").write_text("Unusual license text\n")
        manifest, text = module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))
        self.assertIn("Unusual license text", text)
        self.assertEqual("terms.txt", manifest["packages"][1]["files"][0]["path"])

    def test_zstd_bundled_license_texts_are_not_omitted(self):
        metadata = self.metadata()
        metadata["packages"][2]["name"] = "zstd-sys"
        nested = self.registry / "runtime" / "zstd"
        nested.mkdir()
        (nested / "LICENSE").write_bytes(b"Bundled BSD terms\r\n")
        (nested / "COPYING").write_bytes(b"Bundled GPL alternative\n")
        manifest, text = module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))
        record = next(p for p in manifest["packages"] if p["name"] == "zstd-sys")
        self.assertIn("Bundled BSD terms\r\n", text)
        self.assertIn("Bundled GPL alternative\n", text)
        self.assertIn("zstd/LICENSE", [f["path"] for f in record["files"]])

    def test_sqlite_embedded_disclaimer_is_collected_without_source_body(self):
        metadata = self.metadata()
        metadata["packages"][2]["name"] = "libsqlite3-sys"
        nested = self.registry / "runtime" / "sqlite3"
        nested.mkdir()
        disclaimer = (b"/*\n** 2001 September 15\n**\n"
                      b"** The author disclaims copyright to this source code.  In place of\n"
                      b"** a legal notice, here is a blessing:\n**\n"
                      b"**    May you do good and not evil.\n*/")
        (nested / "sqlite3.c").write_bytes(b"/* amalgamation */\n" + disclaimer + b"\nint not_a_notice;\n")
        manifest, text = module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))
        record = next(p for p in manifest["packages"] if p["name"] == "libsqlite3-sys")
        self.assertIn(disclaimer.decode(), text)
        self.assertNotIn("int not_a_notice", text)
        entry = next(f for f in record["files"] if f["path"] == "sqlite3/sqlite3.c#copyright-disclaimer")
        self.assertEqual(hashlib.sha256(disclaimer).hexdigest(), entry["sha256"])

    def test_sqlite_missing_or_changed_disclaimer_blocks_collection(self):
        metadata = self.metadata()
        metadata["packages"][2]["name"] = "libsqlite3-sys"
        nested = self.registry / "runtime" / "sqlite3"
        nested.mkdir()
        for content in (None, b"/* changed terms */", b"/* The author disclaims copyright to this source code."):
            with self.subTest(content=content):
                if content is not None:
                    (nested / "sqlite3.c").write_bytes(content)
                with self.assertRaisesRegex(ValueError, "SQLite.*disclaimer"):
                    module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))

    def test_nonworkspace_path_dependency_is_not_silently_omitted(self):
        metadata = self.metadata()
        metadata["packages"][2]["source"] = None
        external = tempfile.TemporaryDirectory()
        self.addCleanup(external.cleanup)
        metadata["packages"][2]["manifest_path"] = str(Path(external.name) / "Cargo.toml")
        with self.assertRaisesRegex(ValueError, "outside workspace"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))

    def test_invalid_utf8_fails_instead_of_changing_text(self):
        metadata = self.metadata()
        (self.registry / "runtime" / "LICENSE").write_bytes(b"\xff")
        with self.assertRaisesRegex(ValueError, "UTF-8"):
            module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))

    def test_evidence_binds_target_profile_and_metadata_digest(self):
        metadata = self.metadata()
        manifest, _ = module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"),
                                      target="aarch64-apple-darwin", feature_profile="server=default,ui;ingest=default",
                                      metadata_sha256="a" * 64)
        self.assertEqual("aarch64-apple-darwin", manifest["declared_target"])
        self.assertEqual("server=default,ui;ingest=default", manifest["declared_feature_profile"])
        self.assertEqual("a" * 64, manifest["metadata_sha256"])

    def test_cli_writes_exact_hashed_utf8_bytes(self):
        metadata_path = self.root / "metadata.json"
        metadata_path.write_text(json.dumps(self.metadata()), encoding="utf-8")
        manifest_path = self.root / "manifest.json"
        text_path = self.root / "notice.txt"
        result = subprocess.run([sys.executable, str(SCRIPT), "--metadata", str(metadata_path),
                                 "--target", "aarch64-apple-darwin", "--feature-profile", "server=default,ui;ingest=default",
                                 "--workspace", str(self.root),
                                 "--manifest", str(manifest_path), "--text", str(text_path)],
                                capture_output=True, text=True)
        self.assertEqual(0, result.returncode, result.stderr)
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        self.assertEqual(hashlib.sha256(metadata_path.read_bytes()).hexdigest(), manifest["metadata_sha256"])
        self.assertEqual(hashlib.sha256(text_path.read_bytes()).hexdigest(), manifest["text_sha256"])

    def test_license_file_only_without_expression_is_collected(self):
        metadata = self.metadata()
        package = metadata["packages"][2]
        package["license"] = None
        package["license_file"] = "terms.txt"
        (self.registry / "runtime" / "LICENSE").unlink()
        (self.registry / "runtime" / "terms.txt").write_text("Explicit license text\n")
        manifest, text = module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))
        self.assertIsNone(manifest["packages"][1]["license_expression"])
        self.assertIn("Explicit license text", text)

    def test_nested_native_license_alone_is_collected(self):
        metadata = self.metadata()
        metadata["packages"][2]["name"] = "ring"
        (self.registry / "runtime" / "LICENSE").unlink()
        nested = self.registry / "runtime" / "vendor"
        nested.mkdir()
        (nested / "LICENSE").write_text("Vendor license\n")
        manifest, text = module.generate(metadata, self.root, ("tellurion", "tellurion-ingest"))
        self.assertIn("Vendor license", text)
        self.assertEqual("vendor/LICENSE", manifest["packages"][1]["files"][0]["path"])


if __name__ == "__main__":
    unittest.main()
