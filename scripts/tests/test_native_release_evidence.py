"""Synthetic, offline contract tests for native archive evidence admission."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parents[1]
VALIDATOR = SCRIPTS / "check-native-release-evidence.py"
GATE = SCRIPTS / "check-native-binary-release-readiness.sh"
GENERATOR = SCRIPTS / "generate-native-third-party-notices.py"
spec = importlib.util.spec_from_file_location("native_notices_for_gate_tests", GENERATOR)
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)
LINUX = "x86_64-unknown-linux-musl"
WINDOWS = "x86_64-pc-windows-msvc"
REVISION = "8bab26f4f68e0e26f0bb7960be334d5b520ea452"
PROFILE = "server=default,ui;ingest=default"


def digest(value):
    return hashlib.sha256(value).hexdigest()


class NativeEvidenceTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.workspace = self.root / "workspace"
        self.workspace.mkdir()
        self.evidence = self.root / "evidence"
        self.evidence.mkdir()
        self.sysroot = self.root / "sysroot"
        self.docs = self.sysroot / "share/doc/rust"
        (self.docs / "licenses").mkdir(parents=True)
        (self.docs / "COPYRIGHT.html").write_text("Rust copyright\n")
        (self.docs / "COPYRIGHT-library.html").write_text("Library copyright\n")
        (self.docs / "licenses" / "MIT.txt").write_text("MIT terms\n")
        self.stdlib = self.evidence / "licenses/rust-stdlib"
        (self.stdlib / "licenses").mkdir(parents=True)
        for relative in ("COPYRIGHT.html", "COPYRIGHT-library.html", "licenses/MIT.txt"):
            (self.stdlib / relative).write_bytes((self.docs / relative).read_bytes())
        self.registry = self.root / "registry"
        self.registry.mkdir()
        for name in ("tellurion", "tellurion-ingest"):
            path = self.workspace / name
            path.mkdir()
            (path / "Cargo.toml").write_text("[package]\nname='x'\nversion='1'\n")
        dependency = self.registry / "runtime"
        dependency.mkdir()
        (dependency / "Cargo.toml").write_text("[package]\nname='runtime'\nversion='1'\n")
        (dependency / "LICENSE").write_text("Reviewed dependency license\n")
        packages = [
            {"id": name, "name": name, "version": "1.0.0", "source": None,
             "manifest_path": str(self.workspace / name / "Cargo.toml"), "license": "AGPL-3.0-only"}
            for name in ("tellurion", "tellurion-ingest")
        ]
        packages.append({"id": "runtime", "name": "runtime", "version": "1.0.0",
                         "source": "registry+https://example.test", "manifest_path": str(dependency / "Cargo.toml"),
                         "license": "MIT"})
        self.metadata = {"packages": packages, "resolve": {"nodes": [
            {"id": "tellurion", "deps": [{"pkg": "runtime", "dep_kinds": [{"kind": None}]}]},
            {"id": "tellurion-ingest", "deps": []}, {"id": "runtime", "deps": []},
        ]}}
        self.fresh_metadata = self.root / "fresh-metadata.json"
        self.fresh_metadata.write_text(json.dumps(self.metadata))
        metadata_bytes = self.fresh_metadata.read_bytes()
        (self.evidence / "cargo-metadata.json").write_bytes(metadata_bytes)
        distribution = self.workspace / "distribution/native-notices"
        distribution.mkdir(parents=True)
        (distribution / "fallbacks.json").write_text('{"schema_version":1,"packages":[]}')
        self.musl_copyright = b"musl license evidence\n"
        self.musl_source = b"synthetic musl source archive bytes\n"
        (distribution / "musl-1.2.5-COPYRIGHT.txt").write_bytes(self.musl_copyright)
        self.provenance = {"schema_version": 1, "rust_version": "1.97.1", "rust_revision": REVISION,
                           "linux_musl": {"version": "1.2.5", "copyright_path": "musl-1.2.5-COPYRIGHT.txt",
                                          "copyright_sha256": digest(self.musl_copyright),
                                          "source_sha256": digest(self.musl_source)}}
        pinned = distribution / "runtime-provenance.json"
        pinned.write_text(json.dumps(self.provenance, sort_keys=True) + "\n")
        (self.evidence / "runtime-provenance.json").write_bytes(pinned.read_bytes())
        musl = self.evidence / "licenses/musl"
        musl.mkdir()
        (musl / "COPYRIGHT.txt").write_bytes(self.musl_copyright)
        (musl / "musl-1.2.5.tar.gz").write_bytes(self.musl_source)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self._tools()
        self.prepare_notices(LINUX)

    def _tools(self):
        cargo = self.bin / "cargo"
        cargo.write_text("#!/usr/bin/env python3\nimport pathlib,sys\n"
                         f"expected={['+1.97.1', 'metadata', '--locked', '--offline', '--format-version', '1', '--filter-platform', '__TARGET__', '--features', 'tellurion/ui']!r}\n"
                         "expected[7]=sys.argv[8] if len(sys.argv)>8 else ''\n"
                         "if sys.argv[1:]!=expected: sys.exit(17)\n"
                         f"sys.stdout.buffer.write(pathlib.Path({str(self.fresh_metadata)!r}).read_bytes())\n")
        rustc = self.bin / "rustc"
        rustc.write_text("#!/usr/bin/env python3\nimport pathlib,sys\n"
                         f"root=pathlib.Path({str(self.root)!r})\n"
                         "if sys.argv[1:]==['+1.97.1','--version','--verbose']:\n"
                         " sys.stdout.buffer.write((root/'rustc-version.txt').read_bytes())\n"
                         "elif sys.argv[1:]==['+1.97.1','--print','sysroot']:\n"
                         " sys.stdout.write(str(root/'sysroot')+'\\n')\n"
                         "else: sys.exit(18)\n")
        for tool in (cargo, rustc):
            tool.chmod(0o755)
        (self.root / "rustc-version.txt").write_text(
            f"rustc 1.97.1 ({REVISION[:9]} 2026-01-01)\nrelease: 1.97.1\ncommit-hash: {REVISION}\n")

    def prepare_notices(self, target):
        metadata_bytes = (self.evidence / "cargo-metadata.json").read_bytes()
        manifest, text = notices.generate(
            self.metadata, self.workspace, ("tellurion", "tellurion-ingest"), target=target,
            feature_profile=PROFILE, metadata_sha256=digest(metadata_bytes),
            fallbacks_path=self.workspace / "distribution/native-notices/fallbacks.json")
        (self.evidence / "RUST_THIRD_PARTY_NOTICES.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        (self.evidence / "RUST_THIRD_PARTY_NOTICES.txt").write_text(text)
        (self.evidence / "STATUS").write_text(f"target={target}\nfeature-profile={PROFILE}\nstatus=collected\n")

    def check(self, target=LINUX, flags=None):
        environment = os.environ.copy()
        environment["PATH"] = str(self.bin) + os.pathsep + environment.get("PATH", "")
        environment["RUSTFLAGS"] = "-C target-feature=+crt-static" if flags is None else flags
        return subprocess.run(
            [sys.executable, str(VALIDATOR), "--evidence", str(self.evidence), "--target", target,
             "--workspace", str(self.workspace)], capture_output=True, text=True, env=environment,
        )

    def assert_blocked(self, fragment, target=LINUX, flags=None):
        result = self.check(target, flags)
        self.assertNotEqual(0, result.returncode, result.stdout)
        self.assertIn(fragment, result.stderr)

    def test_complete_linux_and_windows_evidence_admits_internal_archive_only(self):
        linux = self.check()
        self.assertEqual(0, linux.returncode, linux.stderr)
        self.assertIn("internal archive", linux.stdout)
        self.prepare_notices(WINDOWS)
        version = (self.root / "rustc-version.txt")
        version.write_bytes(version.read_text().replace("\n", "\r\n").encode())
        windows = self.check(WINDOWS)
        self.assertEqual(0, windows.returncode, windows.stderr)

    def test_shell_gate_accepts_only_explicit_validated_evidence(self):
        environment = os.environ.copy()
        environment["PATH"] = str(self.bin) + os.pathsep + environment.get("PATH", "")
        blocked = subprocess.run([str(GATE)], capture_output=True, text=True, env=environment)
        self.assertNotEqual(0, blocked.returncode)
        self.assertIn("prebuilt native binary archives are not ready", blocked.stderr)
        accepted = subprocess.run(
            [str(GATE), "--evidence", str(self.evidence), "--target", LINUX,
             "--workspace", str(self.workspace)], capture_output=True, text=True, env=environment)
        self.assertEqual(0, accepted.returncode, accepted.stderr)

    def test_status_alone_does_not_admit_stale_metadata_or_tampered_notices(self):
        (self.evidence / "cargo-metadata.json").write_text('{"packages":[]}')
        self.assert_blocked("metadata")
        self.fresh_metadata.write_text(json.dumps(self.metadata))
        (self.evidence / "cargo-metadata.json").write_text(json.dumps(self.metadata))
        (self.evidence / "RUST_THIRD_PARTY_NOTICES.txt").write_text("tampered")
        self.assert_blocked("notice")

    def test_missing_or_tampered_runtime_and_stdlib_docs_fail_closed(self):
        (self.evidence / "runtime-provenance.json").write_text("{}")
        self.assert_blocked("provenance")
        (self.evidence / "runtime-provenance.json").write_bytes(
            (self.workspace / "distribution/native-notices/runtime-provenance.json").read_bytes())
        (self.stdlib / "licenses/MIT.txt").write_text("tampered")
        self.assert_blocked("rust-stdlib")
        (self.stdlib / "licenses/MIT.txt").unlink()
        self.assert_blocked("rust-stdlib")

    def test_symlink_or_empty_stdlib_doc_fails_closed(self):
        (self.stdlib / "COPYRIGHT.html").unlink()
        (self.stdlib / "COPYRIGHT.html").symlink_to(self.docs / "COPYRIGHT.html")
        self.assert_blocked("symlink")
        (self.stdlib / "COPYRIGHT.html").unlink()
        (self.stdlib / "COPYRIGHT.html").write_text("")
        self.assert_blocked("empty")

    def test_symlinked_evidence_parent_fails_even_when_contents_match(self):
        original = self.evidence / "licenses"
        moved = self.root / "moved-licenses"
        original.rename(moved)
        original.symlink_to(moved, target_is_directory=True)
        self.assert_blocked("symlink")

    def test_wrong_target_runtime_musl_hash_and_windows_crt_fail(self):
        self.assert_blocked("unsupported", target="aarch64-apple-darwin")
        self.prepare_notices(WINDOWS)
        self.assert_blocked("RUSTFLAGS", target=WINDOWS, flags="")
        self.prepare_notices(LINUX)
        (self.evidence / "licenses/musl/musl-1.2.5.tar.gz").write_text("tampered")
        self.assert_blocked("musl")
        (self.evidence / "licenses/musl/musl-1.2.5.tar.gz").write_bytes(self.musl_source)
        (self.root / "rustc-version.txt").write_text("release: 1.97.1\ncommit-hash: " + "0" * 40 + "\n")
        self.assert_blocked("rustc")

    def test_collected_status_is_required_but_not_sufficient(self):
        (self.evidence / "STATUS").write_text(f"target={LINUX}\nfeature-profile={PROFILE}\nstatus=blocked\n")
        self.assert_blocked("STATUS")


if __name__ == "__main__":
    unittest.main()
