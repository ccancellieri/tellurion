#!/usr/bin/env python3
"""Additively export the public gallery to a clean publication checkout.

Never deletes old URLs, pushes commits, or copies repository credentials.
"""
import argparse
import json
from pathlib import Path
import subprocess


ROOTS = ('.nojekyll', 'italy', 'index.html', 'styles.css', 'favicon.svg', 'LICENSE', 'NOTICE.md',
         'README.md', 'assets', 'demos', 'docs', 'proof', 'data', 'evidence',
         'releases', 'snapshots', 'scripts', 'tests', 'deploy', 'dist',
         'Dockerfile', 'Dockerfile-3d', 'Dockerfile-zarr', 'Dockerfile.raster',
         'Dockerfile.stac-harvest', 'render.yaml')


def plan_export(source, target):
    metadata = target / 'publication.json'
    if metadata.is_symlink() or (metadata.exists() and not metadata.is_file()):
        raise ValueError('Unsafe publication metadata destination')
    if not (source / 'index.html').is_file():
        raise ValueError('Source needs a gallery index')
    planned = {}
    for name in ROOTS:
        item = source / name
        if item.is_symlink():
            raise ValueError(f'source symlink: {name}')
        paths = item.rglob('*') if item.is_dir() else [item]
        for path in paths:
            relative = path.relative_to(source)
            if relative.as_posix() != '.nojekyll' and any(
                    part.startswith('.') or part == '__pycache__' for part in relative.parts):
                continue
            if path.is_symlink():
                raise ValueError(f'source symlink: {relative}')
            if not path.is_file():
                continue
            destination = target / relative
            for ancestor in (destination, *destination.parents):
                if ancestor == target.parent:
                    break
                if ancestor.is_symlink():
                    raise ValueError(f'destination symlink: {relative}')
            content = path.read_bytes()
            mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
            immutable = relative.parts[0] in ('snapshots', 'releases') and len(relative.parts) > 2
            if immutable and (target / relative.parts[0] / relative.parts[1]).exists():
                if (not destination.is_file() or destination.read_bytes() != content
                        or destination.stat().st_mode & 0o777 != mode):
                    raise ValueError(f'immutable version differs: {relative}')
            planned[relative] = (content, mode)
    return planned


def apply_export(target, planned):
    for relative, (content, mode) in planned.items():
        destination = target / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(content)
        destination.chmod(mode)


def git(root, *arguments):
    return subprocess.check_output(['git', '-C', str(root), *arguments], text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('target', type=Path)
    parser.add_argument('--apply', action='store_true', help='write files; default is a dry run')
    args = parser.parse_args()
    source = Path(__file__).resolve().parents[1]
    target = args.target.resolve()
    if source == target or source in target.parents or target in source.parents:
        raise ValueError('Source and target must be separate checkouts')
    if Path(git(target, 'rev-parse', '--show-toplevel')).resolve() != target:
        raise ValueError('Target must be a repository root')
    for root in (source, target):
        if git(root, 'status', '--porcelain', '--untracked-files=all'):
            raise ValueError(f'Commit or preserve local changes before export: {root}')
    planned = plan_export(source, target)
    revision = git(source, 'rev-parse', 'HEAD')
    if args.apply:
        apply_export(target, planned)
        (target / 'publication.json').write_text(json.dumps({
            'source_repository': 'https://github.com/ccancellieri/tellurion',
            'source_commit': revision,
            'source_directory': 'demo/gallery',
        }, indent=2) + '\n')
    print(f'{"Exported" if args.apply else "Would export"} {len(planned)} files from {revision}; no files deleted')


if __name__ == '__main__':
    main()
