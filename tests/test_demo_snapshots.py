import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts' / 'demo_snapshots.py'
spec = importlib.util.spec_from_file_location('demo_snapshots', SCRIPT)
snapshots = importlib.util.module_from_spec(spec)
spec.loader.exec_module(snapshots)


class SnapshotTests(unittest.TestCase):
    def test_freezes_committed_files_not_dirty_worktree_and_detects_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            (root / 'index.html').write_text('<title>Original</title>')
            subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
            subprocess.run(['git', '-C', str(root), '-c', 'user.name=Test',
                            '-c', 'user.email=snapshot-fixture.invalid', 'commit', '-qm', 'Fixture'], check=True)
            (root / 'index.html').write_text('Uncommitted replacement')
            destination = root / 'snapshots' / 'test'
            snapshots.freeze(root, 'HEAD', destination)
            self.assertEqual((destination / 'index.html').read_text(), '<title>Original</title>')
            snapshots.verify(destination)
            with self.assertRaises(FileExistsError):
                snapshots.freeze(root, 'HEAD', destination)
            (destination / 'index.html').write_text('Changed')
            with self.assertRaisesRegex(ValueError, 'changed'):
                snapshots.verify(destination)

    def test_rejects_missing_and_extra_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'snapshot.json').write_text('{"files":{"index.html":"abc"}}')
            with self.assertRaisesRegex(ValueError, 'missing'):
                snapshots.verify(root)
            (root / 'index.html').write_text('x')
            (root / 'extra.js').write_text('x')
            with self.assertRaisesRegex(ValueError, 'unexpected'):
                snapshots.verify(root)


if __name__ == '__main__':
    unittest.main()
