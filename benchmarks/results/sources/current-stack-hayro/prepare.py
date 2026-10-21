"""Prepare, build and freeze one current-stack comparison; never optimize products."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[4]
PACKAGE = Path(__file__).resolve().parent
LOCAL = ROOT / 'benchmarks/results/local/current-stack-hayro'
sys.path[:0] = [str(ROOT / 'benchmarks'), str(ROOT / 'tools')]
from paired_render import source_identity
from benchkit.config import source_digest
from benchkit.images import normalize
import f32_painted_interval as intervals

EXPECTED_COMMIT = '402e3afe3a08bd178d427ba965883535e75a9dd5'
CACHE = Path('/Users/jonasgrosch/.cache/pagura-competitors')
TARGET = Path('/private/tmp/current-stack-hayro-pagura-402e3afe')
HAYRO_SOURCE = Path('/private/tmp/current-stack-hayro-adapter-402e3afe')
HAYRO_TARGET = Path('/private/tmp/current-stack-hayro-target-402e3afe')
CARGO = '/Users/jonasgrosch/.cargo/bin/cargo'


def pin(path):
    path = Path(path).resolve()
    return {'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


def write(path, value):
    with Path(path).open('x') as stream:
        stream.write(json.dumps(value, indent=2, allow_nan=False) + '\n')


def prepare():
    product = source_identity(ROOT)
    assert product['commit'] == EXPECTED_COMMIT
    legacy = ROOT / 'benchmarks/results/sources/analytic19-conversion'
    refs = json.loads((legacy / 'references.json').read_bytes())
    assert pin(legacy / 'references.json')['sha256'] == '924e20d6495990e8206c837d420c0172d5404ea1b3ede56417a9d59bb84bbcaf'
    from benchkit.reference_cases import (validate_case_reference, load_reference_sources,
                                         invocation_settings, current_render_policy_sha256)
    cases, profile_checks = [], []
    source_files = {str(p.resolve()): pin(p)['sha256'] for p in (
        Path(__file__), PACKAGE / 'run.py', PACKAGE / 'README.md', legacy / 'references.json', legacy / 'manifest.json',
        legacy / 'audit.py', *map(Path, intervals.FIXTURE_SOURCES),
        ROOT / 'benchmarks/f32_painted_interval.py', ROOT / 'benchmarks/paired_render.py',
        legacy / 'load_observer.py', legacy / 'idle_then.py')}
    snapshots = LOCAL / 'inputs'
    snapshots.mkdir(exist_ok=False)
    def add(name, cohort, pdf, oracle):
        number = len(cases)
        pdf_path, oracle_path = snapshots / f'{number:02d}.pdf', snapshots / f'{number:02d}.ppm'
        pdf_path.write_bytes(pdf)
        oracle_path.write_bytes(oracle)
        pdf_path.chmod(0o400)
        oracle_path.chmod(0o400)
        width, height, _ = normalize(oracle)
        cases.append({'id': name, 'cohort': cohort, 'pdf': pin(pdf_path), 'oracle': pin(oracle_path),
                      'width': width, 'height': height})
    for case in refs['cases']:
        pdf = (ROOT / case['path']).read_bytes()
        oracle = (legacy / 'oracles' / Path(case['oracle']).name).read_bytes()
        assert hashlib.sha256(pdf).hexdigest() == case['sha256']
        assert hashlib.sha256(oracle).hexdigest() == case['oracle_sha256']
        if 'render_references' in case:
            metadata, digest = validate_case_reference(case, 'exact')
            load_reference_sources(ROOT, metadata)
            actual = invocation_settings(case['sha256'], page=0, dpi=72, accuracy='exact',
                entrypoint='render_page', validation='full',
                render_policy_sha256=current_render_policy_sha256(ROOT))
            assert metadata['settings'] == actual and metadata['oracle_sha256'] == case['oracle_sha256']
            profile_checks.append({'case': case['id'], 'reference_sha256': digest,
                                   'scope': 'source-authenticated Exact profile; common RGB policy for every engine/mode'})
        add(case['id'], 'legacy19', pdf, oracle)
    assert len(cases) == 19
    for name in intervals.CASES:
        pdf, oracle = intervals.fixture(name)
        add('interval/' + name, 'interval20', pdf, oracle)
    assert len(cases) == len({c['id'] for c in cases}) == 39
    HAYRO_SOURCE.mkdir(exist_ok=False)
    (HAYRO_SOURCE / 'src').mkdir()
    for name in ('Cargo.toml', 'Cargo.lock', 'src/main.rs'):
        original = ROOT / 'benchmarks/adapters/hayro' / name
        target = HAYRO_SOURCE / name
        target.write_bytes(original.read_bytes())
        source_files[str(original)] = pin(original)['sha256']
        source_files[str(target)] = pin(target)['sha256']
    (HAYRO_SOURCE / 'rust-toolchain.toml').write_bytes((ROOT / 'rust-toolchain.toml').read_bytes())
    source_files[str(HAYRO_SOURCE / 'rust-toolchain.toml')] = pin(HAYRO_SOURCE / 'rust-toolchain.toml')['sha256']
    write(LOCAL / 'prepared.json', {'schema': 1, 'experiment': 'current-stack-hayro',
        'product': product, 'runner_source_sha256': source_digest(), 'cases': cases,
        'source_files': source_files, 'legacy_references': pin(legacy / 'references.json'),
        'reference_profile_checks': profile_checks,
        'protocol': {'samples': 100, 'warmups': 5, 'seed': 20261022,
            'modes': ['exact', 'competitive'], 'dpi': 72, 'page': 0,
            'primary_metric': 'engine_ns', 'expected_cells': 78, 'expected_outputs': 16380,
            'background': 'white', 'accuracy': 'zero-error common independent RGB references',
            'secondary_metrics': ['render_ns', 'open_ns', 'emit_ns', 'elapsed_ns'],
            'memory': 'per-child process RSS; report full distributions and maximum ratios',
            'retry': False, 'renderer_changes': False, 'release_admission': False}})
    print('Prepared 39 immutable input/reference pairs; no renderer executed.', flush=True)


def build():
    prepared = json.loads((LOCAL / 'prepared.json').read_bytes())
    assert source_identity(ROOT) == prepared['product']
    for relative in ('Cargo.toml', 'Cargo.lock', 'src/main.rs'):
        assert (ROOT / 'benchmarks/adapters/hayro' / relative).read_bytes() == (HAYRO_SOURCE / relative).read_bytes()
    commands = [
        {'engine': 'pagura', 'cwd': str(ROOT), 'env': {}, 'command': [CARGO, 'build', '--locked', '--offline',
         '--release', '-p', 'pagura', '--target-dir', str(TARGET), '-j', '1'],
         'binary': str(TARGET / 'release/pagura')},
        {'engine': 'hayro', 'cwd': str(HAYRO_SOURCE), 'env': {'CARGO_HOME': str(CACHE / 'cargo-home')},
         'command': [CARGO, '+1.99.1', 'build', '--locked', '--offline', '--release', '--manifest-path',
         str(HAYRO_SOURCE / 'Cargo.toml'), '--target-dir', str(HAYRO_TARGET), '-j', '1'],
         'binary': str(HAYRO_TARGET / 'release/pagura-hayro-competitor')}]
    report = {'schema': 1, 'status': 'building', 'prepared': pin(LOCAL / 'prepared.json'),
              'source_before': source_identity(ROOT), 'argv': sys.orig_argv,
              'run_id': os.environ['PAGURA_RUN_ID'], 'run_receipt': os.environ['PAGURA_RUN_RECEIPT'],
              'builds': []}
    try:
        for item in commands:
            item['exit_code'] = None
            report['builds'].append(item)
            with (LOCAL / ('build-' + item['engine'] + '.log')).open('x') as log:
                item['exit_code'] = subprocess.run(item['command'], cwd=item['cwd'],
                    env={**os.environ, **item['env'], 'CARGO_BUILD_JOBS': '1'},
                    stdout=log, stderr=subprocess.STDOUT, check=False).returncode
            assert item['exit_code'] == 0, item['engine'] + ' build failed'
            item['artifact'] = pin(item['binary'])
            item['version'] = subprocess.check_output([item['binary'], '--version'], text=True).strip()
        report['source_after'] = source_identity(ROOT)
        assert report['source_before'] == report['source_after'] == prepared['product']
        report['status'] = 'completed'
    finally:
        write(LOCAL / 'build.json', report)
    print('Both release builds completed sequentially.', flush=True)


def freeze():
    config = json.loads((LOCAL / 'prepared.json').read_bytes())
    build_report = json.loads((LOCAL / 'build.json').read_bytes())
    receipt = json.loads((LOCAL / 'build-001-receipt.json').read_bytes())
    guard = json.loads((LOCAL / 'build-001-observer/guard.json').read_bytes())
    assert build_report['status'] == receipt['status'] == guard['status'] == 'completed'
    assert receipt['exit_code'] == receipt['wrapper_exit_code'] == 0 and receipt['source_unchanged']
    assert not receipt['errors'] and not receipt['artifact_errors']
    assert not any(e['matches'] for e in guard['events'])
    assert source_identity(ROOT) == config['product'] and source_digest() == config['runner_source_sha256']
    for path, digest in config['source_files'].items():
        assert pin(path)['sha256'] == digest
    config['engines'] = {}
    for build_item in build_report['builds']:
        artifact = pin(build_item['binary'])
        assert artifact == build_item['artifact'] and receipt['artifacts'][artifact['path']] == artifact['sha256']
        config['engines'][build_item['engine']] = {**artifact, 'version': build_item['version']}
    config['build'] = {key: pin(LOCAL / name) for key, name in (
        ('receipt', 'build-001-receipt.json'), ('report', 'build.json'), ('guard', 'build-001-observer/guard.json'))}
    config['policy'] = pin(ROOT / 'benchmarks/policy.json')
    config['runtime'] = pin(Path(sys.executable).resolve())
    config['launcher'] = pin(Path(sys.orig_argv[0]).resolve())
    config['status'] = 'frozen-before-measurement'
    write(LOCAL / 'config.json', config)
    print(json.dumps(pin(LOCAL / 'config.json')))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase', choices=('prepare', 'build', 'freeze'))
    args = parser.parse_args()
    {'prepare': prepare, 'build': build, 'freeze': freeze}[args.phase]()
