#!/usr/bin/env python3
"""Behavioral tests for the private, redacted public-history audit."""

from __future__ import annotations

import contextlib
from collections import Counter
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).parents[1] / "audit-public-history.py"
SPEC = importlib.util.spec_from_file_location("audit_public_history", SCRIPT)
assert SPEC and SPEC.loader
audit_public_history = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = audit_public_history
SPEC.loader.exec_module(audit_public_history)


class PublicHistoryAuditTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.fixture_index = 0

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def run_git(self, repo: Path, *args: str) -> None:
        subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True)

    def make_history_fixture(
        self,
        *,
        gitleaks_version: str = "8.30.1",
        gitleaks_payload: object | None = None,
        gitleaks_report_text: str | None = None,
    ) -> tuple[Path, Path]:
        self.fixture_index += 1
        repo = self.root / f"history-fixture-{self.fixture_index}"
        repo.mkdir()
        self.run_git(repo, "init", "-b", "main")
        self.run_git(repo, "config", "user.name", "Fixture Author")
        self.run_git(repo, "config", "user.email", "audit-person@example.invalid")
        (repo / "contact.txt").write_text("Reach audit-person@example.invalid\n")
        (repo / "large.bin").write_bytes(b"x" * (10 * 1024 * 1024 + 1))
        (repo / "binary.bin").write_bytes(b"\x00\x01fixture\xff")
        self.run_git(repo, "add", ".")
        self.run_git(repo, "commit", "-m", "fixture main")
        self.run_git(repo, "switch", "-c", "sensitive-branch")
        (repo / "branch-secret.txt").write_text("credential=example-secret-value\n")
        self.run_git(repo, "add", "branch-secret.txt")
        self.run_git(repo, "commit", "-m", "fixture sensitive")
        self.run_git(repo, "switch", "main")

        fake_gitleaks = self.root / "fake-gitleaks"
        payload = gitleaks_payload if gitleaks_payload is not None else [
            {
                "RuleID": "fixture-secret-rule",
                "Commit": "0123456789abcdef",
                "File": "branch-secret.txt",
                "Secret": "example-secret-value",
                "Match": "example-secret-value",
            }
        ]
        fake_gitleaks.write_text(
            "#!/usr/bin/env python3\n"
            "import json, pathlib, sys\n"
            f"version = {gitleaks_version!r}\n"
            f"payload = {payload!r}\n"
            "if sys.argv[1:] == ['version']:\n"
            "    print(version)\n"
            "    raise SystemExit(0)\n"
            "report = next(arg.split('=', 1)[1] for arg in sys.argv if arg.startswith('--report-path='))\n"
            + (
                f"pathlib.Path(report).write_text({gitleaks_report_text!r})\n"
                if gitleaks_report_text is not None
                else "pathlib.Path(report).write_text(json.dumps(payload))\n"
            )
            + "raise SystemExit(1)\n"
        )
        fake_gitleaks.chmod(fake_gitleaks.stat().st_mode | stat.S_IXUSR)
        return repo, fake_gitleaks

    def private_output(self) -> Path:
        return self.root / "private-history-report.json"

    def test_all_refs_are_scanned_and_sensitive_values_stay_out_of_report_and_stdout(self) -> None:
        repo, fake_gitleaks = self.make_history_fixture()
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            report = audit_public_history.audit_repository(repo, fake_gitleaks, self.private_output())

        rendered = json.dumps(report.as_dict()) + stdout.getvalue()
        self.assertEqual({"main", "sensitive-branch"}, set(report.branch_names))
        self.assertGreater(report.finding_counts["gitleaks"], 0)
        self.assertIn("fixture-secret-rule", rendered)
        self.assertIn("0123456789abcdef", rendered)
        self.assertNotIn("branch-secret.txt", rendered)
        self.assertRegex(
            next(location.path for location in report.locations if location.rule_id == "fixture-secret-rule"),
            r"^path-[0-9a-f]{16}$",
        )
        for prohibited in (
            "example-secret-value",
            "audit-person@example.invalid",
            "Fixture Author",
            "example.invalid",
        ):
            self.assertNotIn(prohibited, rendered)
        self.assertGreater(report.finding_counts["email-in-content"], 0)
        self.assertGreater(report.finding_counts["non-noreply-git-identity"], 0)
        self.assertGreater(report.finding_counts["oversized-blob"], 0)
        self.assertGreater(report.finding_counts["binary-blob"], 0)
        self.assertTrue(self.private_output().is_file())

    def test_report_inside_repository_is_refused(self) -> None:
        repo, fake_gitleaks = self.make_history_fixture()
        with self.assertRaisesRegex(ValueError, "outside the repository"):
            audit_public_history.audit_repository(repo, fake_gitleaks, repo / "audit.json")

    def test_wrong_or_unparseable_gitleaks_version_is_a_redacted_prerequisite_error(self) -> None:
        for version in ("8.29.0-sensitive", "unparseable-version-secret"):
            with self.subTest(version_kind=version.split("-")[0]):
                repo, fake_gitleaks = self.make_history_fixture(gitleaks_version=version)
                completed = subprocess.run(
                    [
                        sys.executable,
                        str(SCRIPT),
                        "--repo",
                        str(repo),
                        "--gitleaks-bin",
                        str(fake_gitleaks),
                        "--output",
                        str(self.private_output()),
                    ],
                    text=True,
                    capture_output=True,
                )

                self.assertEqual(2, completed.returncode)
                self.assertNotIn(version, completed.stdout + completed.stderr)
                self.assertNotIn("Traceback", completed.stderr)
                self.assertFalse(self.private_output().exists())

    def test_adversarial_gitleaks_fields_are_opaque_and_json_safe(self) -> None:
        payload = [{
            "RuleID": "rule-attacker@example.invalid\n\"leaked-token",
            "Commit": "commit-attacker@example.invalid\rleaked-token",
            "File": "private/attacker@example.invalid/leaked-token.json\nnext",
            "Secret": "leaked-token",
        }]
        repo, fake_gitleaks = self.make_history_fixture(gitleaks_payload=payload)
        self.run_git(repo, "branch", "attacker@example.invalid")

        report = audit_public_history.audit_repository(repo, fake_gitleaks, self.private_output())

        rendered = json.dumps(report.as_dict())
        for prohibited in (
            "attacker@example.invalid",
            "example.invalid",
            "leaked-token",
            "private/",
        ):
            self.assertNotIn(prohibited, rendered)
        gitleaks_location = next(
            location for location in report.locations if location.rule_id.startswith("rule-")
        )
        self.assertRegex(gitleaks_location.rule_id, r"^rule-[0-9a-f]{16}$")
        self.assertRegex(gitleaks_location.object_id, r"^object-[0-9a-f]{16}$")
        self.assertRegex(gitleaks_location.path, r"^path-[0-9a-f]{16}$")

    def test_cli_returns_blocker_status_without_sensitive_stdout(self) -> None:
        repo, fake_gitleaks = self.make_history_fixture()
        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--repo",
                str(repo),
                "--gitleaks-bin",
                str(fake_gitleaks),
                "--output",
                str(self.private_output()),
            ],
            text=True,
            capture_output=True,
        )

        self.assertEqual(1, completed.returncode)
        self.assertIn("blocked", completed.stdout)
        self.assertEqual("", completed.stderr)
        for prohibited in (
            "example-secret-value",
            "audit-person@example.invalid",
            "Fixture Author",
            "example.invalid",
        ):
            self.assertNotIn(prohibited, completed.stdout + completed.stderr)

    def test_cli_accepts_the_exact_task_seven_flags(self) -> None:
        repo, fake_gitleaks = self.make_history_fixture()
        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--repo",
                str(repo),
                "--gitleaks-bin",
                str(fake_gitleaks),
                "--output",
                str(self.private_output()),
            ],
            text=True,
            capture_output=True,
        )

        self.assertEqual(1, completed.returncode, completed.stdout + completed.stderr)
        self.assertEqual("history audit: blocked\n", completed.stdout)
        self.assertEqual("", completed.stderr)
        self.assertTrue(self.private_output().is_file())

    def assert_cli_rejects_flag(self, *arguments: str) -> None:
        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                *arguments,
                "--output",
                str(self.private_output()),
            ],
            text=True,
            capture_output=True,
        )

        self.assertEqual(2, completed.returncode)
        self.assertIn("unrecognized arguments", completed.stderr)
        self.assertFalse(self.private_output().exists())

    def test_cli_rejects_obsolete_repository_flag(self) -> None:
        self.assert_cli_rejects_flag(
            "--repository", str(self.root), "--gitleaks-bin", str(self.root / "gitleaks")
        )

    def test_cli_rejects_abbreviated_repo_flag(self) -> None:
        self.assert_cli_rejects_flag(
            "--rep", str(self.root), "--gitleaks-bin", str(self.root / "gitleaks")
        )

    def test_cli_rejects_obsolete_gitleaks_flag(self) -> None:
        self.assert_cli_rejects_flag(
            "--repo", str(self.root), "--gitleaks", str(self.root / "gitleaks")
        )

    def test_cli_rejects_abbreviated_gitleaks_bin_flag(self) -> None:
        self.assert_cli_rejects_flag(
            "--repo", str(self.root), "--gitleaks-b", str(self.root / "gitleaks")
        )

    def test_blob_reader_streams_each_object_through_one_batch_process(self) -> None:
        class FakeStdout:
            def __init__(self) -> None:
                self.index = 0
                self.header_reads = 0

            def readline(self) -> bytes:
                self.header_reads += 1
                return f"object-{self.index} blob 4\n".encode()

            def read(self, size: int) -> bytes:
                if size == 1:
                    self.index += 1
                    return b"\n"
                return b"safe"

            def close(self) -> None:
                pass

            def flush(self) -> None:
                pass

        class FakeStdin:
            def __init__(self, stdout: FakeStdout) -> None:
                self.stdout = stdout
                self.writes = 0

            def write(self, _value: bytes) -> int:
                if self.writes > self.stdout.header_reads:
                    raise AssertionError("batch input was queued before output was consumed")
                self.writes += 1
                return 1

            def close(self) -> None:
                pass

            def flush(self) -> None:
                pass

        class FakeProcess:
            def __init__(self) -> None:
                self.stdout = FakeStdout()
                self.stdin = FakeStdin(self.stdout)

            def wait(self) -> int:
                return 0

        with mock.patch.object(audit_public_history.subprocess, "Popen", return_value=FakeProcess()):
            audit_public_history._scan_blobs(
                self.root,
                {"object-0": "first.txt", "object-1": "second.txt"},
                Counter(),
                [],
            )

    def test_oversized_blob_is_drained_with_bounded_reads(self) -> None:
        oversized = audit_public_history.MAX_TEXT_BLOB_BYTES + 17

        class LargeStdout:
            def __init__(self) -> None:
                self.remaining = oversized
                self.requests: list[int] = []

            def readline(self) -> bytes:
                return f"large-object blob {oversized}\n".encode()

            def read(self, size: int) -> bytes:
                self.requests.append(size)
                if size > 64 * 1024:
                    raise AssertionError("oversized blob was read without a bound")
                if self.remaining:
                    amount = min(size, self.remaining)
                    self.remaining -= amount
                    return b"x" * amount
                return b"\n"

            def close(self) -> None:
                pass

        class Input:
            def write(self, value: bytes) -> int:
                return len(value)

            def flush(self) -> None:
                pass

            def close(self) -> None:
                pass

        class Process:
            def __init__(self) -> None:
                self.stdout = LargeStdout()
                self.stdin = Input()

            def wait(self) -> int:
                return 0

        process = Process()
        counts: Counter[str] = Counter()
        locations: list[object] = []
        with mock.patch.object(audit_public_history.subprocess, "Popen", return_value=process):
            audit_public_history._scan_blobs(
                self.root, {"large-object": "large.bin"}, counts, locations
            )

        self.assertEqual(1, counts["oversized-blob"])
        self.assertEqual(0, process.stdout.remaining)
        self.assertLessEqual(max(process.stdout.requests), 64 * 1024)

    def test_identity_in_nested_annotated_tag_is_a_blocker(self) -> None:
        repo, fake_gitleaks = self.make_history_fixture()
        before = audit_public_history.audit_repository(repo, fake_gitleaks, self.private_output())
        self.run_git(repo, "config", "user.name", "GitHub Fixture")
        self.run_git(repo, "config", "user.email", "123@users.noreply.github.com")
        tag_environment = {
            **os.environ,
            "GIT_COMMITTER_NAME": "Nested Tag Fixture",
            "GIT_COMMITTER_EMAIL": "nested-tagger@example.internal",
        }
        subprocess.run(
            ["git", "tag", "-a", "inner-sensitive", "-m", "inner"],
            cwd=repo,
            env=tag_environment,
            check=True,
            capture_output=True,
        )
        self.run_git(repo, "tag", "-a", "outer-public", "inner-sensitive", "-m", "outer")
        self.run_git(repo, "tag", "-d", "inner-sensitive")

        after = audit_public_history.audit_repository(repo, fake_gitleaks, self.private_output())

        self.assertEqual(
            before.finding_counts["non-noreply-git-identity"] + 1,
            after.finding_counts["non-noreply-git-identity"],
        )
        rendered = json.dumps(after.as_dict())
        self.assertNotIn("nested-tagger@example.internal", rendered)
        self.assertNotIn("example.internal", rendered)
        self.assertNotIn("Nested Tag Fixture", rendered)

    def test_malformed_gitleaks_schema_is_a_redacted_prerequisite_error(self) -> None:
        for payload, report_text in (
            ({"attacker@example.invalid": "leaked-token"}, None),
            ([42], None),
            (None, '{"attacker@example.invalid": leaked-token}'),
        ):
            with self.subTest(payload_type=type(payload).__name__, invalid_json=report_text is not None):
                repo, fake_gitleaks = self.make_history_fixture(
                    gitleaks_payload=payload, gitleaks_report_text=report_text
                )
                completed = subprocess.run(
                    [
                        sys.executable,
                        str(SCRIPT),
                        "--repo",
                        str(repo),
                        "--gitleaks-bin",
                        str(fake_gitleaks),
                        "--output",
                        str(self.private_output()),
                    ],
                    text=True,
                    capture_output=True,
                )

                self.assertEqual(2, completed.returncode)
                self.assertNotIn("Traceback", completed.stderr)
                self.assertNotIn("attacker@example.invalid", completed.stdout + completed.stderr)
                self.assertNotIn("leaked-token", completed.stdout + completed.stderr)

    def test_gitleaks_findings_require_nonempty_typed_report_fields(self) -> None:
        findings = (
            {},
            {"RuleID": "", "Commit": "0123456", "File": "file.txt"},
            {"RuleID": "safe-rule", "Commit": "", "File": "file.txt"},
            {"RuleID": "safe-rule", "Commit": "0123456", "File": ""},
            {"RuleID": ["attacker@example.invalid"], "Commit": "0123456", "File": "file.txt"},
            {"RuleID": "safe-rule", "Commit": {"token": "leaked-token"}, "File": "file.txt"},
            {"RuleID": "safe-rule", "Commit": "0123456", "File": ["private-file"]},
        )
        for index, finding in enumerate(findings):
            with self.subTest(index=index):
                repo, fake_gitleaks = self.make_history_fixture(gitleaks_payload=[finding])
                output = self.root / f"invalid-finding-{index}.json"
                completed = subprocess.run(
                    [
                        sys.executable,
                        str(SCRIPT),
                        "--repo",
                        str(repo),
                        "--gitleaks-bin",
                        str(fake_gitleaks),
                        "--output",
                        str(output),
                    ],
                    text=True,
                    capture_output=True,
                )

                self.assertEqual(2, completed.returncode)
                self.assertEqual("history audit unavailable\n", completed.stderr)
                self.assertNotIn("Traceback", completed.stderr)
                self.assertNotIn("attacker@example.invalid", completed.stdout + completed.stderr)
                self.assertNotIn("leaked-token", completed.stdout + completed.stderr)
                self.assertFalse(output.exists())

    def test_history_report_write_error_is_redacted_prerequisite_error(self) -> None:
        repo, fake_gitleaks = self.make_history_fixture()
        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--repo",
                str(repo),
                "--gitleaks-bin",
                str(fake_gitleaks),
                "--output",
                str(self.root),
            ],
            text=True,
            capture_output=True,
        )

        self.assertEqual(2, completed.returncode)
        self.assertNotIn("Traceback", completed.stderr)
        self.assertEqual("history audit unavailable\n", completed.stderr)


if __name__ == "__main__":
    unittest.main()
