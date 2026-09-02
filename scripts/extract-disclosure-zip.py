#!/usr/bin/env python3
"""Safely extract a downloaded Actions ZIP for private disclosure scanning."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import sys
import tempfile
import zipfile


@dataclass(frozen=True)
class ExtractionPolicy:
    max_entries: int = 10_000
    max_total_bytes: int = 2 * 1024 * 1024 * 1024
    max_path_depth: int = 16


def _unsafe() -> ValueError:
    return ValueError("unsafe ZIP archive")


def _validated_members(
    archive: zipfile.ZipFile, policy: ExtractionPolicy
) -> list[tuple[zipfile.ZipInfo, PurePosixPath]]:
    members = archive.infolist()
    if len(members) > policy.max_entries:
        raise _unsafe()

    total_bytes = 0
    targets: set[str] = set()
    validated: list[tuple[zipfile.ZipInfo, PurePosixPath]] = []
    for member in members:
        name = member.filename
        raw_parts = name.split("/")
        if (
            not name
            or "\\" in name
            or name.startswith("/")
            or re.match(r"^[A-Za-z]:", name)
            or any(part in {"", ".", ".."} for part in raw_parts if part or not name.endswith("/"))
        ):
            raise _unsafe()
        path = PurePosixPath(name)
        parts = tuple(part for part in path.parts if part not in {"", "."})
        if not parts or len(parts) > policy.max_path_depth:
            raise _unsafe()
        target_key = "/".join(parts).rstrip("/")
        if target_key in targets:
            raise _unsafe()
        targets.add(target_key)

        mode = member.external_attr >> 16
        file_type = stat.S_IFMT(mode)
        if stat.S_ISLNK(mode) or file_type not in (0, stat.S_IFREG, stat.S_IFDIR):
            raise _unsafe()
        if member.file_size < 0:
            raise _unsafe()
        total_bytes += member.file_size
        if total_bytes > policy.max_total_bytes:
            raise _unsafe()
        validated.append((member, PurePosixPath(*parts)))
    return validated


def extract_archive(
    archive_path: Path, destination: Path, policy: ExtractionPolicy
) -> None:
    archive_path = archive_path.resolve()
    destination = destination.resolve()
    if destination.exists() or destination.is_symlink():
        raise ValueError("extraction destination must not exist")
    destination.parent.mkdir(parents=True, exist_ok=True)

    temporary = Path(
        tempfile.mkdtemp(prefix=f".{destination.name}-", dir=destination.parent)
    )
    try:
        with zipfile.ZipFile(archive_path) as archive:
            members = _validated_members(archive, policy)
            extracted_bytes = 0
            for member, relative in members:
                target = temporary.joinpath(*relative.parts)
                if member.is_dir():
                    target.mkdir(parents=True, exist_ok=True)
                    continue
                target.parent.mkdir(parents=True, exist_ok=True)
                member_bytes = 0
                with archive.open(member) as source, target.open("xb") as output:
                    while chunk := source.read(1024 * 1024):
                        member_bytes += len(chunk)
                        extracted_bytes += len(chunk)
                        if (
                            member_bytes > member.file_size
                            or extracted_bytes > policy.max_total_bytes
                        ):
                            raise _unsafe()
                        output.write(chunk)
                if member_bytes != member.file_size:
                    raise _unsafe()
        temporary.rename(destination)
    except Exception:
        shutil.rmtree(temporary, ignore_errors=True)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("destination", type=Path)
    arguments = parser.parse_args()
    try:
        extract_archive(arguments.archive, arguments.destination, ExtractionPolicy())
    except (OSError, ValueError, zipfile.BadZipFile):
        print("disclosure ZIP extraction refused", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
