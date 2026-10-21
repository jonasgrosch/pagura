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
from rectangle_reserve import require_baseline
from benchkit.measurement import run_process

CONTROL = Path('/Users/jonasgrosch/.codex/worktrees/rectangle-reserve-control/pagura')
local = ROOT / 'benchmarks/results/local/rectangle-reserve/build-001'
local.mkdir(exist_ok=False)
require_baseline(CONTROL)
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
    example = Path('crates/pagura-render/examples/scene_index_profile.rs')
    for relative in (example, Path('rust-toolchain.toml'), Path('.cargo/config.toml')):
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
    require_baseline(CONTROL)
    for identity in report['identities'].values():
        if source_identity(identity['root']) != {k: identity[k] for k in ('root', 'commit', 'source_sha256')}:
            raise RuntimeError('Source changed before build completion')
    if any(sha(path) != digest for path, digest in report['config_sha256'].items()):
        raise RuntimeError('Cargo configuration changed during builds')
    report['status'] = 'built-both-arms'
finally:
    (local / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(report['status'], flush=True)
