"""Apply/reverse unchanged layout and Q4 patches after both roots precheck cleanly."""
import hashlib
import json
from pathlib import Path
import signal
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path('/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura')
sys.path.insert(0, str(ROOT / 'benchmarks'))
from paired_render import source_identity

FOLDER = ROOT / 'benchmarks/results/local/f32-painted-interval/diagnostic-preparation-001'
ROOTS = {'baseline': ROOT.parent.parent / 'renderer-f32-interval-base/pagura', 'candidate': ROOT}
PATCHDIR = ROOT / 'benchmarks/results/sources/n16-painted-interval-validation'
PATCHES = [PATCHDIR / 'layout-probe.patch', PATCHDIR / 'pi-q4-diagnostic.patch']
FILES = ('crates/pagura-render/src/compositor.rs', 'crates/pagura-render/src/display.rs',
         'crates/pagura-render/src/compositor/sources_tests.rs',
         'crates/pagura-render/src/compositor/translucency_tests.rs')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require(value, message):
    if not value:
        raise ValueError(message)


def git(root, *args):
    return subprocess.run(['git', '-C', str(root), *args], check=True,
                          capture_output=True, text=True, timeout=30).stdout


def file_hashes(root):
    return {name: sha(root / name) for name in FILES}


def write(name, record):
    path = FOLDER / name
    require(not path.exists(), 'refuse replacing preparation evidence: ' + name)
    temporary = FOLDER / (name + '.pending')
    with temporary.open('x') as output:
        json.dump(record, output, indent=2)
        output.write('\n')
    temporary.replace(path)


def apply_both(reverse=False, check=False, root=None):
    flags = (['--reverse'] if reverse else []) + (['--check'] if check else [])
    return git(root, 'apply', *flags, *map(str, PATCHES))


def transact(reverse, before, finish):
    # Check every hunk of both patches in BOTH roots before changing either root.
    for root in ROOTS.values():
        apply_both(reverse, True, root)
    touched = []
    try:
        for arm, root in ROOTS.items():
            touched.append(arm)
            apply_both(reverse, False, root)
        # Installed identities and terminal record are part of the owned
        # transaction: interruption here must not leave unrecorded probes.
        finish()
    except BaseException as error:
        rollback_errors = []
        # A failed git apply is normally atomic. If this invocation changed a
        # root, reverse only these exact owned patches; never reset other edits.
        for arm in reversed(touched):
            root = ROOTS[arm]
            try:
                if file_hashes(root) != before[arm]:
                    apply_both(not reverse, True, root)
                    apply_both(not reverse, False, root)
                require(file_hashes(root) == before[arm], 'owned rollback differs: ' + arm)
            except BaseException as rollback:
                rollback_errors.append(f'{arm}: {type(rollback).__name__}: {rollback}')
        write('remove-failed.json' if reverse else 'install-failed.json', {
            'schema': 1, 'error': f'{type(error).__name__}: {error}',
            'rollback_errors': rollback_errors,
            'files_after': {arm: file_hashes(root) for arm, root in ROOTS.items()},
            'restored_before_attempt': not rollback_errors,
        })
        raise


def main():
    require(len(sys.argv) == 2 and sys.argv[1] in ('install', 'remove'), 'install or remove required')
    action = sys.argv[1]
    if action == 'install':
        FOLDER.mkdir(parents=True, exist_ok=False)
        require(git(ROOTS['baseline'], 'rev-parse', 'HEAD').strip()
                == 'f9738174757252567730ff62b5d36620c01d1e6c', 'wrong retained baseline commit')
        require(git(ROOTS['baseline'], 'status', '--porcelain=v1', '--untracked-files=all') == '',
                'baseline is not pristine')
        record = {'schema': 1, 'roots': {arm: {
            'before': source_identity(root), 'files': file_hashes(root),
            'status_before': git(root, 'status', '--porcelain=v1', '--untracked-files=all'),
        } for arm, root in ROOTS.items()},
            'patches': [{'path': str(path), 'sha256': sha(path)} for path in PATCHES],
            'reason': 'Fresh F32 interval comparison after coordinator-verified uninstrumented candidate native tests; historical patches unchanged.'}
        write('before.json', record)
        def finish_install():
            for arm, root in ROOTS.items():
                record['roots'][arm]['installed'] = source_identity(root)
            write('installed.json', record)
        transact(False, {arm: item['files'] for arm, item in record['roots'].items()}, finish_install)
        print(sha(FOLDER / 'installed.json'))
    else:
        record = json.loads((FOLDER / 'installed.json').read_text())
        require(record['patches'] == [{'path': str(path), 'sha256': sha(path)} for path in PATCHES],
                'owned diagnostic patch changed')
        require(all(source_identity(root) == record['roots'][arm]['installed']
                    for arm, root in ROOTS.items()), 'installed source changed; refuse removal')
        def finish_remove():
            restored = {arm: source_identity(root) for arm, root in ROOTS.items()}
            require(all(file_hashes(root) == record['roots'][arm]['files']
                        and restored[arm] == record['roots'][arm]['before']
                        for arm, root in ROOTS.items()), 'exact source restoration differs')
            write('removed.json', {'schema': 1, 'preparation_sha256': sha(FOLDER / 'installed.json'),
                                  'removed': restored, 'restored': True})
        transact(True, {arm: file_hashes(root) for arm, root in ROOTS.items()}, finish_remove)
        print((FOLDER / 'removed.json').read_text())


if __name__ == '__main__':
    previous = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
    def cancel(number, _frame):
        raise SystemExit(128 + number)
    try:
        for signum in previous:
            signal.signal(signum, cancel)
        main()
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
