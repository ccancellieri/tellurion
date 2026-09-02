#!/usr/bin/env python3
"""Behavioral tests for bounded disclosure-surface ZIP extraction."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import stat
import sys
import tempfile
import unittest
import zipfile


SCRIPT = Path(__file__).parents[1] / "extract-disclosure-zip.py"


def load_extractor():
    spec = importlib.util.spec_from_file_location("extract_disclosure_zip", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load disclosure ZIP extractor")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class DisclosureZipExtractionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary_directory.name)
        self.extractor = load_extractor()

    def tearDown(self) -> None:
        self.temporary_directory.cleanup()

    def archive(self, name: str, entries: list[tuple[zipfile.ZipInfo | str, bytes]]) -> Path:
        archive = self.root / name
        with zipfile.ZipFile(archive, "w") as bundle:
            for entry, data in entries:
                bundle.writestr(entry, data)
        return archive

    def test_extracts_a_regular_file_for_disclosure_scanning(self) -> None:
        archive = self.archive(
            "actions-log.zip",
            [("nested/credential.txt", b"token=synthetic-credential-for-scanner\n")],
        )
        destination = self.root / "extracted"

        self.extractor.extract_archive(
            archive, destination, self.extractor.ExtractionPolicy()
        )

        self.assertEqual(
            "token=synthetic-credential-for-scanner\n",
            (destination / "nested" / "credential.txt").read_text(),
        )

    def test_rejects_traversal_absolute_and_symlink_entries_without_partial_output(self) -> None:
        symlink = zipfile.ZipInfo("link")
        symlink.create_system = 3
        symlink.external_attr = (stat.S_IFLNK | 0o777) << 16
        cases = (
            ("parent.zip", [("../escape.txt", b"escape")]),
            ("absolute.zip", [("/absolute.txt", b"escape")]),
            ("windows.zip", [(r"..\escape.txt", b"escape")]),
            ("symlink.zip", [(symlink, b"target")]),
        )
        for index, (name, entries) in enumerate(cases):
            with self.subTest(name=name):
                destination = self.root / f"unsafe-{index}"
                with self.assertRaisesRegex(ValueError, "unsafe ZIP archive"):
                    self.extractor.extract_archive(
                        self.archive(name, entries),
                        destination,
                        self.extractor.ExtractionPolicy(),
                    )
                self.assertFalse(destination.exists())

    def test_rejects_entry_count_total_size_and_path_depth_limits(self) -> None:
        cases = (
            (
                "entries.zip",
                [("one.txt", b"1"), ("two.txt", b"2")],
                self.extractor.ExtractionPolicy(max_entries=1),
            ),
            (
                "size.zip",
                [("large.txt", b"12345")],
                self.extractor.ExtractionPolicy(max_total_bytes=4),
            ),
            (
                "depth.zip",
                [("one/two/three.txt", b"deep")],
                self.extractor.ExtractionPolicy(max_path_depth=2),
            ),
        )
        for index, (name, entries, policy) in enumerate(cases):
            with self.subTest(name=name):
                destination = self.root / f"limited-{index}"
                with self.assertRaisesRegex(ValueError, "unsafe ZIP archive"):
                    self.extractor.extract_archive(
                        self.archive(name, entries), destination, policy
                    )
                self.assertFalse(destination.exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
