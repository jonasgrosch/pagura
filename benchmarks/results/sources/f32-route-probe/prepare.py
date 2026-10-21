"""Candidate-only administrative install/remove of the exact owned route patch."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import signal
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path('/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura')
sys.path[:0] = [str(ROOT / 'benchmarks'), str(ROOT / 'tools')]
from paired_render import source_identity
from benchkit.config import source_digest
from benchkit.artifacts import read_bounded_regular, hash_regular_file

FOLDER = ROOT / 'benchmarks/results/local/f32-painted-interval/route-preparation-001'
SOURCE = ROOT / 'benchmarks/results/sources/f32-route-probe'
PATCH = SOURCE / 'route-probe.patch'
MODULE = 'crates/pagura-render/src/compositor/temporary_f32_routes.rs'
BASE = {'crates/pagura-render/src/compositor.rs': '11cd68c675c49c02483a51a06e977f28cb167287',
        'crates/pagura-render/src/compositor/narrow.rs': '07aa8d1df7b64cfe8d696debf32fbaf6acd57da1'}
CASES = ('sparse64', 'image-only', 'f32-shading-dense', 'f32-pattern-dense', 'opaque64', 'default-knockout64')


def require(value, message):
    if not value:
        raise ValueError(message)


def descriptor(path):
    path = Path(path).resolve()
    return {'path': str(path), 'sha256': hash_regular_file(path)}


def files():
    return {name: hash_regular_file(ROOT / name) if (ROOT / name).exists() else None for name in (*BASE, MODULE)}


def snapshot():
    product = source_identity(ROOT)
    return {'product': product, 'runner': {'commit': product['commit'], 'sha256': source_digest()}, 'files': files()}


def write(name, value):
    path, pending = FOLDER / name, FOLDER / (name + '.pending')
    require(not path.exists(), 'Refuse replacing preparation record: ' + name)
    with pending.open('x') as output:
        json.dump(value, output, indent=2)
        output.write('\n')
    pending.replace(path)


def apply(reverse=False, check=False):
    flags = (['--reverse'] if reverse else []) + (['--check'] if check else [])
    subprocess.run(['git', '-C', str(ROOT), 'apply', *flags, str(PATCH)],
                   check=True, capture_output=True, text=True, timeout=30)


def transact(reverse, original, finish):
    apply(reverse, True)
    try:
        apply(reverse)
        finish()  # Identity capture and terminal evidence are inside rollback scope.
    except BaseException as error:
        rollback_error = None
        try:
            if files() != original['files']:
                apply(not reverse, True)
                apply(not reverse)
            require(snapshot() == original, 'Owned rollback did not restore exact prior identities')
        except BaseException as rollback:
            rollback_error = f'{type(rollback).__name__}: {rollback}'
        write('remove-failed.json' if reverse else 'install-failed.json', {
            'schema': 1, 'error': f'{type(error).__name__}: {error}', 'rollback_error': rollback_error,
            'restored_before_attempt': rollback_error is None, 'actual': snapshot()})
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('install', 'remove'))
    parser.add_argument('--preflight', type=Path)
    parser.add_argument('--preflight-sha256')
    parser.add_argument('--installed-sha256')
    args = parser.parse_args()
    require(Path.cwd().resolve() == ROOT, 'Run from the candidate checkout')
    if args.action == 'install':
        require(args.preflight and args.preflight_sha256 and not args.installed_sha256, 'Pinned completed preflight required')
        require(hash_regular_file(args.preflight) == args.preflight_sha256, 'Preflight pin differs')
        report = json.loads(read_bounded_regular(args.preflight, 64 << 20))
        require(report['status'] == 'verified-native-preflight' and report['all_correct'] is True
                and report['identities_unchanged'] is True and report['expected_outputs'] == report['completed_outputs'] == 112
                and len(report['records']) == 112 and all(r['verified'] is True for r in report['records']), 'Preflight did not complete')
        original = snapshot()
        require(original['product'] == {k: report['identities']['candidate'][k] for k in ('root', 'commit', 'source_sha256')}
                and original['files'][MODULE] is None, 'Candidate is not the preflight product or already instrumented')
        for name, expected in BASE.items():
            raw = read_bounded_regular(ROOT / name, 8 << 20)
            require(hashlib.sha1(b'blob ' + str(len(raw)).encode() + b'\0' + raw).hexdigest() == expected, 'Route patch base differs: ' + name)
        patch = read_bounded_regular(PATCH, 2 << 20).decode('utf-8')
        require(set(re.findall(r'^\+\+\+ b/(.+)$', patch, re.M)) == set((*BASE, MODULE)), 'Patch escapes the three owned files')
        apply(check=True)
        FOLDER.mkdir(parents=True, exist_ok=False)
        directory = FOLDER / 'inputs'
        directory.mkdir()
        inputs = {'schema': 1, 'directory': str(directory), 'files': {}, 'preflight': descriptor(args.preflight)}
        for name in CASES:
            item = report['inputs'][name]['pdf']
            raw = read_bounded_regular(item['path'], 2 << 20)
            require(len(raw) == item['bytes'] and hashlib.sha256(raw).hexdigest() == item['sha256'], 'Preflight PDF changed')
            path = directory / (name + '.pdf')
            with path.open('xb') as output:
                output.write(raw)
            path.chmod(0o400)
            inputs['files'][name] = {**descriptor(path), 'bytes': len(raw)}
        write('inputs.json', inputs)
        record = {'schema': 1, 'status': 'before-owned-patch', 'before': original,
                  'patch': descriptor(PATCH), 'preflight': descriptor(args.preflight),
                  'inputs': descriptor(FOLDER / 'inputs.json'),
                  'helper_sources': {str(p.resolve()): hash_regular_file(p) for p in (Path(__file__), SOURCE / 'run.py', SOURCE / 'README.md')}}
        write('before.json', record)
        def finish_install():
            raw = read_bounded_regular(ROOT / MODULE, 2 << 20)
            archived = FOLDER / 'temporary_f32_routes.rs'
            with archived.open('xb') as output:
                output.write(raw)
            archived.chmod(0o400)
            write('installed.json', {**record, 'status': 'installed-not-tested', 'installed': snapshot(),
                                    'before_record': descriptor(FOLDER / 'before.json'), 'test_module': descriptor(archived)})
        transact(False, original, finish_install)
    else:
        require(args.installed_sha256 and not args.preflight and not args.preflight_sha256, 'Pinned installed record required')
        installed = FOLDER / 'installed.json'
        require(hash_regular_file(installed) == args.installed_sha256, 'Installed record pin differs')
        record = json.loads(read_bounded_regular(installed, 2 << 20))
        require(record['patch'] == descriptor(PATCH) and snapshot() == record['installed'], 'Installed state changed; refuse removal')
        require(hash_regular_file(ROOT / MODULE) == record['test_module']['sha256'], 'Actual test module differs')
        def finish_remove():
            after = snapshot()
            require(after == record['before'], 'Exact clean product/runner/files not restored')
            write('removed.json', {'schema': 1, 'status': 'removed-and-restored', 'restored': True,
                                  'preparation': descriptor(installed), 'before': record['before'], 'after': after,
                                  'inputs': record['inputs'], 'test_module': record['test_module']})
        transact(True, record['installed'], finish_remove)
    print(FOLDER / ('installed.json' if args.action == 'install' else 'removed.json'))


if __name__ == '__main__':
    handlers = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
    def cancel(number, _frame):
        raise SystemExit(128 + number)
    try:
        for signum in handlers:
            signal.signal(signum, cancel)
        main()
    finally:
        for signum, handler in handlers.items():
            signal.signal(signum, handler)
