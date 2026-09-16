import io
import subprocess
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/release-artifacts.yml"


def aggregate_script() -> str:
    lines = WORKFLOW.read_text().splitlines()
    start = lines.index("      - name: Assemble aggregate checksums")
    body_start = start + 3
    end = next(index for index in range(body_start, len(lines))
               if lines[index].startswith("      - name: "))
    return "\n".join(line[10:] if line.startswith("          ") else line
                       for line in lines[body_start:end])


def write_large_archive_fixture(root: Path, *, include_rust_text: bool) -> None:
    dist = root / "dist"
    dist.mkdir()
    for name, contents in (
        ("tellurion.spdx.json", "{}"),
        ("THIRD_PARTY_NOTICES.json", "{}"),
        ("THIRD_PARTY_NOTICES.txt", "notice\n"),
    ):
        (dist / name).write_text(contents)

    source = dist / "tellurion-v0.5.0-source-test.zip"
    with zipfile.ZipFile(source, "w", compression=zipfile.ZIP_STORED) as archive:
        archive.writestr("THIRD_PARTY_NOTICES.json", "{}")
        archive.writestr("THIRD_PARTY_NOTICES.txt", "notice\n")

    tar_path = dist / "tellurion-v0.5.0-x86_64-unknown-linux-musl.tar.gz"
    with tarfile.open(tar_path, "w:gz") as archive:
        def add_tar(name: str, data: bytes = b"evidence") -> None:
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))

        add_tar("tellurion/THIRD_PARTY_NOTICES.json", b"{}")
        if include_rust_text:
            add_tar("tellurion/RUST_THIRD_PARTY_NOTICES.json", b"{}")
            add_tar("tellurion/RUST_THIRD_PARTY_NOTICES.txt")
        add_tar("tellurion/licenses/musl/musl-1.2.5.tar.gz")
        for index in range(12_000):
            add_tar(f"tellurion/filler/{index:05d}.txt", b"x")

    windows_path = dist / "tellurion-v0.5.0-x86_64-pc-windows-msvc.zip"
    with zipfile.ZipFile(windows_path, "w", compression=zipfile.ZIP_STORED) as archive:
        archive.writestr("tellurion/THIRD_PARTY_NOTICES.json", "{}")
        archive.writestr("tellurion/RUST_THIRD_PARTY_NOTICES.json", "{}")
        archive.writestr("tellurion/RUST_THIRD_PARTY_NOTICES.txt", "evidence")
        for index in range(12_000):
            archive.writestr(f"tellurion/filler/{index:05d}.txt", "x")


class ReleaseArchiveAggregationTests(unittest.TestCase):
    def run_aggregate(self, *, include_rust_text: bool) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory(prefix="tellurion-aggregate-") as directory:
            root = Path(directory)
            write_large_archive_fixture(root, include_rust_text=include_rust_text)
            return subprocess.run(
                ["bash", "-euo", "pipefail", "-c", aggregate_script()],
                cwd=root,
                text=True,
                capture_output=True,
            )

    def test_large_tar_and_zip_listings_consume_full_stream(self) -> None:
        result = self.run_aggregate(include_rust_text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_missing_required_native_notice_fails(self) -> None:
        result = self.run_aggregate(include_rust_text=False)
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
