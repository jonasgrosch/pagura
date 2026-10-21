#!/usr/bin/env python3
"""Audit the exact retained prefix and resumed suffix; never launch a renderer.

The shared validator authenticates the frozen launch/admission evidence. Schedule,
prefix, pair attribution, journal and pixel checks below do not call the producer's
schedule/prefix/completion functions. Bootstrap bounds retain the historical
auditor limitation: validate point estimates and bounds, do not recompute them.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import random
import stat
import statistics
import sys

sys.dont_write_bytecode = True
import f32_painted_interval as experiment
import f32_painted_interval_continuation as continuation
from f32_interval_admission import (HARNESS, ROOT, decode, descriptor, driver_command,
                                    evidence, pin, read, require)
from evaluate_n16_output import THRESHOLDS
from paired_render import expected_image
from benchkit.statistics import quantile

SIDES = ('baseline', 'candidate')
ARM_FIELDS = {'command', 'process', 'diagnostic', 'stages', 'blob', 'output_sha256',
              'pixels_sha256', 'width', 'height', 'oracle_equal'}


def positive(value):
    return type(value) is int and value > 0


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def fixed_schedule():
    require(len(experiment.CASES) == 20 and len(experiment.CELLS) == 56
            and len(set(experiment.CELLS)) == 56, 'Fixed matrix differs')
    rng = random.Random(20261110)
    cells = list(experiment.CELLS)
    rng.shuffle(cells)
    ordered = []
    for index, cell in enumerate(cells):
        for number in range(-5, 200):
            order = list(SIDES)
            rng.shuffle(order)
            ordered.append({'sequence': len(ordered), 'cell_index': index,
                            'cell': list(cell), 'pair': number, 'order': order})
    return cells, ordered


def retained_prefix(old, cells, ordered):
    """A structurally complete pair is kept even without its final timestamp."""
    kept, abandoned = [], []
    require(0 < len(old['cells']) <= 56, 'Original cell prefix is empty/oversized')
    for index, cell in enumerate(old['cells']):
        require(tuple(cell[k] for k in ('case', 'mode', 'operation')) == cells[index]
                and len(cell['pairs']) <= 205, 'Original cell prefix/order differs')
        if index + 1 < len(old['cells']):
            require(len(cell['pairs']) == 205, 'Original prefix skipped an unfinished cell')
        for offset, pair in enumerate(cell['pairs']):
            entry = ordered[index * 205 + offset]
            require(not abandoned and pair['pair'] == entry['pair'] and pair['order'] == entry['order'],
                    'Original pair schedule has a gap/order change')
            complete = all(isinstance(pair.get(s), dict) and ARM_FIELDS <= pair[s].keys() for s in SIDES)
            if complete:
                require(entry['sequence'] == len(kept), 'Original complete pairs are not a prefix')
                kept.append(pair)
            else:
                require(index == len(old['cells']) - 1 and offset == len(cell['pairs']) - 1,
                        'Only the original terminal pair may be incomplete')
                abandoned.append({'sequence': entry['sequence'], 'cell_index': index,
                                  'pair': pair['pair'], 'segment': 'original', 'attempt': pair,
                                  'reason': 'Pair structurally incomplete; retain attempt, rerun both arms.'})
    return kept, abandoned


def check_arm(record, identity, directory, cell, expected, checked):
    name, mode, operation = cell
    require(ARM_FIELDS <= record.keys() and record['oracle_equal'] is True, 'Incomplete/incorrect arm')
    process, stage = record['process'], record['stages']
    require(type(process.get('exit_code')) is int and process['exit_code'] == 0
            and process.get('status') in ('failed', 'ok') and positive(process.get('peak_rss_bytes')),
            'Failed sample or unavailable RSS')
    require(record['command'] == [identity['binary'], str(directory / 'inputs' / (name + '.pdf')),
            mode, operation, str(directory / 'pending.rgba'), '72'], 'Wrong actual cohort command')
    require(all(positive(stage.get(k)) for k in ('open_ns', 'compile_ns', 'render_ns', 'total_ns', 'metric_ns'))
            and (stage.get('schema'), stage.get('operation'), stage.get('accuracy'), stage.get('dpi'),
                 stage.get('width'), stage.get('height')) == (1, operation, mode, 72, *expected[:2])
            and stage['metric_ns'] == stage['total_ns' if operation == 'cold' else 'render_ns']
            and stage['total_ns'] >= sum(stage[k] for k in ('open_ns', 'compile_ns', 'render_ns')),
            'Stage identity/metric differs')
    logs = [decode(line.encode()) for line in record['diagnostic'].splitlines() if line.startswith('{')]
    require(logs == [stage], 'Retained diagnostic and stage differ')
    require((record['width'], record['height']) == expected[:2]
            and record['pixels_sha256'] == sha(expected[2]), 'Pixel metadata differs')
    digest = record['output_sha256']
    require(isinstance(digest, str) and len(digest) == 64 and all(c in '0123456789abcdef' for c in digest)
            and record['blob'] == 'blobs/' + digest + '.rgba', 'Unsafe/noncanonical blob name')
    # Every distinct physical blob is read and hashed in each cohort. Reused
    # content only reuses pixel validation for the same frozen expected bytes.
    key = (str(directory), digest, expected[0], expected[1], record['pixels_sha256'])
    if key not in checked:
        raw = read(directory / record['blob'])
        header = f'RGBA8 {expected[0]} {expected[1]} premultiplied\n'.encode()
        pixels = raw[len(header):]
        require(sha(raw) == digest and raw.startswith(header) and len(pixels) == expected[0] * expected[1] * 4
                and pixels[3::4] == b'\xff' * (expected[0] * expected[1])
                and all(pixels[c::4] == expected[2][c::3] for c in range(3)), 'Independent RGBA mismatch')
        checked.add(key)


def check_journal(path, report, ordered, prefix_count):
    """Stream a bounded regular journal; its final hash is receipt-bound."""
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(fd, 'rb') as stream:
        metadata = os.fstat(stream.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_size <= 1024**3, 'Unbounded/nonregular journal')
        previous, count = 0, 0
        def event(kind):
            nonlocal previous, count
            line = stream.readline(1024 * 1024 + 1)
            require(line.endswith(b'\n') and len(line) <= 1024 * 1024, 'Missing/oversized journal event')
            value = decode(line)
            require(value['event'] == kind and positive(value['monotonic_ns'])
                    and value['monotonic_ns'] >= previous, 'Journal order/time differs')
            previous, count = value['monotonic_ns'], count + 1
            return value
        start = event('run-start')
        require(start['config'] == report['config'] and start['original_report'] == report['original_report']
                and start['original_completed_pairs'] == prefix_count, 'Journal start differs')
        for entry in ordered[prefix_count:]:
            pair = report['cells'][entry['cell_index']]['pairs'][entry['pair'] + 5]
            begin = event('pair-start')
            require(begin['schedule'] == entry and begin['sample'] == {k: pair[k] for k in
                    ('pair', 'order', 'started_monotonic_ns')}, 'Journal pair start differs')
            for side in entry['order']:
                arm = event('arm-result')
                require(arm['sequence'] == entry['sequence'] and arm['side'] == side
                        and arm['record'] == pair[side], 'Journal arm differs')
            end = event('pair-complete')
            require(end['sequence'] == entry['sequence'] and end['sample'] == pair, 'Journal completed pair differs')
        end = event('run-finished')
        require(end['status'] == report['status'] and end['new_completed_pairs'] == len(ordered) - prefix_count
                and stream.read(1) == b'', 'Journal has wrong terminal status/trailing attempts')
        return count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', choices=('confirmation-001',))
    parser.add_argument('--config', required=True, type=Path)
    parser.add_argument('--config-sha256', required=True)
    args = parser.parse_args()
    config_path = args.config.resolve()
    config, bindings, amendment = continuation.validate(config_path, args.config_sha256)
    local, name = continuation.LOCAL, continuation.NAME
    directory = local / args.run
    launcher = [config['python']['path'], str(ROOT / 'benchmarks/f32_painted_interval_continuation.py'),
                '--config', str(config_path), '--config-sha256', args.config_sha256]
    driver = [*launcher, '--execute']
    item = {'receipt': descriptor(local / (args.run + '-receipt.json')),
            'artifact': descriptor(directory / 'report.json'),
            'guard': descriptor(local / (args.run + '-observer/guard.json'))}
    receipt, report, guard = evidence(item, bindings, launcher)
    intent_pin = descriptor(local / (args.run + '-launch.json'))
    start_pin = descriptor(local / (args.run + '-driver-start.json'))
    journal_pin = descriptor(directory / 'journal.jsonl')
    config_pin = {'path': str(config_path), 'sha256': args.config_sha256}
    for entry in (config_pin, intent_pin, start_pin, journal_pin):
        require(receipt['artifacts'].get(entry['path']) == entry['sha256'], 'Final receipt misses launch/journal evidence')
    intent, start = pin(intent_pin), pin(start_pin)
    require(intent['status'] == 'launch-intent-not-completion' and intent['argv'] == launcher
            and intent['command'] == driver and start['status'] == 'driver-start-not-completion'
            and start['argv'] == driver and start['intent'] == intent_pin, 'Actual direct driver chain differs')
    for record in (intent, start):
        require(record['experiment'] == name and record['run_id'] == receipt['run_id']
                and record['run_receipt'] == item['receipt']['path'] and record['config'] == config_pin,
                'Launch run/config differs')
    old = pin(amendment['original']['report'])
    old_receipt = pin(amendment['original']['receipt'])
    old_intent, old_start = (pin(amendment['original'][key]) for key in ('intent', 'start'))
    old_launcher = [bindings['python']['path'], str(ROOT / 'benchmarks/launch_f32_painted_interval.py'),
                    '--config', amendment['original_config']['path'], '--config-sha256',
                    amendment['original_config']['sha256'], '--phase', 'confirmation']
    require(old_intent['status'] == 'launch-intent-not-completion' and old_intent['argv'] == old_launcher
            and old_intent['command'] == old_start['argv'] == driver_command(bindings, 'confirmation')
            and old_start['status'] == 'driver-start-not-completion'
            and old_start['intent'] == amendment['original']['intent'], 'Original post-exec chain differs')
    for record in (old_intent, old_start):
        require(record['experiment'] == 'f32-painted-interval' and record['phase'] == 'confirmation'
                and record['run_id'] == old_receipt['run_id']
                and record['run_receipt'] == amendment['original']['receipt']['path']
                and record['config'] == amendment['original_config'], 'Original run/config differs')
    require(old['experiment'] == 'f32-painted-interval' and old['status'] == 'invalid-or-incomplete'
            and old['phase'] == report['phase'] == 'confirmation'
            and (old['pairs'], old['warmups'], old['seed'], old['dpi']) == (200, 5, 20261110, 72)
            and old['identities'] == report['identities'] == bindings['identities']
            and old['environment']['run_id'] == old_receipt['run_id']
            and old['environment']['run_receipt'] == amendment['original']['receipt']['path'], 'Original report identity differs')
    for key in ('pairs', 'warmups', 'seed', 'dpi', 'thresholds', 'statistics', 'inputs', 'declared_cells',
                'phase_plan', 'example_sha256', 'driver_sha256', 'harness_sources_sha256', 'environment'):
        require(report[key] == old[key], 'Original metadata changed: ' + key)
    require(report['thresholds'] == THRESHOLDS
            and report['statistics'] == {'method': 'paired percentile bootstrap', 'resamples': 5000,
                 'one_sided_confidence': .95, 'seed': 42017,
                 'scope': 'marginal estimates; temporal dependence and p95 precision need review'}
            and report['harness_sources_sha256'] == {n: bindings['bound_files_sha256'][n] for n in HARNESS}
            and report['driver_sha256'] == bindings['bound_files_sha256']['benchmarks/f32_painted_interval.py']
            and report['example_sha256'] == bindings['bound_files_sha256']['crates/pagura-render/examples/scene_index_profile.rs'],
            'Frozen statistics/helper metadata differs')
    original_directory = Path(amendment['original']['report']['path']).parent
    require(report['experiment'] == name and report['original_experiment'] == old['experiment']
            and report['status'] == 'complete-amended-exploratory' and report['admission'] is False
            and report['config'] == config_pin and report['amendment'] == config['amendment']
            and report['original_report'] == amendment['original']['report']
            and report['segments'] == {
                'original': {**amendment['original'], 'directory': str(original_directory)},
                'continuation': {'directory': str(directory), 'run_id': receipt['run_id'],
                    'receipt': item['receipt']['path'], 'guard': item['guard']['path'],
                    'intent': intent_pin, 'start': start_pin}}, 'Segment/report attribution differs')
    require(report['environment_scope'] == 'retained-original'
            and report['continuation_environment']['run_id'] == receipt['run_id']
            and report['continuation_environment']['run_receipt'] == item['receipt']['path']
            and report['continuation_driver_sha256'] == bindings['bound_files_sha256'][
                'benchmarks/f32_painted_interval_continuation.py'], 'Continuation environment/helper differs')
    identities = {s: {k: bindings['identities'][s][k] for k in ('root', 'commit', 'source_sha256')} for s in SIDES}
    require(report['source_before'] == report['source_after'] == identities, 'Continuation source changed')
    cells, ordered = fixed_schedule()
    require(len(report['cells']) == 56 and report['declared_cells'] == [
        {'case': n, 'mode': m, 'operation': o, 'role': 'target' if n == 'sparse64' and o == 'full' else 'protected'}
        for n, m, o in experiment.CELLS], 'Declared cell matrix differs')
    kept, abandoned = retained_prefix(old, cells, ordered)
    require(report['original_completed_pairs'] == len(kept)
            and report['new_completed_pairs'] == len(ordered) - len(kept)
            and report['abandoned_attempts'] == abandoned, 'Original prefix/tail was changed')
    require(report['pair_sources'] == [
        {k: entry[k] for k in ('sequence', 'cell_index', 'pair')} |
        {'segment': 'original' if entry['sequence'] < len(kept) else 'continuation'} for entry in ordered],
        'Combined schedule/cohort attribution differs')
    oracles = {}
    for fixture in experiment.CASES:
        pdf, oracle = experiment.fixture(fixture)
        require(report['inputs'][fixture] == {'pdf_sha256': sha(pdf), 'oracle_sha256': sha(oracle)}, 'Frozen input/oracle differs')
        for root in (original_directory, directory):
            require(read(root / 'inputs' / (fixture + '.pdf')) == pdf
                    and read(root / 'oracles' / (fixture + '.ppm')) == oracle, 'Cohort input/oracle bytes differ')
        oracles[fixture] = oracle
    rows, checked, outputs = [], set(), 0
    for index, cell in enumerate(report['cells']):
        identity = tuple(cell[k] for k in ('case', 'mode', 'operation'))
        role = 'target' if identity[0] == 'sparse64' and identity[2] == 'full' else 'protected'
        require(identity == cells[index] and cell['role'] == role and len(cell['pairs']) == 205, 'Combined cell differs')
        expected = expected_image(oracles[identity[0]], identity[2])
        for offset, pair in enumerate(cell['pairs']):
            entry = ordered[index * 205 + offset]
            require(pair['pair'] == entry['pair'] and pair['order'] == entry['order'], 'Combined pair order differs')
            if entry['sequence'] < len(kept):
                require(pair == kept[entry['sequence']], 'Completed original pair was rewritten')
                root = original_directory
            else:
                root = directory
                require(positive(pair['started_monotonic_ns']) and positive(pair['finished_monotonic_ns'])
                        and pair['started_monotonic_ns'] <= pair['finished_monotonic_ns'], 'New pair did not finish')
            for side in SIDES:
                check_arm(pair[side], bindings['identities'][side], root, identity, expected, checked)
                outputs += 1
            require(pair['baseline']['output_sha256'] == pair['candidate']['output_sha256'], 'Pair bytes differ')
        measured = cell['pairs'][5:]
        values = {s: [p[s]['stages']['metric_ns'] for p in measured] for s in SIDES}
        latency = cell['latency']
        require(set(latency) == {n + s for n in ('median', 'p95') for s in ('_ratio', '_lower', '_upper')}
                and all(type(v) in (int, float) and math.isfinite(v) and v > 0 for v in latency.values()), 'Invalid statistics')
        for key, function in (('median', statistics.median), ('p95', lambda x: quantile(x, .95))):
            require(latency[key + '_ratio'] == function(values['candidate']) / function(values['baseline'])
                    and latency[key + '_lower'] <= latency[key + '_upper'], 'Point ratio/bounds differ')
        peaks = {s: max(p[s]['process']['peak_rss_bytes'] for p in measured) for s in SIDES}
        rss = peaks['candidate'] / peaks['baseline']
        require(cell['peak_rss_ratio'] == rss, 'RSS maximum ratio differs')
        rows.append({k: cell[k] for k in ('case', 'mode', 'operation', 'role')} | {
            'latency': latency, 'peak_rss_ratio': rss, 'peak_rss_mib': {s: peaks[s] / 2**20 for s in SIDES},
            'baseline_median_ms': statistics.median(values['baseline']) / 1e6,
            'candidate_median_ms': statistics.median(values['candidate']) / 1e6,
            'median_pass': latency['median_upper'] <= THRESHOLDS[role + '_median_upper'],
            'p95_pass': latency['p95_upper'] <= THRESHOLDS[role + '_p95_upper'],
            'memory_pass': rss <= THRESHOLDS['peak_rss_ratio_max']})
    require(outputs == 22960, 'Admitted output count differs')
    abandoned_verified = 0
    for attempt in abandoned:
        cell = cells[attempt['cell_index']]
        expected = expected_image(oracles[cell[0]], cell[2])
        for side in SIDES:
            record = attempt['attempt'].get(side)
            if isinstance(record, dict) and ARM_FIELDS <= record.keys():
                check_arm(record, bindings['identities'][side], original_directory, cell, expected, checked)
                abandoned_verified += 1
    journal_events = check_journal(Path(journal_pin['path']), report, ordered, len(kept))
    # Recheck every terminal evidence pin after reading; original evidence is
    # never rewritten or reclassified as a completed strict run.
    for entry in (item['receipt'], item['artifact'], item['guard'], intent_pin, start_pin, journal_pin,
                  config_pin, *amendment['original'].values()):
        require(descriptor(entry['path']) == entry, 'Evidence changed during audit')
    summary = {'schema': 1, 'experiment': name, 'run': args.run, 'phase': 'confirmation',
        'status': 'verified-amended-continuation', 'admission': False, 'uninterrupted_confirmation': False,
        'original_process_presence_observed': True, 'continuation_process_presence_observed': False,
        'background_contention_observed': None, 'observed_contention_blocks_admission': None,
        'contention_scope': 'Original guard proves process presence, not CPU consumption; new strict segment has no observed matches.',
        'report_sha256': item['artifact']['sha256'], 'receipt_sha256': item['receipt']['sha256'],
        'guard_sha256': item['guard']['sha256'], 'plan_sha256': config['plan']['sha256'],
        'bindings_sha256': config['bindings']['sha256'], 'journal_sha256': journal_pin['sha256'],
        'audit_source': descriptor(Path(__file__).resolve()),
        'independently_verified_outputs': outputs, 'distinct_output_blobs_rehashed': len(checked),
        'original_completed_pairs': len(kept), 'new_completed_pairs': len(ordered) - len(kept),
        'abandoned_attempts_count': len(abandoned), 'journal_events': journal_events,
        'abandoned_original_recorded_arms': sum(isinstance(a['attempt'].get(s), dict) for a in abandoned for s in SIDES),
        'abandoned_original_verified_outputs': abandoned_verified,
        'bootstrap_bounds_independently_recomputed': False,
        'numeric_gates_pass': all(c['median_pass'] and c['p95_pass'] and c['memory_pass'] for c in rows),
        'target_latency_passes': sum(c['median_pass'] and c['p95_pass'] for c in rows if c['role'] == 'target'),
        'protected_latency_passes': sum(c['median_pass'] and c['p95_pass'] for c in rows if c['role'] == 'protected'),
        'memory_passes': sum(c['memory_pass'] for c in rows), 'cells': rows,
        'limitations': ['Post-stop method amendment with two time cohorts; not fresh/uninterrupted confirmation.',
                       'Original growing JSON checkpoint changed to flushed event journal for the continuation.',
                       'Original completed pairs are preserved; abandoned partial attempts are additional work, outside 22960 admitted outputs.',
                       'Marginal bootstrap bounds are producer-reported; point estimates and sample coverage independently checked.']}
    output = local / (args.run + '-analysis.json')
    with output.open('x') as stream:
        stream.write(json.dumps(summary, indent=2, allow_nan=False) + '\n')
    print(json.dumps({k: v for k, v in summary.items() if k != 'cells'}, indent=2))


if __name__ == '__main__':
    main()
