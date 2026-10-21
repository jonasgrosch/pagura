"""Build the two fixed native arms serially and retain their exact identities."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys

ROOT = Path.cwd()
sys.path.insert(0, str(ROOT / 'benchmarks'))
from paired_render import source_identity
import subprocess
from benchkit.measurement import run_process

CONTROL = Path('/Users/jonasgrosch/.codex/worktrees/renderer-baseline/pagura')

def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()

def require_sources():
    if git(CONTROL, 'rev-parse', 'HEAD') != '676a872eb9f6c279cbf1e3929ace3a0c99db9fc3':
        raise ValueError('Wrong starting control')
    if git(ROOT, 'rev-parse', 'HEAD') != 'a9814ee476c1b17a27b1e9a904f1f4fac8d5cd9c':
        raise ValueError('Wrong reconciled candidate')
    if git(CONTROL, 'status', '--porcelain=v1', '--untracked-files=all') != '?? crates/pagura-render/examples/scene_index_profile.rs':
        raise ValueError('Unexpected starting-control changes')
    if git(CONTROL, 'diff', '359e19dda5b78a23675678b6a153da7ab7fdc2c5', 'HEAD', '--', 'crates', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo/config.toml'):
        raise ValueError('Control is not the starting product')
    if git(ROOT, 'diff', 'HEAD', '--', 'crates', 'tools', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo/config.toml'):
        raise ValueError('Candidate product has changed')

local = ROOT / 'benchmarks/results/local/start-current/build-001'
local.mkdir(parents=True, exist_ok=False)
os.environ.update(CARGO_BUILD_JOBS='1', RUST_TEST_THREADS='1', PYTHONDONTWRITEBYTECODE='1')
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
report = {'status': 'incomplete', 'identities': {}, 'builds': {},
          'driver_sha256': sha(__file__), 'run_id': os.environ.get('PAGURA_RUN_ID'),
          'build_environment': {name: os.environ.get(name) for name in
              ('CARGO_BUILD_JOBS', 'CARGO_BUILD_TARGET', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER',
               'RUSTC_WORKSPACE_WRAPPER', 'CARGO_PROFILE_RELEASE_DEBUG', 'CARGO_PROFILE_RELEASE_LTO')},
          'config_sha256': {str(root / '.cargo/config.toml'): sha(root / '.cargo/config.toml')
                            for root in (ROOT, CONTROL)}}
try:
    forbidden = ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS',
                 'CARGO_BUILD_TARGET', 'CARGO_TARGET_DIR', 'RUSTC', 'RUSTC_WRAPPER',
                 'RUSTC_WORKSPACE_WRAPPER', 'RUSTUP_TOOLCHAIN')
    if any(os.environ.get(name) for name in forbidden) or any(value for name, value in os.environ.items()
            if name.startswith('CARGO_PROFILE_') or name.startswith('CARGO_TARGET_')):
        raise ValueError('Unexpected build override')
    report['toolchain'] = {name: subprocess.check_output(command, text=True) for name, command in
        [('rustc', ['rustc', '-vV']), ('cargo', ['cargo', '-vV'])]}
    report['session_start'] = '359e19dda5b78a23675678b6a153da7ab7fdc2c5'
    report['example_overlay'] = 'Only the identical untracked profile example is added to the starting control.'
    require_sources()
    example = Path('crates/pagura-render/examples/scene_index_profile.rs')
    for relative in (example, Path('rust-toolchain.toml'), Path('.cargo/config.toml'), Path('Cargo.toml'), Path('Cargo.lock')):
        if (ROOT / relative).read_bytes() != (CONTROL / relative).read_bytes():
            raise ValueError('Build/example configuration differs: ' + str(relative))
    for side, root in (('baseline', CONTROL), ('candidate', ROOT)):
        before = source_identity(root)
        command = ['cargo', 'build', '--locked', '--offline', '--release', '-p', 'pagura-render',
                   '--example', 'scene_index_profile', '--manifest-path', str(root / 'Cargo.toml'),
                   '--target-dir', str(root / 'target'), '--message-format=json', '-j', '1']
        result, log = run_process(command, {'timeout_seconds': 600, 'measure_rss': False},
                                  log_budget=1024 * 1024)
        (local / (side + '.log')).write_text(log)
        report['builds'][side] = {'command': command, 'process': result, 'source_before': before,
                                 'source_after': source_identity(root), 'log_sha256': sha(local / (side + '.log'))}
        if result['exit_code'] != 0 or result['status'] == 'timeout':
            raise RuntimeError(side + ' build failed')
        if before != report['builds'][side]['source_after']:
            raise RuntimeError(side + ' source changed')
        binary = local / side
        artifacts = [item for line in log.splitlines() if line.startswith('{')
                     for item in [json.loads(line)]
                     if item.get('reason') == 'compiler-artifact'
                     and item.get('target', {}).get('name') == 'scene_index_profile'
                     and item.get('executable')]
        if len(artifacts) != 1:
            raise RuntimeError('Expected exactly one Cargo example artifact')
        artifact = artifacts[0]
        report['builds'][side]['cargo_artifact'] = artifact
        native_path = root / 'target/release/examples/scene_index_profile'
        if Path(artifact['executable']).resolve() != native_path.resolve():
            raise RuntimeError('Cargo did not select the expected native artifact')
        shutil.copy2(artifact['executable'], binary)
        binary.chmod(0o500)
        report['identities'][side] = {**before, 'binary': str(binary), 'binary_sha256': sha(binary)}
    require_sources()
    for identity in report['identities'].values():
        if source_identity(identity['root']) != {k: identity[k] for k in ('root', 'commit', 'source_sha256')}:
            raise RuntimeError('Source changed before build completion')
    if any(sha(path) != digest for path, digest in report['config_sha256'].items()):
        raise RuntimeError('Cargo configuration changed during builds')
    report['status'] = 'built-both-arms'
finally:
    (local / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(report['status'], flush=True)
