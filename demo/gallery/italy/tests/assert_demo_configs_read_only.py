#!/usr/bin/env python3
import sys
from pathlib import Path

import yaml


def assert_read_only(path: Path) -> None:
    document = yaml.safe_load(path.read_text(encoding="utf-8"))
    collections = document.get("collections", []) if isinstance(document, dict) else []

    for index, collection in enumerate(collections):
        if not isinstance(collection, dict):
            continue
        routing = collection.get("routing")
        if isinstance(routing, dict) and "write" in routing:
            collection_id = collection.get("id", f"index {index}")
            raise ValueError(f"{path}: collection {collection_id} configures routing.write")


def main(paths: list[str]) -> int:
    try:
        for value in paths:
            assert_read_only(Path(value))
    except (OSError, ValueError, yaml.YAMLError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
