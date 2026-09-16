#!/usr/bin/env python3
"""Dependency-free structural and local-link checks for the static site."""

from html.parser import HTMLParser
from pathlib import Path
import re
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[1]
PAGES = (
    ROOT / "index.html",
    ROOT / "docs" / "index.html",
    ROOT / "proof" / "index.html",
    ROOT / "demos" / "stac" / "index.html",
) + tuple(sorted((ROOT / 'releases').rglob('*.html')))


class PageParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.ids: set[str] = set()
        self.hrefs: list[str] = []
        self.landmarks: dict[str, int] = {name: 0 for name in ("header", "nav", "main", "footer")}
        self.lang = ""
        self.title_depth = 0
        self.title_text = ""

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = dict(attrs)
        if tag == "html":
            self.lang = values.get("lang") or ""
        if tag in self.landmarks:
            self.landmarks[tag] += 1
        if tag == "a" and values.get("href"):
            self.hrefs.append(values["href"] or "")
        if values.get("id"):
            self.ids.add(values["id"] or "")
        if tag == "title":
            self.title_depth += 1

    def handle_endtag(self, tag: str) -> None:
        if tag == "title":
            self.title_depth -= 1

    def handle_data(self, data: str) -> None:
        if self.title_depth:
            self.title_text += data


def parse(path: Path) -> PageParser:
    parser = PageParser()
    parser.feed(path.read_text(encoding="utf-8"))
    return parser


def local_target(page: Path, href: str) -> tuple[Path, str] | None:
    split = urlsplit(href)
    if split.scheme or split.netloc or href.startswith(("mailto:", "tel:")):
        return None
    raw_path = unquote(split.path)
    target = (page.parent / raw_path).resolve() if raw_path else page
    if target.is_dir():
        target /= "index.html"
    return target, unquote(split.fragment)


def main() -> None:
    failures: list[str] = []
    parsed: dict[Path, PageParser] = {}
    for page in PAGES:
        if not page.exists():
            failures.append(f"{page.relative_to(ROOT)}: missing public page")
            continue
        parsed[page] = parse(page)

    for page, document in parsed.items():
        relative = page.relative_to(ROOT)
        if document.lang != "en":
            failures.append(f"{relative}: html lang must be en")
        if not document.title_text.strip():
            failures.append(f"{relative}: non-empty title required")
        for landmark, count in document.landmarks.items():
            if count < 1:
                failures.append(f"{relative}: missing {landmark} landmark")
        for href in document.hrefs:
            target_url = urlsplit(href)
            resolved = local_target(page, href)
            if resolved is None:
                continue
            target, fragment = resolved
            if not target.exists():
                failures.append(f"{relative}: missing local target {href}")
                continue
            if fragment and target.suffix.lower() in {".html", ""}:
                target_doc = parsed.get(target) or parse(target)
                if fragment not in target_doc.ids:
                    failures.append(f"{relative}: missing fragment target {href}")

    expected = {
        line.strip()
        for line in (ROOT / "tests/fixtures/docs_external_links.txt").read_text().splitlines()
        if line.strip() and not line.startswith("#")
    }
    docs_page = ROOT / "docs" / "index.html"
    if docs_page in parsed:
        docs_hrefs = set(parsed[docs_page].hrefs)
        missing = sorted(expected - docs_hrefs)
        if missing:
            failures.append("docs/index.html missing checked external links: " + ", ".join(missing))

    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    checker = "https://github.com/ccancellieri/tellurion/blob/main/scripts/check-gallery.sh"
    if checker not in readme:
        failures.append("README.md: missing canonical gallery checker link")
    if not (ROOT.parents[1] / "scripts" / "check-gallery.sh").is_file():
        failures.append("README.md: gallery checker link targets missing file")

    if failures:
        raise SystemExit("\n".join(failures))
    print("static site links and semantics: ok")


if __name__ == "__main__":
    main()
