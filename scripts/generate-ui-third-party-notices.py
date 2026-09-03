#!/usr/bin/env python3
"""Render a deterministic, fail-closed notice file for shipped UI packages."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
from typing import Any
from urllib.parse import urlsplit


NOTICE_PREFIXES = ("license", "licence", "copyright", "notice")
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
EMAIL_PATTERN = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
AND_OPERATOR_PATTERN = re.compile(r"(?:^|[\s(])AND(?:[\s)])")


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sha256_tree(path: Path) -> str:
    digest = hashlib.sha256()
    for child in sorted(candidate for candidate in path.rglob("*") if candidate.is_file()):
        relative = child.relative_to(path).as_posix().encode("utf-8")
        digest.update(len(relative).to_bytes(8, "big"))
        digest.update(relative)
        digest.update(bytes.fromhex(sha256_file(child)))
    return digest.hexdigest()


def read_notice_text(path: Path) -> str:
    """Read UTF-8 notice text without newline or whitespace transformation."""

    return path.read_bytes().decode("utf-8")


def load_fallbacks(path: Path) -> dict[tuple[str, str], tuple[str, str, str, str, tuple[str, ...]]]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if (
        not isinstance(document, dict)
        or set(document) != {"schema_version", "fallbacks"}
        or document["schema_version"] != 2
        or not isinstance(document["fallbacks"], list)
    ):
        raise ValueError("invalid fallback schema")
    fallbacks: dict[tuple[str, str], tuple[str, str, str, str, tuple[str, ...]]] = {}
    for item in document["fallbacks"]:
        required_fields = {
            "name",
            "version",
            "source",
            "integrity",
            "package_json_sha256",
            "notice_file",
            "notice_sha256",
        }
        if not isinstance(item, dict) or set(item) not in (required_fields, required_fields | {"attribution"}):
            raise ValueError("invalid fallback schema")
        name = item.get("name")
        version = item.get("version")
        source = item.get("source")
        integrity = item.get("integrity")
        package_json_hash = item.get("package_json_sha256")
        notice_file = item.get("notice_file")
        notice_hash = item.get("notice_sha256")
        attribution = item.get("attribution", [])
        parsed_source = urlsplit(source) if isinstance(source, str) else None
        if (
            not all(
                isinstance(value, str) and value
                for value in (name, version, source, integrity, package_json_hash, notice_file, notice_hash)
            )
            or parsed_source is None
            or parsed_source.scheme != "https"
            or not parsed_source.hostname
            or parsed_source.username is not None
            or parsed_source.password is not None
            or not isinstance(integrity, str)
            or not integrity.startswith("sha512-")
            or not SHA256_PATTERN.fullmatch(package_json_hash)
            or not SHA256_PATTERN.fullmatch(notice_hash)
            or not isinstance(attribution, list)
            or any(not isinstance(line, str) or not line or EMAIL_PATTERN.search(line) for line in attribution)
        ):
            raise ValueError("invalid fallback schema")
        notice_path = (path.parent / notice_file).resolve()
        if path.parent.resolve() not in notice_path.parents or not notice_path.is_file():
            raise ValueError("invalid fallback path")
        notice_text = read_notice_text(notice_path)
        if not notice_text or sha256_file(notice_path) != notice_hash or EMAIL_PATTERN.search(notice_text):
            raise ValueError("invalid fallback text")
        key = (name, version)
        if key in fallbacks:
            raise ValueError("duplicate fallback")
        fallbacks[key] = (source, integrity, package_json_hash, notice_text, tuple(attribution))
    return fallbacks


def package_license_expression(package_json: dict[str, Any]) -> str | None:
    declared = package_json.get("license")
    if isinstance(declared, str) and declared:
        return declared
    legacy = package_json.get("licenses")
    if (
        isinstance(legacy, list)
        and len(legacy) == 1
        and isinstance(legacy[0], dict)
        and isinstance(legacy[0].get("type"), str)
        and legacy[0]["type"]
    ):
        return legacy[0]["type"]
    return None


def shipped_packages(lockfile: Path, package_root: Path) -> list[tuple[str, str, str, Path, str | None, str | None]]:
    document = json.loads(lockfile.read_text(encoding="utf-8"))
    packages = document.get("packages") if isinstance(document, dict) else None
    if document.get("lockfileVersion") != 3 or not isinstance(packages, dict):
        raise ValueError("unsupported package lock")
    shipped: list[tuple[str, str, str, Path, str | None, str | None]] = []
    for lock_path, metadata in packages.items():
        if (
            not isinstance(lock_path, str)
            or not lock_path.startswith("node_modules/")
            or not isinstance(metadata, dict)
            or metadata.get("dev") is True
        ):
            continue
        version = metadata.get("version")
        locked_expression = metadata.get("license")
        package_path = package_root.parent / lock_path
        package_json = json.loads((package_path / "package.json").read_text(encoding="utf-8"))
        name = package_json.get("name") if isinstance(package_json, dict) else None
        package_expression = package_license_expression(package_json) if isinstance(package_json, dict) else None
        expression = locked_expression or package_expression
        if (
            not isinstance(name, str)
            or not name
            or not isinstance(version, str)
            or not version
            or package_json.get("version") != version
            or not isinstance(expression, str)
            or not expression
            or (
                isinstance(locked_expression, str)
                and package_expression is not None
                and locked_expression != package_expression
            )
        ):
            raise ValueError("invalid shipped package")
        resolved = metadata.get("resolved")
        integrity = metadata.get("integrity")
        if (resolved is not None and not isinstance(resolved, str)) or (
            integrity is not None and not isinstance(integrity, str)
        ):
            raise ValueError("invalid shipped package")
        shipped.append((name, version, expression, package_path, resolved, integrity))
    if not shipped:
        raise ValueError("empty shipped package selection")
    return sorted(shipped, key=lambda package: (package[0], package[1], package[3].as_posix()))


def package_notice_files(path: Path) -> list[Path]:
    return sorted(
        candidate
        for candidate in path.iterdir()
        if candidate.is_file() and candidate.name.lower().startswith(NOTICE_PREFIXES)
    )


def license_expression_requires_fallback(expression: str) -> bool:
    """Return whether one or more package files cannot prove full coverage."""

    return AND_OPERATOR_PATTERN.search(expression) is not None


def render_notice(
    lockfile: Path,
    package_root: Path,
    operator_bundle: Path,
    public_demo_bundle: Path,
    fallbacks: Path,
) -> str:
    fallback_text = load_fallbacks(fallbacks)
    packages = shipped_packages(lockfile, package_root)
    lines = [
        "Tellurion UI third-party notices",
        "",
        "This file is generated from the locked production dependency union.",
        "Sections with `notice-origin: package:*` reproduce the source UTF-8 text unchanged.",
        "Sections with `notice-origin: reviewed-fallback` are pinned, privacy-safe curated",
        "notices. Each fallback identifies its provenance and any omission in its own text.",
        f"package-lock-sha256: {sha256_file(lockfile)}",
        f"operator-bundle-sha256: {sha256_tree(operator_bundle)}",
        f"public-demo-bundle-sha256: {sha256_tree(public_demo_bundle)}",
        f"production-package-count: {len(packages)}",
    ]
    for name, version, expression, package_path, resolved, integrity in packages:
        files = package_notice_files(package_path)
        requires_fallback = license_expression_requires_fallback(expression) or any(
            EMAIL_PATTERN.search(read_notice_text(path)) for path in files
        )
        if files and not requires_fallback:
            notices = [
                (f"package:{path.name}", None, read_notice_text(path))
                for path in files
            ]
        else:
            fallback = fallback_text.get((name, version))
            if fallback is None:
                raise ValueError("unreviewed package text")
            package_json = package_path / "package.json"
            if (
                resolved != fallback[0]
                or integrity != fallback[1]
                or sha256_file(package_json) != fallback[2]
            ):
                raise ValueError("unreviewed package metadata")
            notices = [
                (
                    "reviewed-fallback",
                    fallback[0],
                    "\n".join((*fallback[4], "", fallback[3])) if fallback[4] else fallback[3],
                )
            ]
        if any(not text for _, _, text in notices):
            raise ValueError("empty package notice")
        lines.extend(("", "=" * 78, f"package: {name}@{version}", f"license-expression: {expression}"))
        for origin, source, text in notices:
            lines.append(f"notice-origin: {origin}")
            if source is not None:
                lines.append(f"source: {source}")
            lines.extend(("", text))
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lockfile", type=Path, required=True)
    parser.add_argument("--package-root", type=Path, required=True)
    parser.add_argument("--operator-bundle", type=Path, required=True)
    parser.add_argument("--public-demo-bundle", type=Path, required=True)
    parser.add_argument("--fallbacks", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if not arguments.package_root.is_dir() or not arguments.operator_bundle.is_dir() or not arguments.public_demo_bundle.is_dir():
            raise ValueError("missing input directory")
        rendered = render_notice(
            arguments.lockfile,
            arguments.package_root,
            arguments.operator_bundle,
            arguments.public_demo_bundle,
            arguments.fallbacks,
        )
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        arguments.output.write_text(rendered, encoding="utf-8")
    except (OSError, ValueError, UnicodeDecodeError, json.JSONDecodeError):
        print("ui third-party notice generation unavailable", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
