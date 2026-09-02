#!/usr/bin/env python3
"""Create a private, redacted readiness report for every reachable Git object."""

from __future__ import annotations

import argparse
from collections import Counter
from collections.abc import Mapping
from dataclasses import asdict, dataclass, field
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from typing import Any, Iterable


MAX_TEXT_BLOB_BYTES = 10 * 1024 * 1024
READ_CHUNK_BYTES = 64 * 1024
REQUIRED_GITLEAKS_VERSION = "8.30.1"
GITHUB_NOREPLY_SUFFIX = "@users.noreply.github.com"
EMAIL_PATTERN = re.compile(rb"\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b", re.IGNORECASE)
PRIVATE_NETWORK_PATTERN = re.compile(
    rb"\b(?:10(?:\.\d{1,3}){3}|192\.168(?:\.\d{1,3}){2}|172\.(?:1[6-9]|2\d|3[01])(?:\.\d{1,3}){2})(?::\d+)?\b"
)
PRIVATE_KEY_PATTERN = re.compile(rb"-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----")
PRIVATE_CONTACT_PATTERN = re.compile(
    rb"\b(?:phone|mobile|telephone|tel)\s*[:=]\s*(?:\+?\d[\d .()/-]{6,}\d)", re.IGNORECASE
)
IDENTITY_PATTERN = re.compile(r"^(?:author|committer|tagger) .+ <([^>]+)>", re.MULTILINE)
SAFE_RULE_ID_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
SAFE_OBJECT_ID_PATTERN = re.compile(r"^[0-9a-f]{7,64}$")


@dataclass(frozen=True)
class Location:
    rule_id: str
    object_id: str
    path: str


@dataclass
class HistoryReport:
    branch_names: list[str]
    finding_counts: dict[str, int]
    locations: list[Location] = field(default_factory=list)
    tool_versions: dict[str, str] = field(default_factory=lambda: {"gitleaks": "8.30.1"})
    status: str = "ready"

    def as_dict(self) -> dict[str, Any]:
        return {
            "status": self.status,
            "branches": [_opaque_identifier("branch", branch) for branch in self.branch_names],
            "finding_counts": self.finding_counts,
            "locations": [asdict(location) for location in self.locations],
            "tool_versions": self.tool_versions,
        }


def _git(repository: Path, arguments: list[str]) -> bytes:
    completed = subprocess.run(
        ["git", *arguments], cwd=repository, check=True, capture_output=True
    )
    return completed.stdout


def _redacted_path(path: str) -> str:
    return _opaque_identifier("path", path)


def _opaque_identifier(kind: str, value: str) -> str:
    digest = hashlib.sha256(value.encode("utf-8", errors="replace")).hexdigest()[:16]
    return f"{kind}-{digest}"


def _safe_rule_id(value: Any) -> str:
    rendered = value if isinstance(value, str) else ""
    return rendered if SAFE_RULE_ID_PATTERN.fullmatch(rendered) else _opaque_identifier("rule", rendered)


def _safe_object_id(value: Any) -> str:
    rendered = value if isinstance(value, str) else ""
    return rendered if SAFE_OBJECT_ID_PATTERN.fullmatch(rendered) else _opaque_identifier("object", rendered)


def _ensure_private_output(repository: Path, output: Path) -> Path:
    resolved_repository = repository.resolve()
    resolved_output = output.resolve()
    try:
        resolved_output.relative_to(resolved_repository)
    except ValueError:
        return resolved_output
    raise ValueError("report output must be outside the repository")


def _reachable_objects(repository: Path) -> dict[str, str]:
    objects: dict[str, str] = {}
    for line in _git(repository, ["rev-list", "--objects", "--all"]).decode().splitlines():
        object_id, _, path = line.partition(" ")
        objects.setdefault(object_id, path)
    return objects


def _record_content_findings(
    data: bytes, object_id: str, path: str, counts: Counter[str], locations: list[Location]
) -> None:
    for rule_id, pattern in (
        ("email-in-content", EMAIL_PATTERN),
        ("private-network-url", PRIVATE_NETWORK_PATTERN),
        ("private-key-header", PRIVATE_KEY_PATTERN),
        ("private-contact", PRIVATE_CONTACT_PATTERN),
    ):
        if pattern.search(data):
            counts[rule_id] += 1
            locations.append(Location(rule_id, object_id, _redacted_path(path)))


def _record_metadata_findings(data: bytes, counts: Counter[str]) -> None:
    headers = data.partition(b"\n\n")[0].decode("utf-8", errors="replace")
    for mailbox in IDENTITY_PATTERN.findall(headers):
        if not mailbox.lower().endswith(GITHUB_NOREPLY_SUFFIX):
            counts["non-noreply-git-identity"] += 1


def _drain_payload(stream: Any, size: int) -> None:
    remaining = size
    while remaining:
        chunk = stream.read(min(READ_CHUNK_BYTES, remaining))
        if not chunk:
            raise RuntimeError("truncated Git object payload")
        remaining -= len(chunk)


def _read_payload(stream: Any, size: int) -> bytes:
    chunks: list[bytes] = []
    remaining = size
    while remaining:
        chunk = stream.read(min(READ_CHUNK_BYTES, remaining))
        if not chunk:
            raise RuntimeError("truncated Git object payload")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def _consume_frame(stream: Any) -> None:
    if stream.read(1) != b"\n":
        raise RuntimeError("invalid Git batch framing")


