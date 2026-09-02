#!/usr/bin/env python3
"""Audit locked Rust and npm dependency licences without shell interpolation."""

from __future__ import annotations

import argparse
from collections import Counter
from collections.abc import Mapping
from dataclasses import asdict, dataclass
import json
from pathlib import Path
import re
import subprocess
import sys
from typing import Any, Iterable
from urllib.parse import urlsplit


CARGO_METADATA_COMMAND = ["cargo", "metadata", "--locked", "--format-version", "1"]
NPM_SBOM_COMMAND = [
    "npm",
    "--prefix",
    "ui",
    "sbom",
    "--package-lock-only",
    "--sbom-format",
    "cyclonedx",
]
CARGO_VERSION_COMMAND = ["cargo", "--version"]
NPM_VERSION_COMMAND = ["npm", "--version"]
FORBIDDEN_LICENSES = {
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "SSPL-1.0",
}
SPDX_IDENTIFIER_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9.+-]*")
SPDX_TOKEN_PATTERN = re.compile(r"\s*(\(|\)|AND|OR|WITH|[A-Za-z0-9][A-Za-z0-9.+-]*)")
REPOSITORY_ROOT = Path(__file__).resolve().parent.parent
LICENSE_OVERRIDES_PATH = REPOSITORY_ROOT / "scripts" / "dependency-license-overrides.json"
REVIEWED_OVERRIDE_LICENSES = {"MIT"}


@dataclass(frozen=True)
class LicenseFinding:
    name: str
    version: str
    license_expression: str | None
    source: str
    rule_id: str


@dataclass(frozen=True)
class LicenseOverride:
    license_expression: str
    evidence: str
    reason: str


@dataclass(frozen=True)
class DependencyPackage:
    name: str
    version: str
    license_expression: str | None
    source: str
    license_origin: str
    license_evidence: str | None
    license_reason: str | None


@dataclass
class DependencyReport:
    blockers: list[LicenseFinding]
    compound_expressions: list[str]
    packages: list[DependencyPackage]

    @property
    def status(self) -> str:
        return "blocked" if self.blockers else "ready"

    def summary(self, tool_versions: dict[str, str]) -> dict[str, Any]:
        finding_counts = Counter(blocker.rule_id for blocker in self.blockers)
        dependency_counts = Counter(package.source for package in self.packages)
        category_counts: dict[str, int] = {}
        if self.compound_expressions:
            category_counts["compound-license-expression"] = len(self.compound_expressions)
        return {
            "status": self.status,
            "dependency_counts": {
                "cargo": dependency_counts["cargo"],
                "npm": dependency_counts["npm"],
                "total": len(self.packages),
            },
            "finding_counts": dict(sorted(finding_counts.items())),
            "category_counts": category_counts,
            "rule_ids": sorted(finding_counts),
            "tool_versions": dict(sorted(tool_versions.items())),
        }

    def inventory(self, tool_versions: dict[str, str]) -> dict[str, Any]:
        return {
            "schema_version": 1,
            "packages": [asdict(package) for package in self.packages],
            "tool_versions": dict(sorted(tool_versions.items())),
        }


def _is_compound(expression: str) -> bool:
    return " AND " in expression or " OR " in expression or " WITH " in expression


def _spdx_tokens(expression: str) -> list[str] | None:
    tokens: list[str] = []
    offset = 0
    while offset < len(expression):
        match = SPDX_TOKEN_PATTERN.match(expression, offset)
        if not match:
            return None
        tokens.append(match.group(1))
        offset = match.end()
    return tokens


