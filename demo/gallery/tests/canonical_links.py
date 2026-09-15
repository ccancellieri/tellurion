#!/usr/bin/env python3
import argparse
from collections import Counter
import json
from pathlib import Path
import sys
from urllib.parse import parse_qsl, urlsplit


class LinkContractError(ValueError):
    pass


def _links(document: object) -> list[dict[str, object]]:
    if not isinstance(document, dict) or not isinstance(document.get("links"), list):
        raise LinkContractError("response must contain a links array")
    links = document["links"]
    if not links or not all(isinstance(link, dict) for link in links):
        raise LinkContractError("response links must be non-empty objects")
    return links


def _canonical_href(href: object, expected_base: str, rel: object) -> str:
    if not isinstance(href, str):
        raise LinkContractError(f"{rel!r} link must have a string href")

    expected = urlsplit(expected_base)
    actual = urlsplit(href)
    if not actual.scheme or not actual.netloc:
        raise LinkContractError(f"{rel!r} link must be absolute: {href!r}")
    if (actual.scheme, actual.netloc) != (expected.scheme, expected.netloc):
        raise LinkContractError(
            f"{rel!r} link must use the configured origin {expected.scheme}://{expected.netloc}: {href!r}"
        )
    return href


def _link_by_rel(links: list[dict[str, object]], rel: str) -> str:
    matches = [link for link in links if link.get("rel") == rel]
    if len(matches) != 1:
        raise LinkContractError(f"response must contain exactly one {rel!r} link")
    href = matches[0].get("href")
    if not isinstance(href, str):
        raise LinkContractError(f"{rel!r} link must have a string href")
    return href


def validate_landing(document: object, expected_base: str) -> None:
    for link in _links(document):
        _canonical_href(link.get("href"), expected_base, link.get("rel"))


def validate_items(document: object, expected_base: str, request_url: str) -> None:
    links = _links(document)
    self_href = _canonical_href(_link_by_rel(links, "self"), expected_base, "self")
    next_href = _canonical_href(_link_by_rel(links, "next"), expected_base, "next")

    request = urlsplit(request_url)
    self_link = urlsplit(self_href)
    next_link = urlsplit(next_href)
    if self_link.path != request.path:
        raise LinkContractError("self link must preserve the requested path")
    if next_link.path != request.path:
        raise LinkContractError("next link must preserve the requested path")

    request_query = Counter(parse_qsl(request.query, keep_blank_values=True))
    self_query = Counter(parse_qsl(self_link.query, keep_blank_values=True))
    next_query = Counter(parse_qsl(next_link.query, keep_blank_values=True))
    if self_query != request_query:
        raise LinkContractError("self link must preserve the complete request query")
    if any(next_query[pair] < count for pair, count in request_query.items()):
        raise LinkContractError("next link must preserve the complete request query")

    tokens = [value for key, value in parse_qsl(next_link.query, keep_blank_values=True) if key == "token"]
    if len(tokens) != 1 or not tokens[0]:
        raise LinkContractError("next link must contain one non-empty pagination token")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Verify canonical links in a Tellurion JSON response")
    parser.add_argument("--kind", choices=("landing", "items"), required=True)
    parser.add_argument("--body", type=Path, required=True)
    parser.add_argument("--expected-base", required=True)
    parser.add_argument("--request-url", required=True)
    args = parser.parse_args(argv)

    try:
        document = json.loads(args.body.read_text(encoding="utf-8"))
        if args.kind == "landing":
            validate_landing(document, args.expected_base)
        else:
            validate_items(document, args.expected_base, args.request_url)
    except (OSError, json.JSONDecodeError, LinkContractError) as error:
        print(f"canonical link contract failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
