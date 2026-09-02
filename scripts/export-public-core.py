#!/usr/bin/env python3
"""Create a deterministic public-core export from a clean Git worktree."""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
from fnmatch import fnmatchcase
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import tomllib


@dataclass(frozen=True)
class ExportPolicy:
    root_files: frozenset[str]
    include_directories: tuple[str, ...]
    exclude_globs: tuple[str, ...]


@dataclass(frozen=True)
class ExportedFile:
    path: str
    mode: str
    sha256: str


@dataclass(frozen=True)
class ExportManifest:
    candidate_commit: str
    copied_paths: tuple[str, ...]
    excluded_paths: tuple[str, ...]
    files: tuple[ExportedFile, ...]
    bare_issue_references: int

    def as_dict(self) -> dict[str, object]:
        return asdict(self)


def is_allowed(path: str, policy: ExportPolicy) -> bool:
    included = path in policy.root_files or any(
        path == root or path.startswith(root + "/")
        for root in policy.include_directories
    )
    denied = any(_matches_glob(path, pattern) for pattern in policy.exclude_globs)
    return included and not denied


def _matches_glob(path: str, pattern: str) -> bool:
    path_parts = PurePosixPath(path).parts
    pattern_parts = PurePosixPath(pattern).parts

    def matches(path_index: int, pattern_index: int) -> bool:
        if pattern_index == len(pattern_parts):
            return path_index == len(path_parts)
        if pattern_parts[pattern_index] == "**":
            return matches(path_index, pattern_index + 1) or (
                path_index < len(path_parts)
                and matches(path_index + 1, pattern_index)
            )
        return (
            path_index < len(path_parts)
            and fnmatchcase(path_parts[path_index], pattern_parts[pattern_index])
            and matches(path_index + 1, pattern_index + 1)
        )

    return matches(0, 0)


def load_policy(path: Path) -> ExportPolicy:
    with path.open("rb") as policy_file:
        contents = tomllib.load(policy_file)
    if contents.get("version") != 1:
        raise ValueError("unsupported export policy version")
    return ExportPolicy(
        root_files=frozenset(contents["root_files"]),
        include_directories=tuple(contents["include_directories"]),
        exclude_globs=tuple(contents["exclude_globs"]),
    )


def export_repository(
    source: Path,
    destination: Path,
    policy: ExportPolicy,
    *,
    allow_dirty: bool = False,
) -> ExportManifest:
    source = source.resolve()
    destination = destination.resolve()
    _validate_destination(source, destination)
    tracked_paths = _tracked_paths(source)
    _validate_policy(policy, tracked_paths)
    if not allow_dirty and _is_dirty(source):
        raise ValueError("source must be a clean Git worktree")

    copied_paths = tuple(path for path in tracked_paths if is_allowed(path, policy))
    excluded_paths = tuple(path for path in tracked_paths if path not in copied_paths)
    source_files = [(path, _source_file(source, path)) for path in copied_paths]

    destination.mkdir(parents=True)
    exported_files = []
    bare_issue_references = 0
    for relative_path, source_file in source_files:
        target = destination / relative_path
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source_file, target)
        exported_files.append(
            ExportedFile(
                path=relative_path,
                mode=oct(stat.S_IMODE(source_file.stat().st_mode)),
                sha256=_sha256(source_file),
            )
        )
        if target.suffix.lower() in {".md", ".markdown"}:
            bare_issue_references += len(_bare_issue_references(target))

    return ExportManifest(
        candidate_commit=_git_output(source, "rev-parse", "HEAD"),
        copied_paths=copied_paths,
        excluded_paths=excluded_paths,
        files=tuple(exported_files),
        bare_issue_references=bare_issue_references,
    )


def _validate_destination(source: Path, destination: Path) -> None:
    if destination == source or destination.is_relative_to(source):
        raise ValueError("destination must be empty and outside the source")
    if destination.exists():
        if not destination.is_dir() or any(destination.iterdir()):
            raise ValueError("destination must be empty and outside the source")


def _tracked_paths(source: Path) -> tuple[str, ...]:
    output = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=source,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    ).stdout
    return tuple(sorted(path.decode("utf-8") for path in output.split(b"\0") if path))


def _validate_policy(policy: ExportPolicy, tracked_paths: tuple[str, ...]) -> None:
    for path in (*policy.root_files, *policy.include_directories):
        pure_path = PurePosixPath(path)
        if (
            path in {"", "."}
            or pure_path.is_absolute()
            or ".." in pure_path.parts
            or (pure_path.parts and pure_path.parts[0] == ".git")
        ):
            raise ValueError("policy contains an unsafe path")
    for path in policy.root_files:
        if path not in tracked_paths:
            raise ValueError("policy includes a path not tracked by Git")
    for directory in policy.include_directories:
        if not any(
            path == directory or path.startswith(directory + "/")
            for path in tracked_paths
        ):
            raise ValueError("policy includes a directory with no tracked path")


def _is_dirty(source: Path) -> bool:
    return bool(_git_output(source, "status", "--porcelain", "--untracked-files=no"))


def _source_file(source: Path, relative_path: str) -> Path:
    source_file = source / relative_path
    resolved_file = source_file.resolve()
    if not resolved_file.is_relative_to(source):
        raise ValueError("tracked symlink escapes the source")
    if not resolved_file.is_file():
        raise ValueError("tracked path is not a regular file")
    return resolved_file


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _bare_issue_references(path: Path) -> tuple[str, ...]:
    import re

    return tuple(re.findall(r"(?<![\w/])#\d+\b", path.read_text(encoding="utf-8")))


def _git_output(source: Path, *arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments],
        cwd=source,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    ).stdout.strip()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    arguments = parser.parse_args()
    policy_path = Path(__file__).resolve().parents[1] / "distribution" / "public-core.toml"
    manifest = export_repository(
        arguments.source,
        arguments.destination,
        load_policy(policy_path),
    )
    arguments.manifest.parent.mkdir(parents=True, exist_ok=True)
    arguments.manifest.write_text(
        json.dumps(manifest.as_dict(), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
