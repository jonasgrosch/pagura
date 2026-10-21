"""Build identical off-tree accuracy adapters without changing either product."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib

ROOT = Path.cwd()
sys.path.insert(0, str(ROOT / 'benchmarks'))
from paired_render import source_identity
from benchkit.measurement import run_process

HARNESS = Path('/private/tmp/pagura-start-current/accuracy')
LOCAL = ROOT / 'benchmarks/results/local/start-current/accuracy-build-001'
PROFILE = ROOT / 'benchmarks/results/local/start-current/build-001/report.json'
LOCAL.mkdir(parents=True, exist_ok=False)
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
profile = json.loads(PROFILE.read_text())
identities = profile['identities']
sources = [HARNESS / 'accuracy_probe.rs', *(HARNESS / side / 'Cargo.toml' for side in identities)]
pins = {str(path): sha(path) for path in sources}
pins[str(ROOT / '.cargo/config.toml')] = sha(ROOT / '.cargo/config.toml')
pins[str(ROOT / 'rust-toolchain.toml')] = sha(ROOT / 'rust-toolchain.toml')
report = {'schema': 1, 'status': 'incomplete', 'identities': {}, 'builds': {},
          'profile_build': {'path': str(PROFILE), 'sha256': sha(PROFILE)},
          'harness_sha256': pins, 'driver_sha256': sha(__file__),
          'run_id': os.environ.get('PAGURA_RUN_ID')}

def check_sources():
    for side, identity in identities.items():
        assert source_identity(identity['root']) == {k: identity[k] for k in ('root', 'commit', 'source_sha256')}, side
    assert all(sha(path) == digest for path, digest in pins.items())

def run(side, stage, command):
    process, log = run_process(command, {'timeout_seconds': 600, 'measure_rss': False}, log_budget=1024 * 1024)
    path = LOCAL / f'{side}-{stage}.log'
    path.write_text(log)
    report['builds'][side][stage] = {'command': command, 'process': process, 'log': str(path), 'log_sha256': sha(path)}
    if process['exit_code'] != 0 or process['status'] == 'timeout':
        raise RuntimeError(f'{side} {stage} failed: {path}')
    return log

try:
    forbidden = ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS',
                 'CARGO_BUILD_TARGET', 'CARGO_TARGET_DIR', 'RUSTC', 'RUSTC_WRAPPER',
                 'RUSTC_WORKSPACE_WRAPPER', 'RUSTUP_TOOLCHAIN')
    if any(os.environ.get(name) for name in forbidden) or any(value for name, value in os.environ.items()
            if name.startswith('CARGO_PROFILE_') or name.startswith('CARGO_TARGET_')):
        raise ValueError('Unexpected build override')
    os.environ.update(CARGO_BUILD_JOBS='1', RUST_TEST_THREADS='1', PYTHONDONTWRITEBYTECODE='1')
    report['toolchain'] = {name: subprocess.check_output([name, '-vV'], text=True) for name in ('rustc', 'cargo')}
    assert report['toolchain'] == profile['toolchain']
    check_sources()
    for side, identity in identities.items():
        manifest = HARNESS / side / 'Cargo.toml'
        metadata = tomllib.loads(manifest.read_text())
        assert metadata['bin'] == [{'name': 'accuracy_probe', 'path': '../accuracy_probe.rs'}]
        assert metadata['profile']['release'] == {'lto': 'thin', 'codegen-units': 1, 'overflow-checks': True}
        assert metadata['dependencies'] == {name: {'path': str(Path(identity['root']) / 'crates' / name)}
                                            for name in ('pagura-core', 'pagura-document', 'pagura-raster', 'pagura-render')}
        report['builds'][side] = {'source_before': source_identity(identity['root'])}
        run(side, 'lock', ['cargo', 'generate-lockfile', '--offline', '--manifest-path', str(manifest)])
        lock = manifest.with_name('Cargo.lock')
        packages = tomllib.loads(lock.read_text())['package']
        assert all(set(p) <= {'name', 'version', 'dependencies'} for p in packages), 'non-local dependency'
        shutil.copy2(lock, LOCAL / (side + '-Cargo.lock'))
        pins[str(lock)] = sha(lock)
        target = ROOT / 'target' / ('start-current-accuracy-' + side)
        log = run(side, 'build', ['cargo', 'build', '--locked', '--offline', '--release',
                  '--manifest-path', str(manifest), '--target-dir', str(target), '--message-format=json', '-j', '1'])
        artifacts = [item for line in log.splitlines() if line.startswith('{') for item in [json.loads(line)]
                     if item.get('reason') == 'compiler-artifact' and item.get('target', {}).get('name') == 'accuracy_probe'
                     and item.get('executable')]
        assert len(artifacts) == 1
        artifact = artifacts[0]
        assert Path(artifact['executable']).resolve() == target / 'release/accuracy_probe'
        binary = LOCAL / side
        shutil.copy2(artifact['executable'], binary)
        binary.chmod(0o500)
        report['identities'][side] = {**{k: identity[k] for k in ('root', 'commit', 'source_sha256')},
                                     'binary': str(binary), 'binary_sha256': sha(binary)}
        report['builds'][side].update(cargo_artifact=artifact, source_after=source_identity(identity['root']))
        check_sources()
    report['status'] = 'built-both-arms'
finally:
    (LOCAL / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(report['status'], flush=True)
