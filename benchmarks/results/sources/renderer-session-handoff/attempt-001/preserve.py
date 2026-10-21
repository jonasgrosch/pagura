#!/usr/bin/env python3
"""Preserve this session's evidence and control overlays without modifying them.

Run serially under tools/runner.py. Archives are publication exports, not a
claim that excluded private process evidence can be re-audited from GitHub.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import tarfile
import time

NAMES = (
    'unified-renderer-plan', 'renderer-baseline', 'renderer-stacked-gains',
    'renderer-stacked-base', 'glyph-memo-control', 'opaque-precision-control',
    'opaque-output-control', 'n16-conversion-control', 'n16-painted-control',
    'rectangle-reserve-control', 'renderer-n16-interval',
    'renderer-n16-interval-base', 'renderer-f32-interval-base',
)
TEXT = {'.json', '.jsonl', '.txt', '.log', '.md', '.py', '.rs', '.mjs',
        '.patch', '.stdout', '.stderr', '.stdin', '.sha256'}
SECRET = re.compile(
    rb'-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----|'
    rb'\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|'
    rb'AKIA[A-Z0-9]{16}|ASIA[A-Z0-9]{16}|sk_live_[A-Za-z0-9]{20,})\b'
)


def git(root, *args, binary=False):
    result = subprocess.run(['git', *args], cwd=root, check=True,
                            capture_output=True, text=not binary)
    return result.stdout if binary else result.stdout.strip()


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def omission(path):
    if path.is_symlink():
        return 'symlink not followed'
    if path.name.startswith('snapshot-') or path.name in {'ps.txt', 'processes.txt'}:
        return 'unrelated process-command inventory kept private'
    if path.suffix in TEXT:
        with path.open('rb') as stream:
            previous = b''
            while block := stream.read(1024 * 1024):
                if SECRET.search(previous + block):
                    return 'credential-like bytes kept private'
                previous = block[-256:]
        if path.name == 'guard.json':
            value = json.loads(path.read_text())
            if any(event.get('matches') for event in value.get('events', [])):
                return 'foreign process-command match kept private'
    return None


class HashReader:
    def __init__(self, stream):
        self.stream, self.digest = stream, hashlib.sha256()

    def read(self, size=-1):
        data = self.stream.read(size)
        self.digest.update(data)
        return data


def add_file(archive, path, relative, records):
    before = path.lstat()
    record = {'path': relative, 'bytes': before.st_size, 'included': False}
    if not stat.S_ISREG(before.st_mode):
        record['reason'] = 'nonregular entry not followed'
        records.append(record)
        return
    reason = omission(path)
    if reason:
        record.update(reason=reason, sha256=sha(path))
    else:
        info = archive.gettarinfo(str(path), arcname=relative)
        info.uid = info.gid = 0
        info.uname = info.gname = ''
        info.mode = 0o644
        with path.open('rb') as stream:
            reader = HashReader(stream)
            archive.addfile(info, reader)
            record.update(included=True, sha256=reader.digest.hexdigest())
    after = path.lstat()
    if (before.st_size, before.st_mtime_ns, before.st_ino) != (
            after.st_size, after.st_mtime_ns, after.st_ino):
        raise RuntimeError('Evidence changed during export: ' + str(path))
    records.append(record)


def add_bytes(archive, name, raw, records):
    if SECRET.search(raw):
        raise RuntimeError('Credential-like bytes in generated overlay; no publication')
    info = tarfile.TarInfo(name)
    info.size, info.mode, info.mtime = len(raw), 0o644, int(time.time())
    archive.addfile(info, io.BytesIO(raw))
    records.append({'path': name, 'bytes': len(raw), 'included': True,
                    'sha256': hashlib.sha256(raw).hexdigest()})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worktrees', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    combined = args.worktrees / 'renderer-n16-interval/pagura'
    summary = {'schema': 1, 'started_unix_ns': time.time_ns(),
               'status': 'incomplete', 'release_tag': 'renderer-session-handoff-2026-10-21',
               'source_scope': 'attached worktrees from this renderer session only',
               'exports_are_historical_measurements': False, 'worktrees': []}
    for name in NAMES:
        root = args.worktrees / name / 'pagura'
        head = git(root, 'rev-parse', 'HEAD')
        ancestor = subprocess.run(['git', 'merge-base', '--is-ancestor', head, 'HEAD'],
                                  cwd=combined, capture_output=True).returncode
        if ancestor not in (0, 1):
            raise RuntimeError('Cannot establish commit reachability')
        item = {'name': name, 'head': head,
                'branch': git(root, 'branch', '--show-current'),
                'ancestor_of_combined_before_handoff': ancestor == 0,
                'status_before_export': git(root, 'status', '--porcelain'),
                'asset': name + '.tar.gz'}
        if name == 'renderer-n16-interval':
            item['overlay_note'] = 'handoff edits are committed on the combined branch, not duplicated as a control overlay'
        records = []
        target = args.output / item['asset']
        temporary = target.with_suffix(target.suffix + '.incomplete')
        with tarfile.open(temporary, 'w:gz', compresslevel=1) as archive:
            if name != 'renderer-n16-interval':
                patch = git(root, 'diff', '--binary', 'HEAD', '--', binary=True)
                add_bytes(archive, 'overlay/tracked.patch', patch, records)
                raw = git(root, 'ls-files', '--others', '--exclude-standard', '-z', binary=True)
                for relative in sorted(os.fsdecode(p) for p in raw.split(b'\0') if p):
                    add_file(archive, root / relative, 'overlay/untracked/' + relative, records)
            local = root / 'benchmarks/results/local'
            if local.exists():
                for path in sorted(local.rglob('*')):
                    if path.is_file() or path.is_symlink():
                        add_file(archive, path, 'raw-local/' + path.relative_to(local).as_posix(), records)
            inventory = {'worktree': item, 'files': records,
                         'limitation': 'Omitted entries remain private; their original hashes do not prove their contents or permit a complete public raw audit.'}
            manifest = json.dumps(inventory, indent=2, sort_keys=True).encode() + b'\n'
            add_bytes(archive, 'inventory.json', manifest, [])
        temporary.rename(target)
        item.update(archive_sha256=sha(target), archive_bytes=target.stat().st_size,
                    inventory_sha256=hashlib.sha256(manifest).hexdigest(),
                    included_files=sum(r['included'] for r in records),
                    excluded_files=[r for r in records if not r['included']],
                    included_uncompressed_bytes=sum(r['bytes'] for r in records if r['included']))
        summary['worktrees'].append(item)
        (args.output / 'archive-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
        print(json.dumps({k: item[k] for k in ('name', 'included_files', 'archive_bytes')}), flush=True)
    summary.update(status='complete', finished_unix_ns=time.time_ns())
    (args.output / 'archive-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    checksum = '\n'.join(sha(p) + '  ' + p.name for p in sorted(args.output.iterdir())
                         if p.is_file()) + '\n'
    (args.output / 'SHA256SUMS').write_text(checksum)


if __name__ == '__main__':
    main()
