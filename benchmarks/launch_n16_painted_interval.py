#!/usr/bin/env python3
"""Preflight one frozen phase, record intent, then exec under the existing runner.

This does not acquire a lease, schedule work, monitor contention or impose a
deadline. The coordinator wraps it in the pinned strict observer and runner.
The independent launch-config SHA is supplied outside the config itself.
"""
import argparse
import json
import os
from pathlib import Path
import sys
import time

sys.dont_write_bytecode = True
from evaluate_n16_painted_interval_wasi import decode, file_hash, pinned, read, require, success
from evaluate_n16_output import THRESHOLDS, cell_map, declared_cells, validate_guard
from paired_render import ROOT, source_identity
import n16_painted_interval as experiment

SIDES = ('baseline', 'candidate')
GATES = ('PI-Q1', 'PI-Q2', 'PI-Q3', 'PI-Q4', 'PI-Q5', 'PI-M1', 'PI-REACH')


def pin(item):
    return pinned(Path(item['path']).resolve(), item['sha256'])


def completed_evidence(item):
    """A pinned completed receipt must bind this exact artifact and source."""
    receipt, artifact = pin(item['receipt']), pin(item['artifact'])
    success(receipt)
    require(receipt['artifacts'].get(str(Path(item['artifact']['path']).resolve()))
            == item['artifact']['sha256'], 'Receipt does not bind prerequisite artifact')
    return receipt, artifact