def _requires_forbidden_license(expression: str) -> bool:
    """Return whether every valid licensing choice includes a forbidden licence."""

    tokens = _spdx_tokens(expression)
    identifiers = set(SPDX_IDENTIFIER_PATTERN.findall(expression))
    if not tokens:
        return bool(identifiers & FORBIDDEN_LICENSES)
    offset = 0

    def primary() -> bool:
        nonlocal offset
        if offset >= len(tokens):
            raise ValueError("missing SPDX operand")
        token = tokens[offset]
        offset += 1
        if token == "(":
            value = disjunction()
            if offset >= len(tokens) or tokens[offset] != ")":
                raise ValueError("unclosed SPDX expression")
            offset += 1
            return value
        if token in {"AND", "OR", "WITH", ")"}:
            raise ValueError("invalid SPDX operand")
        return token in FORBIDDEN_LICENSES

    def with_exception() -> bool:
        nonlocal offset
        value = primary()
        if offset < len(tokens) and tokens[offset] == "WITH":
            offset += 1
            if offset >= len(tokens) or tokens[offset] in {"AND", "OR", "WITH", "(", ")"}:
                raise ValueError("invalid SPDX exception")
            if tokens[offset] in FORBIDDEN_LICENSES:
                raise ValueError("licence identifier used as SPDX exception")
            offset += 1
        return value

    def conjunction() -> bool:
        nonlocal offset
        value = with_exception()
        while offset < len(tokens) and tokens[offset] == "AND":
            offset += 1
            operand = with_exception()
            value = value or operand
        return value

    def disjunction() -> bool:
        nonlocal offset
        value = conjunction()
        while offset < len(tokens) and tokens[offset] == "OR":
            offset += 1
            operand = conjunction()
            value = value and operand
        return value

    try:
        required = disjunction()
        if offset != len(tokens):
            raise ValueError("trailing SPDX input")
        return required
    except ValueError:
        # Unknown or legacy expressions remain fail-closed when they mention a
        # forbidden identifier; the audit must never turn malformed metadata
        # into permission to ship it.
        return bool(identifiers & FORBIDDEN_LICENSES)


def load_license_overrides(path: Path) -> dict[tuple[str, str, str], LicenseOverride]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if (
        not isinstance(document, Mapping)
        or set(document) != {"schema_version", "overrides"}
        or type(document.get("schema_version")) is not int
        or document.get("schema_version") != 1
    ):
        raise RuntimeError("dependency licence override schema is invalid")
    entries = document.get("overrides")
    if not isinstance(entries, list):
        raise RuntimeError("dependency licence override schema is invalid")
    overrides: dict[tuple[str, str, str], LicenseOverride] = {}
    expected_fields = {"source", "name", "version", "license", "evidence", "reason"}
    for entry in entries:
        if not isinstance(entry, Mapping) or set(entry) != expected_fields:
            raise RuntimeError("dependency licence override schema is invalid")
        source = entry.get("source")
        name = entry.get("name")
        version = entry.get("version")
        expression = entry.get("license")
        evidence = entry.get("evidence")
        reason = entry.get("reason")
        values = (source, name, version, expression, evidence, reason)
        evidence_url = urlsplit(evidence.strip()) if isinstance(evidence, str) else None
        if (
            not all(isinstance(value, str) and value.strip() for value in values)
            or source not in {"cargo", "npm"}
            or expression.strip() not in REVIEWED_OVERRIDE_LICENSES
            or evidence_url is None
            or evidence_url.scheme != "https"
            or not evidence_url.hostname
            or not evidence_url.path.strip("/")
            or evidence_url.username is not None
            or evidence_url.password is not None
        ):
            raise RuntimeError("dependency licence override schema is invalid")
        key = (source.strip(), name.strip(), version.strip())
        if key in overrides:
            raise RuntimeError("duplicate dependency licence override")
        overrides[key] = LicenseOverride(
            license_expression=expression.strip(),
            evidence=evidence.strip(),
            reason=reason.strip(),
        )
    return overrides


