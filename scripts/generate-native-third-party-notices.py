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
from urllib.parse import urlparse


NOTICE_NAME = re.compile(r"^(?:licen[cs]e|copying|copyright|notice)(?:[._-].*)?$", re.I)
NATIVE_NAMES = ("sqlite", "ring", "aws-lc", "zstd")


def _sqlite_disclaimer(directory: Path) -> tuple[str, bytes]:
    relative = "sqlite3/sqlite3.c"
    path = directory / relative
    if path.is_symlink() or not path.resolve().is_relative_to(directory) or not path.is_file():
        raise ValueError("SQLite copyright disclaimer source missing or unsafe")
    # The amalgamation can be many megabytes. Its initial copyright block is
    # sufficient; never embed the implementation in a notice document.
    with path.open("rb") as source:
        prefix = source.read(65536)
    marker = b"The author disclaims copyright to this source code."
    position = prefix.find(marker)
    start = prefix.rfind(b"/*", 0, position) if position >= 0 else -1
    end = prefix.find(b"*/", position) if position >= 0 else -1
    if start < 0 or end < 0 or prefix.find(b"*/", start) != end:
        raise ValueError("SQLite copyright disclaimer missing or changed")
    return relative + "#copyright-disclaimer", prefix[start:end + 2]


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
    if package["name"] == "libsqlite3-sys":
        found.append(_sqlite_disclaimer(directory))
    found.sort(key=lambda item: item[0])
    return found


def _validate_license(package: dict, found: list[tuple[str, bytes]], fallback: list[bytes]) -> None:
    declared = package.get("license_file")
    license_files = [(path, content) for path, content in found if path == declared or
                     re.match(r"^(?:licen[cs]e|copying)(?:[._-].*)?$", Path(path).name, re.I)]
    if not license_files and not fallback:
        raise ValueError(f"license text missing: {package['id']}")
    if license_files and not any(content.strip() for _, content in license_files):
        raise ValueError(f"empty license text: {package['id']}")
    if fallback and not all(content.strip() for content in fallback):
        raise ValueError(f"empty fallback license text: {package['id']}")


def _fallback_files(package: dict, entry: dict, directory: Path) -> list[tuple[str, bytes, str]]:
    identity = ("name", "version", "source", "repository", "license_expression")
    expected = (package["name"], package["version"], package["source"], package.get("repository"), package.get("license"))
    if tuple(entry.get(key) for key in identity) != expected:
        raise ValueError(f"fallback package identity mismatch: {package['id']}")
    revision = entry.get("revision")
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError(f"invalid fallback revision: {package['id']}")
    vcs_path = Path(package["manifest_path"]).parent / ".cargo_vcs_info.json"
    if vcs_path.is_symlink() or not vcs_path.is_file():
        raise ValueError(f"missing safe Cargo VCS info: {package['id']}")
    try:
        vcs_revision = json.loads(vcs_path.read_text(encoding="utf-8"))["git"]["sha1"]
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise ValueError(f"invalid Cargo VCS info: {package['id']}") from error
    if vcs_revision != revision:
        raise ValueError(f"fallback revision mismatch: {package['id']}")
    repo = urlparse(entry["repository"])
    if repo.scheme != "https" or repo.netloc != "github.com" or repo.query or repo.fragment or not re.fullmatch(r"/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/?", repo.path):
        raise ValueError(f"invalid fallback repository: {package['id']}")
    owner, name = repo.path.strip("/").split("/")
    prefix = f"https://raw.githubusercontent.com/{owner}/{name}/{revision}/"
    found = []
    seen_files = set()
    for file in entry["files"]:
        relative = file["path"]
        if relative in seen_files:
            raise ValueError(f"duplicate fallback file: {package['id']}: {relative}")
        seen_files.add(relative)
        if not isinstance(relative, str) or not relative or Path(relative).is_absolute() or ".." in Path(relative).parts:
            raise ValueError(f"unsafe fallback path: {package['id']}: {relative}")
        path = directory / relative
        if any(part.is_symlink() for part in (path, *path.parents) if part == directory or directory in part.parents):
            raise ValueError(f"unsafe fallback path: {package['id']}: {relative}")
        if not path.resolve().is_relative_to(directory) or not path.is_file():
            raise ValueError(f"unsafe fallback path: {package['id']}: {relative}")
        digest = file["sha256"]
        content = path.read_bytes()
        if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest) or hashlib.sha256(content).hexdigest() != digest:
            raise ValueError(f"fallback digest mismatch: {package['id']}: {relative}")
        url = file["url"]
        parsed = urlparse(url) if isinstance(url, str) else None
        suffix = url[len(prefix):] if isinstance(url, str) and url.startswith(prefix) else ""
        components = suffix.split("/")
        if (parsed is None or parsed.scheme != "https" or parsed.netloc != "raw.githubusercontent.com"
                or parsed.query or parsed.fragment or not suffix
                or any(part in ("", ".", "..") or not re.fullmatch(r"[A-Za-z0-9._~-]+", part)
                       for part in components)):
            raise ValueError(f"invalid fallback URL: {package['id']}: {relative}")
        found.append((relative, content, url))
    return found


