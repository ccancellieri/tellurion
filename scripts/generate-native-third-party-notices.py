#!/usr/bin/env python3
"""Collect review evidence for native Cargo binary third-party notices.

Input metadata must be generated with the exact target and feature profile for
the archive under review. This tool does not grant redistribution permission.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys


NOTICE_NAME = re.compile(r"^(?:licen[cs]e|copying|copyright|notice)(?:[._-].*)?$", re.I)
NATIVE_NAMES = ("sqlite", "ring", "aws-lc")


def _files(package: dict) -> list[tuple[str, bytes]]:
    directory = Path(package["manifest_path"]).parent.resolve(strict=True)
    if not directory.is_dir():
        raise ValueError(f"unsafe package directory: {package['id']}")
    native = any(name in package["name"] for name in NATIVE_NAMES)
    candidates = directory.rglob("*") if native else directory.iterdir()
    found = []
    declared = package.get("license_file")
    if declared:
        declared_path = directory / declared
        if declared_path.is_symlink() or not declared_path.resolve().is_relative_to(directory) or not declared_path.is_file():
            raise ValueError(f"unsafe declared license path: {package['id']}")
        found.append((declared_path.relative_to(directory).as_posix(), declared_path.read_bytes()))
    for path in candidates:
        if not NOTICE_NAME.match(path.name):
            continue
        if path.is_symlink() or not path.resolve().is_relative_to(directory):
            raise ValueError(f"unsafe notice path: {package['id']}: {path}")
        if path.is_file():
            relative = path.relative_to(directory).as_posix()
            if relative != declared:
                found.append((relative, path.read_bytes()))
    found.sort(key=lambda item: item[0])
    license_files = [(path, content) for path, content in found if path == declared or
                     re.match(r"^(?:licen[cs]e|copying)(?:[._-].*)?$", Path(path).name, re.I)]
    if not license_files:
        raise ValueError(f"license text missing: {package['id']}")
    if not any(content.strip() for _, content in license_files):
        raise ValueError(f"empty license text: {package['id']}")
    return found


def generate(metadata: dict, workspace: Path, roots: tuple[str, ...], *,
             target: str = "unspecified", feature_profile: str = "unspecified",
             metadata_sha256: str = "unspecified") -> tuple[dict, str]:
    """Return a sorted evidence manifest and the exact collected source texts."""
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    workspace = workspace.resolve(strict=True)
    root_ids = []
    for name in roots:
        matches = [p["id"] for p in packages.values() if p["name"] == name and p["source"] is None
                   and Path(p["manifest_path"]).resolve().is_relative_to(workspace)]
        if len(matches) != 1:
            raise ValueError(f"unresolved root: {name}")
        root_ids.extend(matches)

    visited = set()
    pending = list(root_ids)
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        if package_id not in packages or package_id not in nodes:
            raise ValueError(f"unresolved package: {package_id}")
        visited.add(package_id)
        for dep in nodes[package_id]["deps"]:
            # Build dependencies are conservative runtime-distribution evidence;
            # dev-only edges do not participate in native binary builds.
            if any(kind.get("kind") != "dev" for kind in dep["dep_kinds"]):
                pending.append(dep["pkg"])

    for pid in visited:
        package = packages[pid]
        if package["source"] is None and not Path(package["manifest_path"]).resolve().is_relative_to(workspace):
            raise ValueError(f"path dependency outside workspace: {pid}")

    records = []
    errors = []
    sections = ["Rust third-party source notices\n", "Collected evidence only; review licenses and redistribution obligations.\n"]
    for package in sorted((packages[pid] for pid in visited if packages[pid]["source"] is not None),
                          key=lambda p: (p["name"], p["version"], p["id"])):
        if not package.get("license") and not package.get("license_file"):
            errors.append(f"license metadata missing: {package['id']}")
            continue
        try:
            files = _files(package)
        except (OSError, ValueError) as error:
            errors.append(str(error))
            continue
        record = {"id": package["id"], "name": package["name"], "version": package["version"],
                  "source": package["source"], "license_expression": package.get("license"), "files": []}
        sections.append(f"\n===== {package['name']} {package['version']} ({package['id']}) =====\n")
        for relative, content in files:
            record["files"].append({"path": relative, "sha256": hashlib.sha256(content).hexdigest()})
            sections.append(f"\n----- {relative} -----\n")
            try:
                sections.append(content.decode("utf-8"))
            except UnicodeDecodeError as error:
                errors.append(f"notice is not UTF-8: {package['id']}: {relative}: {error}")
            sections.append("\n")
        records.append(record)
    if errors:
        raise ValueError("\n".join(errors))
    output = "".join(sections)
    manifest = {"schema_version": 1, "roots": list(roots), "declared_target": target,
                "declared_feature_profile": feature_profile,
                "metadata_sha256": metadata_sha256, "packages": records,
                "text_sha256": hashlib.sha256(output.encode()).hexdigest()}
    return manifest, output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata", type=Path, required=True, help="cargo metadata --locked --offline --format-version 1 output")
    parser.add_argument("--target", required=True, help="Cargo target triple used to resolve the supplied metadata")
    parser.add_argument("--feature-profile", required=True, help="reviewer-declared Cargo feature selection")
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--text", type=Path, required=True)
    parser.add_argument("--root", action="append", help="override default executable roots; repeat per root")
    args = parser.parse_args()
    try:
        roots = tuple(args.root or ("tellurion", "tellurion-ingest"))
        metadata_bytes = args.metadata.read_bytes()
        manifest, notice = generate(json.loads(metadata_bytes.decode("utf-8")), args.workspace, roots,
                                    target=args.target, feature_profile=args.feature_profile,
                                    metadata_sha256=hashlib.sha256(metadata_bytes).hexdigest())
        args.manifest.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="")
        args.text.write_text(notice, encoding="utf-8", newline="")
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"native notice generation blocked: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
