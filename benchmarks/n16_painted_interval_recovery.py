#!/usr/bin/env python3
"""One separately declared recovery confirmation; original evidence is untouched."""
import argparse
import copy
import json
import os
from pathlib import Path
import sys
import time

sys.dont_write_bytecode = True
import n16_painted_interval as original
from launch_n16_painted_interval import GATES, completed_evidence, prior_run
from evaluate_n16_output import ROOT, THRESHOLDS, declared_cells, cell_map, load, require, validate_guard
from evaluate_n16_painted_interval_wasi import pinned, file_hash, success
from paired_render import run_experiment, source_identity

NAME = 'n16-painted-interval-recovery'
RUN = 'confirmation-001'
CASES, CELLS, fixture = original.CASES, original.CELLS, original.fixture
PHASE_PLAN = {'confirmation': {'pairs': 200, 'warmups': 5, 'seed': 20261108}}
RESULTS = ROOT / 'benchmarks/results'
LOCAL = RESULTS / 'local' / NAME
DRIVER = 'benchmarks/n16_painted_interval_recovery.py'
AUDITOR = 'benchmarks/evaluate_n16_painted_interval_recovery.py'
FREEZER = 'benchmarks/prepare_n16_painted_interval_recovery.py'
DOC = 'docs/plans/n16-painted-interval-recovery.md'
ORIGINAL_CONFIG = RESULTS / 'local/n16-painted-interval/confirmation-config.json'
ANCHORS = {
 'config': '2f67e5abdc20da200424c7df4159cc5aec508cad4b35f47c4572a0fe9e215066',
 'plan': '84d8932bcc23089b71d44ff5e513f9a549e4cff77aa57b7825544149680b55d8',
 'bindings': '1d1068bcfe81843728d29c1b5267d3207bfb3a3d6f03d3bdf1196f35641e9e6a',
 'receipt': '7123383335df77dbd0902961976a65ce9228ccddaa32c6142e579c489f519528',
 'guard': '8dd70cc5b000baa05251badc2c1f70182f4776500f7752f83db1eef36f1a7dc6',
}
previous = original.previous
guarded, n16 = previous.previous, previous.previous.previous
opaque = n16.opaque
FIXTURE_SOURCES = (original.__file__, original.intervals.__file__, previous.__file__, guarded.__file__,
                   n16.__file__, opaque.__file__, opaque.memo.__file__, opaque.memo.legacy.__file__,
                   opaque.memo.outlines.__file__, opaque.memo.clips.__file__)
HARNESS = {DRIVER, 'benchmarks/paired_render.py',
           *(Path(path).resolve().relative_to(ROOT).as_posix() for path in FIXTURE_SOURCES)}


def pin(item):
    return pinned(Path(item['path']), item['sha256'])


def make_plan(old, recovery):
    result = copy.deepcopy(old)
    result.update(experiment=NAME, status='frozen-before-confirmation', phases=PHASE_PLAN,
                  expected_verified_outputs={'confirmation': 21320}, recovery=recovery)
    return result


def frozen_plan():
    return load(RESULTS / (NAME + '-plan.json'))


def commands(config, config_path, digest):
    driver = [config['python']['path'], str(ROOT / DRIVER), '--config', str(config_path), '--config-sha256', digest]
    observer = [config['python']['path'], config['guard']['path'], '--evidence-dir',
                str(LOCAL / (RUN + '-observer')), '--', *driver]
    measurement = [str(ROOT / DRIVER), '--phase', 'confirmation', '--output', str(LOCAL / RUN)]
    for side in ('baseline', 'candidate'):
        identity = config['identities'][side]
        measurement += ['--' + side, identity['binary'], '--' + side + '-source', identity['root']]
    return driver, observer, measurement


