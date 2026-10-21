#!/usr/bin/env python3
"""Stream-check published archive bytes and each included payload; never extract."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import tarfile


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    directory = args.directory.resolve()
    summary = json.loads((directory / 'archive-summary.json').read_text())
    require(summary['status'] == 'complete', 'Export is incomplete')
    checked_assets = set()
    for line in (directory / 'SHA256SUMS').read_text().splitlines():
        expected, name = line.split('  ', 1)
        require(Path(name).name == name and name not in checked_assets, 'Invalid checksum asset name')
        require(digest(directory / name) == expected, 'Asset hash mismatch: ' + name)
        checked_assets.add(name)
    require(checked_assets == {'archive-summary.json'} | {
        item['asset'] for item in summary['worktrees']}, 'Checksum asset set differs')
    total = 0
    for item in summary['worktrees']:
        path = directory / item['asset']
        require(path.stat().st_size == item['archive_bytes'], 'Archive size differs')
        require(digest(path) == item['archive_sha256'], 'Summary archive hash differs')
        actual, inventory = {}, None
        with tarfile.open(path, 'r|gz') as archive:
            for member in archive:
                name = member.name
                relative = PurePosixPath(name)
                require(not relative.is_absolute() and '..' not in relative.parts,
                        'Unsafe archive path')
                require(member.isfile(), 'Nonregular archive member')
                require(name not in actual, 'Duplicate archive member')
                stream = archive.extractfile(member)
                require(stream is not None, 'Missing member data')
                if name == 'inventory.json':
                    require(inventory is None and member.size <= 128 * 1024 * 1024,
                            'Duplicate or oversized inventory')
                    raw = stream.read()
                    require(hashlib.sha256(raw).hexdigest() == item['inventory_sha256'],
                            'Inventory hash differs')
                    inventory = json.loads(raw)
                else:
                    actual[name] = {'bytes': member.size,
                                    'sha256': hashlib.file_digest(stream, 'sha256').hexdigest()}
        require(inventory is not None, 'Missing inventory')
        expected = {r['path']: {'bytes': r['bytes'], 'sha256': r['sha256']}
                    for r in inventory['files'] if r['included']}
        require(len(expected) == item['included_files'], 'Included count differs')
        require(expected == actual, 'Archive payload set, size or hash differs')
        require(inventory['worktree']['head'] == item['head'], 'Control head differs')
        require([r for r in inventory['files'] if not r['included']] == item['excluded_files'],
                'Exclusion inventory differs')
        total += len(actual)
        print(json.dumps({'asset': item['asset'], 'verified_payloads': len(actual)}), flush=True)
    print(json.dumps({'status': 'verified', 'worktrees': len(summary['worktrees']),
                      'included_payloads': total}), flush=True)


if __name__ == '__main__':
    main()
