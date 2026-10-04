import argparse
import json
import os
import re
import stat
import sys
import tempfile
import tomllib
from pathlib import Path


NUMBER = r'(?:0|[1-9][0-9]*)'
PRE = r'(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)'
SEMVER = re.compile(rf'{NUMBER}\.{NUMBER}\.{NUMBER}(?:-{PRE}(?:\.{PRE})*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?')


class VersionMismatch(ValueError):
    pass


def local_file(root, relative):
    current = root
    for part in Path(relative).parts:
        current /= part
        if current.is_symlink():
            raise ValueError('Version files must not follow symlinks')
    if not current.is_file():
        raise ValueError('Version file is missing: ' + relative)
    return current


def workspace_version(root):
    source = local_file(root, 'Cargo.toml')
    with source.open('rb') as handle:
        cargo = tomllib.load(handle)
    version = cargo.get('workspace', {}).get('package', {}).get('version')
    if not isinstance(version, str) or not SEMVER.fullmatch(version):
        raise ValueError('workspace.package.version must be a canonical SemVer string')
    return version


def unique_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('Duplicate JSON key: ' + key)
        result[key] = value
    return result


def reject_constant(value):
    raise ValueError('Nonstandard JSON number: ' + value)


def atomic_json(path, value):
    mode = stat.S_IMODE(path.stat().st_mode)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', newline='\n', dir=path.parent, prefix='.' + path.name + '.', delete=False) as handle:
            temporary = Path(handle.name)
            json.dump(value, handle, ensure_ascii=False, indent=2, allow_nan=False)
            handle.write('\n')
            handle.flush()
            os.fsync(handle.fileno())
        temporary.chmod(mode)
        os.replace(temporary, path)
        temporary = None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def synchronize(root, mode, tag=None):
    root = Path(root).resolve(strict=True)
    version = workspace_version(root)
    if tag is not None and tag != 'v' + version:
        raise VersionMismatch(f'Tag {tag!r} does not match Cargo version v{version}')
    if mode == 'print':
        return version
    path = local_file(root, 'apps/desktop/tauri.conf.json')
    config = json.loads(path.read_text(encoding='utf-8'), object_pairs_hook=unique_keys, parse_constant=reject_constant)
    if not isinstance(config, dict):
        raise ValueError('tauri.conf.json must contain a JSON object')
    if config.get('version') == version:
        return 'Version is synchronized: ' + version
    if mode == 'check':
        raise VersionMismatch(f'Tauri version {config.get("version")!r} does not match Cargo version {version}; run --write and review the change')
    config['version'] = version
    atomic_json(path, config)
    return 'Synchronized Tauri version to ' + version


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, required=True)
    modes = parser.add_mutually_exclusive_group(required=True)
    for mode in ['check', 'write', 'print']:
        modes.add_argument('--' + mode, dest='mode', action='store_const', const=mode)
    parser.add_argument('--tag')
    args = parser.parse_args(argv)
    if args.tag is not None and args.mode != 'check':
        parser.error('--tag is only valid with --check')
    try:
        print(synchronize(args.root, args.mode, args.tag))
        return 0
    except VersionMismatch as error:
        print(str(error), file=sys.stderr)
        return 1
    except (OSError, ValueError, TypeError, AttributeError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
