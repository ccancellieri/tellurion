#!/usr/bin/env python3
"""Validate native notice and runtime evidence for an internal archive build.

This does not authorize publication or replace an install smoke test.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys


TARGETS = {"x86_64-unknown-linux-musl", "x86_64-pc-windows-msvc"}
PROFILE = "server=default,ui;ingest=default"
ROOTS = ("tellurion", "tellurion-ingest")
RUST_VERSION = "1.97.1"


class EvidenceError(ValueError):
    pass


def read_file(path: Path, label: str) -> bytes:
    if path.is_symlink():
        raise EvidenceError(f"{label} contains a symlink")
    if not path.is_file():
        raise EvidenceError(f"{label} is missing")
    content = path.read_bytes()
    if not content.strip():
        raise EvidenceError(f"{label} is empty")
    return content


def read_json(path: Path, label: str) -> tuple[dict, bytes]:
    content = read_file(path, label)
    try:
        value = json.loads(content)
    except (UnicodeDecodeError, ValueError) as error:
        raise EvidenceError(f"{label} is invalid JSON") from error
    if not isinstance(value, dict):
        raise EvidenceError(f"{label} must be an object")
    return value, content


def files_in_tree(directory: Path, label: str) -> dict[str, bytes]:
    if directory.is_symlink():
        raise EvidenceError(f"{label} contains a symlink")
    if not directory.is_dir():
        raise EvidenceError(f"{label} is missing")
    files: dict[str, bytes] = {}
    def walk_error(error: OSError) -> None:
        raise EvidenceError(f"{label} cannot be traversed") from error

    for parent, directories, names in os.walk(directory, followlinks=False, onerror=walk_error):
        base = Path(parent)
        for name in directories:
            path = base / name
            if path.is_symlink():
                raise EvidenceError(f"{label} contains a symlink")
            if not path.is_dir():
                raise EvidenceError(f"{label} contains an unsafe directory")
        for name in names:
            path = base / name
            relative = path.relative_to(directory).as_posix()
            files[relative] = read_file(path, label)
    if not files:
        raise EvidenceError(f"{label} is empty")
    return files


def run_checked(arguments: list[str], workspace: Path) -> bytes:
    try:
        result = subprocess.run(arguments, cwd=workspace, capture_output=True, check=False)
    except OSError as error:
        raise EvidenceError(f"{arguments[0]} verification could not run") from error
    if result.returncode != 0 or not result.stdout.strip():
        raise EvidenceError(f"{arguments[0]} verification failed")
    return result.stdout


def check_status(evidence: Path, target: str) -> None:
    lines = read_file(evidence / "STATUS", "STATUS").decode("utf-8").splitlines()
    entries = [line.split("=", 1) for line in lines]
    if any(len(entry) != 2 for entry in entries) or len(entries) != 3:
        raise EvidenceError("STATUS is invalid")
    values = dict(entries)
    if len(values) != 3 or values != {"target": target, "feature-profile": PROFILE, "status": "collected"}:
        raise EvidenceError("STATUS target, profile, or collection state is invalid")


def check_windows_crt() -> None:
    try:
        flags = shlex.split(os.environ.get("RUSTFLAGS", ""))
    except ValueError as error:
        raise EvidenceError("Windows RUSTFLAGS is invalid") from error
    if not any(flags[index:index + 2] == ["-C", "target-feature=+crt-static"]
               for index in range(len(flags) - 1)):
        raise EvidenceError("Windows RUSTFLAGS must enable +crt-static")


def check_rust_runtime(evidence: Path, workspace: Path) -> tuple[dict, Path]:
    pinned_path = workspace / "distribution/native-notices/runtime-provenance.json"
    pinned, pinned_bytes = read_json(pinned_path, "pinned runtime provenance")
    evidence_bytes = read_file(evidence / "runtime-provenance.json", "runtime provenance")
    if evidence_bytes != pinned_bytes:
        raise EvidenceError("runtime provenance differs from checked-in bytes")
    revision = pinned.get("rust_revision")
    if pinned.get("schema_version") != 1 or pinned.get("rust_version") != RUST_VERSION or not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise EvidenceError("runtime provenance has an invalid Rust identity")
    verbose_lines = run_checked(["rustc", f"+{RUST_VERSION}", "--version", "--verbose"], workspace).decode("utf-8").splitlines()
    releases = [line.removeprefix("release: ") for line in verbose_lines if line.startswith("release: ")]
    commits = [line.removeprefix("commit-hash: ") for line in verbose_lines if line.startswith("commit-hash: ")]
    if releases != [RUST_VERSION] or commits != [revision]:
        raise EvidenceError("rustc release or commit does not match runtime provenance")
    sysroot_text = run_checked(["rustc", f"+{RUST_VERSION}", "--print", "sysroot"], workspace).decode("utf-8").strip()
    sysroot = Path(sysroot_text)
    if not sysroot.is_absolute() or not sysroot.is_dir():
        raise EvidenceError("rustc sysroot is invalid")
    return pinned, sysroot


def check_stdlib(evidence: Path, sysroot: Path) -> None:
    source = sysroot / "share/doc/rust"
    copied = evidence / "licenses/rust-stdlib"
    for path in (sysroot / "share", sysroot / "share/doc", source):
        if path.is_symlink():
            raise EvidenceError("rust-stdlib source contains a symlink")
    for name in ("COPYRIGHT.html", "COPYRIGHT-library.html"):
        if read_file(source / name, "rust-stdlib source") != read_file(copied / name, "rust-stdlib evidence"):
            raise EvidenceError("rust-stdlib copyright differs from pinned sysroot")
    source_licenses = files_in_tree(source / "licenses", "rust-stdlib source licenses")
    copied_licenses = files_in_tree(copied / "licenses", "rust-stdlib evidence licenses")
    if source_licenses != copied_licenses:
        raise EvidenceError("rust-stdlib licenses differ from pinned sysroot")


def check_musl(evidence: Path, workspace: Path, provenance: dict) -> None:
    musl = provenance.get("linux_musl")
    if not isinstance(musl, dict) or musl.get("version") != "1.2.5":
        raise EvidenceError("musl runtime provenance is invalid")
    name = musl.get("copyright_path")
    if not isinstance(name, str) or name != "musl-1.2.5-COPYRIGHT.txt":
        raise EvidenceError("musl copyright path is invalid")
    expected_copyright = musl.get("copyright_sha256")
    expected_source = musl.get("source_sha256")
    if not all(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value)
               for value in (expected_copyright, expected_source)):
        raise EvidenceError("musl provenance hashes are invalid")
    pinned_copyright = read_file(workspace / "distribution/native-notices" / name, "musl pinned copyright")
    copied_copyright = read_file(evidence / "licenses/musl/COPYRIGHT.txt", "musl copyright evidence")
    source_archive = read_file(evidence / "licenses/musl/musl-1.2.5.tar.gz", "musl source evidence")
    if pinned_copyright != copied_copyright or hashlib.sha256(copied_copyright).hexdigest() != expected_copyright:
        raise EvidenceError("musl copyright hash or copied bytes differ from provenance")
    if hashlib.sha256(source_archive).hexdigest() != expected_source:
        raise EvidenceError("musl source archive hash differs from provenance")


def check_notices(evidence: Path, target: str, workspace: Path) -> None:
    metadata_path = evidence / "cargo-metadata.json"
    recorded, metadata_bytes = read_json(metadata_path, "recorded cargo metadata")
    fresh_bytes = run_checked([
        "cargo", f"+{RUST_VERSION}", "metadata", "--locked", "--offline", "--format-version", "1",
        "--filter-platform", target, "--features", "tellurion/ui",
    ], workspace)
    try:
        fresh = json.loads(fresh_bytes)
    except (UnicodeDecodeError, ValueError) as error:
        raise EvidenceError("fresh cargo metadata is invalid") from error
    if fresh != recorded:
        raise EvidenceError("recorded cargo metadata is stale or differs from fresh resolution")
    generator_path = Path(__file__).with_name("generate-native-third-party-notices.py")
    spec = importlib.util.spec_from_file_location("native_notices_for_gate", generator_path)
    if spec is None or spec.loader is None:
        raise EvidenceError("native notice generator is unavailable")
    generator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(generator)
    try:
        manifest, notice = generator.generate(
            recorded, workspace, ROOTS, target=target, feature_profile=PROFILE,
            metadata_sha256=hashlib.sha256(metadata_bytes).hexdigest(),
            fallbacks_path=workspace / "distribution/native-notices/fallbacks.json",
        )
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise EvidenceError(f"native notice regeneration failed: {error}") from error
    collected_manifest, _ = read_json(evidence / "RUST_THIRD_PARTY_NOTICES.json", "native notice manifest")
    if collected_manifest != manifest:
        raise EvidenceError("native notice manifest differs from regenerated evidence")
    collected_text = read_file(evidence / "RUST_THIRD_PARTY_NOTICES.txt", "native notice text")
    if collected_text != notice.encode("utf-8"):
        raise EvidenceError("native notice text differs from regenerated evidence")


def validate(evidence: Path, target: str, workspace: Path) -> None:
    if target not in TARGETS:
        raise EvidenceError("unsupported native release target")
    if evidence.is_symlink() or not evidence.is_dir():
        raise EvidenceError("native evidence directory is missing or unsafe")
    if any(path.is_symlink() for path in evidence.rglob("*")):
        raise EvidenceError("native evidence contains a symlink")
    if workspace.is_symlink() or not workspace.is_dir():
        raise EvidenceError("workspace directory is missing or unsafe")
    if target == "x86_64-pc-windows-msvc":
        check_windows_crt()
    check_status(evidence, target)
    provenance, sysroot = check_rust_runtime(evidence, workspace)
    check_stdlib(evidence, sysroot)
    if target == "x86_64-unknown-linux-musl":
        check_musl(evidence, workspace, provenance)
    check_notices(evidence, target, workspace)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    args = parser.parse_args()
    try:
        validate(args.evidence, args.target, args.workspace)
    except (EvidenceError, OSError, ValueError, KeyError, TypeError, UnicodeDecodeError) as error:
        print(f"BLOCKED: native release evidence is incomplete or stale: {error}", file=sys.stderr)
        return 1
    print("Native evidence validated for internal archive build only; install smoke is still required before release.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