def generate(metadata: dict, workspace: Path, roots: tuple[str, ...], *,
             target: str = "unspecified", feature_profile: str = "unspecified",
             metadata_sha256: str = "unspecified", fallbacks_path: Path | None = None) -> tuple[dict, str]:
    """Return a sorted evidence manifest and the exact collected source texts."""
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    workspace = workspace.resolve(strict=True)
    fallback_entries = []
    fallback_directory = None
    if fallbacks_path is not None:
        fallback_path = Path(fallbacks_path)
        if fallback_path.is_symlink():
            raise ValueError("unsafe fallback manifest path")
        fallback_directory = fallback_path.parent.resolve(strict=True)
        fallback_data = json.loads(fallback_path.read_text(encoding="utf-8"))
        if not isinstance(fallback_data, dict) or fallback_data.get("schema_version") != 1 or not isinstance(fallback_data.get("packages"), list):
            raise ValueError("invalid fallback manifest schema")
        fallback_entries = fallback_data["packages"]
        identities = set()
        for entry in fallback_entries:
            if not isinstance(entry, dict) or not all(isinstance(entry.get(key), str) for key in
                    ("name", "version", "source", "repository", "revision", "license_expression")) or not isinstance(entry.get("files"), list) or not entry["files"]:
                raise ValueError("invalid fallback manifest package")
            if any(not isinstance(file, dict) or not all(isinstance(file.get(key), str) for key in ("path", "sha256", "url"))
                   for file in entry["files"]):
                raise ValueError("invalid fallback manifest file")
            identity = (entry["name"], entry["version"], entry["source"])
            if identity in identities:
                raise ValueError(f"duplicate fallback package: {identity}")
            identities.add(identity)
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
            additions = []
            for entry in fallback_entries:
                if entry.get("name") == package["name"]:
                    additions.extend(_fallback_files(package, entry, fallback_directory))
            existing = {path: content for path, content in files}
            provenance = {}
            for relative, content, url in additions:
                if relative in existing and existing[relative] != content:
                    raise ValueError(f"fallback path collision: {package['id']}: {relative}")
                if relative not in existing:
                    files.append((relative, content))
                    provenance[relative] = url
            files.sort(key=lambda item: item[0])
            _validate_license(package, files, [content for _, content, _ in additions])
        except (OSError, ValueError) as error:
            errors.append(str(error))
            continue
        record = {"id": package["id"], "name": package["name"], "version": package["version"],
                  "source": package["source"], "license_expression": package.get("license"), "files": []}
        sections.append(f"\n===== {package['name']} {package['version']} ({package['id']}) =====\n")
        for relative, content in files:
            file_record = {"path": relative, "sha256": hashlib.sha256(content).hexdigest()}
            if relative in provenance:
                file_record.update({"provenance": "fallback", "url": provenance[relative]})
            record["files"].append(file_record)
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
    parser.add_argument("--fallbacks", type=Path, help="reviewed, pinned fallback license texts manifest")
    args = parser.parse_args()
    try:
        roots = tuple(args.root or ("tellurion", "tellurion-ingest"))
        metadata_bytes = args.metadata.read_bytes()
        manifest, notice = generate(json.loads(metadata_bytes.decode("utf-8")), args.workspace, roots,
                                    target=args.target, feature_profile=args.feature_profile,
                                    metadata_sha256=hashlib.sha256(metadata_bytes).hexdigest(), fallbacks_path=args.fallbacks)
        args.manifest.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="")
        args.text.write_text(notice, encoding="utf-8", newline="")
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"native notice generation blocked: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