def prior_run(item, outputs, bindings_sha, plan_sha, wasi=False):
    receipt, report = completed_evidence(item)
    guard, audit = pin(item['guard']), pin(item['audit'])
    validate_guard(guard, 'strict')
    require(receipt['artifacts'].get(str(Path(item['guard']['path']).resolve())) == item['guard']['sha256'],
            'Prerequisite observer is not bound by its measurement receipt')
    require(audit['receipt_sha256'] == item['receipt']['sha256']
            and audit['report_sha256'] == item['artifact']['sha256']
            and audit['guard_sha256'] == item['guard']['sha256']
            and audit['background_contention_observed'] is False, 'Prerequisite audit binding/contention mismatch')
    if wasi:
        require(report['status'] == 'completed' and audit['status'] == 'verified'
                and audit['verified_outputs'] == outputs
                and audit['native_bindings_sha256'] == bindings_sha
                and audit['frozen_plan_sha256'] == plan_sha
                and not audit['suspension_cells'], 'WASI diagnostic is incomplete or suspends promotion')
    else:
        require(report['status'] == 'complete-exploratory' and report['phase'] == 'pilot'
                and audit['phase'] == 'pilot' and audit['independently_verified_outputs'] == outputs
                and audit['bindings_sha256'] == bindings_sha and audit['plan_sha256'] == plan_sha,
                'Pilot evidence is incomplete or belongs to another freeze')
    return receipt, audit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True, type=Path)
    parser.add_argument('--config-sha256', required=True)
    parser.add_argument('--phase', required=True, choices=('pilot', 'wasi', 'confirmation'))
    parser.add_argument('--dry-run', action='store_true', help='verify pins and print command; never launch or write')
    args = parser.parse_args()
    config = pinned(args.config.resolve(), args.config_sha256)
    require(config['experiment'] == 'n16-painted-interval' and config['phase'] == args.phase,
            'Wrong experiment or phase config')
    require(config['status'] == 'frozen-before-launch', 'Launch config is not frozen')
    require(isinstance(config['decision_reason'], str) and config['decision_reason'].strip(),
            'Coordinator must record the reason for launching this fixed phase')
    plan, bindings = pin(config['plan']), pin(config['bindings'])
    require(plan == experiment.frozen_plan(), 'Launcher and driver frozen plans differ')
    require(plan['case_count'] == 18 and plan['cell_count'] == 52
            and plan['target_count'] == 2 and plan['protected_count'] == 50
            and cell_map(plan['cells'], 52) == declared_cells(experiment, 18, 52)
            and plan['phases'] == experiment.PHASE_PLAN
            and plan['expected_verified_outputs'] == {'pilot': 1040, 'confirmation': 21320}
            and plan['thresholds'] == THRESHOLDS
            and plan['analysis'] == {'method': 'paired-percentile-bootstrap', 'resamples': 5000,
                                     'confidence': .95, 'seed': 42017}, 'Frozen native matrix or gates differ')
    require(config['plan']['path'] == str(ROOT / 'benchmarks/results/n16-painted-interval-plan.json')
            and config['bindings']['path'] == str(ROOT / 'benchmarks/results/n16-painted-interval-bindings.json'),
            'Use the experiment fixed plan and binding destinations')
    require(bindings['candidate_runner_source_sha256'] == bindings['runner_sources']['candidate']['sha256'],
            'Native and runner source digest bindings differ')
    for relative, digest in bindings['bound_files_sha256'].items():
        path = (ROOT / relative).resolve()
        require(path.is_relative_to(ROOT) and file_hash(path) == digest, 'Frozen source changed: ' + relative)
    required = {'benchmarks/' + name for name in (
        'n16_painted_interval.py', 'n16_painted_interval_fixtures.py', 'paired_render.py',
        'n16_output_u16.py', 'n16_output_guarded.py', 'n16_output.py', 'opaque_composition.py',
        'glyph_memo.py', 'glyph_memo_fixtures.py', 'scene_index.py', 'clip_viewport_fixtures.py',
        'evaluate_n16_output.py', 'evaluate_n16_painted_interval.py',
        'n16_painted_interval_wasi.py', 'evaluate_n16_painted_interval_wasi.py',
        'run_profile_wasi.mjs', 'launch_n16_painted_interval.py')}
    required |= {'crates/pagura-render/examples/scene_index_profile.rs',
                 'benchmarks/results/n16-painted-interval-plan.json'}
    require(required <= bindings['bound_files_sha256'].keys(), 'Incomplete experiment source bindings')
    python = Path(config['python']['path']).resolve()
    require(python == Path(sys.executable).resolve() and file_hash(python) == config['python']['sha256'],
            'Use the pinned Python executable')
    guard_path = Path(config['guard']['path']).resolve()
    require(guard_path.is_relative_to(ROOT) and file_hash(guard_path) == config['guard']['sha256']
            and bindings['bound_files_sha256'].get(guard_path.relative_to(ROOT).as_posix()) == config['guard']['sha256'],
            'Strict observer source is not frozen')
    experiment.require_baseline(bindings['identities']['baseline']['root'])
    native_commands = []
    for side in SIDES:
        identity = bindings['identities'][side]
        require(all(identity[key] == plan['identities_to_freeze'][side][key]
                    for key in ('root', 'commit', 'source_sha256', 'binary', 'binary_sha256')),
                'Plan/native identity mismatch: ' + side)
        require(source_identity(identity['root']) == {key: identity[key] for key in ('root', 'commit', 'source_sha256')},
                'Current source differs from frozen identity: ' + side)
        require(file_hash(Path(identity['binary'])) == identity['binary_sha256'], 'Native binary changed: ' + side)
        build = bindings['native_builds'][side]
        receipt = pinned(Path(build['receipt']), build['receipt_sha256'])
        success(receipt)
        require(receipt['source_root'] == identity['root']
                and receipt['source_before'] == bindings['runner_sources'][side]
                and receipt['source_before']['commit'] == identity['commit']
                and receipt['artifacts'].get(identity['binary']) == identity['binary_sha256'],
                'Native build/source/artifact binding missing: ' + side)
        require('cargo' in receipt['argv'], 'No recorded native Cargo build command')
        native_commands.append(receipt['argv'][receipt['argv'].index('cargo'):])
    require(native_commands[0] == native_commands[1], 'Native Cargo profile/argument lists differ')
    quality_receipt, quality = completed_evidence(config['prerequisites']['quality'])
    require(quality_receipt['source_before'] == bindings['runner_sources']['candidate']
            and quality_receipt['source_root'] == str(ROOT) and quality['status'] == 'passed'
            and quality['experiment'] == 'n16-painted-interval'
            and quality['source'] == bindings['runner_sources']['candidate']
            and quality['gates'] == {name: 'passed' for name in GATES}, 'Mandatory quality evidence is incomplete')
    layouts = quality['layouts']
    require(len(layouts) == 8 and {(row['target'], row['type']) for row in layouts}
            == {(target, name) for target in ('native', 'wasm32-wasip1')
                for name in ('Buffer', 'Option<Buffer>', 'Surface', 'MaskSink')}, 'Missing measured enclosing layouts')
    for row in layouts:
        for field in ('size', 'alignment'):
            require(type(row['baseline'][field]) is int and type(row['candidate'][field]) is int
                    and 0 < row['candidate'][field] <= row['baseline'][field], 'Enclosing layout grew or is unavailable')
    if args.phase in ('wasi', 'confirmation'):
        prior_receipt, _ = prior_run(config['prerequisites']['pilot'], 1040,
                                    config['bindings']['sha256'], config['plan']['sha256'])
        require(prior_receipt['source_before'] == bindings['runner_sources']['candidate'], 'Pilot source differs')
    if args.phase == 'confirmation':
        prior_receipt, wasi = prior_run(config['prerequisites']['wasi'], 336,
                                       config['bindings']['sha256'], config['plan']['sha256'], wasi=True)
        require(prior_receipt['source_before'] == bindings['runner_sources']['candidate'], 'WASI source differs')
        require(not wasi['fallback_review_cells'] or isinstance(config.get('fallback_review_reason'), str)
                and config['fallback_review_reason'].strip(), 'Material fallback signal needs explicit coordinator review')
    local = ROOT / 'benchmarks/results/local/n16-painted-interval'
    run = 'wasi-001' if args.phase == 'wasi' else args.phase + '-001'
    output, record = local / run, local / (run + '-launch.json')
    require(not output.exists() and not record.exists(), 'This phase was already launched; no automatic retry')
    if args.phase == 'wasi':
        builds = pin(config['wasi_build_bindings'])
        node = config['node']
        require(file_hash(Path(node['path'])) == node['sha256'], 'Node runtime changed')
        command = [str(python), str(ROOT / 'benchmarks/n16_painted_interval_wasi.py'),
                   '--bindings', config['bindings']['path'], '--bindings-sha256', config['bindings']['sha256'],
                   '--plan', config['plan']['path'], '--plan-sha256', config['plan']['sha256'],
                   '--build-bindings', config['wasi_build_bindings']['path'],
                   '--build-bindings-sha256', config['wasi_build_bindings']['sha256'],
                   '--node', node['path'], '--output', str(output), '--run-id', os.environ.get('PAGURA_RUN_ID', '<runner-run-id>')]
        for side in SIDES:
            build = builds[side]
            require(build['source'] == bindings['runner_sources'][side]
                    and file_hash(Path(build['path'])) == build['sha256'], 'WASI source/module changed')
            receipt = pinned(Path(build['receipt']), build['receipt_sha256'])
            success(receipt)
            require(receipt['source_root'] == build['source_root'] == bindings['identities'][side]['root']
                    and receipt['source_before'] == build['source']
                    and receipt['artifacts'].get(build['path']) == build['sha256'], 'WASI build/module binding missing')
            argv = receipt['argv']
            require('cargo' in argv and argv[argv.index('cargo'):] == [
                    'cargo', 'build', '--locked', '--offline', '--release', '-p', 'pagura-render',
                    '--example', 'scene_index_profile', '--target', 'wasm32-wasip1', '-j', '1'],
                    'WASI build command differs from fixed profile')
            command += ['--' + side + '-source', bindings['identities'][side]['root'],
                        '--' + side + '-wasm', build['path'], '--' + side + '-build-receipt', build['receipt']]
    else:
        command = [str(python), str(ROOT / 'benchmarks/n16_painted_interval.py'), '--phase', args.phase, '--output', str(output)]
        for side in SIDES:
            identity = bindings['identities'][side]
            command += ['--' + side, identity['binary'], '--' + side + '-source', identity['root']]
    if args.dry_run:
        print(json.dumps({'command': command, 'record': str(record), 'launches_nothing': True}, indent=2))
        return
    require(os.environ.get('PAGURA_RUN_ID') and os.environ.get('PAGURA_RUN_RECEIPT'), 'Run inside the coordinator runner')
    live = decode(read(Path(os.environ['PAGURA_RUN_RECEIPT'])))
    require(live['status'] == 'running' and live['run_id'] == os.environ['PAGURA_RUN_ID']
            and live['source_before'] == bindings['runner_sources']['candidate'], 'Live runner source/run identity differs')
    local.mkdir(parents=True, exist_ok=True)
    with record.open('x') as handle:
        json.dump({'schema': 1, 'experiment': 'n16-painted-interval', 'phase': args.phase,
                   'status': 'launch-intent-not-completion', 'created_unix_ns': time.time_ns(),
                   'config': {'path': str(args.config.resolve()), 'sha256': args.config_sha256},
                   'argv': sys.orig_argv, 'command': command,
                   'python': config['python'], 'guard': config['guard'],
                   'run_id': live['run_id'], 'run_receipt': os.environ['PAGURA_RUN_RECEIPT'],
                   'decision_reason': config['decision_reason'], 'admission': False}, handle, indent=2)
        handle.write('\n')
    os.execv(str(python), command)


if __name__ == '__main__':
    main()
