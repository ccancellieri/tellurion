import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('publish_gallery', Path(__file__).resolve().parents[1] / 'scripts/publish_gallery.py')
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class PublicationTests(unittest.TestCase):
    def test_rejects_publication_metadata_symlink(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, target = Path(temporary) / 'source', Path(temporary) / 'target'
            source.mkdir()
            target.mkdir()
            (source / 'index.html').write_text('Gallery')
            outside = Path(temporary) / 'outside.json'
            outside.write_text('Do not overwrite')
            (target / 'publication.json').symlink_to(outside)
            with self.assertRaisesRegex(ValueError, 'publication metadata'):
                publisher.plan_export(source, target)
            self.assertEqual(outside.read_text(), 'Do not overwrite')

    def test_rejects_new_files_inside_existing_immutable_directories(self):
        for directory in ('snapshots/old', 'releases/v0.3.0'):
            with self.subTest(directory=directory), tempfile.TemporaryDirectory() as temporary:
                source, target = Path(temporary) / 'source', Path(temporary) / 'target'
                for root in (source, target):
                    (root / directory).mkdir(parents=True)
                (source / 'index.html').write_text('Gallery')
                (target / directory / 'index.html').write_text('Original')
                (source / directory / 'new.html').write_text('Injected')
                with self.assertRaisesRegex(ValueError, 'immutable'):
                    publisher.plan_export(source, target)
                self.assertFalse((target / directory / 'new.html').exists())

    def test_preserves_executable_scripts(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, target = Path(temporary) / 'source', Path(temporary) / 'target'
            (source / 'scripts').mkdir(parents=True)
            target.mkdir()
            (source / 'index.html').write_text('Gallery')
            script = source / 'scripts/run.sh'
            script.write_text('#!/bin/sh\nexit 0\n')
            script.chmod(0o755)
            publisher.apply_export(target, publisher.plan_export(source, target))
            self.assertEqual((target / 'scripts/run.sh').stat().st_mode & 0o777, 0o755)

    def test_preserves_old_versions_and_host_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, target = Path(temporary) / 'source', Path(temporary) / 'target'
            source.mkdir()
            (target / 'releases/older').mkdir(parents=True)
            (target / 'releases/older/index.html').write_text('Older')
            (target / '.gitignore').write_text('Host settings')
            (source / 'index.html').write_text('New gallery')
            (source / '.env').write_text('Must not export')
            publisher.apply_export(target, publisher.plan_export(source, target))
            self.assertEqual((target / 'releases/older/index.html').read_text(), 'Older')
            self.assertEqual((target / '.gitignore').read_text(), 'Host settings')
            self.assertFalse((target / '.env').exists())
            self.assertEqual((target / 'index.html').read_text(), 'New gallery')

    def test_rejects_overwriting_an_existing_snapshot_before_any_write(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, target = Path(temporary) / 'source', Path(temporary) / 'target'
            for root in (source, target):
                (root / 'snapshots/old').mkdir(parents=True)
            (source / 'index.html').write_text('Gallery')
            (source / 'snapshots/old/index.html').write_text('Changed')
            (target / 'snapshots/old/index.html').write_text('Original')
            with self.assertRaisesRegex(ValueError, 'immutable'):
                publisher.plan_export(source, target)
            self.assertEqual((target / 'snapshots/old/index.html').read_text(), 'Original')

    def test_rejects_destination_symlink(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, target = Path(temporary) / 'source', Path(temporary) / 'target'
            source.mkdir()
            target.mkdir()
            (source / 'index.html').write_text('New')
            (target / 'index.html').symlink_to(source / 'index.html')
            with self.assertRaisesRegex(ValueError, 'symlink'):
                publisher.plan_export(source, target)


if __name__ == '__main__':
    unittest.main()
