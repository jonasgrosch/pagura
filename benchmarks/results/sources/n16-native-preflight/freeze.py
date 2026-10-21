"""Freeze unchanged native/WASI builds and a completed quality record before pilot."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--quality', required=True, type=Path)
    parser.add_argument('--quality-receipt', required=True, type=Path)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    sys.path[:0] = [str(root / 'benchmarks'), str(root / 'tools')]
    from paired_render import source_identity
    from benchkit.artifacts import hash_regular_file
    from benchkit.config import source_digest
    from evaluate_n16_output import validate_guard
    from evaluate_n16_painted_interval_wasi import success
    results = root / 'benchmarks/results'
    local = results / 'local/n16-painted-interval'

    def require(value, message):
        if not value:
            raise ValueError(message)

    def pin(path):
        path = Path(path).resolve(strict=True)
        return {'path': str(path), 'sha256': hash_regular_file(path)}

    def read(path):
        return json.loads(Path(path).read_bytes())

    def write(path, value):
        with path.open('x') as handle:
            json.dump(value, handle, indent=2, allow_nan=False)
            handle.write('\n')

    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    runner_source = {'commit': commit, 'sha256': source_digest()}
    quality, receipt = read(args.quality), read(args.quality_receipt)
    success(receipt)
    require(receipt['source_root'] == str(root) and receipt['source_before'] == runner_source
            and receipt['artifacts'].get(str(args.quality.resolve())) == hash_regular_file(args.quality)
            and quality['status'] == 'passed' and quality['source'] == runner_source
            and quality['gates'] == {key: 'passed' for key in
                ('PI-Q1', 'PI-Q2', 'PI-Q3', 'PI-Q4', 'PI-Q5', 'PI-M1', 'PI-REACH')},
            'Quality does not cover the restored current candidate')
    identities, native_builds, runner_sources, wasi_builds = {}, {}, {}, {}
    for kind in ('native', 'wasi'):
        for side in ('baseline', 'candidate'):
            stem = f'{kind}-{side}-build-001'
            receipt_path = local / (stem + '-receipt.json')
            build = read(receipt_path)
            success(build)
            guard = local / (stem + '-observer/guard.json')
            validate_guard(read(guard), 'strict')
            require(build['artifacts'].get(str(guard)) == hash_regular_file(guard), 'Unbound build guard')
            for name, expected in build['artifacts'].items():
                require(hash_regular_file(name) == expected, 'Build artifact changed: ' + name)
            binary = next(name for name in build['artifacts'] if '/release/examples/' in name)
            identity = {**source_identity(build['source_root']), 'binary': binary,
                        'binary_sha256': hash_regular_file(binary)}
            require(identity['commit'] == build['source_before']['commit'], 'Build HEAD changed')
            context = read(local / (stem + '-context.json'))
            require(context['root'] == build['source_root'] and context['CARGO_BUILD_JOBS'] == '1'
                    and context['RUST_TEST_THREADS'] == '1' and context['RUSTFLAGS'] is None
                    and context['CARGO_ENCODED_RUSTFLAGS'] is None, 'Build environment differs')
            if kind == 'native':
                identities[side] = identity
                runner_sources[side] = build['source_before']
                native_builds[side] = {'receipt': str(receipt_path), 'receipt_sha256': hash_regular_file(receipt_path),
                                       'guard': pin(guard), 'context': pin(local / (stem + '-context.json'))}
            else:
                require(build['source_before'] == runner_sources[side], 'WASI/native source differs')
                wasi_builds[side] = {**pin(binary), 'source': build['source_before'],
                                    'source_root': build['source_root'], 'receipt': str(receipt_path),
                                    'receipt_sha256': hash_regular_file(receipt_path),
                                    'guard': pin(guard), 'context': pin(local / (stem + '-context.json'))}
    require(runner_sources['candidate'] == runner_source, 'Candidate differs from its build')
    baseline = identities['baseline']['root']
    require(not subprocess.check_output(['git', '-C', baseline, 'status', '--porcelain=v1', '--untracked-files=all']),
            'Baseline is not pristine')
    baseline_digest = subprocess.check_output([sys.executable, '-B', '-c',
        'from benchkit.config import source_digest; print(source_digest())'],
        cwd=Path(baseline) / 'tools', text=True).strip()
    require(baseline_digest == runner_sources['baseline']['sha256'], 'Baseline differs from its build')
    plan = read(results / 'n16-painted-interval-plan.template.json')
    require(plan['status'] == 'template-not-frozen', 'Wrong template')
    plan['status'] = 'frozen-before-pilot'
    plan['builds_on']['commit'] = identities['baseline']['commit']
    plan['identities_to_freeze'].update(identities)
    profile = read(results / 'n16-post-u16-profile.json')
    profile_results = Path(profile['binary']['path']).parents[2]
    profile_pins = {}
    for relative, expected in profile['evidence_sha256'].items():
        path = profile_results / relative
        require(hash_regular_file(path) == expected, 'Historical profile artifact changed: ' + relative)
        profile_pins[str(path)] = expected
    require(hash_regular_file(profile['binary']['path']) == profile['binary']['sha256'],
            'Historical profiled binary changed')
    plan['identities_to_freeze']['profile_evidence'] = {
        **pin(results / 'n16-post-u16-profile.json'),
        'binary': profile['binary'], 'evidence_sha256': profile_pins,
        'scope': 'Historical retained-product profile; selection rationale, not current baseline timing.'}
    for key in ('native_layout_evidence', 'wasi_layout_evidence', 'structural_dispatch_and_reach_evidence'):
        plan['identities_to_freeze'][key] = {'quality': pin(args.quality), 'receipt': pin(args.quality_receipt)}
    plan['structural_revision'] = {
        'version': 'R2', 'prior_result': 'R1 retained as failed six-row Competitive prediction',
        'outputs': 24, 'reason': 'Source-derived padded mask submission plus three fresh geometry controls',
        'timing_matrix_or_threshold_changes': False}
    plan_path = results / 'n16-painted-interval-plan.json'
    write(plan_path, plan)
    files = set(root.glob('benchmarks/*.py')) | set(root.glob('benchmarks/*.mjs'))
    files |= set((root / 'tools/benchkit').rglob('*.py')) | set((root / 'tools/fixturegen').rglob('*.py'))
    files |= {plan_path, root / 'tools/runner.py', root / 'crates/pagura-render/examples/scene_index_profile.rs',
              root / 'benchmarks/results/sources/analytic19-conversion/load_observer.py',
              Path(__file__).resolve(), Path(__file__).with_name('run.py').resolve(), root / '.cargo/config.toml'}
    bindings = {'schema': 1, 'experiment': 'n16-painted-interval', 'identities': identities,
                'native_builds': native_builds, 'runner_sources': runner_sources,
                'candidate_runner_source_sha256': runner_source['sha256'],
                'bound_files_sha256': {p.relative_to(root).as_posix(): hash_regular_file(p) for p in sorted(files)},
                'quality': {'receipt': pin(args.quality_receipt), 'artifact': pin(args.quality)},
                'limitations': ['Frozen warm-file identities; no cold-I/O or competitor claim.',
                                'Instrumented structural checks are separate from these uninstrumented binaries.']}
    binding_path = results / 'n16-painted-interval-bindings.json'
    write(binding_path, bindings)
    wasi_path = results / 'n16-painted-interval-wasi-build-bindings.json'
    write(wasi_path, wasi_builds)
    config = {'schema': 1, 'experiment': 'n16-painted-interval', 'phase': 'pilot',
              'status': 'frozen-before-launch', 'plan': pin(plan_path), 'bindings': pin(binding_path),
              'python': pin(sys.orig_argv[0]), 'python_executable': pin(sys.executable),
              'guard': pin(root / 'benchmarks/results/sources/analytic19-conversion/load_observer.py'),
              'prerequisites': {'quality': bindings['quality']},
              'decision_reason': 'All mandatory quality gates passed, including explicitly revised R2 reach. '
                                 'Launch only after the separate 104-output native preflight passes. '
                                 'Collect the fixed exploratory pilot without changing cells, seeds or thresholds.'}
    write(local / 'pilot-config.json', config)
    print(json.dumps({'plan': pin(plan_path), 'bindings': pin(binding_path),
                      'wasi_builds': pin(wasi_path), 'pilot_config': pin(local / 'pilot-config.json')}, indent=2))


if __name__ == '__main__':
    main()
