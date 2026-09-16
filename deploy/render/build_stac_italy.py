#!/usr/bin/env python3
"""Build the immutable local COG set used by the Render Italy demo."""

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
from urllib.parse import urlsplit
from urllib.request import Request, urlopen


OFFICIAL_HOST = "esa-worldcover.s3.eu-central-1.amazonaws.com"
SOURCE_ID = re.compile(r"^ESA_WorldCover_10m_2021_v200_[NS][0-9]{2}[EW][0-9]{3}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
MAX_SOURCES = 32
CHUNK_BYTES = 1024 * 1024


def _validated_sources(plan_path):
    plan_path = Path(plan_path)
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    if plan.get("id") != "esa_worldcover_2021_italy":
        raise ValueError("mosaic id must be esa_worldcover_2021_italy")
    sources = plan.get("sources")
    if not isinstance(sources, list) or not 1 <= len(sources) <= MAX_SOURCES:
        raise ValueError(f"mosaic must contain 1..{MAX_SOURCES} sources")

    ids = []
    for source in sources:
        source_id = source.get("id")
        if not isinstance(source_id, str) or not SOURCE_ID.fullmatch(source_id):
            raise ValueError(f"invalid source id: {source_id!r}")
        ids.append(source_id)

        href = source.get("href")
        if not isinstance(href, str):
            raise ValueError(f"source {source_id} has no href")
        parsed = urlsplit(href)
        path_parts = PurePosixPath(parsed.path).parts
        if (
            parsed.scheme != "https"
            or parsed.hostname != OFFICIAL_HOST
            or parsed.username is not None
            or parsed.password is not None
            or parsed.port is not None
            or parsed.query
            or parsed.fragment
            or ".." in path_parts
        ):
            raise ValueError(f"source {source_id} must use an unsigned official ESA URL")
        expected_name = f"{source_id}_Map.tif"
        if PurePosixPath(parsed.path).name != expected_name:
            raise ValueError(f"source {source_id} URL must end in {expected_name}")

        length = source.get("byte_length")
        digest = source.get("sha256")
        bbox = source.get("bbox")
        if not isinstance(length, int) or isinstance(length, bool) or length <= 0:
            raise ValueError(f"source {source_id} has an invalid byte_length")
        if not isinstance(digest, str) or not SHA256.fullmatch(digest):
            raise ValueError(f"source {source_id} has an invalid sha256")
        if (
            not isinstance(bbox, list)
            or len(bbox) != 4
            or not all(isinstance(value, (int, float)) and not isinstance(value, bool) for value in bbox)
            or bbox[0] >= bbox[2]
            or bbox[1] >= bbox[3]
        ):
            raise ValueError(f"source {source_id} has an invalid bbox")

    if ids != sorted(ids) or len(ids) != len(set(ids)):
        raise ValueError("mosaic sources must have unique ids in ascending order")
    return sources


def _measure(path):
    digest = hashlib.sha256()
    length = 0
    with Path(path).open("rb") as source_file:
        while chunk := source_file.read(CHUNK_BYTES):
            digest.update(chunk)
            length += len(chunk)
    return length, digest.hexdigest()


def _assert_measurement(path, source):
    length, digest = _measure(path)
    if length != source["byte_length"]:
        raise ValueError(
            f"source {source['id']} byte_length mismatch: expected {source['byte_length']}, got {length}"
        )
    if digest != source["sha256"]:
        raise ValueError(
            f"source {source['id']} sha256 mismatch: expected {source['sha256']}, got {digest}"
        )


def download_sources(plan_path, output_dir, opener=urlopen, timeout=90):
    sources = _validated_sources(plan_path)
    output_dir = Path(output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    paths = []

    for source in sources:
        filename = PurePosixPath(urlsplit(source["href"]).path).name
        target = output_dir / filename
        partial = output_dir / f"{filename}.part"
        if target.exists():
            _assert_measurement(target, source)
            paths.append(target)
            continue

        request = Request(source["href"], headers={"User-Agent": "tellurion-demos-build/1"})
        digest = hashlib.sha256()
        length = 0
        try:
            with opener(request, timeout=timeout) as response, partial.open("wb") as output:
                while chunk := response.read(CHUNK_BYTES):
                    output.write(chunk)
                    digest.update(chunk)
                    length += len(chunk)
            if length != source["byte_length"]:
                raise ValueError(
                    f"source {source['id']} byte_length mismatch: expected {source['byte_length']}, got {length}"
                )
            actual_digest = digest.hexdigest()
            if actual_digest != source["sha256"]:
                raise ValueError(
                    f"source {source['id']} sha256 mismatch: expected {source['sha256']}, got {actual_digest}"
                )
            partial.replace(target)
        except Exception:
            partial.unlink(missing_ok=True)
            raise
        paths.append(target)

    return paths


def author_mosaic(sources, output, ingest_binary, runner=subprocess.run):
    command = [str(ingest_binary), "cog", "mosaic"]
    for source in sorted(map(Path, sources)):
        command.extend(["--source", str(source)])
    command.extend(
        [
            "--output",
            str(output),
            "--collection",
            "esa_worldcover_2021_italy",
            "--catalog",
            "default",
            "--storage",
            "harvested_italy_mosaic",
        ]
    )
    runner(command, check=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--mosaic-manifest", type=Path, required=True)
    parser.add_argument("--ingest-binary", type=Path, required=True)
    args = parser.parse_args()

    sources = download_sources(args.manifest, args.output_dir)
    author_mosaic(sources, args.mosaic_manifest, args.ingest_binary)
    print(f"Verified {len(sources)} local WorldCover COGs and authored {args.mosaic_manifest}")


if __name__ == "__main__":
    main()