def _audit_packages(
    packages: Iterable[dict[str, Any]],
    source: str,
    first_party_ids: set[str] | None = None,
    *,
    require_id: bool = False,
    license_overrides: Mapping[tuple[str, str, str], LicenseOverride] | None = None,
) -> DependencyReport:
    blockers: list[LicenseFinding] = []
    compounds: list[str] = []
    audited_packages: list[DependencyPackage] = []
    first_party_ids = first_party_ids or set()
    license_overrides = license_overrides or {}
    for package in packages:
        if not isinstance(package, Mapping):
            raise RuntimeError("dependency package schema is invalid")
        package_id_value = package.get("id", "")
        name_value = package.get("name")
        version_value = package.get("version")
        expression_value = package.get("license")
        if (
            not isinstance(package_id_value, str)
            or not isinstance(name_value, str)
            or not isinstance(version_value, str)
            or (expression_value is not None and not isinstance(expression_value, str))
        ):
            raise RuntimeError("dependency package schema is invalid")
        if (
            (require_id and not package_id_value.strip())
            or not name_value.strip()
            or not version_value.strip()
        ):
            raise RuntimeError("dependency package identity is invalid")
        package_id = package_id_value
        if package_id in first_party_ids:
            continue
        name = name_value
        version = version_value
        expression = expression_value.strip() if expression_value else None
        license_origin = "declared" if expression else "missing"
        license_evidence = None
        license_reason = None
        if not expression:
            override = license_overrides.get((source, name, version))
            if override:
                expression = override.license_expression
                license_origin = "reviewed-override"
                license_evidence = override.evidence
                license_reason = override.reason
        audited_packages.append(
            DependencyPackage(
                name=name,
                version=version,
                license_expression=expression,
                source=source,
                license_origin=license_origin,
                license_evidence=license_evidence,
                license_reason=license_reason,
            )
        )
        if not expression:
            blockers.append(LicenseFinding(name, version, None, source, "missing-license"))
            continue
        if _is_compound(expression):
            compounds.append(expression)
        if _requires_forbidden_license(expression):
            blockers.append(LicenseFinding(name, version, expression, source, "forbidden-license"))
    return DependencyReport(blockers, compounds, audited_packages)


def audit_cargo_metadata(
    metadata: dict[str, Any],
    license_overrides: Mapping[tuple[str, str, str], LicenseOverride] | None = None,
) -> DependencyReport:
    if not isinstance(metadata, Mapping):
        raise RuntimeError("Cargo metadata schema is invalid")
    packages = metadata.get("packages")
    workspace_members = metadata.get("workspace_members")
    if (
        not isinstance(packages, list)
        or not isinstance(workspace_members, list)
        or not all(isinstance(member, str) and member.strip() for member in workspace_members)
    ):
        raise RuntimeError("Cargo metadata schema is invalid")
    return _audit_packages(
        packages,
        "cargo",
        set(workspace_members),
        require_id=True,
        license_overrides=license_overrides,
    )


def _npm_license(component: dict[str, Any]) -> str | None:
    for field in ("license", "licenseDeclared", "licenseConcluded"):
        value = component.get(field)
        if isinstance(value, str) and value.strip():
            return value.strip()
    licenses = component.get("licenses")
    if licenses is None:
        return None
    if not isinstance(licenses, list):
        raise RuntimeError("npm licence schema is invalid")
    expressions: list[str] = []
    for entry in licenses:
        if not isinstance(entry, (str, Mapping)):
            raise RuntimeError("npm licence schema is invalid")
        candidate = entry.get("license", entry) if isinstance(entry, dict) else entry
        if isinstance(candidate, dict):
            candidate = candidate.get("id") or candidate.get("name") or candidate.get("expression")
        if isinstance(candidate, str) and candidate.strip():
            expressions.append(candidate.strip())
        else:
            raise RuntimeError("npm licence schema is invalid")
    return " OR ".join(expressions) if expressions else None


