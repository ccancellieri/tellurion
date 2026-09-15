#!/usr/bin/env python3
"""Inspect canonical GitHub Actions structure and enforce exact permissions."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys


READ_ONLY = {"contents": "read"}
AGGREGATION = {
    "attestations": "write",
    "contents": "read",
    "id-token": "write",
}
PUBLISH = {
    "actions": "read",
    "contents": "read",
    "id-token": "write",
}
PUBLISH_VERIFY = {
    "actions": "read",
    "contents": "read",
}
JOB_KEY = re.compile(r"  ([A-Za-z0-9_-]+):$")
PERMISSION_ENTRY = re.compile(r"([a-z][a-z-]*): (read|write|none)$")
MAPPING_ENTRY = re.compile(
    r"^(?:(?P<sequence>-[ ]+))?(?P<key>[A-Za-z0-9_-]+|\"[^\"]*\"|'[^']*')"
    r"[ ]*:[ ]*(?P<value>.*)$"
)
BLOCK_SCALAR = re.compile(
    r"^[|>](?:(?P<indent_first>[1-9])(?P<chomp_after>[+-])?"
    r"|(?P<chomp_first>[+-])(?P<indent_after>[1-9])?)?$"
)
SEQUENCE_BLOCK_SCALAR = re.compile(
    r"^-[ ]+(?P<value>[|>](?:[1-9][+-]?|[+-][1-9]?|[+-])?)$"
)
EXPLICIT_KEY = re.compile(
    r"^\?[ ]+(?P<key>[A-Za-z0-9_-]+|\"[^\"]*\"|'[^']*')[ ]*$"
)
CANONICAL_ACTION = re.compile(
    r"^[ ]*(?:-[ ]+)?uses:[ ]+(?P<ref>[^\s#]+)[ ]+#[ ]+"
    r"(?P<comment>[^\s#]+)[ ]*$"
)


def _unquoted_comment(line: str) -> str:
    single_quoted = False
    double_quoted = False
    escaped = False
    for index, character in enumerate(line):
        if double_quoted and escaped:
            escaped = False
            continue
        if double_quoted and character == "\\":
            escaped = True
            continue
        if not double_quoted and character == "'":
            single_quoted = not single_quoted
            continue
        if not single_quoted and character == '"':
            double_quoted = not double_quoted
            continue
        if (
            character == "#"
            and not single_quoted
            and not double_quoted
            and (index == 0 or line[index - 1].isspace())
        ):
            return line[:index].rstrip()
    return line.rstrip()


def _key_name(token: str) -> str:
    return token[1:-1] if token[:1] in {"'", '"'} and token[-1:] == token[:1] else token


def _block_scalar_indent(value: str) -> int | None:
    match = BLOCK_SCALAR.fullmatch(value)
    if not match:
        return None
    indicator = match.group("indent_first") or match.group("indent_after")
    return int(indicator) if indicator else 0


def _flow_mapping_source(syntax: str) -> str | None:
    body = syntax
    if body.startswith("- "):
        body = body[2:].lstrip(" ")
    if body.startswith("{"):
        return body
    mapping = MAPPING_ENTRY.fullmatch(syntax)
    if mapping and mapping.group("value").startswith("{"):
        return mapping.group("value")
    return None


def _quoted_end(source: str, start: int) -> int:
    quote = source[start]
    index = start + 1
    escaped = False
    while index < len(source):
        character = source[index]
        if quote == '"' and escaped:
            escaped = False
        elif quote == '"' and character == "\\":
            escaped = True
        elif character == quote:
            if quote == "'" and index + 1 < len(source) and source[index + 1] == "'":
                index += 1
            else:
                return index + 1
        index += 1
    return index


def _flow_mapping_keys(source: str) -> list[str]:
    keys: list[str] = []
    index = 0
    while index < len(source):
        if source[index] in "'\"":
            index = _quoted_end(source, index)
            continue
        if source[index] not in "{,":
            index += 1
            continue
        index += 1
        while index < len(source) and source[index] == " ":
            index += 1
        if index >= len(source):
            break

        if source[index] in "'\"":
            start = index
            index = _quoted_end(source, index)
            token = source[start:index]
        else:
            start = index
            while index < len(source) and (source[index].isalnum() or source[index] in "_-"):
                index += 1
            token = source[start:index]

        while index < len(source) and source[index] == " ":
            index += 1
        if token and index < len(source) and source[index] == ":":
            keys.append(_key_name(token))
    return keys


def _structural_lines(path: Path) -> list[str]:
    structural: list[str] = []
    scalar_key_indent: int | None = None
    scalar_content_indent: int | None = None
    for line in path.read_text(encoding="utf-8").splitlines():
        leading_whitespace = line[: len(line) - len(line.lstrip())]
        if "\t" in leading_whitespace:
            raise ValueError("noncanonical workflow indentation")
        stripped = line.strip()
        indent = len(line) - len(line.lstrip(" "))

        if scalar_key_indent is not None:
            if not stripped:
                continue
            if scalar_content_indent is None:
                if indent > scalar_key_indent:
                    scalar_content_indent = indent
                    continue
            elif indent >= scalar_content_indent:
                continue
            scalar_key_indent = None
            scalar_content_indent = None

        syntax = _unquoted_comment(line).strip()
        if not syntax:
            structural.append(line)
            continue
        if "\t" in syntax:
            raise ValueError("noncanonical workflow whitespace")

        mapping = MAPPING_ENTRY.fullmatch(syntax)
        if mapping:
            value = mapping.group("value")
            scalar_indicator = _block_scalar_indent(value)
            if scalar_indicator is not None:
                scalar_key_indent = indent + len(mapping.group("sequence") or "")
                scalar_content_indent = (
                    scalar_key_indent + scalar_indicator if scalar_indicator else None
                )
        else:
            sequence_scalar = SEQUENCE_BLOCK_SCALAR.fullmatch(syntax)
            if sequence_scalar:
                scalar_indicator = _block_scalar_indent(sequence_scalar.group("value"))
                if scalar_indicator is None:
                    raise ValueError("invalid block scalar indicator")
                scalar_key_indent = indent
                scalar_content_indent = indent + scalar_indicator if scalar_indicator else None

        structural.append(line)
    return structural


def action_records(paths: list[Path]) -> list[tuple[str, str]]:
    records: list[tuple[str, str]] = []
    for path in paths:
        for line in _structural_lines(path):
            syntax = _unquoted_comment(line).strip()
            if not syntax:
                continue
            flow_source = _flow_mapping_source(syntax)
            if flow_source and "uses" in _flow_mapping_keys(flow_source):
                records.append(("invalid", line.strip()))
                continue
            mapping = MAPPING_ENTRY.fullmatch(syntax)
            if mapping and _key_name(mapping.group("key")) == "uses":
                canonical = CANONICAL_ACTION.fullmatch(line)
                if canonical:
                    records.append((canonical.group("ref"), canonical.group("comment")))
                else:
                    records.append(("invalid", line.strip()))
                continue
            explicit_key = EXPLICIT_KEY.fullmatch(syntax)
            if explicit_key and _key_name(explicit_key.group("key")) == "uses":
                records.append(("invalid", line.strip()))
    return records


def permission_blocks(path: Path) -> tuple[list[dict[str, str]], dict[str, list[dict[str, str]]]]:
    lines = _structural_lines(path)
    workflow_blocks: list[dict[str, str]] = []
    job_blocks: dict[str, list[dict[str, str]]] = {}
    current_job: str | None = None
    in_jobs = False
    index = 0
    while index < len(lines):
        line = lines[index]
        stripped = line.strip()
        indent = len(line) - len(line.lstrip(" "))
        syntax = _unquoted_comment(line).strip()
        mapping_entry = MAPPING_ENTRY.fullmatch(syntax)
        flow_source = _flow_mapping_source(syntax)
        explicit_key = EXPLICIT_KEY.fullmatch(syntax)
        if flow_source and "permissions" in _flow_mapping_keys(flow_source):
            raise ValueError("noncanonical permissions mapping")
        if explicit_key and _key_name(explicit_key.group("key")) == "permissions":
            raise ValueError("noncanonical permissions mapping")
        if mapping_entry and _key_name(mapping_entry.group("key")) == "permissions":
            if syntax != "permissions:" or indent not in {0, 4}:
                raise ValueError("noncanonical permissions mapping")
        if line.startswith("\t"):
            raise ValueError("noncanonical workflow indentation")
        if indent == 0 and stripped == "jobs:":
            in_jobs = True
            current_job = None
        elif indent == 0 and stripped and not stripped.startswith("#"):
            current_job = None
        elif in_jobs:
            match = JOB_KEY.fullmatch(line)
            if match:
                current_job = match.group(1)

        if stripped == "permissions:":
            if indent == 0:
                target = workflow_blocks
            elif current_job:
                target = job_blocks.setdefault(current_job, [])
            else:
                raise ValueError("permissions mapping outside a workflow or job")

            mapping: dict[str, str] = {}
            index += 1
            while index < len(lines):
                child = lines[index]
                child_stripped = child.strip()
                child_indent = len(child) - len(child.lstrip(" "))
                if child_stripped and not child_stripped.startswith("#") and child_indent <= indent:
                    index -= 1
                    break
                if child_stripped and not child_stripped.startswith("#"):
                    if child_indent != indent + 2:
                        raise ValueError("noncanonical permissions mapping")
                    entry = PERMISSION_ENTRY.fullmatch(child_stripped)
                    if not entry or entry.group(1) in mapping:
                        raise ValueError("noncanonical permissions mapping")
                    mapping[entry.group(1)] = entry.group(2)
                index += 1
            if not mapping:
                raise ValueError("empty permissions mapping")
            target.append(mapping)
        index += 1
    return workflow_blocks, job_blocks


def require_read_only(path: Path) -> None:
    workflow_blocks, job_blocks = permission_blocks(path)
    if workflow_blocks != [READ_ONLY] or job_blocks:
        raise ValueError("workflow permissions are not exact read-only")


def require_release(path: Path) -> None:
    workflow_blocks, job_blocks = permission_blocks(path)
    if workflow_blocks != [READ_ONLY] or job_blocks != {"release-candidate": [AGGREGATION]}:
        raise ValueError("release permissions are not exact canonical mappings")


def require_publish(path: Path) -> None:
    workflow_blocks, job_blocks = permission_blocks(path)
    if workflow_blocks != [READ_ONLY] or job_blocks != {
        "publish": [PUBLISH],
        "verify": [PUBLISH_VERIFY],
    }:
        raise ValueError("publish permissions are not exact canonical mappings")


def workflow_paths(directory: Path) -> list[Path]:
    return sorted(
        path
        for path in directory.rglob("*")
        if path.is_file() and path.suffix in {".yml", ".yaml"}
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--read-only-workflow", type=Path, action="append", default=[])
    parser.add_argument("--release-workflow", type=Path)
    parser.add_argument("--publish-workflow", type=Path)
    parser.add_argument("--workflow-dir", type=Path)
    parser.add_argument("--list-actions", type=Path, nargs="+")
    arguments = parser.parse_args()

    if arguments.list_actions:
        try:
            records = action_records(arguments.list_actions)
        except (OSError, ValueError):
            records = [("invalid", "workflow structure")]
        for action, comment in records:
            print(f"{action}\t{comment}")
        return 0

    try:
        if arguments.workflow_dir:
            discovered = workflow_paths(arguments.workflow_dir)
            if not discovered:
                raise ValueError("no workflow files")
            release_workflow = arguments.workflow_dir / "release-artifacts.yml"
            publish_workflow = arguments.workflow_dir / "publish-crates.yml"
            for workflow in discovered:
                if workflow == release_workflow:
                    require_release(workflow)
                elif workflow == publish_workflow:
                    require_publish(workflow)
                else:
                    require_read_only(workflow)
        for workflow in arguments.read_only_workflow:
            require_read_only(workflow)
        if arguments.release_workflow:
            require_release(arguments.release_workflow)
        if arguments.publish_workflow:
            require_publish(arguments.publish_workflow)
    except (OSError, ValueError):
        print("FAIL: workflow permissions must use exact canonical mappings", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