def _scan_blobs(
    repository: Path, objects: dict[str, str], counts: Counter[str], locations: list[Location]
) -> None:
    process = subprocess.Popen(
        ["git", "cat-file", "--batch"],
        cwd=repository,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    assert process.stdin and process.stdout
    for object_id, path in objects.items():
        process.stdin.write(f"{object_id}\n".encode())
        process.stdin.flush()
        header = process.stdout.readline().decode("ascii", errors="replace").strip()
        parts = header.split()
        if len(parts) < 3 or parts[1] == "missing":
            continue
        object_type, size_text = parts[1], parts[2]
        size = int(size_text)
        if object_type == "blob" and size > MAX_TEXT_BLOB_BYTES:
            _drain_payload(process.stdout, size)
            _consume_frame(process.stdout)
            counts["oversized-blob"] += 1
            locations.append(Location("oversized-blob", object_id, _redacted_path(path)))
            continue
        data = _read_payload(process.stdout, size)
        _consume_frame(process.stdout)
        if object_type in ("commit", "tag"):
            _record_metadata_findings(data, counts)
            continue
        if object_type != "blob":
            continue
        if b"\x00" in data:
            counts["binary-blob"] += 1
            locations.append(Location("binary-blob", object_id, _redacted_path(path)))
            continue
        _record_content_findings(data, object_id, path, counts, locations)

    process.stdin.close()
    process.stdout.close()
    if process.wait() != 0:
        raise RuntimeError("unable to read reachable Git objects")


def _gitleaks_findings(
    repository: Path, executable: Path, counts: Counter[str], locations: list[Location]
) -> None:
    with tempfile.NamedTemporaryFile(
        dir="/private/tmp", prefix="tellurion-gitleaks-", suffix=".json", delete=False
    ) as report_file:
        temporary_report = Path(report_file.name)
    try:
        completed = subprocess.run(
            [
                str(executable),
                "git",
                "--log-opts=--all",
                "--redact=100",
                "--no-banner",
                "--no-color",
                "--report-format=json",
                f"--report-path={temporary_report}",
                ".",
            ],
            cwd=repository,
            capture_output=True,
            text=True,
        )
        if completed.returncode not in (0, 1):
            raise RuntimeError("Gitleaks did not complete its audit")
        findings = json.loads(temporary_report.read_text()) if temporary_report.stat().st_size else []
    finally:
        temporary_report.unlink(missing_ok=True)

    if not isinstance(findings, list) or not all(isinstance(finding, Mapping) for finding in findings):
        raise RuntimeError("Gitleaks report schema is invalid")
    for finding in findings:
        required_values = (finding.get("RuleID"), finding.get("Commit"), finding.get("File"))
        if not all(isinstance(value, str) and value.strip() for value in required_values):
            raise RuntimeError("Gitleaks finding schema is invalid")
    for finding in findings:
        counts["gitleaks"] += 1
        locations.append(
            Location(
                _safe_rule_id(finding.get("RuleID")),
                _safe_object_id(finding.get("Commit")),
                _redacted_path(str(finding.get("File", ""))),
            )
        )


def _verify_gitleaks(executable: Path) -> str:
    completed = subprocess.run(
        [str(executable), "version"], capture_output=True, text=True
    )
    if completed.returncode != 0 or completed.stdout.strip() != REQUIRED_GITLEAKS_VERSION:
        raise RuntimeError("required Gitleaks version is unavailable")
    return REQUIRED_GITLEAKS_VERSION


def audit_repository(repository: Path, gitleaks_executable: Path, output: Path) -> HistoryReport:
    """Audit every local ref, retaining only redacted findings in ``output``."""
    repository = repository.resolve()
    output = _ensure_private_output(repository, output)
    gitleaks_version = _verify_gitleaks(gitleaks_executable)
    ref_lines = _git(repository, ["for-each-ref", "--format=%(refname)"]).decode().splitlines()
    branch_names = sorted(ref.removeprefix("refs/heads/") for ref in ref_lines if ref.startswith("refs/heads/"))
    counts: Counter[str] = Counter()
    locations: list[Location] = []
    objects = _reachable_objects(repository)
    _scan_blobs(repository, objects, counts, locations)
    _gitleaks_findings(repository, gitleaks_executable, counts, locations)
    report = HistoryReport(
        branch_names=branch_names,
        finding_counts=dict(sorted(counts.items())),
        locations=locations,
        tool_versions={"gitleaks": gitleaks_version},
        status="blocked" if counts else "ready",
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report.as_dict(), indent=2, sort_keys=True) + "\n")
    return report


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--repo", dest="repository", type=Path, default=Path("."))
    parser.add_argument("--gitleaks-bin", dest="gitleaks", type=Path, default=Path("gitleaks"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        report = audit_repository(args.repository, args.gitleaks, args.output)
    except (OSError, subprocess.CalledProcessError, ValueError, RuntimeError):
        print("history audit unavailable", file=sys.stderr)
        return 2
    print(f"history audit: {report.status}")
    return 0 if report.status == "ready" else 1


if __name__ == "__main__":
    raise SystemExit(main())
