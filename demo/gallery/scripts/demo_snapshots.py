#!/usr/bin/env python3
"""Freeze public gallery files from an exact Git tree; verify retained bytes."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


PUBLIC_PATHS = ('index.html', 'styles.css', 'favicon.svg', 'assets', 'demos',
                'docs', 'proof', 'data', 'evidence', 'LICENSE', 'NOTICE.md')


def git(root, *arguments):
    return subprocess.check_output(['git', '-C', str(root), *arguments])


def freeze(root, revision, destination):
    commit = git(root, 'rev-parse', '--verify', f'{revision}^{{commit}}').decode().strip()
    entries = git(root, 'ls-tree', '-rz', commit, '--', *PUBLIC_PATHS).split(b'\0')
    files = {}
    for entry in filter(None, entries):
        header, name = entry.split(b'\t', 1)
        mode, kind, oid = header.decode().split()
        path = name.decode()
        if kind != 'blob' or mode not in ('100644', '100755'):
            raise ValueError(f'Unsupported snapshot entry: {path}')
        files[path] = git(root, 'cat-file', 'blob', oid)
    if 'index.html' not in files:
        raise ValueError('Snapshot needs a gallery index')
    destination.mkdir(parents=True, exist_ok=False)
    for name, content in files.items():
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
    manifest = {'gallery_commit': commit, 'files': {
        name: hashlib.sha256(content).hexdigest() for name, content in sorted(files.items())}}
    (destination / 'snapshot.json').write_text(json.dumps(manifest, indent=2) + '\n')


def verify(destination):
    manifest = json.loads((destination / 'snapshot.json').read_text())
    expected = manifest['files']
    actual = {str(path.relative_to(destination)) for path in destination.rglob('*')
              if path.is_file() and path != destination / 'snapshot.json'}
    if set(expected) - actual:
        raise ValueError(f'missing snapshot files: {sorted(set(expected) - actual)}')
    if actual - set(expected):
        raise ValueError(f'unexpected snapshot files: {sorted(actual - set(expected))}')
    for name, digest in expected.items():
        path = destination / name
        if path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f'changed snapshot file: {name}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--freeze', metavar='GIT_REVISION')
    parser.add_argument('--source-root', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    if args.freeze:
        freeze(args.source_root, args.freeze, args.directory)
    verify(args.directory)
    print(f'Verified snapshot: {args.directory}')


if __name__ == '__main__':
    main()