def audit_npm_sbom(
    sbom: dict[str, Any],
    license_overrides: Mapping[tuple[str, str, str], LicenseOverride] | None = None,
) -> DependencyReport:
    if not isinstance(sbom, Mapping):
        raise RuntimeError("npm SBOM schema is invalid")
    if "components" in sbom:
        packages = sbom["components"]
    elif "packages" in sbom:
        packages = sbom["packages"]
    else:
        raise RuntimeError("npm SBOM inventory is missing")
    if not isinstance(packages, list) or not all(isinstance(item, Mapping) for item in packages):
        raise RuntimeError("npm SBOM schema is invalid")
    normalized = [
        {
            "name": component.get("name"),
            "version": component.get("version"),
            "license": _npm_license(component),
        }
        for component in packages
        if isinstance(component, dict)
    ]
    return _audit_packages(normalized, "npm", license_overrides=license_overrides)


def _merge_reports(*reports: DependencyReport) -> DependencyReport:
    return DependencyReport(
        blockers=[blocker for report in reports for blocker in report.blockers],
        compound_expressions=sorted(
            expression for report in reports for expression in report.compound_expressions
        ),
        packages=sorted(
            (package for report in reports for package in report.packages),
            key=lambda package: (
                package.name,
                package.version,
                package.source,
                package.license_expression or "",
            ),
        ),
    )


def _run_json(command: list[str]) -> dict[str, Any]:
    completed = subprocess.run(command, check=False, text=True, capture_output=True)
    if completed.returncode != 0:
        raise RuntimeError("dependency inventory command failed")
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError("dependency inventory command returned invalid JSON") from error


def _run_tool_version(command: list[str]) -> str:
    completed = subprocess.run(command, check=False, text=True, capture_output=True)
    version = completed.stdout.strip()
    if (
        completed.returncode != 0
        or not version
        or "\n" in version
        or "\r" in version
        or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9 .()+_-]{0,127}", version)
    ):
        raise RuntimeError("dependency inventory tool version is unavailable")
    return version


def load_source_json(
    cargo_metadata_fixture: Path | None, npm_sbom_fixture: Path | None
) -> tuple[dict[str, Any], dict[str, Any], dict[str, str]]:
    cargo = (
        json.loads(cargo_metadata_fixture.read_text())
        if cargo_metadata_fixture
        else _run_json(CARGO_METADATA_COMMAND)
    )
    npm = json.loads(npm_sbom_fixture.read_text()) if npm_sbom_fixture else _run_json(NPM_SBOM_COMMAND)
    tool_versions = {
        "cargo": _run_tool_version(CARGO_VERSION_COMMAND),
        "npm": _run_tool_version(NPM_VERSION_COMMAND),
    }
    return cargo, npm, tool_versions


def _ensure_private_output(repository: Path, output: Path) -> Path:
    output = output.resolve()
    try:
        output.relative_to(repository.resolve())
    except ValueError:
        return output
    raise ValueError("report output must be outside the repository")


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo-metadata-fixture", type=Path)
    parser.add_argument("--npm-sbom-fixture", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--inventory-output", type=Path)
    args = parser.parse_args(argv)
    try:
        output = _ensure_private_output(REPOSITORY_ROOT, args.output)
        inventory_output = (
            _ensure_private_output(REPOSITORY_ROOT, args.inventory_output)
            if args.inventory_output
            else None
        )
        if inventory_output == output:
            raise ValueError("summary and inventory outputs must differ")
        cargo, npm, tool_versions = load_source_json(
            args.cargo_metadata_fixture, args.npm_sbom_fixture
        )
        license_overrides = load_license_overrides(LICENSE_OVERRIDES_PATH)
        report = _merge_reports(
            audit_cargo_metadata(cargo, license_overrides),
            audit_npm_sbom(npm, license_overrides),
        )
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(
            json.dumps(report.summary(tool_versions), indent=2, sort_keys=True) + "\n"
        )
        if inventory_output:
            inventory_output.parent.mkdir(parents=True, exist_ok=True)
            inventory_output.write_text(
                json.dumps(report.inventory(tool_versions), indent=2, sort_keys=True) + "\n"
            )
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError):
        print("dependency audit unavailable", file=sys.stderr)
        return 2
    print(f"dependency audit: {report.status}")
    return 0 if report.status == "ready" else 1


if __name__ == "__main__":
    raise SystemExit(main())