def validate(config_path, digest):
    require(config_path == LOCAL / 'confirmation-config.json' and Path.cwd().resolve() == ROOT, 'fixed config/cwd required')
    config = pinned(config_path, digest)
    require(config['experiment'] == NAME and config['status'] == 'frozen-before-launch'
            and config['phase'] == 'confirmation', 'recovery config differs')
    old = pinned(ORIGINAL_CONFIG, ANCHORS['config'])
    require(old['plan']['sha256'] == ANCHORS['plan'] and old['bindings']['sha256'] == ANCHORS['bindings'], 'original anchors differ')
    old_plan, old_bindings = pin(old['plan']), pin(old['bindings'])
    plan, bindings = pin(config['plan']), pin(config['bindings'])
    require(config['plan']['path'] == str(RESULTS / (NAME + '-plan.json'))
            and config['bindings']['path'] == str(RESULTS / (NAME + '-bindings.json')), 'recovery destinations differ')
    require(config['original_config'] == {'path': str(ORIGINAL_CONFIG), 'sha256': ANCHORS['config']}
            and config['prerequisites'] == old['prerequisites'], 'original prerequisite pins changed')
    require(plan == make_plan(old_plan, config['recovery']) and plan['thresholds'] == THRESHOLDS
            and plan['maximum_launched_confirmations'] == 1
            and cell_map(plan['cells'], 52) == declared_cells(original, 18, 52), 'recovery plan changed more than declared')
    require(config['recovery']['decision_reason'].strip() and config['recovery']['excluded_samples'] == 'all'
            and config['recovery']['partial_timing_inspected'] is False
            and config['recovery']['automatic_retry'] is False, 'explicit outcome-independent recovery declaration missing')
    require(bindings['identities'] == config['identities'] == old_bindings['identities']
            == {side: plan['identities_to_freeze'][side] for side in ('baseline', 'candidate')}
            and bindings['runner_sources'] == old_bindings['runner_sources']
            and bindings['native_builds'] == old_bindings['native_builds'], 'product/build identity changed')
    require(bindings['candidate_runner_source_sha256'] == old_bindings['candidate_runner_source_sha256'], 'runner source changed')
    bound = bindings['bound_files_sha256']
    require(all(bound.get(path) == value for path, value in old_bindings['bound_files_sha256'].items())
            and {DRIVER, AUDITOR, FREEZER, DOC, 'benchmarks/results/' + NAME + '-plan.json'} <= set(bound), 'original/new source closure incomplete')
    for path, value in bound.items():
        resolved = (ROOT / path).resolve()
        require(resolved.is_relative_to(ROOT) and file_hash(resolved) == value, 'frozen helper changed: ' + path)
    for path, value in bindings['original_evidence_files_sha256'].items():
        require(Path(path).is_absolute() and file_hash(Path(path)) == value, 'original evidence changed')
    for key, actual in (('python', sys.orig_argv[0]), ('python_executable', sys.executable)):
        require(config[key] == old[key] and Path(actual).resolve() == Path(config[key]['path']).resolve()
                and file_hash(Path(config[key]['path'])) == config[key]['sha256'], 'Python identity changed')
    require(config['guard'] == old['guard'] and file_hash(Path(config['guard']['path'])) == config['guard']['sha256'], 'observer changed')
    original.require_baseline(bindings['identities']['baseline']['root'])
    for side, identity in bindings['identities'].items():
        require(source_identity(identity['root']) == {key: identity[key] for key in ('root', 'commit', 'source_sha256')}
                and file_hash(Path(identity['binary'])) == identity['binary_sha256'], 'source/binary changed')
        build = bindings['native_builds'][side]
        receipt = pinned(Path(build['receipt']), build['receipt_sha256']); success(receipt)
        require(receipt['source_root'] == identity['root'] and receipt['source_before'] == bindings['runner_sources'][side]
                and receipt['artifacts'].get(identity['binary']) == identity['binary_sha256'], 'build receipt differs')
    receipt, quality = completed_evidence(old['prerequisites']['quality'])
    require(receipt['source_before'] == quality['source'] == bindings['runner_sources']['candidate']
            and quality['status'] == 'passed' and quality['gates'] == {gate: 'passed' for gate in GATES}, 'quality prerequisite differs')
    for phase, count in (('pilot', 1040), ('wasi', 336)):
        receipt, audit = prior_run(old['prerequisites'][phase], count, ANCHORS['bindings'], ANCHORS['plan'], wasi=phase == 'wasi')
        require(receipt['source_before'] == bindings['runner_sources']['candidate'], 'prerequisite source differs')
        if phase == 'wasi':
            require(not audit['fallback_review_cells'] or bool(old.get('fallback_review_reason', '').strip()), 'WASI fallback requires review')
    preflight = config['native_preflight']
    receipt, report = completed_evidence(preflight)
    guard = pin(preflight['guard']); validate_guard(guard, 'strict')
    require(receipt['source_before'] == bindings['runner_sources']['candidate']
            and receipt['artifacts'].get(preflight['guard']['path']) == preflight['guard']['sha256']
            and report['status'] == 'verified-native-preflight' and report['all_correct'] is True
            and report['expected_outputs'] == report['completed_outputs'] == 104
            and report['bindings_sha256'] == ANCHORS['bindings'] and report['identities'] == bindings['identities'], 'native preflight differs')
    excluded = config['recovery']['excluded_confirmation']
    require(excluded['receipt']['sha256'] == ANCHORS['receipt'] and excluded['guard']['sha256'] == ANCHORS['guard'], 'interrupted attempt anchor differs')
    receipt, guard = pin(excluded['receipt']), pin(excluded['guard'])
    require(receipt['status'] == 'failed' and receipt['exit_code'] == receipt['wrapper_exit_code'] == 143
            and receipt['source_unchanged'] is True and not receipt['errors'] and not receipt['artifact_errors']
            and receipt['source_before'] == receipt['source_after'] == bindings['runner_sources']['candidate']
            and guard['status'] == 'invalid-external-contention' and any(event['matches'] for event in guard['events']), 'external cancellation not established')
    for key in ('guard', 'report', 'intent'):
        item = excluded[key]
        require(receipt['artifacts'].get(item['path']) == item['sha256'] and file_hash(Path(item['path'])) == item['sha256'], 'interrupted artifact changed')
    # Never parse or inspect the interrupted numerical report.
    return config, plan, bindings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True, type=Path)
    parser.add_argument('--config-sha256', required=True)
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    config, _, bindings = validate(args.config.resolve(), args.config_sha256)
    driver, observer, measurement = commands(config, args.config.resolve(), args.config_sha256)
    intent_path = LOCAL / (RUN + '-launch.json')
    require(not (LOCAL / RUN).exists() and not intent_path.exists(), 'recovery already launched; no retry')
    if args.dry_run:
        print(json.dumps({'observer_argv': observer, 'measurement_arguments': measurement, 'intent': str(intent_path), 'launches_nothing': True}, indent=2)); return
    require(sys.orig_argv == driver and os.environ.get('PAGURA_RUN_ID'), 'actual driver/runner argv differs')
    receipt_path = LOCAL / (RUN + '-receipt.json')
    require(os.environ.get('PAGURA_RUN_RECEIPT') == str(receipt_path), 'fixed receipt path required')
    live = load(receipt_path)
    require(live['status'] == 'running' and live['run_id'] == os.environ['PAGURA_RUN_ID']
            and live['argv'] == observer and live['source_root'] == str(ROOT)
            and live['source_before'] == bindings['runner_sources']['candidate'], 'live observer/driver chain differs')
    LOCAL.mkdir(parents=True, exist_ok=True)
    with intent_path.open('x') as stream:
        json.dump({'schema': 1, 'experiment': NAME, 'phase': 'confirmation', 'status': 'launch-intent-not-completion',
                   'config': {'path': str(args.config.resolve()), 'sha256': args.config_sha256},
                   'producer_argv': sys.orig_argv, 'observer_argv': observer,
                   'measurement_arguments': measurement, 'execution': 'in-process call to unchanged paired_render.run_experiment',
                   'run_id': live['run_id'], 'run_receipt': str(receipt_path), 'created_unix_ns': time.time_ns(),
                   'recovery': config['recovery'], 'admission': False}, stream, indent=2)
        stream.write('\n')
    # The outer invocation remains sys.orig_argv; these are only shared-parser arguments.
    prior_argv = sys.argv
    try:
        sys.argv = measurement
        run_experiment(__file__, description=__doc__, experiment=NAME, cases=CASES, cells=CELLS,
            target_case='opaque64', fixture=fixture, fixture_sources=FIXTURE_SOURCES, phase_plan=PHASE_PLAN,
            target_scope='Unchanged N16 painted-output interval; separately declared external-contention recovery',
            limitations=('Original interrupted confirmation is entirely excluded and remains unavailable',
                         'One fresh fixed recovery confirmation only; no retry, pooling or changed gates',
                         'Default Repair opening and full compile_page; diagnostics not emitted',
                         'Prior quality/pilot/WASI evidence applies through unchanged original identity pins'))
    finally:
        sys.argv = prior_argv


if __name__ == '__main__':
    main()
